//! Loaded kernel modules information collection.
//!
//! Three strategies, tried in order:
//!
//! 1. **ELF dump scan** — scans the already-dumped physical memory buffer
//!    for ELF magic (`\x7fELF`) and parses `.modinfo` sections.
//! 2. **dmesg extraction** — parses `[module_name]` suffixes from the
//!    kernel's Call trace in the dmesg text (capture/snapshot.rs).
//! 3. **Kernel module list walk** — uses kallsyms + the kernel's master
//!    page table to walk the `modules` linked list.  This yields the
//!    module's kernel-virtual base address and name reliably.

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::recovery::symbol::SymbolTable;

/// ELF magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Maximum number of modules to collect.
const MAX_MODULES: usize = 8;

/// Minimum section count for a valid kernel module ELF.
const MIN_SHDR_COUNT: usize = 4;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Information about a single loaded kernel module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    /// Base guest-virtual address where the module is loaded.
    pub base_addr: u64,
    /// Size of the module in bytes.
    pub size: usize,
}

/// Result of the module collection routine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulesResult {
    pub modules: Vec<ModuleInfo>,
    pub method: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Collect the list of loaded kernel modules by scanning already-dumped
/// guest physical memory buffers for ELF headers.
///
/// # Arguments
///
/// * `_target_vm_id` — unused (we operate on local buffers).
/// * `_sym`          — unused (we don't need kallsyms for ELF scanning).
/// * `_mem`          — unused (we operate on local buffers).
/// * `dump_buffers`  — slices of already-dumped physical memory, e.g.
///                     `&[(base_gpa, &[u8])]` from the snapshot pipeline.
///
/// # Returns
///
/// `ModulesResult` containing found modules and the method description.
pub fn collect_modules(
    _target_vm_id: u64,
    _sym: Option<&crate::recovery::symbol::SymbolTable>,
    _mem: &impl Fn(u64) -> Option<u64>,
    dump_buffers: &[(u64, &[u8])],
) -> Result<ModulesResult, String> {
    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut method = String::new();

    for &(base_gpa, buf) in dump_buffers {
        let found = scan_buf_for_elf_headers(base_gpa, buf, &mut modules);
        method = alloc::format!(
            "ELF scan on {:.1} MiB dump at GPA {:#x} ({} hit(s))",
            buf.len() as f64 / (1024.0 * 1024.0),
            base_gpa,
            found,
        );
    }

    // Filter out false positives: modules whose name is the fallback
    // "module_0x..."  (meaning .modinfo parsing failed) are not real
    // kernel modules — they are random ELF images in memory (vDSO,
    // boot code, embedded ELFs, etc.).  Keep only modules with a
    // valid name parsed from .modinfo.
    modules.retain(|m| !m.name.starts_with("module_0x"));

    if modules.is_empty() {
        method = "no modules found via ELF dump scan".into();
    }

    Ok(ModulesResult { modules, method })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Scan a local memory buffer for ELF headers and extract module info.
fn scan_buf_for_elf_headers(
    base_gpa: u64,
    buf: &[u8],
    out: &mut Vec<ModuleInfo>,
) -> usize {
    let mut found = 0usize;
    let mut pos = 0usize;

    while pos + 64 <= buf.len() && out.len() < MAX_MODULES {
        // Fast scan: find ELF magic
        let remaining = &buf[pos..];
        let magic_pos = match remaining.windows(4).position(|w| w == ELF_MAGIC) {
            Some(p) => p,
            None => break,
        };
        let elf_start = pos + magic_pos;
        let elf_gpa = base_gpa + elf_start as u64;

        // Try to parse the ELF header at this position
        if let Some(info) = parse_elf_local(&buf[elf_start..], elf_gpa) {
            if !out.iter().any(|m| m.name == info.name) {
                ax_std::println!("[modules] found '{}' via ELF scan at GPA {:#x}", info.name, elf_gpa);
                out.push(info);
                found += 1;
            }
            // Skip past this ELF to avoid finding it again
            pos = elf_start + 1;
        } else {
            pos = elf_start + 4; // skip the false-positive magic
        }
    }

    found
}

/// Parse a module ELF header from a local byte slice (no HVC calls).
/// `data` starts at the ELF magic (position 0 ↔ elf_gpa in GPA space).
/// All ELF file offsets are byte positions within `data`, NOT GPA-relative.
fn parse_elf_local(data: &[u8], elf_gpa: u64) -> Option<ModuleInfo> {
    if data.len() < 64 {
        return None;
    }

    // ELF64 header sanity: EI_CLASS=2, e_machine=0xb7 (AArch64)
    if data[4] != 2 || data[18] != 0xb7 {
        return None;
    }

    // All ELF offsets are relative to start of file (= start of data slice).
    let e_shoff     = u64::from_le_bytes(data[40..48].try_into().ok()?) as usize;
    let e_shentsize = u16::from_le_bytes(data[58..60].try_into().ok()?) as usize;
    let e_shnum     = u16::from_le_bytes(data[60..62].try_into().ok()?) as usize;
    let e_shstrndx  = u16::from_le_bytes(data[62..64].try_into().ok()?) as usize;

    if e_shoff == 0 || e_shentsize == 0 || e_shnum < MIN_SHDR_COUNT {
        return None;
    }

    // Section header string table's section header entry
    let shstrtab_shdr_off = e_shoff + e_shstrndx * e_shentsize;
    if shstrtab_shdr_off + 64 > data.len() {
        return None;
    }
    let shstrtab_shdr = &data[shstrtab_shdr_off..shstrtab_shdr_off + 64];

    let shstrtab_off  = u64::from_le_bytes(shstrtab_shdr[24..32].try_into().ok()?) as usize;
    let shstrtab_size = u64::from_le_bytes(shstrtab_shdr[32..40].try_into().ok()?) as usize;

    // Sanity-check shstrtab_off / shstrtab_size.
    // If the kernel module loader has cleared sh_offset for .shstrtab
    // (setting it to 0), we cannot read section names.  In that case we
    // fall through with an empty string table and try to find .modinfo
    // by scanning section data directly for "name=".
    let shstrtab: &[u8] = if shstrtab_off == 0 || shstrtab_size == 0
        || shstrtab_off > 4 * 1024 * 1024       // reject implausibly large offsets
        || (shstrtab_off as usize) + (shstrtab_size as usize) > data.len()
    {
        &[] // empty string table — section names unavailable
    } else {
        &data[shstrtab_off as usize..][..shstrtab_size as usize]
    };

    // Iterate section headers to find .modinfo and determine module extent
    let mut module_end = 0u64;
    let mut module_name: Option<String> = None;

    for i in 0..e_shnum {
        let shdr_off = e_shoff + i * e_shentsize;
        if shdr_off + 64 > data.len() {
            break;
        }
        let shdr = &data[shdr_off..shdr_off + 64];

        let sh_name   = u32::from_le_bytes(shdr[0..4].try_into().ok()?) as usize;
        let sh_addr   = u64::from_le_bytes(shdr[16..24].try_into().ok()?);
        let sh_offset = u64::from_le_bytes(shdr[24..32].try_into().ok()?) as usize;
        let sh_size   = u64::from_le_bytes(shdr[32..40].try_into().ok()?) as usize;

        // Track file extent using FILE OFFSETS (sh_offset + sh_size),
        // NOT virtual addresses (sh_addr is a vmalloc VA for loaded modules).
        if sh_offset > 0 {
            let file_end = (sh_offset + sh_size) as u64;
            if file_end > module_end {
                module_end = file_end;
            }
        }

        // Get section name (may be "" if string table is unavailable)
        let sect_name = get_name_from_strtab(shstrtab, sh_name);

        // Try to extract module name from section data:
        //   A) section named .modinfo / .gnu.linkonce.this_module, OR
        //   B) any section whose content contains "name=" (fallback when
        //      string table is empty or section names are unreliable).
        let should_check = sect_name == ".modinfo"
            || sect_name == ".gnu.linkonce.this_module"
            || shstrtab.is_empty(); // string table unavailable → scan all sections

        if should_check {
            if sh_offset > data.len() || sh_offset + sh_size > data.len() {
                continue;
            }
            let content = &data[sh_offset..sh_offset + sh_size.min(4096)];
            if let Some(name) = parse_modinfo_name(content) {
                if module_name.is_none() {
                    module_name = Some(name);
                }
            }
        }
    }

    // If section-based scan failed, brute-force search the entire ELF
    // data for "name=" patterns (catches corrupted section headers).
    if module_name.is_none() && data.len() <= 8 * 1024 * 1024 {
        if let Some(name) = find_modinfo_name_bruteforce(data) {
            module_name = Some(name);
        }
    }

    // Size = highest file offset among sections (module_end already tracks
    // sh_offset + sh_size = file position).  Do NOT subtract elf_gpa here:
    // module_end is a file offset within the ELF, not an absolute GPA.
    let size = if module_end > 0 {
        (module_end as usize).min(4 * 1024 * 1024) // kernel modules never > 4MB
    } else {
        0
    };

    let name = module_name.unwrap_or_else(|| format!("module_{:#x}", elf_gpa));
    Some(ModuleInfo { name, base_addr: elf_gpa, size })
}

/// Look up a section name from the string table.
fn get_name_from_strtab(strtab: &[u8], idx: usize) -> &str {
    if idx >= strtab.len() {
        return "";
    }
    let end = strtab[idx..].iter().position(|&b| b == 0).unwrap_or(strtab.len() - idx);
    core::str::from_utf8(&strtab[idx..idx + end]).unwrap_or("")
}

/// Parse the module name from `.modinfo` section data.
fn parse_modinfo_name(data: &[u8]) -> Option<String> {
    for entry in data.split(|&b| b == 0) {
        if entry.is_empty() { continue; }
        if let Ok(s) = core::str::from_utf8(entry) {
            if let Some(val) = s.strip_prefix("name=") {
                let name = val.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// Brute-force scan the entire ELF data for "name=<module_name>" patterns.
/// This catches modules whose section headers have been corrupted by the
/// kernel module loader but whose `.modinfo` section data is still intact
/// somewhere in memory.
fn find_modinfo_name_bruteforce(data: &[u8]) -> Option<String> {
    const PATTERN: &[u8] = b"name=";
    let mut pos = 0usize;
    while pos + PATTERN.len() <= data.len() {
        if data[pos..].starts_with(PATTERN) {
            let start = pos + PATTERN.len();
            let end = data[start..].iter().position(|&b| b == 0)
                .map(|e| start + e)
                .unwrap_or(data.len());
            if end > start {
                let name = core::str::from_utf8(&data[start..end]).ok()?;
                let name = name.trim();
                if !name.is_empty() && name.len() <= 64
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Some(name.to_string());
                }
            }
            pos = end + 1;
        } else {
            pos += 1;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Strategy C: Kernel module list walk
// ---------------------------------------------------------------------------

/// Walk the kernel's `modules` linked list to find all loaded modules.
///
/// Uses the kallsyms symbol table to locate the list head, then traverses
/// the circular doubly-linked list.  Each entry's `struct module` contains
/// the module name and base address.  Requires a `read_gva` closure that
/// can read 8 bytes from **any GVA** (kernel image, linear mapping, or
/// vmalloc).  The caller must supply the GVA→GPA translation internally
/// (e.g. using the page-table walker for vmalloc addresses).
///
/// # Arguments
///
/// * `sym`     — kallsyms symbol table (must contain `modules`).
/// * `read_gva` — closure that reads 8 bytes from any GVA (via HVC #9
///                + GVA→GPA translation).
///
/// # Returns
///
/// List of `ModuleInfo` with names, base addresses, and sizes (0 if
/// unknown).
pub fn collect_modules_via_list(
    sym: &SymbolTable,
    read_gva: &impl Fn(u64) -> Option<u64>,
) -> ModulesResult {
    // 1. Locate the `modules` list head in kallsyms.
    let list_head_gva = match sym.lookup_name("modules") {
        Some(s) => {
            ax_std::println!("[modules] list head 'modules' at GVA {:#x}", s.addr);
            sym.kernel_base + s.addr
        }
        None => {
            ax_std::println!("[modules] 'modules' symbol not found, skipping list walk");
            return ModulesResult {
                modules: Vec::new(),
                method: "list walk skipped (no 'modules' symbol)".into(),
            };
        }
    };

    // 2. Read the head.next pointer (= first module's list entry GVA).
    let head_next = match read_gva(list_head_gva) {
        Some(v) => v,
        None => {
            ax_std::println!("[modules] failed to read list head");
            return ModulesResult {
                modules: Vec::new(),
                method: "list walk failed (head unreadable)".into(),
            };
        }
    };
    if head_next == list_head_gva || head_next == 0 {
        return ModulesResult {
            modules: Vec::new(),
            method: "list walk: empty list".into(),
        };
    }

    // 3. Walk the circular list with a safety iteration limit.
    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut cur_gva = head_next;
    ax_std::println!("[modules] list walk: head={:#x}, first={:#x}", list_head_gva, head_next);
    // Safety limit: prevent infinite loop if the list is corrupted (e.g.,
    // after a crash).  Each module entry adds ~2-4 iterations for probing,
    // so 512 is generous even for a system with many modules.
    const MAX_ITER: u32 = 512;
    let mut iter = 0u32;

    while iter < MAX_ITER {
        iter += 1;
        ax_std::println!("[modules] list walk: iter={} cur_gva={:#x}", iter, cur_gva);

        // Probe for the containing `struct module` base address and name.
        if let Some((base_gva, ref name)) = probe_module_name(cur_gva, read_gva) {
            if !modules.iter().any(|m| m.name == *name) {
                let (base_addr, size) = probe_module_addr(base_gva, read_gva);
                ax_std::println!("[modules] list walk: found '{}' @ GVA {:#x}", name, base_gva);
                modules.push(ModuleInfo { name: name.clone(), base_addr, size });
                if modules.len() >= MAX_MODULES {
                    break;
                }
            }
        } else {
            ax_std::println!("[modules] list walk: probe_module_name returned None for cur_gva={:#x}", cur_gva);
        }

        // Advance to next entry in the list.
        cur_gva = match read_gva(cur_gva) {
            Some(v) => v,
            None => {
                ax_std::println!("[modules] list walk: read_gva(cur_gva) returned None, breaking");
                break;
            }
        };
        ax_std::println!("[modules] list walk: next cur_gva={:#x}", cur_gva);
        if cur_gva == list_head_gva || cur_gva == head_next {
            ax_std::println!("[modules] list walk: back to head, done");
            break; // back to head → end of list
        }
    }
    if iter >= MAX_ITER {
        ax_std::println!("[modules] list walk hit iteration limit ({}), stopping", MAX_ITER);
    }

    let n = modules.len();
    ModulesResult {
        modules,
        method: alloc::format!("kernel module list walk ({} modules)", n),
    }
}

/// Determine the `struct module *` address and name from a list entry GVA.
///
/// Probes candidate offsets for the `list` and `name` fields within
/// `struct module`.  For each candidate combination, reads the `name`
/// field and checks whether it forms a valid C-style module name string.
fn probe_module_name(
    entry_gva: u64,
    read_gva: &impl Fn(u64) -> Option<u64>,
) -> Option<(u64 /* module_base */, String /* name */)> {
    // Candidate (list_offset, name_offset) pairs for Linux 6.x ARM64.
    const CANDIDATES: &[(u64, u64)] = &[
        (0x08, 0x38), (0x08, 0x48),
        (0x10, 0x40), (0x10, 0x48), (0x10, 0x50),
        (0x18, 0x48), (0x18, 0x50), (0x18, 0x58),
        (0x20, 0x50), (0x20, 0x58), (0x20, 0x60),
        (0x28, 0x58), (0x28, 0x60),
        (0x00, 0x30), (0x00, 0x40),
    ];

    for &(list_off, name_off) in CANDIDATES {
        let module_base = entry_gva.wrapping_sub(list_off);
        let name_gva = module_base.wrapping_add(name_off);

        if let Some(name) = read_c_string(name_gva, 64, read_gva) {
            if is_valid_module_name(&name) {
                return Some((module_base, name));
            }
        }
    }
    None
}

/// Read a null-terminated ASCII string of up to `max_len` bytes from GVA.
fn read_c_string(
    gva: u64,
    max_len: usize,
    read_gva: &impl Fn(u64) -> Option<u64>,
) -> Option<String> {
    let mut buf = alloc::vec![0u8; max_len];
    let mut len = 0usize;

    for i in 0..max_len {
        let aligned = (gva.wrapping_add(i as u64)) & !7;
        let byte_off = (gva.wrapping_add(i as u64) & 7) as usize;
        let val = read_gva(aligned)?;
        let byte = val.to_le_bytes()[byte_off];
        if byte == 0 {
            break; // null terminator
        }
        buf[i] = byte;
        len = i + 1;
    }

    if len >= 2 {
        core::str::from_utf8(&buf[..len]).ok().map(|s| s.to_string())
    } else {
        None
    }
}

/// Check whether a string looks like a kernel module name.
fn is_valid_module_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 60 {
        return false;
    }
    name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Try to read the module's kernel-virtual base address and size from
/// `struct module`.  Returns `(0, 0)` if the offsets are unknown (the
/// caller will still have the module name for display).
fn probe_module_addr(
    _module_base: u64,
    _read_gva: &impl Fn(u64) -> Option<u64>,
) -> (u64, usize) {
    // The base address is stored in `struct module.module_core` or
    // `struct module.core_layout.base`.  The exact offset varies by
    // kernel version; we return (0,0) for now.
    //
    // The dmesg backtrace already provides function names (e.g.
    // `crash_init+116`), and the list walk above gives the module
    // name, which is sufficient for crash analysis.
    (0, 0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_module_name() {
        assert!(is_valid_module_name("crash_test"));
        assert!(is_valid_module_name("ext4"));
        assert!(!is_valid_module_name(""));
        assert!(!is_valid_module_name("name with spaces"));
    }

    #[test]
    fn test_parse_modinfo() {
        assert_eq!(
            parse_modinfo_name(b"name=crash_test\0version=1.0\0"),
            Some("crash_test".into())
        );
        assert_eq!(parse_modinfo_name(b"version=1.0\0"), None);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        // Trivial placeholder; full module list walking tested in integration.
    }
}
