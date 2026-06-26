//! Interactive crash analysis console.
//!
//! Provides a command-line interface on the UART serial port after a crash
//! has been analysed.  Commands: `bt`, `regs`, `info`, `dmesg`, `modules`,
//! `memory`, `help`, `quit`.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::capture::storage::VmcoreFile;
use crate::recovery::analyzer::AnalysisResult;
use crate::recovery::symbol::SymbolTable;

/// Linux ARM64 with 48-bit VA: PAGE_OFFSET = 0xffff_0000_0000_0000
const PHYS_VIRT_OFFSET: u64 = 0xffff_0000_0000_0000;

const BANNER: &str = "\
═══════════════════════════════════════════════════════════
  Crash Analysis — Interactive Console
  Type 'help' for available commands.

  ⚠  axvisor 的调试信息可能与控制台输出交错。
     建议将 axvisor 日志级别设为 error 以减少干扰：
       LOG=error cargo xtask axvisor qemu --arch aarch64 ...
═══════════════════════════════════════════════════════════";

const HELP: &str = "\
Available commands:
  bt              — print call stack (backtrace)
  regs            — print CPU registers + ESR decoding
  info            — print analysis summary + data-structure checks
  dmesg           — print kernel log buffer
  modules         — print loaded kernel modules
  memory <addr>   — read 8 bytes at a guest-physical address
  help            — print this message
  quit            — exit interactive mode";

/// Enter the interactive analysis shell.
pub fn interactive_shell(
    vmcore: &VmcoreFile,
    mem: &impl Fn(u64) -> Option<u64>,
    _sym: Option<&SymbolTable>,
    result: &AnalysisResult,
) {
    ax_std::println!("{}", BANNER);
    loop {
        ax_std::print!("crash> ");
        let line = read_line();
        let line = line.trim();

        match line {
            "bt" => cmd_bt(result),
            "regs" => cmd_regs(result),
            "info" => cmd_info(result),
            "dmesg" => cmd_dmesg(vmcore),
            "modules" => cmd_modules(vmcore),
            s if s.starts_with("memory") => cmd_memory(s, mem),
            "help" => ax_std::println!("{}", HELP),
            "quit" => {
                ax_std::println!("[console] exit");
                break;
            }
            "" => {}
            _ => ax_std::println!("Unknown command: '{}'. Type 'help'.", line),
        }
        // Yield to flush any buffered UART TX output before the next prompt.
        ax_std::thread::sleep(core::time::Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Command implementations
//
// Each command prints a clear header (── cmd ──) before its output so that
// even if axvisor log messages interleave, the user can still identify the
// start of the console response.
// ---------------------------------------------------------------------------

fn cmd_bt(result: &AnalysisResult) {
    ax_std::println!("── bt ──");
    if result.backtrace.is_empty() {
        ax_std::println!("  (no backtrace available)");
        return;
    }
    for (i, frame) in result.backtrace.iter().enumerate() {
        let func = frame.func_name.as_deref().unwrap_or("<unknown>");
        let off = frame.func_offset.map(|o| format!("+{}", o)).unwrap_or_default();
        if frame.pc != 0 {
            ax_std::println!("  #{:<3} {:#018x}  {}{}", i, frame.pc, func, off);
        } else {
            // PC not available (module symbol or dmesg-extracted frame)
            ax_std::println!("  #{:<3}                  {}{}", i, func, off);
        }
    }
}

fn cmd_regs(result: &AnalysisResult) {
    ax_std::println!("── regs ──");
    for (name, val) in &result.key_registers {
        let decoded = if name == "ESR_EL1" {
            decode_esr_short(*val)
        } else {
            String::new()
        };
        ax_std::println!("  {:<12} {:#018x} {}", name, val, decoded);
    }
}

fn cmd_info(result: &AnalysisResult) {
    ax_std::println!("── info ──");
    ax_std::println!("  Event:     {}", result.crash_event);
    ax_std::println!("  PC:        {:#018x}", result.crash_pc);
    if let Some(ref func) = result.crash_function {
        let off = result.crash_function_offset.map(|o| format!("+{}", o)).unwrap_or_default();
        ax_std::println!("  Function:  {}{}", func, off);
    }
    ax_std::println!("  Summary:   {}", result.summary);
    ax_std::println!("  Process:   {} (PID: {:?})", result.process.name, result.process.pid);
}

fn cmd_dmesg(vmcore: &VmcoreFile) {
    if let Some(ref log) = vmcore.kernel_log {
        ax_std::println!("{}", log);
        return;
    }
    // Fallback: try loading from separate .log file (written by storage.rs)
    let log_file = alloc::format!("vmcore_{}_{}.log", vmcore.timestamp, vmcore.crash_event);
    let log_path = alloc::format!("/vmcore/{}", log_file);
    if let Ok(content) = ax_std::fs::read_to_string(&log_path) {
        if !content.is_empty() {
            ax_std::println!("{}", content);
            return;
        }
    }
    ax_std::println!("(kernel log not captured)");
}

fn cmd_modules(vmcore: &VmcoreFile) {
    if vmcore.modules.is_empty() {
        ax_std::println!("(no kernel modules loaded or not captured)");
        return;
    }
    ax_std::println!("Loaded kernel modules:");
    for m in &vmcore.modules {
        if m.base_addr != 0 {
            ax_std::println!("  {:<20} @ {:#018x} ({} bytes)", m.name, m.base_addr, m.size);
        } else {
            ax_std::println!("  {}", m.name);
        }
    }
}

fn cmd_memory(arg: &str, mem: &impl Fn(u64) -> Option<u64>) {
    let args: Vec<&str> = arg.split_whitespace().collect();
    let addr = match args.get(1).and_then(|s| parse_u64(s)) {
        Some(a) => a,
        None => {
            ax_std::println!("Usage: memory <hex_addr>");
            return;
        }
    };
    let gpa = if addr >= PHYS_VIRT_OFFSET {
        addr.wrapping_sub(PHYS_VIRT_OFFSET)
    } else {
        addr
    };
    match mem(gpa) {
        Some(val) => ax_std::println!("  {:#018x} = {:#018x}", addr, val),
        None => ax_std::println!("  could not read {:#018x}", addr),
    }
}

// ---------------------------------------------------------------------------
// I/O helpers
//
// Characters are read directly from the PL011 UART via MMIO (same as
// ax_std::println! writes to TX).  This avoids HVC #12 entirely, which
// eliminates the VM-exit / UART contention issue between the monitor
// guest (EL1) and the hypervisor (EL2).
//
// PL011 register offsets (QEMU virt base = 0x900_0000):
//   DR   = 0x000  (Data Register:   write=TX, read=RX)
//   FR   = 0x018  (Flag Register)
//   FR_RXFE = bit 4  (RX FIFO Empty)
// ---------------------------------------------------------------------------

/// PL011 physical base address (QEMU virt platform).
const PL011_PBASE: u64 = 0x900_0000;

/// Lazily-initialized PL011 virtual base address.
fn pl011_vbase() -> u64 {
    use ax_hal::mem::{phys_to_virt, PhysAddr};
    phys_to_virt(PhysAddr::from_usize(PL011_PBASE as usize)).as_usize() as u64
}

/// Read a line from the console (blocking, with local echo).
fn read_line() -> String {
    let mut buf = String::new();
    let vbase = pl011_vbase();
    loop {
        let ch = read_char_direct(vbase);
        match ch {
            '\0' => {
                // No data available — yield to let QEMU deliver the character.
                ax_std::thread::sleep(core::time::Duration::from_millis(50));
            }
            '\r' | '\n' => {
                ax_std::println!();
                return buf;
            }
            '\x08' | '\x7f' => {
                if buf.pop().is_some() {
                    ax_std::print!("\x08 \x08");
                }
            }
            c if c.is_ascii_control() => {}
            c => {
                buf.push(c);
                ax_std::print!("{}", c);
            }
        }
    }
}

/// Read a single character directly from the PL011 UART RX FIFO.
/// `vbase` is the virtual address of the PL011 registers.
/// Returns '\0' if no character is available.
fn read_char_direct(vbase: u64) -> char {
    let fr_ptr = (vbase + 0x018) as *const u32;
    let dr_ptr = vbase as *const u32;
    unsafe {
        let fr = core::ptr::read_volatile(fr_ptr);
        if fr & (1 << 4) == 0 {
            // RXFE == 0 → data available
            let dr = core::ptr::read_volatile(dr_ptr);
            ((dr & 0xFF) as u8) as char
        } else {
            '\0'
        }
    }
}

fn parse_u64(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.starts_with("0x") || s.starts_with("0X") {
        u64::from_str_radix(&s[2..], 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

// ---------------------------------------------------------------------------
// ESR short decoder
// ---------------------------------------------------------------------------

fn decode_esr_short(esr: u64) -> String {
    if esr == 0 {
        return String::new();
    }
    let ec = (esr >> 26) & 0x3F;
    let dfsc = esr & 0x3F;
    let is_write = (esr >> 6) & 1;
    let access = if is_write == 1 { "WRITE" } else { "READ" };

    let ec_name = match ec {
        0x20 | 0x21 => "InstrAbort",
        0x22 => "UndefinedInstr",
        0x23 => "PCAlignFault",
        0x24 | 0x25 => "DataAbort",
        0x30 => "SError",
        _ => "Unknown",
    };

    if ec == 0x24 || ec == 0x25 {
        let dfsc_name = match dfsc {
            0b000100..=0b000111 => "Translation fault",
            0b001001..=0b001011 => "Access flag fault",
            0b001101..=0b001111 => "Permission fault",
            0b010000 => "Alignment fault",
            _ => "",
        };
        format!("({} {} {})", ec_name, dfsc_name, access)
    } else {
        format!("({})", ec_name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_u64_hex() {
        assert_eq!(parse_u64("0x1234"), Some(0x1234));
        assert_eq!(parse_u64("0x0"), Some(0));
    }

    #[test]
    fn test_parse_u64_dec() {
        assert_eq!(parse_u64("1234"), Some(1234));
    }

    #[test]
    fn test_parse_u64_invalid() {
        assert_eq!(parse_u64("not_a_number"), None);
    }

    #[test]
    fn test_decode_esr_null_ptr() {
        let s = decode_esr_short(0x96000044);
        assert!(s.contains("DataAbort"));
        assert!(s.contains("READ"));
    }

    #[test]
    fn test_decode_esr_zero() {
        assert_eq!(decode_esr_short(0), "");
    }
}
