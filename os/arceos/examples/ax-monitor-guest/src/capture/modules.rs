//! Loaded kernel modules information collection.
//!
//! Reads the target guest's loaded kernel modules from its physical memory
//! via HVC #9 (`CrashReadGuestMem`).
//!
//! # Strategy
//!
//! 1. **kallsyms table** — if a `.kallsyms` symbol is found at a known
//!    address, we read it and extract module symbols (symbols whose names
//!    contain `[module_name]` or that reside outside the main kernel image).
//! 2. **ELF header scan** — scan known GPA ranges for ELF magic bytes
//!    (`\x7fELF`).  Each hit is a loaded module whose section headers
//!    contain the module name.
//! 3. **Fallback** — if neither method succeeds, return an empty list and
//!    record the reason.
//!
//! # Usage in snapshot pipeline
//!
//! ```ignore
//! let mods = modules::collect_modules(TARGET_VM_ID, None);
//! snapshot.modules = mods.modules;
//! ```

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::capture::register;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Information about a single loaded kernel module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// Module name, e.g. `"ext4"`, `"kprint"`.
    pub name: String,
    /// Base guest-virtual address where the module is loaded.
    pub base_addr: u64,
    /// Size of the module in bytes.
    pub size: usize,
}

/// Result of the module collection routine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulesResult {
    /// List of detected modules (may be empty).
    pub modules: Vec<ModuleInfo>,
    /// Human-readable description of how the list was obtained.
    pub method: String,
}

// ---------------------------------------------------------------------------
// Internal constants
// ---------------------------------------------------------------------------

/// Linear mapping offset (GVA → GPA) used by the target kernel.
const PHYS_VIRT_OFFSET: u64 = 0xffff_8000_0000_0000;

/// ELF magic bytes that identify a valid ELF object.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// Minimum reasonable module size (1 page).
const MIN_MODULE_SIZE: u64 = 4096;

/// Maximum reasonable module size (32 MiB).
const MAX_MODULE_SIZE: u64 = 32 * 1024 * 1024;

/// Maximum number of modules to collect (safety limit).
const MAX_MODULES: usize = 64;

/// Kernel-image GPA bounds used to distinguish built-in symbols from
/// dynamically loaded modules.  These must match the target VM config.
const KERNEL_IMAGE_GPA_START: u64 = 0x8020_0000;
const KERNEL_IMAGE_GPA_END: u64 = 0x8030_0000;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Collect the list of loaded kernel modules from the frozen target VM.
///
/// # Arguments
///
/// * `target_vm_id` — frozen target VM ID (usually 1).
/// * `head_gpa`     — optional guest-physical address of the module list /
///                    kallsyms table.  Pass `None` to let the function probe
///                    known GPA ranges automatically.
///
/// # Returns
///
/// `ModulesResult` containing the module list and a description of how it
/// was obtained.  The list may be empty if the kernel has no loaded modules
/// or if the memory could not be read.
pub fn collect_modules(target_vm_id: u64, head_gpa: Option<u64>) -> Result<ModulesResult, String> {
    // ── Strategy A: explicit head address −────────────
    if let Some(gpa) = head_gpa {
        match read_kallsyms(target_vm_id, gpa) {
            Ok(modules) if !modules.is_empty() => {
                    return Ok(ModulesResult {
                    modules,
                    method: "kallsyms (explicit address)".into(),
                });
            }
            _ => {} // fall through
        }
    }

    // ── Strategy B: scan GPA ranges for ELF headers −──
    let scan_regions: &[(u64, u64)] = &[
        // Module loading area (just after the kernel image).
        (KERNEL_IMAGE_GPA_END, 0x0040_0000), // 4 MiB
    ];

    let mut modules: Vec<ModuleInfo> = Vec::new();
    let mut method = String::new();

    for &(base, size) in scan_regions {
        match scan_elf_headers(target_vm_id, base, base + size, &mut modules) {
            Ok(n) if n > 0 => {
                method = format!("ELF header scan at GPA {:#x}+{:#x} ({} modules)", base, size, n);
            }
            Ok(_) => {}
            Err(e) => {
                method = format!("scan failed: {}", e);
            }
        }
    }

    if modules.is_empty() && method.is_empty() {
        method = "no modules found (kernel may have no dynamic modules)".into();
    }

    Ok(ModulesResult { modules, method })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Translate a guest-virtual address to a guest-physical address using the
/// kernel's linear mapping offset.
fn gva_to_gpa(gva: u64) -> u64 {
    if gva >= PHYS_VIRT_OFFSET {
        gva.wrapping_sub(PHYS_VIRT_OFFSET)
    } else {
        gva
    }
}

/// Try to read the kallsyms table at the given GPA.
///
/// kallsyms is a sequence of null-terminated `"symbol_name\n"` entries
/// (`/proc/kallsyms` format).  We scan for lines that contain `[module]`
/// to identify module symbols, then derive unique module names.
fn read_kallsyms(_target_vm_id: u64, _gpa: u64) -> Result<Vec<ModuleInfo>, String> {
    // TODO: implement kallsyms parsing when the target kernel exposes it.
    // This requires knowing the exact address of the kallsyms buffer,
    // which can be obtained from the kernel symbol table.
    //
    // For now, return empty to fall through to the ELF-scan strategy.
    Ok(Vec::new())
}

/// Scan a guest-physical memory range for ELF headers and extract module
/// information from each one found.
fn scan_elf_headers(
    target_vm_id: u64,
    start_gpa: u64,
    end_gpa: u64,
    out: &mut Vec<ModuleInfo>,
) -> Result<usize, String> {
    let chunk_size: u64 = 4096;  // scan page by page
    let mut buf = alloc::vec![0u8; chunk_size as usize];
    let mut found = 0usize;

    let mut gpa = start_gpa;
    while gpa < end_gpa && out.len() < MAX_MODULES {
        let remaining = (end_gpa - gpa) as usize;
        let cur = remaining.min(chunk_size as usize);
        let buf = &mut buf[..cur];

        match register::read_guest_mem(target_vm_id, gpa, buf) {
            Ok(n) if n > 0 => {
                // Scan the returned bytes for ELF magic.
                let data = &buf[..n];
                for (offset, window) in data.windows(4).enumerate() {
                    if window == ELF_MAGIC {
                        // We found an ELF header — extract module info.
                        let elf_gpa = gpa + offset as u64;
                        if let Some(info) = parse_elf_header(target_vm_id, elf_gpa) {
                            // Deduplicate by name.
                            if !out.iter().any(|m| m.name == info.name) {
                                out.push(info);
                                found += 1;
                            }
                        }
                    }
                }
            }
            Ok(_) => {}     // zero bytes read → unmapped page, skip.
            Err(_) => {}    // read error → skip.
        }

        gpa += cur as u64;
    }

    Ok(found)
}

/// Parse a module's ELF header at the given GPA and extract name + size.
///
/// We read the ELF header (64 bytes), locate the section-name string table,
/// then find the `.modinfo` or `.gnu.linkonce.this_module` section to obtain
/// the module name.
fn parse_elf_header(target_vm_id: u64, elf_gpa: u64) -> Option<ModuleInfo> {
    // Read the first 64 bytes (ELF header for AArch64).
    let mut hdr = [0u8; 64];
    register::read_guest_mem(target_vm_id, elf_gpa, &mut hdr).ok()?;

    // Quick sanity: must be ELF64 (EI_CLASS = 2) and AArch64 (e_machine = 0xb7).
    if hdr[4] != 2 || hdr[18] != 0xb7 {
        return None;
    }

    // Parse relevant ELF header fields (little-endian).
    let e_shoff = u64::from_le_bytes(hdr[40..48].try_into().ok()?);  // section header offset
    let e_shentsize = u16::from_le_bytes(hdr[58..60].try_into().ok()?); // section header entry size
    let e_shnum = u16::from_le_bytes(hdr[60..62].try_into().ok()?);     // number of sections
    let e_shstrndx = u16::from_le_bytes(hdr[62..64].try_into().ok()?);  // section name string table index

    if e_shoff == 0 || e_shentsize == 0 || e_shnum == 0 {
        return None;
    }

    // Read the section header string table.
    let shstrtab_hdr_off = elf_gpa.wrapping_add(e_shoff + e_shstrndx as u64 * e_shentsize as u64);
    let mut shstrtab_hdr = [0u8; 64];
    register::read_guest_mem(target_vm_id, shstrtab_hdr_off, &mut shstrtab_hdr).ok()?;

    let shstrtab_off = u64::from_le_bytes(shstrtab_hdr[24..32].try_into().ok()?);
    let shstrtab_size = u64::from_le_bytes(shstrtab_hdr[32..40].try_into().ok()?);

    if shstrtab_off == 0 || shstrtab_size == 0 {
        return None;
    }

    // Read the section name string table.
    let shstrtab_gpa = elf_gpa.wrapping_add(shstrtab_off);
    let mut shstrtab = alloc::vec![0u8; shstrtab_size as usize];
    register::read_guest_mem(target_vm_id, shstrtab_gpa, &mut shstrtab).ok()?;

    // Determine the module's total size: last section's end - base.
    let mut module_end = 0u64;

    // Scan sections for `.modinfo` to extract the module name.
    let mut module_name: Option<String> = None;

    for i in 0..e_shnum {
        let shdr_off = elf_gpa.wrapping_add(e_shoff + i as u64 * e_shentsize as u64);
        let mut shdr = [0u8; 64];
        register::read_guest_mem(target_vm_id, shdr_off, &mut shdr).ok()?;

        let sh_name = u32::from_le_bytes(shdr[0..4].try_into().ok()?) as usize;
        let sh_addr = u64::from_le_bytes(shdr[16..24].try_into().ok()?);   // virtual address
        let sh_size = u64::from_le_bytes(shdr[32..40].try_into().ok()?);    // section size

        // Track the module extent (highest section end).
        if sh_addr > 0 {
            let sect_end = sh_addr.wrapping_add(sh_size);
            if sect_end > module_end {
                module_end = sect_end;
            }
        }

        // Check the section name.
        let sect_name = get_section_name(&shstrtab, sh_name);
        if sect_name == ".modinfo" || sect_name == ".gnu.linkonce.this_module" {
            // Read the section content and look for "name=".
            let sect_gpa = if sh_addr > 0 {
                gva_to_gpa(sh_addr)
            } else {
                elf_gpa.wrapping_add(u64::from_le_bytes(shdr[24..32].try_into().ok()?)) // sh_offset
            };
            let mut sect_data = alloc::vec![0u8; sh_size.min(4096) as usize];
            register::read_guest_mem(target_vm_id, sect_gpa, &mut sect_data).ok()?;

            // Parse "name=module_name\0" from .modinfo.
            if let Some(name) = parse_modinfo_name(&sect_data) {
                module_name = Some(name);
                break;
            }
        }
    }

    // If we didn't find a name via .modinfo, try to use the first section
    // that has an allocated virtual address as a hint (e.g., `.text`).
    let base_addr = elf_gpa;  // we found it in physical space – report the GPA
    let size = if module_end > 0 {
        (module_end - gva_to_gpa(elf_gpa)) as usize
    } else {
        0
    };

    let name = module_name.unwrap_or_else(|| {
        // Fallback: generate a name from the GPA.
        format!("module_{:#x}", elf_gpa)
    });

    // Sanity-check the size.
    if size >= MIN_MODULE_SIZE as usize && size <= MAX_MODULE_SIZE as usize {
        Some(ModuleInfo { name, base_addr, size })
    } else if size == 0 {
        // Zero-sized section table – probably not a real module.
        None
    } else {
        // Suspicious size but report it anyway.
        Some(ModuleInfo { name, base_addr, size })
    }
}

/// Extract the section name from the string table given a name offset.
fn get_section_name(strtab: &[u8], offset: usize) -> &str {
    if offset >= strtab.len() {
        return "";
    }
    let end = strtab[offset..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(strtab.len() - offset);
    core::str::from_utf8(&strtab[offset..offset + end]).unwrap_or("")
}

/// Parse the module name from `.modinfo` section data.
///
/// The `.modinfo` section contains `"key=value\0"` entries.  We look for
/// `"name=module_name"`.
fn parse_modinfo_name(data: &[u8]) -> Option<String> {
    let name_prefix = b"name=";
    for window in data.windows(name_prefix.len() + 1) {
        if window.starts_with(name_prefix) {
            // Read until the next '\0' or end of data.
            let rest = &window[name_prefix.len()..];
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            if end > 0 {
                return core::str::from_utf8(&rest[..end]).ok().map(|s| s.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_section_name() {
        let strtab = b"\0.text\0.data\0.modinfo\0";
        assert_eq!(get_section_name(strtab, 1), ".text");
        assert_eq!(get_section_name(strtab, 7), ".data");
        assert_eq!(get_section_name(strtab, 13), ".modinfo");
        assert_eq!(get_section_name(strtab, 99), "");
    }

    #[test]
    fn test_parse_modinfo_name() {
        let data = b"version=1.0\0name=test_module\0description=test\0";
        assert_eq!(parse_modinfo_name(data), Some("test_module".into()));

        // No name= entry.
        assert_eq!(parse_modinfo_name(b"version=1.0\0"), None);

        // Empty name.
        assert_eq!(parse_modinfo_name(b"name=\0"), None);
    }

    #[test]
    fn test_gva_to_gpa() {
        // High address → subtract offset.
        assert_eq!(gva_to_gpa(0xffff_8000_8020_1234), 0x8020_1234);
        // Already a physical address.
        assert_eq!(gva_to_gpa(0x8020_0000), 0x8020_0000);
        // Zero.
        assert_eq!(gva_to_gpa(0), 0);
    }
}
