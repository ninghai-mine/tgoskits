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
}
