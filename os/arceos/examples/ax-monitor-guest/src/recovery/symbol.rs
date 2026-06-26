//! Kernel ELF symbol table resolution.
//!
//! Translates virtual addresses to function names using the kernel's ELF
//! symbol table. Uses `ax_std::fs::read` for file access and `goblin`
//! for ELF parsing.

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub addr: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct SymbolTable {
    symbols: Vec<SymbolInfo>,
    pub kernel_base: u64,
}

impl SymbolTable {
    /// Create a SymbolTable from pre-sorted symbol data.
    /// Used by the kallsyms decoder.
    pub fn from_sorted_symbols(symbols: Vec<SymbolInfo>, kernel_base: u64) -> Self {
        SymbolTable { symbols, kernel_base }
    }

    pub fn from_kernel_elf(elf_path: &str, kernel_base: u64) -> Result<Self, String> {
        let buffer = ax_std::fs::read(elf_path)
            .map_err(|e| format!("cannot read {}: {}", elf_path, e))?;
        let elf = goblin::elf::Elf::parse(&buffer)
            .map_err(|e| format!("invalid ELF {}: {}", elf_path, e))?;

        let mut symbols = Vec::new();
        for sym in &elf.syms {
            let st_type = sym.st_type();
            if st_type != goblin::elf::sym::STT_FUNC && st_type != goblin::elf::sym::STT_OBJECT {
                continue;
            }
            if sym.st_value == 0 {
                continue;
            }
            let name = elf.strtab.get_at(sym.st_name).unwrap_or("<unknown>").to_string();
            symbols.push(SymbolInfo { name, addr: sym.st_value, size: sym.st_size });
        }

        symbols.sort_by_key(|s| s.addr);
        symbols.dedup_by_key(|s| s.addr);

        ax_std::println!("[symbol] loaded {} symbols from {}", symbols.len(), elf_path);
        Ok(SymbolTable { symbols, kernel_base })
    }

    /// Load from compact binary format (nm -n extracted function symbols).
    /// Format: u32 count, u64 kernel_base, then count × (u64 addr, u32 name_off),
    /// followed by null-terminated string table.
    pub fn from_compact_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 12 { return Err("symtab: too small".into()); }
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let kernel_base = u64::from_le_bytes([data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11]]);
        let hdr = 12usize; let esz = 12usize;
        let idx_end = hdr + count * esz;
        if data.len() < idx_end { return Err(format!("symtab: truncated")); }
        let mut symbols = Vec::with_capacity(count);
        for i in 0..count {
            let off = hdr + i * esz;
            let addr = u64::from_le_bytes([data[off],data[off+1],data[off+2],data[off+3],data[off+4],data[off+5],data[off+6],data[off+7]]);
            let name_off = u32::from_le_bytes([data[off+8],data[off+9],data[off+10],data[off+11]]) as usize;
            let s = idx_end + name_off;
            let mut e = s;
            while e < data.len() && data[e] != 0 { e += 1; }
            let name = core::str::from_utf8(&data[s..e]).unwrap_or("<invalid>").to_string();
            symbols.push(SymbolInfo { name, addr, size: 0 });
        }
        ax_std::println!("[symbol] loaded {} symbols from embedded symtab ({} KB)", symbols.len(), data.len()/1024);
        Ok(SymbolTable { symbols, kernel_base })
    }

    pub fn lookup(&self, addr: u64) -> Option<&SymbolInfo> {
        let adjusted = addr.wrapping_sub(self.kernel_base);
        let idx = match self.symbols.binary_search_by_key(&adjusted, |s| s.addr) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let sym = &self.symbols[idx];
        // kallsyms does not provide symbol size (size == 0), so we match
        // any address >= sym.addr (up to the next symbol, caught by binary search).
        if adjusted >= sym.addr && (sym.size == 0 || adjusted < sym.addr + sym.size) {
            Some(sym)
        } else {
            None
        }
    }

    pub fn nearest(&self, addr: u64) -> Option<&SymbolInfo> {
        let adjusted = addr.wrapping_sub(self.kernel_base);
        match self.symbols.binary_search_by_key(&adjusted, |s| s.addr) {
            Ok(i) => self.symbols.get(i),
            Err(0) => None,
            Err(i) => self.symbols.get(i - 1),
        }
    }

    /// Look up a symbol by name (linear scan).
    /// Returns the first symbol whose name matches.
    pub fn lookup_name(&self, name: &str) -> Option<&SymbolInfo> {
        self.symbols.iter().find(|s| s.name == name)
    }

    pub fn len(&self) -> usize { self.symbols.len() }
    pub fn is_empty(&self) -> bool { self.symbols.is_empty() }

    /// Look up a symbol by exact name and return its absolute VA.
    /// Returns `None` if not found.
    pub fn find_va(&self, name: &str) -> Option<u64> {
        self.symbols.iter()
            .find(|s| s.name == name)
            .map(|s| self.kernel_base + s.addr)
    }
}
