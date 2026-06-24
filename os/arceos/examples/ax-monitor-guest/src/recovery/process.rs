//! Crash-time process / thread identification.
//!
//! Identifies which process/thread was running when the crash occurred.
//! Uses multiple heuristics:
//!
//! 1. **SPSR exception level** — EL0t → user process, EL1t/EL1h → kernel.
//! 2. **SP range analysis** — is the SP in the kernel stack region?
//! 3. **Backtrace function names** — match known task entry points.
//! 4. **current_task memory read** — if the kernel symbol `CURRENT_TASK` is
//!    available, read the `TaskInner` pointer and extract the TaskId (PID).

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::storage::VcpuRegsEntry;
use crate::recovery::symbol::{SymbolInfo, SymbolTable};
use crate::recovery::unwind::StackFrame;
use serde::{Deserialize, Serialize};

/// Linear mapping offset used for GVA→GPA translation.
/// Linux ARM64 with 48-bit VA: PAGE_OFFSET = 0xffff_0000_0000_0000
const PHYS_VIRT_OFFSET: u64 = 0xffff_0000_0000_0000;

/// Approximate kernel stack region (48-bit VA).
/// With PAGE_OFFSET=0xffff_0000_0000_0000, kernel stacks are allocated
/// in the linear mapping area. This range covers the linear map plus
/// the kernel image identity map region.
const KERNEL_STACK_LOW: u64  = 0xffff_0000_0000_0000;
const KERNEL_STACK_HIGH: u64 = 0xffff_8000_ffff_ffff;

/// User space typical address range (48-bit VA).
const USER_SPACE_LOW: u64  = 0x0000_0000_0000_0000;
const USER_SPACE_HIGH: u64 = 0x0000_ffff_ffff_f000;

/// Known idle/info task function name prefixes.
const IDLE_TASK_NAMES: &[&str] = &["idle", "idle_thread", "default_idle"];
const INIT_TASK_NAMES: &[&str] = &["init", "main", "rust_main", "init_task"];

/// Identified process or thread at crash time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process ID (PID) — None if unrecoverable.
    pub pid: Option<u64>,
    /// Process name / comm string.
    pub name: String,
    /// Current state description (e.g. "running", "interruptible sleep").
    pub state: String,
    /// Whether this is a kernel thread.
    pub is_kernel_thread: bool,
    /// CPU ID the process was running on.
    pub cpu_id: u64,
}

/// Identify the crashing process using multiple heuristics.
///
/// # Arguments
///
/// * `regs`    — Register state of the crashed vCPU.
/// * `frames`  — Unwound call stack (may be empty).
/// * `sym`     — Optional kernel symbol table.
/// * `mem`     — Memory reader closure (GPA → 8 bytes), used to read
///               kernel globals like `current_task` from guest memory.
pub fn identify(
    regs: &VcpuRegsEntry,
    frames: &[StackFrame],
    sym: Option<&SymbolTable>,
    mem: &impl Fn(u64) -> Option<u64>,
) -> ProcessInfo {
    // ── Heuristic 1: SPSR exception level ──
    let el = regs.spsr_el1 & 0xF;
    let (mut name, mut state, mut is_kthread) = match el {
        0 => (
            "<user_process>".to_string(),
            "running (user)".to_string(),
            false,
        ),
        4 | 5 => (
            "<kernel_task>".to_string(),
            "running (kernel)".to_string(),
            true,
        ),
        _ => (
            "<unknown>".to_string(),
            "unknown exception level".to_string(),
            true,
        ),
    };

    // ── Heuristic 2: SP range analysis ──
    let sp = regs.sp_el0;
    if sp >= KERNEL_STACK_LOW && sp <= KERNEL_STACK_HIGH {
        if !is_kthread {
            is_kthread = true;
            state = "running (kernel stack)".into();
        }
    } else if sp >= USER_SPACE_LOW && sp <= USER_SPACE_HIGH && sp > 4096 {
        if is_kthread {
            is_kthread = false;
            state = "running (user)".into();
        }
    } else if sp == 0 {
        state = "SP=0x0 — stack pointer corrupted".into();
    }

    // ── Heuristic 3: Backtrace function names ──
    for frame in frames {
        if let Some(ref func) = frame.func_name {
            let lower = func.to_lowercase();
            if IDLE_TASK_NAMES.iter().any(|n| lower.contains(n)) {
                name = "idle_task".into();
                state = "idle".into();
                is_kthread = true;
                break;
            }
            if INIT_TASK_NAMES.iter().any(|n| lower.contains(n)) {
                if name == "<kernel_task>" || name == "<user_process>" {
                    name = "init_task".into();
                }
            }
        }
    }

    // ── Heuristic 4: Try to read current_task from kernel global ──
    if let Some(sym_table) = sym {
        let current_sym_names = ["CURRENT_TASK", "current_task", "CURRENT", "current"];
        for sym_name in &current_sym_names {
            if let Some(info) = find_symbol_by_name(sym_table, sym_name) {
                // Symbol address is the GVA of the global pointer.
                let gpa = gva_to_gpa(sym_table.kernel_base + info.addr);
                if let Some(arc_ptr) = mem(gpa) {
                    // arc_ptr = heap address of TaskInner (GVA).
                    let task_gpa = gva_to_gpa(arc_ptr);
                    if let Some(task_id_raw) = mem(task_gpa) {
                        let pid = task_id_raw;
                        name = alloc::format!("task_{}", pid);
                        state = "running (from current_task)".into();
                        return ProcessInfo {
                            pid: Some(pid),
                            name,
                            state,
                            is_kernel_thread: true,
                            cpu_id: 0,
                        };
                    }
                }
                break;
            }
        }
    }

    // ── Heuristic 5: Fallback — use crash function name ──
    if name.starts_with('<') {
        if let Some(ref func) = frames.first().and_then(|f| f.func_name.as_ref()) {
            let truncated = if func.len() > 48 {
                format!("{}…", &func[..48])
            } else {
                func.to_string()
            };
            name = truncated;
        }
    }

    ProcessInfo {
        pid: None,
        name,
        state,
        is_kernel_thread: is_kthread,
        cpu_id: 0,
    }
}

/// Look up a symbol by name using the public API.
fn find_symbol_by_name<'a>(sym: &'a SymbolTable, name: &str) -> Option<&'a SymbolInfo> {
    sym.lookup_name(name)
}

/// Translate a Guest Virtual Address to a Guest Physical Address.
fn gva_to_gpa(gva: u64) -> u64 {
    if gva >= PHYS_VIRT_OFFSET {
        gva.wrapping_sub(PHYS_VIRT_OFFSET)
    } else {
        gva
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_regs(spsr: u64, sp: u64, elr: u64) -> VcpuRegsEntry {
        VcpuRegsEntry {
            vcpu_id: 0,
            gpr: [0; 31],
            sp_el0: sp,
            elr_el1: elr,
            spsr_el1: spsr,
        }
    }

    #[test]
    fn test_user_mode() {
        let info = identify(
            &make_regs(0x0, 0x7fff_ffff_f000, 0x4000_0000),
            &[], None, &|_| None,
        );
        assert!(!info.is_kernel_thread);
        assert_eq!(info.name, "<user_process>");
    }

    #[test]
    fn test_kernel_mode_el1h() {
        let info = identify(
            &make_regs(0x5, 0xffff_8000_8123_4000, 0xffff_8000_8020_1234),
            &[], None, &|_| None,
        );
        assert!(info.is_kernel_thread);
    }

    #[test]
    fn test_sp_zero() {
        let info = identify(
            &make_regs(0x5, 0, 0xffff_8000_8020_1234),
            &[], None, &|_| None,
        );
        assert!(info.state.contains("SP=0x0"));
    }

    #[test]
    fn test_elr_corrupted() {
        let info = identify(
            &make_regs(0x5, 0xffff_8000_8123_4000, 0),
            &[], None, &|_| None,
        );
        // Should not panic; state may contain "ELR=0x0" or remain as default.
        assert!(!info.name.is_empty());
    }
}