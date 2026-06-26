//! High-level crash analysis engine.
//!
//! Combines register state, call stack, process context, and symbol
//! information to produce a structured `AnalysisResult` that describes
//! the crash root cause and provides diagnostic hints.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::storage::VmcoreFile;
use crate::recovery::dstruct::{check_dstructs, DstructResult};
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
    /// Data-structure sanity checks (stack, current_task, preempt, etc.).
    pub dstruct_result: DstructResult,
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
        .map(|regs| crate::recovery::process::identify(regs, &backtrace, sym, mem))
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
            let keys = vec![
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

    // 优先使用 ESR/FAR 解码；如果 HVC #13 未传递真实值（ESR=0），
    // 尝试从 dmesg 文本中正则提取 ESR/FAR。
    // storage.rs 将 kernel_log 存为单独 .log 文件，JSON 中为 None
    let (esr, far) = {
        let log_text = vmcore.kernel_log.as_deref().map(|s| alloc::string::String::from(s))
            .or_else(|| {
                let log_file = alloc::format!("/vmcore/vmcore_{}_{}.log", vmcore.timestamp, vmcore.crash_event);
                ax_std::fs::read_to_string(&log_file).ok()
            });
        match (primary_vcpu, log_text) {
            (Some(regs), _) if regs.esr_el1 != 0 => (regs.esr_el1, regs.far_el1),
            (_, Some(ref text)) => extract_esr_far_from_dmesg(text),
            _ => (0, 0),
        }
    };
    // 更新 key_registers 中的 ESR/FAR 为最终用于诊断的值
    let key_registers = {
        let mut kr = key_registers;
        for (name, val) in kr.iter_mut() {
            if *name == "ESR_EL1" { *val = esr; }
            if *name == "FAR_EL1" { *val = far; }
        }
        kr
    };

    let (summary, possible_causes) = if esr != 0 {
        // 有真实 ESR/FAR：decode_esr 给出精准异常解码，再加事件上下文
        let (esr_summary, esr_causes) = decode_esr(esr, far, crash_function.as_deref());
        let ctx_summary = generate_diagnosis(&vmcore.crash_event, crash_pc, crash_function.as_deref(), &backtrace).0;
        (alloc::format!("{} — {}", esr_summary, ctx_summary), esr_causes)
    } else {
        generate_diagnosis(&vmcore.crash_event, crash_pc, crash_function.as_deref(), &backtrace)
    };

    // Step 5 — Fallback: if FP unwind gave <=1 valid frame, try to extract backtrace from dmesg.
    let backtrace_valid = backtrace.len() > 1
        && backtrace.last().map_or(false, |f| {
            // Valid frames have PC in the kernel address space,
            // not in the 0xfffffffffffffffc range or 0.
            (f.pc >= 0xffff_0000_0000_0000u64 && f.pc < 0xffff_8001_0000_0000u64)
                || f.pc == 0 // 0 means dmesg-extracted (no PC available)
        });
    let backtrace = if !backtrace_valid {
        let log_text = get_log_text(vmcore);
        if let Some(ref text) = log_text {
            let dmesg_bt = extract_backtrace_from_dmesg(text, sym);
            if dmesg_bt.len() > 1 {
                ax_std::println!("[recovery] backtrace extracted from dmesg ({} frames)", dmesg_bt.len());
                dmesg_bt
            } else if dmesg_bt.len() == 1 {
                ax_std::println!("[recovery] backtrace from dmesg (1 frame)");
                dmesg_bt
            } else {
                backtrace
            }
        } else {
            backtrace
        }
    } else {
        backtrace
    };

    // Step 5c — Data-structure sanity checks.
    let dstruct_result = primary_vcpu
        // Pass the dmesg-extracted ESR if register ESR is 0, so that
        // exception-nesting detection sees the real exception class.
        .map(|regs| check_dstructs(regs, sym, mem, if esr != 0 { Some(esr) } else { None }))
        .unwrap_or(DstructResult {
            sp_in_stack: None,
            current_task_valid: None,
            irqs_masked: None,
            exception_nested: None,
            sp_aligned: None,
            details: alloc::vec![],
        });

    // Step 6 — Fallback: if PID is None, try to extract from dmesg.
    let process = if process.pid.is_none() {
        let log_text = get_log_text(vmcore);
        if let Some(ref text) = log_text {
            let (pid, comm) = extract_pid_from_dmesg(text);
            if pid.is_some() {
                ProcessInfo {
                    pid,
                    name: comm.unwrap_or_else(|| process.name.clone()),
                    state: process.state.clone(),
                    is_kernel_thread: process.is_kernel_thread,
                    cpu_id: process.cpu_id,
                }
            } else {
                process
            }
        } else {
            process
        }
    } else {
        process
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
        dstruct_result,
    }
}

// ---------------------------------------------------------------------------
// Dmesg text helpers
// ---------------------------------------------------------------------------

/// Get the kernel log text from the vmcore (either inline or from separate log file).
fn get_log_text(vmcore: &VmcoreFile) -> Option<String> {
    vmcore.kernel_log.as_ref().map(|s| s.clone())
        .or_else(|| {
            let log_file = alloc::format!(
                "/vmcore/vmcore_{}_{}.log",
                vmcore.timestamp,
                vmcore.crash_event,
            );
            ax_std::fs::read_to_string(&log_file).ok()
        })
}

/// Try to extract ESR and FAR from kernel log (dmesg) text.
///
/// For hardware-triggered panics (NULL pointer, Data Abort) the kernel
/// prints ESR and FAR before calling panic().  When HVC #13 delivers
/// ESR=0 (because the hardware exception was handled in EL1 and never
/// reached the hypervisor), this function provides a fallback by parsing
/// the prb dump text.
///
/// Returns `(esr, far)`, both 0 if parsing fails (software-triggered
/// panic like SysRq or BUG()).
fn extract_esr_far_from_dmesg(dmesg: &str) -> (u64, u64) {
    // ESR 行: "  ESR = 0x0000000096000044"  (prb 污染可能末尾附多余字符)
    let esr = dmesg
        .lines()
        .find(|l| l.contains("ESR"))
        .and_then(|l| {
            l.split_once("0x")
                .and_then(|(_, hex)| {
                    let clean: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
                    // 截断到前 16 位（64-bit），防 prb 污染
                    u64::from_str_radix(&clean[..clean.len().min(16)], 16).ok()
                })
        })
        .unwrap_or(0);

    // FAR 行: "at virtual address 0000000000000000" (无 0x，无前缀)
    let far = dmesg
        .lines()
        .find(|l| l.contains("virtual address"))
        .and_then(|l| {
            let after = l.split_once("virtual address").map(|(_, s)| s).unwrap_or("");
            let clean: String = after.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            u64::from_str_radix(&clean[..clean.len().min(16)], 16).ok()
        })
        .unwrap_or(0);

    (esr, far)
}

/// Helper: strip the kernel log timestamp prefix `[...]` from a line.
/// Returns the line content after the timestamp, or the original line if no
/// timestamp is found.
fn strip_timestamp(line: &str) -> &str {
    let t = line.trim();
    if t.starts_with('[') {
        if let Some(end) = t.find(']') {
            let after = t[end + 1..].trim();
            if !after.is_empty() {
                return after;
            }
        }
    }
    t
}

/// Try to extract the kernel backtrace (Call trace) from dmesg text.
///
/// The Linux kernel prints a "Call trace:" section during Oops output:
/// ```text
/// [    0.123456] Call trace:
/// [    0.123456]  crash_null+0x28/0x40
/// [    0.123456]  execute_crash+0x14/0x30
/// ```
///
/// Each line has the format: ` func_name+offset/length` (optional `[module]`).
fn extract_backtrace_from_dmesg(dmesg: &str, sym: Option<&SymbolTable>) -> Vec<StackFrame> {
    let mut frames: Vec<StackFrame> = Vec::new();
    let mut in_call_trace = false;
    let kernel_base = sym.map(|s| s.kernel_base).unwrap_or(0);

    for line in dmesg.lines() {
        let content = strip_timestamp(line);
        let trimmed = content.trim();

        // Detect "Call trace:" (case-insensitive)
        if trimmed.eq_ignore_ascii_case("Call trace:") || trimmed.eq_ignore_ascii_case("Call Trace:") {
            in_call_trace = true;
            continue;
        }

        if in_call_trace {
            // Stop at empty line or next section header (line contains colon, no +).
            if trimmed.is_empty() || (trimmed.contains(':') && !trimmed.contains('+')) {
                break;
            }

            // Parse: ` func_name+0xXX/0xYY [module]`
            // Extract module name from trailing [module_name]
            let (core, _module_suffix) = trimmed.split_once(" [")
                .map(|(c, m)| {
                    let m = m.trim_end_matches(']');
                    (c, Some(m.to_string()))
                })
                .unwrap_or((trimmed, None));

            let func_name = core.split('+').next()
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| core.to_string());

            // Extract offset from offset/length (e.g. "+172/0x200")
            let func_offset = core.split('+').nth(1)
                .and_then(|part| part.split('/').next())
                .and_then(|off_str| u64::from_str_radix(off_str.trim_start_matches("0x"), 16).ok());

            // Only add frames with recognizable function names
            if !func_name.is_empty() && (func_name.contains('_') || func_name.as_bytes().first().map_or(false, |c| c.is_ascii_alphabetic())) {
                // Try to resolve PC from kallsyms.
                // Module symbols are stored as "func_name [module_name]" in kallsyms.
                let pc = sym.and_then(|s| {
                    // Try exact name first (kernel symbols)
                    s.lookup_name(&func_name)
                        // If not found, try with module suffix
                        .or_else(|| {
                            _module_suffix.as_ref().and_then(|mod_name| {
                                let full_name = alloc::format!("{} [{}]", func_name, mod_name);
                                s.lookup_name(&full_name)
                            })
                        })
                        .map(|info| {
                            let sym_addr = info.addr;
                            kernel_base + sym_addr + func_offset.unwrap_or(0)
                        })
                }).unwrap_or(0);

                frames.push(StackFrame {
                    pc,
                    sp: 0,
                    fp: 0,
                    func_name: Some(func_name),
                    func_offset,
                });
            }
        }
    }

    frames
}

/// Try to extract PID and process name from dmesg text.
///
/// The Linux kernel prints during Oops:
/// ```text
/// [    0.123456] CPU: 0 PID: 42 Comm: insmod Not tainted 6.12.94 #1
/// ```
fn extract_pid_from_dmesg(dmesg: &str) -> (Option<u64>, Option<String>) {
    for line in dmesg.lines() {
        let content = strip_timestamp(line);
        let line_str = content.trim();

        // Match lines containing both "CPU:" and "PID:" (or just "PID:" / "Comm:")
        // Kernel format: "CPU: 0 PID: 42 Comm: insmod Not tainted 6.12.94 #1"
        // After whitespace split: ["CPU:", "0", "PID:", "42", "Comm:", "insmod", ...]
        if line_str.starts_with("CPU:") || line_str.contains("PID:") || line_str.contains("Comm:") {
            let tokens: Vec<&str> = line_str.split_whitespace().collect();
            let mut pid: Option<u64> = None;
            let mut comm: Option<String> = None;

            // Walk tokens looking for PID: / Comm: followed by their value
            for i in 0..tokens.len() {
                if tokens[i] == "PID:" {
                    if i + 1 < tokens.len() {
                        pid = tokens[i + 1].parse::<u64>().ok();
                    }
                } else if tokens[i] == "Comm:" {
                    if i + 1 < tokens.len() {
                        comm = Some(tokens[i + 1].to_string());
                    }
                } else if let Some(rest) = tokens[i].strip_prefix("PID:") {
                    if !rest.is_empty() {
                        pid = rest.parse::<u64>().ok();
                    } else if i + 1 < tokens.len() {
                        pid = tokens[i + 1].parse::<u64>().ok();
                    }
                } else if let Some(rest) = tokens[i].strip_prefix("Comm:") {
                    if !rest.is_empty() {
                        comm = Some(rest.to_string());
                    } else if i + 1 < tokens.len() {
                        comm = Some(tokens[i + 1].to_string());
                    }
                }
            }

            if pid.is_some() {
                return (pid, comm);
            }
        }

        // Also try "Process: name (pid: 42)" format
        if line_str.starts_with("Process:") {
            let parts: Vec<&str> = line_str.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                let name = parts[1].to_string();
                let pid = if parts.len() >= 3 {
                    let rest = parts[2].trim_start_matches('(');
                    rest.trim_end_matches(')')
                        .strip_prefix("pid:")
                        .and_then(|s| s.trim().parse::<u64>().ok())
                } else {
                    None
                };
                return (pid, Some(name));
            }
        }
    }

    (None, None)
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
                esr_el1: 0,
                far_el1: 0,
                crash_type: 0,
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
