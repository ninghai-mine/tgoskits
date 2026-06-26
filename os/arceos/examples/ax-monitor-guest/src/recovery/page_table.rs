//! ARM64 VMSAv8-64 page table walker.
//!
//! Translates a Guest Virtual Address (GVA) to a physical address (GPA) by
//! walking the kernel's 4-level page table.  This enables reading vmalloc
//! addresses (kernel stacks, module memory) which cannot be translated with
//! the simple linear-mapping / kernel-image formulas.
//!
//! # Page table layout (ARM64, 48-bit VA, 4KB pages, 4 levels)
//!
//! ```text
//! VA bit:  47    39    30    21    12   0
//!          ┌──┬──┬──┬──┬──┬──┬──┬──┬──┬──┐
//!          │PGD│ PUD│ PMD│ PTE│ offset │
//!          │ 9b│ 9b │ 9b │ 9b │  12b  │
//!          └──┴──┴──┴──┴──┴──┴──┴──┴──┴──┘
//! ```
//!
//! Each level indexes a 4 KB table (512 entries × 8 bytes).
//!
//! # Descriptor format (bits[1:0])
//!
//! | bits[1:0] | Level 0-2       | Level 3      |
//! |-----------|-----------------|--------------|
//! | 00        | Invalid (fault) | Invalid      |
//! | 01        | Table (next lv) | Reserved     |
//! | 10        | Block (1G/2M)   | Reserved     |
//! | 11        | Reserved        | Page (4 KB)  |

extern crate alloc;
use alloc::format;
use alloc::string::String;

/// Maximum number of page-table levels on ARM64 with 48-bit VA / 4 KB pages.
const LEVELS: usize = 4;

/// Translate a Guest Virtual Address to a Guest Physical Address by walking
/// the kernel's 4-level page table.
///
/// # Arguments
///
/// * `gva`             — virtual address to translate.
/// * `pgd_pa`          — physical address (GPA) of the PGD (Level 0) table.
/// * `read_64`         — closure that reads 8 bytes from a GPA (via HVC #9).
///
/// # Returns
///
/// `Some(gpa)` on success, `None` if the address is unmapped or invalid.
pub fn gva_to_gpa(
    gva: u64,
    pgd_pa: u64,
    read_64: &impl Fn(u64) -> Option<u64>,
) -> Option<u64> {
    // Only handle kernel-space addresses (we use the kernel's swapper_pg_dir).
    if gva < 0xffff_0000_0000_0000u64 {
        return None;
    }

    let mut table_pa = pgd_pa;

    for level in 0..LEVELS {
        // Shift amount for this level:
        //   Level 0 (PGD): 39 bits (bits 47:39)
        //   Level 1 (PUD): 30 bits (bits 38:30)
        //   Level 2 (PMD): 21 bits (bits 29:21)
        //   Level 3 (PTE): 12 bits (bits 20:12)
        let shift = 39 - (level * 9);
        let idx = ((gva >> shift) & 0x1FF) as usize; // 9-bit index (0..511)

        let entry_pa = table_pa.wrapping_add((idx * 8) as u64);
        let pte = read_64(entry_pa)?;

        let desc_type = pte & 0b11;
        ax_std::println!("[page_table] lvl{} idx={:3} entry_pa={:#x} pte={:#018x} type={}", 
            level, idx, entry_pa, pte, desc_type);

        match desc_type {
            0 => {
                ax_std::println!("[page_table]   → invalid (level {}), stopping", level);
                return None;
            }
            1 => {
                if level >= LEVELS - 1 {
                    return None;
                }
                table_pa = pte & 0xFFFF_FFFF_F000u64;
                ax_std::println!("[page_table]   → table, next PA={:#x}", table_pa);
            }
            2 => {
                if level == 0 || level >= LEVELS - 1 {
                    return None;
                }
                let block_size = if level == 1 { 1u64 << 30 } else { 1u64 << 21 };
                let pa = (pte & !(block_size - 1)) | (gva & (block_size - 1));
                ax_std::println!("[page_table]   → block ({}), GPA={:#x}", 
                    if level == 1 { "1GiB" } else { "2MiB" }, pa);
                return Some(pa);
            }
            3 => {
                if level != LEVELS - 1 { return None; }
                let pa = (pte & 0xFFFF_FFFF_F000u64) | (gva & 0xFFF);
                ax_std::println!("[page_table]   → page (4KiB), GPA={:#x}", pa);
                return Some(pa);
            }
            _ => unreachable!(),
        }
    }

    None
}

/// Find the physical address of the kernel's master page table (PGD) from
/// the kallsyms symbol table.
///
/// Looks up `swapper_pg_dir` first (standard ARM64 kernel symbol), then
/// falls back to `init_mm` → `pgd` field.
///
/// # Returns
///
/// `Ok(pgd_pa)` on success, `Err(reason)` if the symbol is not found.
pub fn find_pgd_pa(
    sym_table: &crate::recovery::symbol::SymbolTable,
    read_64: &impl Fn(u64) -> Option<u64>,
) -> Result<u64, String> {
    let kernel_base = sym_table.kernel_base;

    // Debug: search for any page-table related symbol in the first 1000 entries
    // to understand what names are available.
    for name in &["swapper_pg_dir", "idmap_pg_dir", "tramp_pg_dir", "init_mm",
                  "pgd", "pud", "pmd", "pte"] {
        if sym_table.lookup_name(name).is_some() {
            ax_std::println!("[page_table] found symbol '{}' in kallsyms", name);
        }
    }

    // Try swapper_pg_dir first (kernel's master page table).
    if let Some(sym) = sym_table.lookup_name("swapper_pg_dir") {
        let pgd_gva = kernel_base + sym.addr;
        let pgd_gpa = pgd_gva_to_gpa(pgd_gva)?;
        ax_std::println!("[page_table] swapper_pg_dir @ GVA {:#x} → GPA {:#x}", pgd_gva, pgd_gpa);
        return Ok(pgd_gpa);
    }

    // Fallback: read init_mm.pgd (the pgd pointer stored in init_mm).
    if let Some(sym) = sym_table.lookup_name("init_mm") {
        let mm_gva = kernel_base + sym.addr;
        let mm_gpa = pgd_gva_to_gpa(mm_gva)?;
        ax_std::println!("[page_table] init_mm @ GPA {:#x}, probing pgd offset...", mm_gpa);

        // struct mm_struct.pgd offset — varies by kernel version.
        // Common offsets for ARM64 Linux 6.x: 0x00, 0x08, 0x10, 0x28, 0x40, 0x48.
        let pgd_candidates: &[u64] = &[0x00, 0x08, 0x10, 0x28, 0x38, 0x40, 0x48, 0x60, 0x68];
        for &off in pgd_candidates {
            let field_gpa = mm_gpa.wrapping_add(off);
            if let Some(val) = read_64(field_gpa) {
                let pa = val & 0xFFFF_FFFF_F000u64;
                // Sanity: valid page table must be 4K-aligned in RAM range.
                // Target VM RAM is at 0x223600000 .. 0x233600000 (256 MiB).
                if pa >= 0x223600000 && pa < 0x233600000 {
                    ax_std::println!("[page_table] init_mm.pgd @ offset {:#x} → GPA {:#x}", off, pa);
                    return Ok(pa);
                }
            }
        }
        return Err("init_mm found but pgd offset not determined".into());
    }

    Err("neither swapper_pg_dir nor init_mm found in kallsyms".into())
}

/// Translate a kernel-image GVA to GPA using the standard formula.
fn pgd_gva_to_gpa(pgd_gva: u64) -> Result<u64, String> {
    const KIMAGE_VADDR: u64 = 0xffff_8000_8000_0000;
    const DUMP_BASE: u64 = 0x223600000; // MEMORY_REGIONS[0].0

    if pgd_gva >= KIMAGE_VADDR && pgd_gva < KIMAGE_VADDR + 0x8000_0000 {
        Ok(pgd_gva - KIMAGE_VADDR + DUMP_BASE)
    } else if pgd_gva >= 0xffff_0000_0000_0000u64 {
        // Linear mapping fallback (should not happen for PGD, but be safe).
        let guest_pa = pgd_gva.wrapping_sub(0xffff_0000_0000_0000u64);
        Ok(guest_pa.wrapping_sub(0x8000_0000).wrapping_add(DUMP_BASE))
    } else {
        Err(format!("pgd GVA {:#x} is not in kernel image range", pgd_gva))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gva_to_gpa_invalid() {
        // User-space address → should return None
        let result = gva_to_gpa(0x400000, 0x1000, &|_| None);
        assert!(result.is_none());
    }
}
