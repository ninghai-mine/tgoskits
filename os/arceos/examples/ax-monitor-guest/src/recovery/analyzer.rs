//! High-level crash analysis engine.
//!
//! Combines register state, call stack, process context, and symbol
//! information to produce a structured `AnalysisResult` that describes
//! the crash root cause and provides diagnostic hints.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::storage::VmcoreFile;
use crate::recovery::process::ProcessInfo;
use crate::recovery::symbol::SymbolTable;
use crate::recovery::unwind::{unwind_stack, StackFrame};
use serde::{Deserialize, Serialize};

/// Complete analysis result produced by the analyzer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Schema version for forward compatibility.
    pub analysis_version: String,
    /// Crash event type (Panic, WatchdogTimeout, Exception, Unknown).
    pub crash_event: String,
    /// ISO 8601 timestamp of the crash.
    pub timestamp: String,
    /// Program counter at the crash site.
    pub crash_pc: u64,
    /// Function name at crash site (if symbol table available).
    pub crash_function: Option<String>,
    /// Offset within the crashing function.
    pub crash_function_offset: Option<u64>,
    /// Full reconstructed call stack.
    pub backtrace: Vec<StackFrame>,
    /// Identified process/thread context.
    pub process: ProcessInfo,
    /// Human-readable crash summary.
    pub summary: String,
    /// List of possible root causes.
    pub possible_causes: Vec<String>,
    /// Key registers at crash time (PC, SP, LR, PSR).
    pub key_registers: Vec<(String, u64)>,
}

/// Run the full analysis pipeline on a loaded vmcore.
///
/// # Arguments
///
/// * `vmcore` — Loaded vmcore file (from `storage::load_vmcore`).
/// * `mem`    — Closure reading 8 bytes at a guest physical address.
///              Pass `&\|_\| None` if memory dump is unavailable.
/// * `sym`    — Optional ELF symbol table for function name resolution.
///
/// TODO: Provide actual kernel ELF path and base address to SymbolTable.
///       Example:
///       let sym = SymbolTable::from_kernel_elf(
///           "/path/to/vmlinux",           // ← kernel ELF path
///           0xffff_0000_0000_0000,         // ← kernel base address
///       ).ok();
pub fn analyze(
    vmcore: &VmcoreFile,
    mem: &impl Fn(u64) -> Option<u64>,
    sym: Option<&SymbolTable>,
) -> AnalysisResult {
    let primary_vcpu = vmcore.registers.first();

    // Step 1 — Unwind the call stack.
    let backtrace = primary_vcpu
        .map(|regs| unwind_stack(regs, mem, sym))
        .unwrap_or_default();

    // Step 2 — Identify the crashing process.
    let process = primary_vcpu
        .map(|regs| crate::recovery::process::identify(regs, &backtrace, sym))
        .unwrap_or(ProcessInfo {
            pid: None,
            name: "<unknown>".into(),
            state: "unknown".into(),
            is_kernel_thread: false,
            cpu_id: 0,
        });

    // Step 3 — Extract key registers (including ESR/FAR).
    let key_registers = primary_vcpu
        .map(|regs| {
            let mut keys = vec![
                ("ELR_EL1".into(), regs.elr_el1),
                ("SP_EL0".into(), regs.sp_el0),
                ("SPSR_EL1".into(), regs.spsr_el1),
                ("ESR_EL1".into(), regs.esr_el1),
                ("FAR_EL1".into(), regs.far_el1),
                ("FP (x29)".into(), regs.gpr[29]),
                ("LR (x30)".into(), regs.gpr[30]),
            ];
            keys
        })
        .unwrap_or_default();

    // Step 4 — Generate summary and possible causes using ESR/FAR when available.
    let crash_pc = primary_vcpu.map(|r| r.elr_el1).unwrap_or(0);
    let crash_function = sym
        .and_then(|s| s.lookup(crash_pc))
        .map(|si| si.name.clone());
    let crash_function_offset = sym.and_then(|s| {
        s.nearest(crash_pc).map(|si| {
            crash_pc.wrapping_sub(s.kernel_base) - si.addr
        })
    });

    // 优先使用 ESR/FAR 解码，否则降级到 event-based 诊断
    let (summary, possible_causes) = match primary_vcpu {
        Some(regs) if regs.esr_el1 != 0 => {
            decode_esr(regs.esr_el1, regs.far_el1, crash_function.as_deref())
        }
        _ => {
            generate_diagnosis(&vmcore.crash_event, crash_pc, crash_function.as_deref(), &backtrace)
        }
    };

    AnalysisResult {
        analysis_version: "1.0".into(),
        crash_event: vmcore.crash_event.clone(),
        timestamp: vmcore.timestamp.clone(),
        crash_pc,
        crash_function,
        crash_function_offset,
        backtrace,
        process,
        summary,
        possible_causes,
        key_registers,
    }
}

/// Generate human-readable diagnosis from the analyzed data.
fn generate_diagnosis(
    event: &str,
    crash_pc: u64,
    crash_func: Option<&str>,
    backtrace: &[StackFrame],
) -> (String, Vec<String>) {
    let func_info = crash_func
        .map(|f| format!(" in `{}`", f))
        .unwrap_or_default();

    let summary = match event {
        "Panic" => format!(
            "Kernel panic triggered{} at PC={:#018x}. \
             The kernel called `panic!()` deliberately. Check for \
             assertion failures or unrecoverable errors above in the backtrace.",
            func_info, crash_pc
        ),
        "WatchdogTimeout" => {
            "System hang detected: watchdog timer expired because the kernel \
             stopped sending heartbeats. Possible causes: deadlock, infinite \
             loop with interrupts disabled, or hardware hang."
                .into()
        }
        "Exception" => format!(
            "Synchronous exception{} at PC={:#018x}. \
             Likely a kernel NULL-pointer dereference, use-after-free, or \
             invalid memory access.",
            func_info, crash_pc
        ),
        _ => format!("Unknown crash event{} at PC={:#018x}.", func_info, crash_pc),
    };

    let mut causes = Vec::new();

    // Heuristic checks.
    if let Some(top) = backtrace.first() {
        if top.func_name.as_deref() == Some("panic") {
            causes.push("The kernel intentionally panicked.".into());
            causes.push("Check for a prior assertion failure, BUG_ON, or Oops above this frame.".into());
        }
        if top.pc == 0 || (top.pc & 0xfff) == 0 {
            causes.push("Crash PC is near a page boundary — possible NULL pointer dereference.".into());
        }
    }

    if backtrace.len() <= 1 {
        causes.push("Only 1 frame in backtrace — stack memory may not be available.".into());
        causes.push("Provide guest memory dump and rebuild kernel with frame pointers for full backtrace.".into());
    }

    if let Some(func) = crash_func {
        if func.contains("fault") || func.contains("abort") || func.contains("error") {
            causes.push(format!("Crash landed in `{}` which handles fault conditions.", func));
        }
    }

    (summary, causes)
}

/// Decode ESR_EL1 into a human-readable crash diagnosis.
///
/// Parses Exception Class (EC), Data Fault Status Code (DFSC), and
/// Fault Address (FAR) to generate precise crash descriptions.
pub fn decode_esr(esr: u64, far: u64, crash_func: Option<&str>) -> (String, Vec<String>) {
    let ec = (esr >> 26) & 0x3F;      // Exception Class
    let iss = esr & 0x00FFFFFF;        // Instruction Specific Syndrome
    let dfsc = iss & 0x3F;             // Data Fault Status Code
    let is_write = (iss >> 6) & 1;     // Write-not-Read
    let s1ptw = (iss >> 7) & 1;        // Stage-1 translation fault

    let func_info = crash_func.map(|f| format!(" in `{}`", f)).unwrap_or_default();
    let access_type = if is_write == 1 { "WRITE" } else { "READ" };

    let (summary, mut causes) = match (ec, dfsc, far) {
        // ===== Data Abort (0x24 = from LowerEL, 0x25 = from CurrentEL) =====
        (0x24 | 0x25, 0b000111, 0) => {
            ("NULL pointer dereference".into(), vec![
                format!("Fault address is 0x0 — null pointer access ({})", access_type),
                "Translation fault at page table L3 (page not mapped)".into(),
            ])
        }
        (0x24 | 0x25, 0b000100..=0b000111, _) => {
            let level = dfsc & 0b11;
            (format!("Translation fault at L{}", level), vec![
                format!("Address {:#x} is not mapped in page tables", far),
                format!("Access type: {}", access_type),
            ])
        }
        (0x24 | 0x25, 0b001001..=0b001011, _) => {
            (format!("Permission fault{}", func_info), vec![
                format!("{} at address {:#x}", access_type, far),
                if is_write == 1 { "Write to read-only memory page".into() } else { "Read from non-readable page".into() },
            ])
        }
        (0x24 | 0x25, 0b010000, _) => {
            ("Alignment fault".into(), vec![format!("Misaligned access at {:#x}", far)])
        }
        (0x24 | 0x25, 0b001100, _) => {
            ("Address size fault".into(), vec![format!("Address {:#x} exceeds VA range", far)])
        }
        (0x24 | 0x25, _, _) => {
            (format!("Data Abort{} at PC={:#018x}", func_info, far), vec![
                format!("FAR={:#x}, DFSC={:#06b}", far, dfsc),
                format!("Access type: {}", access_type),
            ])
        }
        // ===== Undefined Instruction (EC = 0x22) =====
        (0x22, _, _) => {
            ("Undefined instruction".into(), vec![
                "CPU attempted to execute an undefined instruction".into(),
                "Possible: instruction set mismatch or code corruption".into(),
            ])
        }
        // ===== Instruction Abort (EC = 0x20, 0x21) =====
        (0x20 | 0x21, _, _) => {
            (format!("Instruction fetch fault{}", func_info), vec![
                format!("Failed to fetch instruction from address {:#x}", far),
            ])
        }
        // ===== PC Alignment Fault (EC = 0x23) =====
        (0x23, _, _) => {
            ("PC alignment fault".into(), vec![
                "Attempted to execute code at non-4-byte-aligned address".into(),
            ])
        }
        // ===== SError (EC = 0x30) =====
        (0x30, _, _) => {
            ("SError (System Error)".into(), vec![
                "Hardware error detected — check for RAS or bus errors".into(),
            ])
        }
        // ===== Unknown =====
        _ => {
            (format!("Unknown exception (EC={:#04x}){}", ec, func_info), vec![
                "Refer to ARM Architecture Reference Manual for ESR decoding".into(),
            ])
        }
    };

    // 对 Data Abort 添加通用辅助信息
    if (ec == 0x24 || ec == 0x25) && s1ptw == 1 {
        causes.push("Stage-1 page table walk fault during translation".into());
    }

    (summary, causes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::storage::VcpuRegsEntry;

    fn make_vmcore(event: &str) -> VmcoreFile {
        VmcoreFile {
            vmcore_version: "1.0".into(),
            timestamp: "20260608T120000".into(),
            target_vm_id: 1,
            crash_event: event.into(),
            vcpu_count: 1,
            registers: vec![VcpuRegsEntry {
                vcpu_id: 0,
                gpr: {
                    let mut g = [0u64; 31];
                    g[29] = 0x9000;
                    g
                },
                sp_el0: 0x9000,
                elr_el1: 0xffff_0000_0000_1234,
                spsr_el1: 0x3c5,
            }],
            memory_dump_offset: None,
            kernel_log: None,
        }
    }

    #[test]
    fn test_analyze_panic() {
        let vmcore = make_vmcore("Panic");
        let result = analyze(&vmcore, &|_| None, None);
        assert_eq!(result.crash_event, "Panic");
        assert!(result.summary.contains("panic"));
        assert_eq!(result.backtrace.len(), 1);
    }

    #[test]
    fn test_analyze_exception() {
        let vmcore = make_vmcore("Exception");
        let result = analyze(&vmcore, &|_| None, None);
        assert!(result.summary.contains("exception"));
    }
}

