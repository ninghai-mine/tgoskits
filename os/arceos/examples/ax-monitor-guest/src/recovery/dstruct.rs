//! Critical kernel data structure sanity checks.
//!
//! Performs heuristic checks on key kernel data structures and CPU state
//! to detect corruption, deadlocks, or abnormal conditions.  Each check
//! is independent and gracefully degrades to `None` when the required
//! data is unavailable.
//!
//! # Design notes (ARM64 Linux)
//!
//! - **FP vs SP_EL0**: ARM64 Linux uses `SP_EL1` as the kernel stack pointer
//!   (`EL1h` mode).  Our register dump (`VcpuRegsEntry`) only captures
//!   `SP_EL0` (user stack) and the GPRs.  **FP (x29)** is the frame pointer
//!   and always points into the kernel stack when in kernel mode — we use it
//!   as a proxy for the kernel stack position.
//!
//! - **current_task**: On ARM64, the kernel derives the current `task_struct`
//!   pointer from `SP_EL1` alone:
//!   ```c
//!   current = (struct task_struct *)(SP_EL1 & ~(THREAD_SIZE - 1));
//!   ```
//!   Since we lack `SP_EL1`, we use FP(x29) as an approximation (FP is
//!   always inside the kernel stack, so `FP & ~(THREAD_SIZE-1)` gives the
//!   same `task_struct` base).
//!
//! - **thread_info** is embedded at offset 0 of `task_struct`.
//!   ```c
//!   struct thread_info {
//!       unsigned long flags;        // offset 0
//!       u64           preempt_count;// offset 8
//!       u32           cpu;          // offset 16
//!   };
//!   ```
//!
//! # Checks performed
//!
//! 1. **FP validity** — is the frame pointer in the kernel linear map?
//! 2. **Preempt count** — read `thread_info.preempt_count` at the derived
//!    `current` address; non-zero means atomic / interrupt-disabled context.
//! 3. **Interrupt state** — were IRQ/FIQ masked at crash time (SPSR bits)?
//! 4. **Exception nesting** — did a second exception occur within a handler?
//! 5. **Stack alignment** — is FP 16-byte aligned (AAPCS64)?

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::capture::storage::VcpuRegsEntry;
use crate::recovery::symbol::SymbolTable;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants (must match target kernel config)
// ---------------------------------------------------------------------------

/// Linear mapping offset (GVA → GPA).
/// Linux ARM64 with 48-bit VA: PAGE_OFFSET = 0xffff_0000_0000_0000
const PAGE_OFFSET: u64 = 0xffff_0000_0000_0000;

/// Kernel linear mapping range (48-bit VA).
/// GPA = GVA - PAGE_OFFSET for addresses in this range.
/// Covers up to 512 GB of physical RAM.
const KERNEL_LINEAR_LOW: u64  = PAGE_OFFSET;
const KERNEL_LINEAR_HIGH: u64 = 0xffff_8000_0000_0000;

/// Vmalloc / kernel image region (48-bit VA).
/// With CONFIG_VMAP_STACK=y, kernel stacks are allocated from vmalloc.
/// Addresses here CANNOT be converted to GPA by simple subtraction;
/// they require a page-table walk (not implemented).
const KERNEL_VMALLOC_LOW: u64  = 0xffff_8000_0000_0000;
const KERNEL_VMALLOC_HIGH: u64 = 0xffff_8001_0000_0000;

/// User space address range (48-bit VA, TTBR0).
const USER_SPACE_LOW: u64  = 0x0000_0000_0000_0000;
const USER_SPACE_HIGH: u64 = 0x0000_ffff_ffff_f000;

/// ARM64 kernel stack size (THREAD_SIZE).
/// For 4 KB pages without KASAN: THREAD_SHIFT = 14 → 16 KB.
const THREAD_SIZE: u64 = 0x4000;
const THREAD_MASK: u64 = !(THREAD_SIZE - 1); // 0xffff_ffff_ffff_c000

/// Offsets within `thread_info` (embedded at offset 0 of `task_struct`).
const TI_PREEMPT_COUNT: u64 = 8;  // u64 preempt_count

/// SPSR_EL1 bit positions.
const SPSR_IRQ_MASK: u64 = 1 << 7;
const SPSR_FIQ_MASK: u64 = 1 << 6;
const SPSR_EL_MASK: u64  = 0xF;

/// Exception class values (ESR_EL1 bits [31:26]).
const EC_DATA_ABORT_LOWER_EL: u64 = 0x24;
const EC_DATA_ABORT_CURRENT_EL: u64 = 0x25;
const EC_INST_ABORT_LOWER_EL: u64 = 0x20;
const EC_INST_ABORT_CURRENT_EL: u64 = 0x21;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of all data-structure sanity checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DstructResult {
    /// Whether the frame pointer (FP/x29) points inside the kernel stack region.
    pub sp_in_stack: Option<bool>,
    /// Whether the derived `current` task_struct address is valid (preempt_count
    /// looks reasonable).
    pub current_task_valid: Option<bool>,
    /// Whether interrupts were masked (SPSR.I = 1).
    pub irqs_masked: Option<bool>,
    /// Whether this looks like a nested exception.
    pub exception_nested: Option<bool>,
    /// Whether SP is 16-byte aligned.
    pub sp_aligned: Option<bool>,
    /// Human-readable descriptions for each check.
    pub details: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run all data-structure sanity checks on a crashed vCPU.
///
/// # Arguments
///
/// * `regs` — Register state of the crashed vCPU.
/// * `sym`  — Optional kernel symbol table (used for preempt_count validation).
/// * `mem`  — Memory reader closure (GPA → 8 bytes).
/// * `eff_esr` — Optional real ESR value.  When the hypervisor does not trap
///   Data Aborts from EL1, `regs.esr_el1` is 0.  Pass the dmesg-extracted ESR
///   here so exception-nesting detection sees the real exception class.
pub fn check_dstructs(
    regs: &VcpuRegsEntry,
    _sym: Option<&SymbolTable>,
    mem: &impl Fn(u64) -> Option<u64>,
    eff_esr: Option<u64>,
) -> DstructResult {
    let mut details = Vec::new();

    // On ARM64 Linux, FP (x29) is the frame pointer — always in the kernel
    // stack when the CPU was in kernel mode.  SP_EL0 is the user stack
    // pointer and is NOT a valid proxy for the kernel stack.
    let fp = regs.gpr[29];  // x29 = frame pointer

    // ── 1. FP validity (proxy for kernel stack pointer) ──
    let sp_in_stack = check_fp_validity(fp, &mut details);

    // ── 2. Stack alignment (AAPCS64 requires 16-byte alignment) ──
    let sp_aligned = check_alignment(fp, &mut details);

    // ── 3. Current-task derivation + preempt_count ──
    let current_task_valid = check_current_task(fp, mem, &mut details);

    // ── 4. Interrupt state (SPSR bits) ──
    let irqs_masked = check_interrupt_state(regs.spsr_el1, &mut details);

    // ── 5. Exception nesting ──
    // Use the effective ESR (from dmesg fallback) if available, because
    // regs.esr_el1 may be 0 when the hypervisor didn't trap the Data Abort.
    let esr_for_nesting = eff_esr.unwrap_or(regs.esr_el1);
    let exception_nested = check_exception_nesting(
        regs.spsr_el1, esr_for_nesting, regs.elr_el1, &mut details,
    );

    DstructResult {
        sp_in_stack,
        current_task_valid,
        irqs_masked,
        exception_nested,
        sp_aligned,
        details,
    }
}

// ---------------------------------------------------------------------------
// Individual check helpers
// ---------------------------------------------------------------------------

/// Check 1: FP validity — is the frame pointer in a reasonable range?
///
/// With CONFIG_VMAP_STACK=y (which this kernel enables), kernel stacks
/// are allocated from the vmalloc region.  Both the linear mapping and
/// vmalloc ranges are valid for FP.
fn check_fp_validity(fp: u64, details: &mut Vec<String>) -> Option<bool> {
    if fp == 0 {
        details.push("FP (x29) = 0x0 — frame pointer is NULL, stack corrupted".into());
        return Some(false);
    }
    if fp >= KERNEL_LINEAR_LOW && fp < KERNEL_LINEAR_HIGH {
        details.push(format!(
            "FP (x29) = {:#018x} — in kernel linear mapping (valid)", fp
        ));
        Some(true)
    } else if fp >= KERNEL_VMALLOC_LOW && fp < KERNEL_VMALLOC_HIGH {
        // VMAP_STACK: stacks live in vmalloc.  This is the normal case.
        details.push(format!(
            "FP (x29) = {:#018x} — in vmalloc region (VMAP_STACK, valid)", fp
        ));
        Some(true)
    } else if fp >= USER_SPACE_LOW && fp <= USER_SPACE_HIGH {
        details.push(format!(
            "FP (x29) = {:#018x} — in user-space range (EL0 crash?)", fp
        ));
        Some(true)
    } else {
        details.push(format!(
            "FP (x29) = {:#018x} — outside any known valid range, possible corruption", fp
        ));
        Some(false)
    }
}

/// Check 2: 16-byte alignment (AAPCS64).
fn check_alignment(fp: u64, details: &mut Vec<String>) -> Option<bool> {
    if fp == 0 {
        return None;
    }
    let aligned = fp & 0xF == 0;
    if aligned {
        details.push("FP (x29) is 16-byte aligned (AAPCS64)".into());
    } else {
        details.push(format!(
            "FP (x29) = {:#018x} — NOT 16-byte aligned (misaligned by {} bytes)",
            fp, fp & 0xF,
        ));
    }
    Some(aligned)
}

/// Check 3: Derive `current` task_struct from FP and read `preempt_count`.
///
/// ARM64 convention:  `current = (task_struct*)(SP_EL1 & ~(THREAD_SIZE-1))`.
/// We approximate SP_EL1 with FP(x29).
///
/// **Important**: This ONLY works when FP is in the LINEAR MAPPING range
/// (0xffff_0000_xxxx_xxxx).  For vmalloc-allocated stacks (VMAP_STACK),
/// the simple GPA = GVA - PAGE_OFFSET conversion does NOT produce a valid
/// physical address — those GVAs need a page-table walk which we don't
/// have.  In that case we note the reason and skip the check.
fn check_current_task(
    fp: u64,
    mem: &impl Fn(u64) -> Option<u64>,
    details: &mut Vec<String>,
) -> Option<bool> {
    if fp == 0 || fp < PAGE_OFFSET {
        details.push("Cannot derive current_task: FP is NULL or outside kernel range".into());
        return None;
    }

    // VMAP_STACK detection: FP in vmalloc range → cannot translate.
    if fp >= KERNEL_VMALLOC_LOW && fp < KERNEL_VMALLOC_HIGH {
        details.push(format!(
            "FP is in vmalloc region (VMAP_STACK) — cannot translate GVA→GPA \
             for current_task derivation without page-table walk"
        ));
        return None;
    }

    // FP must be in linear mapping for the simple GPA conversion to work.
    if fp < KERNEL_LINEAR_LOW || fp >= KERNEL_LINEAR_HIGH {
        details.push(format!(
            "FP = {:#018x} — not in linear mapping range, cannot derive current_task", fp
        ));
        return None;
    }

    let task_candidate_gva = fp & THREAD_MASK;
    let task_candidate_gpa = gva_to_gpa(task_candidate_gva);

    // Read preempt_count at offset 8.
    let preempt_count = mem(task_candidate_gpa + TI_PREEMPT_COUNT);

    match preempt_count {
        Some(pc) => {
            // Sanity check: preempt_count should be a small integer.
            // Negative values mean "in_interrupt()" (softirq/nmi context).
            // Values > 0x00FFFFFF are suspicious.
            if pc > 0x00FF_FFFF {
                details.push(format!(
                    "current_task @ GVA {:#018x} — preempt_count = {} (implausibly large, corruption?)",
                    task_candidate_gva, pc as i64,
                ));
                Some(false)
            } else {
                let context = if pc == 0 {
                    "preemptible".into()
                } else if (pc as i64) < 0 {
                    alloc::format!("in_interrupt (preempt_count = {} sign-bit set)", pc as i64)
                } else {
                    alloc::format!("non-preemptible (preempt_count = {})", pc)
                };
                details.push(format!(
                    "current_task @ GVA {:#018x} — {}",
                    task_candidate_gva, context,
                ));
                Some(true)
            }
        }
        None => {
            details.push(format!(
                "current_task @ GVA {:#018x} — could not read memory (GPA {:#010x})",
                task_candidate_gva, task_candidate_gpa,
            ));
            None
        }
    }
}

/// Check 4: Interrupt state from SPSR_EL1 bits 7 (I) and 6 (F).
fn check_interrupt_state(spsr: u64, details: &mut Vec<String>) -> Option<bool> {
    let irq_masked = (spsr & SPSR_IRQ_MASK) != 0;
    let fiq_masked = (spsr & SPSR_FIQ_MASK) != 0;

    if irq_masked || fiq_masked {
        let mut why = String::from("Interrupts were masked at crash time:");
        if irq_masked { why.push_str(" IRQ"); }
        if fiq_masked { why.push_str(" FIQ"); }
        why.push_str(" — possible spinlock/deadlock context");
        details.push(why);
        Some(true)
    } else {
        details.push("Interrupts were enabled at crash time".into());
        Some(false)
    }
}

/// Check 5: Exception nesting detection.
///
/// If the CPU was already in an exception handler (EL1t mode or PC in
/// handler code) when another abort occurred, this is a nested exception.
fn check_exception_nesting(
    spsr: u64,
    esr: u64,
    elr: u64,
    details: &mut Vec<String>,
) -> Option<bool> {
    let el = spsr & SPSR_EL_MASK;
    let ec = (esr >> 26) & 0x3F;
    let is_abort = matches!(ec,
        EC_DATA_ABORT_LOWER_EL | EC_DATA_ABORT_CURRENT_EL |
        EC_INST_ABORT_LOWER_EL | EC_INST_ABORT_CURRENT_EL
    );

    if !is_abort {
        // Note: if `regs.esr_el1 == 0` and no eff_esr was provided, this
        // may mean the hypervisor didn't trap the exception, not that no
        // exception occurred.
        details.push("Exception class is not an abort — no nesting evidence".into());
        return Some(false);
    }

    match el {
        4 => {
            // EL1t: exception taken while in EL1 using SP_EL0.
            // This means the CPU was INSIDE an exception handler (which
            // switches to SP_EL1 normally) — a true nested exception.
            details.push("NESTED EXCEPTION: abort occurred in EL1t mode (handler re-entered)".into());
            Some(true)
        }
        5 => {
            // EL1h: normal kernel mode. Could still be nested if the
            // handler code triggered a fault.
            if elr >= KERNEL_LINEAR_LOW && elr < KERNEL_VMALLOC_HIGH {
                details.push("Abort in kernel address space — likely a primary fault, not nested".into());
                Some(false)
            } else if elr >= USER_SPACE_LOW && elr <= USER_SPACE_HIGH {
                details.push("Abort at user-space address — unusual for kernel crash".into());
                Some(false)
            } else {
                details.push("Abort at unusual PC — possible nested or corrupted state".into());
                Some(true)
            }
        }
        _ => {
            details.push("Exception taken from non-EL1 mode (EL0?)".into());
            Some(false)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a Guest Virtual Address (GVA) to Guest Physical Address (GPA).
///
/// For addresses in the linear mapping (PAGE_OFFSET .. high):
///   GPA = GVA - PAGE_OFFSET
///
/// For user-space addresses (TTBR0): simply return GVA unchanged (identity
/// mapping in stage-2).
fn gva_to_gpa(gva: u64) -> u64 {
    if gva >= PAGE_OFFSET {
        gva.wrapping_sub(PAGE_OFFSET)
    } else {
        gva
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_regs(spsr: u64, sp: u64, elr: u64, esr: u64) -> VcpuRegsEntry {
        VcpuRegsEntry {
            vcpu_id: 0,
            gpr: [0; 31],
            sp_el0: sp,
            elr_el1: elr,
            spsr_el1: spsr,
            esr_el1: esr,
            far_el1: 0,
            crash_type: 0,
        }
    }

    fn make_regs_with_fp(spsr: u64, fp: u64, elr: u64, esr: u64) -> VcpuRegsEntry {
        let mut regs = make_regs(spsr, 0, elr, esr);
        regs.gpr[29] = fp;
        regs
    }

    #[test]
    fn test_fp_in_kernel_linear() {
        let r = make_regs_with_fp(0x5, 0xffff_0000_0123_4000, 0, 0);
        let res = check_dstructs(&r, None, &|_| None, None);
        assert_eq!(res.sp_in_stack, Some(true));
    }

    #[test]
    fn test_fp_unaligned() {
        let r = make_regs_with_fp(0x5, 0xffff_0000_0123_4007, 0, 0);
        let res = check_dstructs(&r, None, &|_| None, None);
        assert_eq!(res.sp_aligned, Some(false));
    }

    #[test]
    fn test_fp_zero() {
        let r = make_regs(0x5, 0, 0, 0);
        let res = check_dstructs(&r, None, &|_| None, None);
        assert_eq!(res.sp_in_stack, Some(false));
        assert_eq!(res.sp_aligned, None);
    }

    #[test]
    fn test_irqs_masked() {
        // SPSR with IRQ bit (bit 7) set
        let r = make_regs_with_fp(0x80, 0xffff_0000_0123_4000, 0, 0);
        let res = check_dstructs(&r, None, &|_| None, None);
        assert_eq!(res.irqs_masked, Some(true));
        assert_eq!(res.sp_in_stack, Some(true));
    }

    #[test]
    fn test_exception_nested_el1t() {
        // Data Abort (EC=0x25) with SPSR.M=4 (EL1t)
        let r = make_regs_with_fp(0x4, 0xffff_0000_0123_4000, 0, 0x9600_0000);
        let res = check_dstructs(&r, None, &|_| None, None);
        assert_eq!(res.exception_nested, Some(true));
    }

    #[test]
    fn test_current_task_derived() {
        // Simulate FP pointing into kernel stack.  current_task is derived
        // from `FP & THREAD_MASK`.  Mock mem returns Some(0) for preempt_count.
        let r = make_regs_with_fp(0x5, 0xffff_0000_0123_4560, 0, 0);
        let res = check_dstructs(&r, None, &|addr| {
            // GPA = 0x0000_0001_2345_60 — preempt_count offset = +8
            if addr == 0x1_2345_68 { Some(0) } else { None }
        }, None);
        assert_eq!(res.current_task_valid, Some(true));
    }
}
