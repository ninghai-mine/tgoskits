//! Target Guest vCPU register capture via HVC hypercalls.
//! Uses ax_hal::mem::virt_to_phys for correct VA→GPA translation.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use ax_hal::mem::{virt_to_phys, VirtAddr};

use super::hvc::hvc_call;
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

/// Read a chunk of guest physical memory from a frozen target VM via HVC #9 CrashReadGuestMem.
pub fn read_guest_mem(target_vm_id: u64, target_gpa: u64, buffer: &mut [u8]) -> Result<usize, String> {
    let size = buffer.len();
    if size == 0 {
        return Ok(0);
    }
    // Translate buffer VA to GPA so the hypervisor can write into it.
    let buf_addr = buffer.as_ptr() as usize;
    let vaddr = VirtAddr::from_usize(buf_addr);
    let buf_gpa = virt_to_phys(vaddr).as_usize() as u64;
    let ret = hvc_call(9, target_vm_id, target_gpa, buf_gpa, size as u64, 0);
    if (ret as i64) < 0 {
        Err(format!("CrashReadGuestMem failed on VM[{}] GPA={:#x} size={} ret={}",
                     target_vm_id, target_gpa, size, ret))
    } else {
        Ok(ret as usize)
    }
}

/// Poll whether a target VM has crashed via HVC #10 PollCrashStatus.
/// Returns `true` if the target VM is stopped (crashed).
pub fn poll_crash_status(target_vm_id: u64) -> bool {
    let ret = hvc_call(10, target_vm_id, 0, 0, 0, 0);
    ret == 1
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
        // 31*8 + sp_el0(8) + elr_el1(8) + spsr_el1(8) + esr_el1(8) + far_el1(8) + crash_type(1) + _padding(7) = 296
        assert_eq!(core::mem::size_of::<CrashVcpuRegs>(), 31 * 8 + 8 + 8 + 8 + 8 + 8 + 1 + 7);
    }
    #[test]
    fn test_default() {
        let r = CrashVcpuRegs::default();
        assert_eq!(r.gpr.len(), 31);
        assert_eq!(r.sp_el0, 0);
    }
}
