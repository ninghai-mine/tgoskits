//! Critical kernel data structure sanity checks.
//!
//! Performs heuristic checks on key kernel data structures and CPU state
//! to detect corruption, deadlocks, or abnormal conditions.  Each check
//! is independent and gracefully degrades to `None` when the required
//! data is unavailable.
//!
//! # Checks performed
//!
//! 1. **SP validity** — is the stack pointer in a reasonable range?
//! 2. **Current-task pointer** — does `current_task` point to valid memory?
//! 3. **Interrupt state** — were interrupts masked at crash time?
//! 4. **Exception nesting** — did a second exception occur within a handler?
//! 5. **Stack alignment** — is SP 16-byte aligned (AAPCS64)?

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::capture::storage::VcpuRegsEntry;
use crate::recovery::symbol::SymbolTable;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants (must match target kernel config)
// ---------------------------------------------------------------------------

/// Linear mapping offset (GVA → GPA).
const PHYS_VIRT_OFFSET: u64 = 0xffff_8000_0000_0000;

/// Approximate kernel stack region.
const KERNEL_STACK_LOW: u64  = 0xffff_8000_8000_0000;
const KERNEL_STACK_HIGH: u64 = 0xffff_8000_8800_0000;

/// Valid kernel image text range.
const KERNEL_TEXT_LOW: u64  = 0xffff_8000_8020_0000;
const KERNEL_TEXT_HIGH: u64 = 0xffff_8000_8040_0000;

/// User space typical address range (48-bit VA).
const USER_SPACE_LOW: u64  = 0x0000_0000_0000_0000;
const USER_SPACE_HIGH: u64 = 0x0000_ffff_ffff_f000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of all data-structure sanity checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DstructResult {
    /// Whether SP points inside the kernel stack region.
    pub sp_in_stack: Option<bool>,
    /// Whether `current_task` points to a reasonable address.
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
/// * `sym`  — Optional kernel symbol table (used to resolve `current_task`).
/// * `mem`  — Memory reader closure (GPA → 8 bytes).
pub fn check_dstructs(
    regs: &VcpuRegsEntry,
    sym: Option<&SymbolTable>,
    mem: &impl Fn(u64) -> Option<u64>,
) -> DstructResult {
    let mut details = Vec::new();

    // ── 1. SP validity ──
    let sp = regs.sp_el0;
    let sp_in_stack = if sp == 0 {
        details.push("SP = 0x0 — stack pointer is NULL, likely corrupted".into());
        Some(false)
    } else if sp >= KERNEL_STACK_LOW && sp <= KERNEL_STACK_HIGH {
        details.push(format!("SP = {:#018x} — within kernel stack region", sp));
        Some(true)
    } else if sp >= USER_SPACE_LOW && sp <= USER_SPACE_HIGH {
        details.push(format!("SP = {:#018x} — in user-space range (valid for EL0)", sp));
        Some(true)
    } else {
        details.push(format!("SP = {:#018x} — outside any known valid range", sp));
        Some(false)
    };

    // ── 2. Stack alignment (AAPCS64 requires 16-byte alignment) ──
    let sp_aligned = if sp == 0 {
        None
    } else {
        let aligned = sp & 0xF == 0;
        if aligned {
            details.push("SP is 16-byte aligned (AAPCS64)".into());
        } else {
            details.push(format!(
                "SP = {:#018x} — NOT 16-byte aligned (misaligned by {} bytes)",
                sp, sp & 0xF,
            ));
        }
        Some(aligned)
    };

    // ── 3. Interrupt state ──
    // SPSR_EL1 bit 7 = I (IRQ mask), bit 6 = F (FIQ mask).
    let spsr = regs.spsr_el1;
    let irq_masked = (spsr >> 7) & 1;
    let fiq_masked = (spsr >> 6) & 1;
    let irqs_masked = if irq_masked == 1 || fiq_masked == 1 {
        let mut why = String::from("Interrupts were masked at crash time:");
        if irq_masked == 1 { why.push_str(" IRQ"); }
        if fiq_masked == 1 { why.push_str(" FIQ"); }
        why.push_str(" — possible deadlock or spinlock context");
        details.push(why);
        Some(true)
    } else {
        details.push("Interrupts were enabled at crash time".into());
        Some(false)
    };

    // ── 4. Exception nesting check ──
    // If the exception class is Data/Instruction Abort and SPSR.M == EL1t,
    // the CPU was already in an exception handler when the fault occurred.
    let el = spsr & 0xF;
    let ec = (regs.esr_el1 >> 26) & 0x3F;
    let is_abort = matches!(ec, 0x20 | 0x21 | 0x24 | 0x25);
    let exception_nested = if is_abort && el == 4 {
        // EL1t = exception taken from the same EL, with SP_EL0
        // (the handler was using the user stack).
        details.push("Nested exception detected: abort occurred within an EL1 handler (EL1t)".into());
        Some(true)
    } else if is_abort && el == 5 {
        // EL1h = exception taken from the same EL, using SP_EL1
        // (normal for kernel crashes, but could be nested if
        // the handler re-entered).
        if regs.elr_el1 >= KERNEL_TEXT_LOW && regs.elr_el1 <= KERNEL_TEXT_HIGH {
            details.push("Exception in kernel code — likely a primary fault, not nested".into());
            Some(false)
        } else {
            details.push("Exception at unusual PC — possible nested or corrupted state".into());
            Some(true)
        }
    } else {
        details.push("No evidence of exception nesting".into());
        Some(false)
    };

    // ── 5. Current-task pointer ──
    let current_task_valid = if let Some(sym_table) = sym {
        let names = ["CURRENT_TASK", "current_task", "CURRENT", "current"];
        let mut found = false;
        let mut result = None;
        for name in &names {
            if let Some(info) = sym_table.lookup_name(name) {
                found = true;
                let gva = sym_table.kernel_base + info.addr;
                let gpa = gva_to_gpa(gva);
                if let Some(ptr) = mem(gpa) {
                    if ptr >= KERNEL_STACK_LOW && ptr <= KERNEL_STACK_HIGH {
                        details.push(format!("current_task ({}) = {:#018x} — in valid kernel range", name, ptr));
                        result = Some(true);
                    } else if ptr != 0 {
                        details.push(format!("current_task ({}) = {:#018x} — outside kernel range, possible corruption", name, ptr));
                        result = Some(false);
                    } else {
                        details.push(format!("current_task ({}) = 0x0 — NULL", name));
                        result = Some(false);
                    }
                } else {
                    details.push(format!("current_task ({}) — could not read from memory", name));
                    result = None;
                }
                break;
            }
        }
        if !found {
            details.push("current_task symbol not found in kernel ELF".into());
        }
        result
    } else {
        details.push("No kernel symbol table — cannot check current_task".into());
        None
    };

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
// Internal helpers
// ---------------------------------------------------------------------------

fn gva_to_gpa(gva: u64) -> u64 {
    if gva >= PHYS_VIRT_OFFSET {
        gva.wrapping_sub(PHYS_VIRT_OFFSET)
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

    #[test]
    fn test_sp_in_kernel_stack() {
        let r = make_regs(0x5, 0xffff_8000_8123_4000, 0, 0);
        let res = check_dstructs(&r, None, &|_| None);
        assert_eq!(res.sp_in_stack, Some(true));
    }

    #[test]
    fn test_sp_zero() {
        let r = make_regs(0x5, 0, 0, 0);
        let res = check_dstructs(&r, None, &|_| None);
        assert_eq!(res.sp_in_stack, Some(false));
    }

    #[test]
    fn test_irqs_masked() {
        // SPSR with IRQ bit (bit 7) set
        let r = make_regs(0x80, 0xffff_8000_8123_4000, 0, 0);
        let res = check_dstructs(&r, None, &|_| None);
        assert_eq!(res.irqs_masked, Some(true));
    }

    #[test]
    fn test_sp_unaligned() {
        let r = make_regs(0x5, 0xffff_8000_8123_4007, 0, 0);
        let res = check_dstructs(&r, None, &|_| None);
        assert_eq!(res.sp_aligned, Some(false));
    }

    #[test]
    fn test_exception_nested_el1t() {
        // Data Abort (EC=0x25) with SPSR.M=4 (EL1t)
        let r = make_regs(0x4, 0xffff_8000_8123_4000, 0, 0x9600_0000);
        let res = check_dstructs(&r, None, &|_| None);
        assert_eq!(res.exception_nested, Some(true));
    }
}
