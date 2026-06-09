//! AArch64 stack unwinding via Frame Pointer (FP / x29) chain.
//!
//! AArch64 calling convention:
//! - `x29` = Frame Pointer (points to saved FP|LR pair on stack)
//! - `x30` = Link Register (return address)
//! - `SP`  = Stack Pointer
//! - `ELR` = Exception Link Register (crash PC)
//!
//! Stack frame layout (high → low address):
//! ```text
//!   [saved LR (x30)]   ← FP + 8
//!   [saved FP (x29)]   ← FP points here
//!   [local vars]
//!   [SP]               ← SP points here
//! ```

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capture::storage::VcpuRegsEntry;
use crate::recovery::symbol::SymbolTable;
use serde::{Deserialize, Serialize};

/// A single frame in the reconstructed call stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    /// Program counter (instruction address) at this frame.
    pub pc: u64,
    /// Stack pointer at this frame.
    pub sp: u64,
    /// Frame pointer (x29) at this frame.
    pub fp: u64,
    /// Resolved function name (if symbol table is available).
    pub func_name: Option<String>,
    /// Offset from the function start (if symbol was found).
    pub func_offset: Option<u64>,
}

/// Maximum frames to unwind (safety limit against infinite loops).
const MAX_FRAMES: usize = 128;

/// Unwind the call stack from a crashed vCPU's register state.
///
/// Uses the frame pointer (FP / x29) chain walking. Works when the kernel
/// is compiled with `-fno-omit-frame-pointer`.
///
/// # Arguments
///
/// * `regs` — Register state of the crashed vCPU.
/// * `mem`  — Closure reading 8 bytes from a guest physical address.
///            Pass `&\|_\| None` if memory dump is unavailable (returns only 1 frame).
/// * `sym`  — Optional symbol table for function name resolution.
///
/// # Returns
///
/// Frames from the crash site (innermost) outward. Empty vec if chain is corrupt.
pub fn unwind_stack(
    regs: &VcpuRegsEntry,
    mem: &impl Fn(u64) -> Option<u64>,
    sym: Option<&SymbolTable>,
) -> Vec<StackFrame> {
    let mut frames = Vec::new();
    let kernel_base = sym.map(|s| s.kernel_base).unwrap_or(0);

    // --- Frame 0: crash site (PC from ELR_EL1) ---
    let crash_frame = StackFrame {
        pc: regs.elr_el1,
        sp: regs.sp_el0,
        fp: regs.gpr[29],
        func_name: sym.and_then(|s| s.lookup(regs.elr_el1)).map(|si| si.name.clone()),
        func_offset: sym
            .and_then(|s| s.nearest(regs.elr_el1))
            .map(|si| regs.elr_el1.wrapping_sub(kernel_base) - si.addr),
    };
    frames.push(crash_frame);

    // --- Walk the FP chain ---
    let mut fp = regs.gpr[29];

    for _ in 0..MAX_FRAMES {
        if fp == 0 || fp & 0x7 != 0 {
            break;
        }

        let (saved_fp, saved_lr) = match (mem(fp), mem(fp + 8)) {
            (Some(f), Some(lr)) => (f, lr),
            _ => break,
        };

        let pc = saved_lr.wrapping_sub(4);
        let sp = fp + 16;

        frames.push(StackFrame {
            pc,
            sp,
            fp: saved_fp,
            func_name: sym.and_then(|s| s.lookup(pc)).map(|si| si.name.clone()),
            func_offset: sym
                .and_then(|s| s.nearest(pc))
                .map(|si| pc.wrapping_sub(kernel_base) - si.addr),
        });

        fp = saved_fp;
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unwind_no_memory() {
        let regs = VcpuRegsEntry {
            vcpu_id: 0,
            gpr: [0; 31],
            sp_el0: 0x1000,
            elr_el1: 0xffff_0000_0000_1234,
            spsr_el1: 0x3c5,
        };
        let frames = unwind_stack(&regs, &|_| None, None);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].pc, 0xffff_0000_0000_1234);
    }

    #[test]
    fn test_unwind_with_fp_chain() {
        // Simulate a simple FP chain:
        //   Frame 0: FP=0x9000, PC=0x1000
        //   Frame 1: saved at [0x9000] = FP=0x8000, [0x9008] = LR=0x2004
        //   Frame 2: saved at [0x8000] = FP=0x0,     [0x8008] = LR=0x3004
        let mem = |addr: u64| -> Option<u64> {
            match addr {
                0x9000 => Some(0x8000), // saved FP
                0x9008 => Some(0x2004), // saved LR (will become PC=0x2000)
                0x8000 => Some(0x0),    // saved FP → null terminator
                0x8008 => Some(0x3004), // saved LR → PC=0x3000
                _ => None,
            }
        };

        let regs = VcpuRegsEntry {
            vcpu_id: 0,
            gpr: {
                let mut g = [0u64; 31];
                g[29] = 0x9000; // FP = x29
                g
            },
            sp_el0: 0x9000,
            elr_el1: 0x1000,
            spsr_el1: 0x3c5,
        };

        let frames = unwind_stack(&regs, &mem, None);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].pc, 0x1000);
        assert_eq!(frames[1].pc, 0x2000);
        assert_eq!(frames[2].pc, 0x3000);
    }
}
