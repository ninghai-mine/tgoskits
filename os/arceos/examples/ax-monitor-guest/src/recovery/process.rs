//! Crash-time process / thread identification.
//!
//! Identifies which process/thread was running when the crash occurred.
//! On Linux, the `current_task` per-CPU pointer points to `task_struct` which
//! can be traversed from the kernel stack. This module provides a best-effort
//! identification based on register analysis and backtrace context.

extern crate alloc;
use alloc::string::{String, ToString};
use crate::capture::storage::VcpuRegsEntry;
use crate::recovery::symbol::SymbolTable;
use crate::recovery::unwind::StackFrame;
use serde::{Deserialize, Serialize};

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

/// Identify the crashing process from register state and backtrace.
///
/// # Current Heuristic
///
/// Extracts exception level from `SPSR_EL1.M[3:0]`:
/// - `0x0` (EL0t) → user-mode process
/// - `0x4` (EL1t) or `0x5` (EL1h) → kernel thread
///
/// TODO: When full kernel memory dump is available, traverse `current_task`
/// via sp_el1 -> thread_info -> task_struct to read PID and comm.
///
/// TODO: For StarryOS, check the task control block structure for
/// equivalent process/task identification.
pub fn identify(
    regs: &VcpuRegsEntry,
    _frames: &[StackFrame],
    _sym: Option<&SymbolTable>,
) -> ProcessInfo {
    // SPSR_EL1.M[3:0] indicates the exception origin.
    let (name, state, is_kthread) = match regs.spsr_el1 & 0xF {
        0 => (
            "<user_process>".to_string(),
            "running (user mode)".to_string(),
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

    ProcessInfo {
        pid: None,
        name,
        state,
        is_kernel_thread: is_kthread,
        cpu_id: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_regs(spsr_low_bits: u64) -> VcpuRegsEntry {
        VcpuRegsEntry {
            vcpu_id: 0,
            gpr: [0; 31],
            sp_el0: 0,
            elr_el1: 0,
            spsr_el1: spsr_low_bits,
        }
    }

    #[test]
    fn test_user_mode() {
        let info = identify(&make_regs(0x0), &[], None);
        assert!(!info.is_kernel_thread);
        assert_eq!(info.name, "<user_process>");
    }

    #[test]
    fn test_kernel_mode_el1h() {
        let info = identify(&make_regs(0x5), &[], None);
        assert!(info.is_kernel_thread);
        assert_eq!(info.name, "<kernel_task>");
    }
}