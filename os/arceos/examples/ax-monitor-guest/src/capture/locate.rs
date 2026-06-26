//! Dynamic kernel HPA resolution.
//!
//! Uses a hardcoded HPA_BASE fallback.  Dynamic scanning was removed
//! because kernel alternatives patching modifies in-memory .text,
//! making ELF-based fingerprints unreliable at runtime.
//!
//! To support a different HPA_BASE, update `HPA_BASE_FALLBACK` below.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const HPA_BASE_FALLBACK: u64 = 0x2_2360_0000;
const TEXT_OFFSET: u64 = 0x20_0000;

static HPA_BASE: AtomicU64 = AtomicU64::new(HPA_BASE_FALLBACK);
static LOCATED: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy)]
pub struct KernelLocation { pub hpa_base: u64 }

/// Hardcoded kernel symbol VA offsets (VA - 0xffff800080000000).
pub mod offsets {
    pub const __LOG_BUF: u64         = 0x1647fb8;
    pub const PRINTK_RB_STATIC: u64  = 0x1408110;
    pub const KALLSYMS_NUM_SYMS: u64      = 0x0d605c8;
    pub const KALLSYMS_NAMES: u64         = 0x0d605d0;
    pub const KALLSYMS_MARKERS: u64       = 0x0e79ec0;
    pub const KALLSYMS_TOKEN_TABLE: u64   = 0x0e7a3f8;
    pub const KALLSYMS_TOKEN_INDEX: u64   = 0x0e7a788;
    pub const KALLSYMS_OFFSETS: u64       = 0x0e7a988;
    pub const KALLSYMS_RELATIVE_BASE: u64 = 0x0ecde30;
}

pub fn offset_to_hpa(offset: u64) -> u64 {
    HPA_BASE.load(Ordering::Relaxed) + offset + TEXT_OFFSET
}

pub fn locate_kernel(_target_vm_id: u64) -> Option<KernelLocation> {
    let hpa_base = HPA_BASE_FALLBACK;
    ax_std::println!("[locate] hpa_base={:#x} (hardcoded)", hpa_base);
    Some(KernelLocation { hpa_base })
}

pub fn va_to_hpa(va: u64) -> u64 {
    offset_to_hpa(va - 0xffff_8000_8000_0000u64)
}
