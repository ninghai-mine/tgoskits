//! Dynamic kernel HPA resolution via kallsyms_num_syms probing.
//!
//! HPA_BASE changes every axvisor run.  We locate the kernel by
//! probing candidate HPAs for `kallsyms_num_syms`, which has a
//! known value (~46797).  Once found, HPA_BASE is computed and
//! all other GPAs are derived via offset_to_hpa().

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::capture::register;

/// Expected kallsyms_num_syms value range (sane kernel).
const MIN_NUM_SYMS: u32 = 10000;
const MAX_NUM_SYMS: u32 = 1_000_000;

/// Probe range: scan ±16 MiB around the last known HPA_BASE.
const PROBE_STEP: u64 = 0x20_0000;       // 2 MiB
const PROBE_HALF_RANGE: u64 = 16 * PROBE_STEP;  // ±32 MiB

/// Hardcoded kernel symbol VA offsets (VA - 0xffff800080000000).
pub mod offsets {
    pub const __LOG_BUF: u64         = 0x1647fb8;
    pub const PRINTK_RB_STATIC: u64  = 0x1408110;
    pub const KALLSYMS_NUM_SYMS: u64 = 0x0d605c8;
    pub const KALLSYMS_NAMES: u64    = 0x0d605d0;
    pub const KALLSYMS_MARKERS: u64  = 0x0e79ec0;
    pub const KALLSYMS_TOKEN_TABLE: u64  = 0x0e7a3f8;
    pub const KALLSYMS_TOKEN_INDEX: u64  = 0x0e7a788;
    pub const KALLSYMS_OFFSETS: u64      = 0x0e7a988;
    pub const KALLSYMS_RELATIVE_BASE: u64 = 0x0ecde30;
}

static HPA_BASE: AtomicU64 = AtomicU64::new(0);
static LOCATED: AtomicBool = AtomicBool::new(false);

pub fn get_hpa_base() -> Option<u64> {
    if LOCATED.load(Ordering::Acquire) { Some(HPA_BASE.load(Ordering::Relaxed)) } else { None }
}

pub fn offset_to_hpa(offset: u64) -> u64 {
    let base = if LOCATED.load(Ordering::Acquire) {
        HPA_BASE.load(Ordering::Relaxed)
    } else {
        0x223800000  // fallback
    };
    base + offset
}

/// Scan for kallsyms_num_syms to find HPA_BASE.
/// Returns true if HPA_BASE was located.
pub fn locate_kernel(target_vm_id: u64) -> bool {
    if LOCATED.load(Ordering::Acquire) {
        return true;
    }

    let num_syms_offset = offsets::KALLSYMS_NUM_SYMS;
    let candidates = [
        0x223800000u64,
        0x223a00000u64,
        0x223600000u64,
        0x224000000u64,
        0x223000000u64,
    ];

    for &base in &candidates {
        let hpa = base + num_syms_offset;
        if probe_num_syms(target_vm_id, hpa) {
            store(base, hpa);
            return true;
        }
    }

    let center = candidates[0];
    let start = center.saturating_sub(PROBE_HALF_RANGE);
    let end = center.saturating_add(PROBE_HALF_RANGE + 0x10000000);
    ax_std::println!("[locate] scanning [{:#x}, {:#x}) step={:#x}...", start, end, PROBE_STEP);
    let mut count = 0u32;
    for base in (start..end).step_by(PROBE_STEP as usize) {
        count += 1;
        if count % 16 == 0 { ax_std::println!("[locate]   {} candidates...", count); }
        let hpa = base + num_syms_offset;
        if probe_num_syms(target_vm_id, hpa) {
            store(base, hpa);
            return true;
        }
    }

    ax_std::println!("[locate] FAILED — HPA_BASE not found");
    false
}

fn probe_num_syms(target_vm_id: u64, hpa: u64) -> bool {
    let mut buf = [0u8; 4];
    match register::read_guest_mem(target_vm_id, hpa, &mut buf) {
        Ok(4) => {
            let val = u32::from_le_bytes(buf);
            val >= MIN_NUM_SYMS && val <= MAX_NUM_SYMS
        }
        _ => false,
    }
}

fn store(hpa_base: u64, found_at: u64) {
    HPA_BASE.store(hpa_base, Ordering::Relaxed);
    LOCATED.store(true, Ordering::Release);
    ax_std::println!("[locate] HPA_BASE={:#x} (probe at {:#x})", hpa_base, found_at);
}

pub fn va_to_hpa(va: u64) -> u64 {
    offset_to_hpa(va - 0xffff_8000_8000_0000u64)
}
