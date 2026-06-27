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

    // ── Heuristic 4: Try SP_EL0 as current task_struct pointer (Linux ARM64) ──
    // On Linux ARM64, SP_EL0 holds the `current` task_struct pointer when
    // the CPU is in kernel mode (EL1).  This is set by `__switch_to()`.
    let sp_el0 = regs.sp_el0;
    if sp_el0 >= PHYS_VIRT_OFFSET || (sp_el0 >= 0xffff_8000_0000_0000u64) {
        // sp_el0 looks like a kernel address — try to read PID from task_struct.
        // task_struct.pid offset for Linux 6.12 ARM64: typical offset 0x730-0x780.
        // We probe known offsets to find the PID.
        let pid_offsets: &[u64] = &[0x730, 0x738, 0x740, 0x750, 0x780, 0x798, 0x7a0, 0x870, 0x878];
        for &off in pid_offsets {
            if let Some(val) = mem(sp_el0 + off) {
                let pid_candidate = (val & 0xFFFFFFFF) as u32;
                if pid_candidate > 0 && pid_candidate < 100000 {
                    // Try to read process name (comm) at offset ~comm_offset
                    // task_struct.comm is typically at offset 0x780-0x800 on 6.12
                    let comm_offsets: &[u64] = &[0x780, 0x798, 0x7a0, 0x7c0, 0x800, 0x880, 0x8a0];
                    let mut process_name = alloc::format!("task_{}", pid_candidate);
                    for &coff in comm_offsets {
                        if let Some(cval) = mem(sp_el0 + coff) {
                            let bytes = cval.to_le_bytes();
                            if bytes[0].is_ascii_graphic() || bytes[0] == 0 {
                                // Read up to 15 chars as a C string
                                let name_chars: Vec<u8> = (0..15)
                                    .filter_map(|i| {
                                        let b = if i < 8 {
                                            bytes[i]
                                        } else {
                                            mem(sp_el0 + coff + (i as u64 / 8) * 8)
                                                .map(|v| v.to_le_bytes()[i as usize % 8])
                                                .unwrap_or(0)
                                        };
                                        if b == 0 { None } else { Some(b) }
                                    })
                                    .collect();
                                if !name_chars.is_empty() {
                                    let s = alloc::string::String::from_utf8_lossy(&name_chars).to_string();
                                    if s.chars().all(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()) {
                                        process_name = s;
                                    }
                                }
                                break;
                            }
                        }
                    }
                    return ProcessInfo {
                        pid: Some(pid_candidate as u64),
                        name: process_name,
                        state: "running (kernel)".into(),
                        is_kernel_thread: true,
                        cpu_id: 0,
                    };
                }
            }
        }
    }

    // ── Heuristic 5: Try to read current_task from kernel global (Linux percpu) ──
    // This is kept for ArceOS compatibility but Linux uses SP_EL0 instead.
    if let Some(sym_table) = sym {
        let current_sym_names = ["CURRENT_TASK", "current_task", "CURRENT", "current"];
        for sym_name in &current_sym_names {
            if let Some(info) = find_symbol_by_name(sym_table, sym_name) {
                // Pass the GVA directly; mem() handles GVA→GPA translation.
                let sym_gva = sym_table.kernel_base + info.addr;
                if let Some(arc_ptr) = mem(sym_gva) {
                    // arc_ptr = pointer to task (GVA).  Pass directly to mem().
                    if let Some(task_id_raw) = mem(arc_ptr) {
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

/// Translate a Guest Virtual Address to a Guest Physical Address (guest's view).
/// NOTE: This returns the guest's physical address (starting from 0x8000_0000),
/// NOT the monitor's GPA.  For monitor GPA, pass GVA directly to the `mem()`
/// closure which handles the full GVA→HPA translation internally.
/// This function is kept for completeness but prefer using mem() directly.
#[allow(dead_code)]
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