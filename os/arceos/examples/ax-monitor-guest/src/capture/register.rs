//! Target Guest vCPU register capture via HVC hypercalls.
//! Uses ax_hal::mem::virt_to_phys for correct VA→GPA translation.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use ax_hal::mem::{virt_to_phys, VirtAddr};
use serde::{Deserialize, Serialize};

#[repr(C)]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrashVcpuRegs {
    pub gpr: [u64; 31],
    pub sp_el0: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
    /// Exception Syndrome Register (ESR_EL1)
    pub esr_el1: u64,
    /// Fault Address Register (FAR_EL1)
    pub far_el1: u64,
    /// Crash type: 0=None, 1=DataAbort, 2=UndefinedInstr, 3=InstrAbort, 4=PCAlign, 5=SError
    pub crash_type: u8,
    /// Padding for 8-byte alignment
    pub _padding: [u8; 7],
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn hvc_call(code: u64, x1: u64, x2: u64, x3: u64, x4: u64, x5: u64) -> u64 {
    let result: u64;
    unsafe {
        asm!("hvc #0", inout("x0") code => result, in("x1") x1, in("x2") x2, in("x3") x3, in("x4") x4, in("x5") x5, options(nostack));
    }
    result
}

#[cfg(not(target_arch = "aarch64"))]
fn hvc_call(_code: u64, _x1: u64, _x2: u64, _x3: u64, _x4: u64, _x5: u64) -> u64 { u64::MAX }

pub fn freeze_target(target_vm_id: u64) -> Result<(), String> {
    let ret = hvc_call(7, target_vm_id, 0, 0, 0, 0);
    if ret == 0 { Ok(()) } else { Err(format!("CrashFreezeGuest failed on VM[{}], ret={}", target_vm_id, ret)) }
}

pub fn read_vcpu_regs(target_vm_id: u64, target_vcpu_id: u64) -> Result<CrashVcpuRegs, String> {
    let buf_size = core::mem::size_of::<CrashVcpuRegs>();
    let page_aligned_size = (buf_size + 4095) / 4096 * 4096;
    let buf = alloc::vec![0u64; page_aligned_size / 8];
    let buf_addr = buf.as_ptr() as usize;
    let vaddr = VirtAddr::from_usize(buf_addr);
    let gpa = virt_to_phys(vaddr).as_usize();
    let ret = hvc_call(8, target_vm_id, target_vcpu_id, gpa as u64, 0, 0);
    if ret != 0 { return Err(format!("CrashReadGuestRegs failed on VM[{}] VCpu[{}], ret={}", target_vm_id, target_vcpu_id, ret)); }
    let regs = unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const CrashVcpuRegs) };
    Ok(regs)
}

pub fn freeze_and_read_all(target_vm_id: u64, vcpu_count: u64) -> Result<Vec<(u64, CrashVcpuRegs)>, String> {
    freeze_target(target_vm_id)?;
    let mut results = Vec::new();
    for vcpu_id in 0..vcpu_count {
        match read_vcpu_regs(target_vm_id, vcpu_id) {
            Ok(regs) => {
            ax_std::println!("[register] VM[{}] VCpu[{}] ELR={:#018x} ESR={:#x} FAR={:#x}",
                target_vm_id, vcpu_id, regs.elr_el1, regs.esr_el1, regs.far_el1);
                results.push((vcpu_id, regs));
            }
            Err(e) => {
                ax_std::println!("[register] VM[{}] VCpu[{}] read failed: {}", target_vm_id, vcpu_id, e);
                break;
            }
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_size() {
        // 31*8 + sp_el0(8) + elr_el1(8) + spsr_el1(8) + esr_el1(8) + far_el1(8) + crash_type(1) + _padding(7) = 280
        assert_eq!(core::mem::size_of::<CrashVcpuRegs>(), 31 * 8 + 8 + 8 + 8 + 8 + 8 + 1 + 7);
    }
    #[test]
    fn test_default() {
        let r = CrashVcpuRegs::default();
        assert_eq!(r.gpr.len(), 31);
        assert_eq!(r.sp_el0, 0);
    }
}
