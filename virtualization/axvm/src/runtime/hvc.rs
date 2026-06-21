// Copyright 2025 The Axvisor Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use alloc::format;
use ax_errno::{AxResult, ax_err, ax_err_type};
use axhvc::{HyperCallCode, HyperCallResult};
use crate::VMStatus;

use crate::{
    GuestPhysAddr, MappingFlags,
    manager,
    runtime::{
        VMRef,
        ivc::{self, IVCChannel},
    },
};

pub struct HyperCall {
    vm: VMRef,
    code: HyperCallCode,
    args: [u64; 6],
}

impl HyperCall {
    pub fn new(vm: VMRef, code: u64, args: [u64; 6]) -> AxResult<Self> {
        let code = HyperCallCode::try_from(code as u32).map_err(|e| {
            warn!("Invalid hypercall code: {code} e {e:?}");
            ax_err_type!(InvalidInput)
        })?;

        Ok(Self { vm, code, args })
    }

    pub fn execute(&self) -> HyperCallResult {
        match self.code {
            HyperCallCode::HIVCPublishChannel => {
                let key = self.args[0] as usize;
                let shm_base_gpa_ptr = GuestPhysAddr::from_usize(self.args[1] as usize);
                let shm_size_ptr = GuestPhysAddr::from_usize(self.args[2] as usize);

                info!(
                    "VM[{}] HyperCall {:?} key {:#x}",
                    self.vm.id(),
                    self.code,
                    key
                );
                // User will pass the size of the shared memory region,
                // we will allocate the shared memory region based on this size.
                let shm_region_size = self.vm.read_from_guest_of::<usize>(shm_size_ptr)?;
                ivc::ensure_channel_absent(self.vm.id(), key)?;
                let requested_size = shm_region_size.min(ivc::MAX_IVC_CHANNEL_SIZE);
                let (shm_base_gpa, shm_region_size) = self.vm.alloc_ivc_channel(requested_size)?;

                let ivc_channel =
                    match IVCChannel::alloc(self.vm.id(), key, shm_region_size, shm_base_gpa) {
                        Ok(channel) => channel,
                        Err(err) => {
                            if let Err(release_err) =
                                self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                            {
                                warn!(
                                    "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after \
                                     channel allocation failure: {release_err:?}",
                                    self.vm.id()
                                );
                            }
                            return Err(err);
                        }
                    };

                let actual_size = ivc_channel.size();

                if let Err(err) = self.vm.map_region(
                    shm_base_gpa,
                    ivc_channel.base_hpa(),
                    actual_size,
                    MappingFlags::READ | MappingFlags::WRITE,
                ) {
                    if let Err(release_err) =
                        self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                    {
                        warn!(
                            "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after mapping \
                             failure: {release_err:?}",
                            self.vm.id()
                        );
                    }
                    return Err(err);
                }

                if let Err(err) = self
                    .vm
                    .write_to_guest_of(shm_base_gpa_ptr, &shm_base_gpa.as_usize())
                    .and_then(|_| self.vm.write_to_guest_of(shm_size_ptr, &actual_size))
                {
                    if let Err(unmap_err) = self.vm.unmap_region(shm_base_gpa, actual_size) {
                        warn!(
                            "VM[{}] failed to unmap IVC GPA {shm_base_gpa:#x} after guest write \
                             failure: {unmap_err:?}",
                            self.vm.id()
                        );
                    }
                    if let Err(release_err) =
                        self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                    {
                        warn!(
                            "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after guest write \
                             failure: {release_err:?}",
                            self.vm.id()
                        );
                    }
                    return Err(err);
                }

                if let Err(err) = ivc::insert_channel(self.vm.id(), ivc_channel) {
                    if let Err(unmap_err) = self.vm.unmap_region(shm_base_gpa, actual_size) {
                        warn!(
                            "VM[{}] failed to unmap IVC GPA {shm_base_gpa:#x} after channel \
                             insert failure: {unmap_err:?}",
                            self.vm.id()
                        );
                    }
                    if let Err(release_err) =
                        self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                    {
                        warn!(
                            "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after channel \
                             insert failure: {release_err:?}",
                            self.vm.id()
                        );
                    }
                    return Err(err);
                }

                Ok(0)
            }
            HyperCallCode::HIVCUnPublishChannel => {
                let key = self.args[0] as usize;

                info!(
                    "VM[{}] HyperCall {:?} with key {:#x}",
                    self.vm.id(),
                    self.code,
                    key
                );
                let (base_gpa, size) = ivc::unpublish_channel(self.vm.id(), key)?;
                // The publisher's GPA mapping is always unmapped; subscribers keep their own
                // GPA views. The shared HPA frame is freed when the last subscriber leaves.
                self.vm.unmap_region(base_gpa, size)?;
                self.vm.release_ivc_channel(base_gpa, size)?;

                Ok(0)
            }
            HyperCallCode::HIVCSubscribChannel => {
                let publisher_vm_id = self.args[0] as usize;
                let key = self.args[1] as usize;
                let shm_base_gpa_ptr = GuestPhysAddr::from_usize(self.args[2] as usize);
                let shm_size_ptr = GuestPhysAddr::from_usize(self.args[3] as usize);

                info!(
                    "VM[{}] HyperCall {:?} to VM[{}]",
                    self.vm.id(),
                    self.code,
                    publisher_vm_id
                );

                let shm_size = ivc::prepare_subscribe_channel(publisher_vm_id, key, self.vm.id())?;
                let (shm_base_gpa, shm_region_size) = self.vm.alloc_ivc_channel(shm_size)?;

                let subscribe_result = ivc::subscribe_to_channel_of_publisher(
                    publisher_vm_id,
                    key,
                    self.vm.id(),
                    shm_base_gpa,
                );
                let (base_hpa, actual_size) = match subscribe_result {
                    Ok(channel) => channel,
                    Err(err) => {
                        if let Err(release_err) =
                            self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                        {
                            warn!(
                                "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after \
                                 subscribe registration failure: {release_err:?}",
                                self.vm.id()
                            );
                        }
                        return Err(err);
                    }
                };

                // TODO: seperate the mapping flags of metadata and data.
                if let Err(err) = self.vm.map_region(
                    shm_base_gpa,
                    base_hpa,
                    actual_size,
                    MappingFlags::READ | MappingFlags::WRITE,
                ) {
                    if let Err(unsub_err) = ivc::unsubscribe_from_channel_of_publisher(
                        publisher_vm_id,
                        key,
                        self.vm.id(),
                    ) {
                        warn!(
                            "VM[{}] failed to rollback IVC subscription to VM[{}] key {key:#x} \
                             after mapping failure: {unsub_err:?}",
                            self.vm.id(),
                            publisher_vm_id
                        );
                    }
                    if let Err(release_err) =
                        self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                    {
                        warn!(
                            "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after subscribe \
                             mapping failure: {release_err:?}",
                            self.vm.id()
                        );
                    }
                    return Err(err);
                }

                if let Err(err) = self
                    .vm
                    .write_to_guest_of(shm_base_gpa_ptr, &shm_base_gpa.as_usize())
                    .and_then(|_| self.vm.write_to_guest_of(shm_size_ptr, &actual_size))
                {
                    if let Err(unmap_err) = self.vm.unmap_region(shm_base_gpa, actual_size) {
                        warn!(
                            "VM[{}] failed to unmap IVC GPA {shm_base_gpa:#x} after subscribe \
                             guest write failure: {unmap_err:?}",
                            self.vm.id()
                        );
                    }
                    if let Err(unsub_err) = ivc::unsubscribe_from_channel_of_publisher(
                        publisher_vm_id,
                        key,
                        self.vm.id(),
                    ) {
                        warn!(
                            "VM[{}] failed to rollback IVC subscription to VM[{}] key {key:#x} \
                             after guest write failure: {unsub_err:?}",
                            self.vm.id(),
                            publisher_vm_id
                        );
                    }
                    if let Err(release_err) =
                        self.vm.release_ivc_channel(shm_base_gpa, shm_region_size)
                    {
                        warn!(
                            "VM[{}] failed to release IVC GPA {shm_base_gpa:#x} after subscribe \
                             guest write failure: {release_err:?}",
                            self.vm.id()
                        );
                    }
                    return Err(err);
                }

                info!(
                    "VM[{}] HyperCall HIVC_REGISTER_SUBSCRIBER success, base GPA: {:#x}, size: {}",
                    self.vm.id(),
                    shm_base_gpa,
                    actual_size
                );

                Ok(0)
            }
            HyperCallCode::HIVCUnSubscribChannel => {
                let publisher_vm_id = self.args[0] as usize;
                let key = self.args[1] as usize;

                info!(
                    "VM[{}] HyperCall {:?} from VM[{}]",
                    self.vm.id(),
                    self.code,
                    publisher_vm_id
                );
                let (base_gpa, size) =
                    ivc::unsubscribe_from_channel_of_publisher(publisher_vm_id, key, self.vm.id())?;
                self.vm.unmap_region(base_gpa, size)?;
                self.vm.release_ivc_channel(base_gpa, size)?;

                Ok(0)
            }
            HyperCallCode::CrashFreezeGuest => {
                let target_vm_id = self.args[0] as usize;

                info!(
                    "VM[{}] HyperCall {:?} target VM[{}]",
                    self.vm.id(),
                    self.code,
                    target_vm_id
                );

                let target_vm = manager::get_vm_by_id(target_vm_id).ok_or_else(|| {
                    ax_err_type!(NotFound, format!("target VM[{}] not found", target_vm_id))
                })?;

                let current_status = target_vm.vm_status();
                info!(
                    "VM[{}] freezing VM[{}] (status={})",
                    self.vm.id(),
                    target_vm_id,
                    current_status
                );

                // 方案D: snapshot the current ctx ESR/FAR as crash registers.
                // This serves as a fallback when 方案B didn't capture (e.g., watchdog timeout).
                #[cfg(target_arch = "aarch64")]
                for vcpu in target_vm.vcpu_list() {
                    let arch_vcpu = vcpu.get_arch_vcpu();
                    arch_vcpu.ctx.capture_crash_regs();
                }
                #[cfg(not(target_arch = "aarch64"))]
                for _vcpu in target_vm.vcpu_list() {}

                target_vm.set_vm_status(VMStatus::Suspended);

                info!("VM[{}] frozen successfully, status=Suspended", target_vm_id);
                Ok(0)
            }
            HyperCallCode::CrashReadGuestRegs => {
                let target_vm_id = self.args[0] as usize;
                let target_vcpu_id = self.args[1] as usize;
                let regs_out_gpa = GuestPhysAddr::from_usize(self.args[2] as usize);

                info!(
                    "VM[{}] HyperCall {:?} target VM[{}] VCpu[{}]",
                    self.vm.id(),
                    self.code,
                    target_vm_id,
                    target_vcpu_id
                );

                let target_vm = manager::get_vm_by_id(target_vm_id).ok_or_else(|| {
                    ax_err_type!(NotFound, format!("target VM[{}] not found", target_vm_id))
                })?;

                let target_vcpu = target_vm.vcpu(target_vcpu_id).ok_or_else(|| {
                    ax_err_type!(NotFound, format!("target VM[{}] VCpu[{}] not found", target_vm_id, target_vcpu_id))
                })?;

                let regs = read_vcpu_regs(&target_vcpu);

                self.vm.write_to_guest_of(regs_out_gpa, &regs)?;

                info!(
                    "VM[{}] read VCpu[{}] registers of VM[{}]: ELR={:#018x}",
                    self.vm.id(), target_vcpu_id, target_vm_id, regs.elr_el1
                );
                Ok(0)
            }
            HyperCallCode::CrashReadGuestMem => {
                let target_vm_id = self.args[0] as usize;
                let target_gpa = GuestPhysAddr::from_usize(self.args[1] as usize);
                let buf_gpa = GuestPhysAddr::from_usize(self.args[2] as usize);
                let size = self.args[3] as usize;

                info!(
                    "VM[{}] HyperCall {:?} target VM[{}] GPA={:#x} size={}",
                    self.vm.id(), self.code, target_vm_id,
                    target_gpa.as_usize(), size,

                );

                let target_vm = manager::get_vm_by_id(target_vm_id).ok_or_else(|| {
                    ax_err_type!(NotFound, format!("target VM[{}] not found", target_vm_id))
                })?;

                // Clamp to max 1 MB per call to prevent abuse.
                let max_size = size.min(1024 * 1024);

                // Read from target VM memory into a host-local temporary buffer.
                let mut buf = alloc::vec![0u8; max_size];
                let actual = target_vm.read_guest_bytes(target_gpa, &mut buf)?;

                // Write the data from the temporary buffer into the calling VM's buffer.
                let written = self.vm.write_guest_bytes(buf_gpa, &buf[..actual])?;

                info!(
                    "VM[{}] read {} bytes from VM[{}] GPA={:#x}, wrote {} bytes to buffer",
                    self.vm.id(), actual, target_vm_id, target_gpa.as_usize(), written,
                );
                Ok(written)
            }
            HyperCallCode::PollCrashStatus => {
                let target_vm_id = self.args[0] as usize;
                let target_vm = manager::get_vm_by_id(target_vm_id).ok_or_else(|| {
                    ax_err_type!(NotFound, format!("target VM[{}] not found", target_vm_id))
                })?;
                let status = target_vm.vm_status();
                match status {
                    VMStatus::Stopped => {
                        info!("VM[{}] HyperCall PollCrashStatus: target VM[{}] has crashed (Stopped)", self.vm.id(), target_vm_id);
                        Ok(1)
                    }
                    _ => {
                        trace!("VM[{}] HyperCall PollCrashStatus: target VM[{}] status={:?}", self.vm.id(), target_vm_id, status);
                        Ok(0)
                    }
                }
            }
            HyperCallCode::CrashSaveFile => {
                let name_gpa = GuestPhysAddr::from_usize(self.args[0] as usize);
                let data_gpa = GuestPhysAddr::from_usize(self.args[1] as usize);
                let data_len = self.args[2] as usize;

                info!(
                    "VM[{}] HyperCall {:?} name_gpa={:#x} data_len={}",
                    self.vm.id(),
                    self.code,
                    name_gpa.as_usize(),
                    data_len,
                );

                #[cfg(feature = "host-fs")]
                {
                    // Read null-terminated filename from the calling VM.
                    let mut name_buf = [0u8; 256];
                    let n = self.vm.read_guest_bytes(name_gpa, &mut name_buf)?;
                    let end = name_buf.iter().position(|&b| b == 0).unwrap_or(n.min(255));
                    let filename = core::str::from_utf8(&name_buf[..end])
                        .map_err(|_| ax_err_type!(InvalidInput, "invalid UTF-8 filename"))?;

                    // Reject path separators — we write into a fixed directory.
                    if filename.contains('/') || filename.contains('\\') {
                        return Err(ax_err_type!(InvalidInput, "path separator in filename"));
                    }

                    // Read file data from the calling VM.
                    let mut data = alloc::vec![0u8; data_len];
                    self.vm.read_guest_bytes(data_gpa, &mut data)?;

                    // Write to the hypervisor's filesystem.
                    let _ = ax_std::fs::create_dir("/vmcore");
                    let path = alloc::format!("/vmcore/{}", filename);
                    ax_std::fs::write(&path, &data)
                        .map_err(|e| ax_err_type!(Io, format!("write '{}': {:?}", path, e)))?;

                    info!("CrashSaveFile: written '{}' ({} bytes)", path, data_len);
                }

                #[cfg(not(feature = "host-fs"))]
                {
                    warn!(
                        "CrashSaveFile ignored — axvisor built without 'host-fs' feature; \
                         files remain in guest ramfs"
                    );
                }

                Ok(0)
            }
            HyperCallCode::ConsoleGetChar => {
                // Read a character from the PL011 UART on behalf of the
                // calling VM.  We poll the Flag Register until data is
                // available, then return the byte from the Data Register.
                // The base address matches QEMU virt's UART0 (0x900_0000);
                // adjust for other platforms.
                const PL011_BASE: u64 = 0x900_0000;
                const PL011_DR:  u64 = 0x000;
                const PL011_FR:  u64 = 0x018;
                const FR_RXFE: u32 = 1 << 4; // RX FIFO Empty

                #[cfg(target_arch = "aarch64")]
                {
                    let fr_ptr = (PL011_BASE + PL011_FR) as *const u32;
                    let dr_ptr = (PL011_BASE + PL011_DR) as *const u32;

                    // Poll with a small timeout so we don't hang forever
                    // if no input is available.
                    for _ in 0..10_000 {
                        let fr = unsafe { core::ptr::read_volatile(fr_ptr) };
                        if fr & FR_RXFE == 0 {
                            let dr = unsafe { core::ptr::read_volatile(dr_ptr) };
                            let ch = (dr & 0xFF) as u8;
                            trace!("ConsoleGetChar returned '{}' ({:#x})", ch as char, ch);
                            return Ok(ch as usize);
                        }
                        core::hint::spin_loop();
                    }
                    // No data available after timeout.
                    Ok(0)
                }

                #[cfg(not(target_arch = "aarch64"))]
                {
                    warn!("ConsoleGetChar not implemented for this architecture");
                    Ok(0)
                }
            }
            _ => {
                warn!("Unsupported hypercall code: {:?}", self.code);
                ax_err!(Unsupported)?
            }
        }
    }
}

/// Register state of a crashed vCPU, written to Monitor Guest memory.
///
/// This structure mirrors the AArch64 register layout and is written
/// into the Monitor Guest's address space by the `CrashReadGuestRegs`
/// hypercall handler.
#[repr(C)]
pub struct CrashVcpuRegs {
    /// General-purpose registers X0–X30 (31 registers)
    pub gpr: [u64; 31],
    /// Stack pointer (SP_EL0 at time of trap)
    pub sp_el0: u64,
    /// Exception Link Register (ELR_EL1 — the trapped PC)
    pub elr_el1: u64,
    /// Saved Program Status Register (SPSR_EL1)
    pub spsr_el1: u64,
    /// Exception Syndrome Register (ESR_EL1)
    ///   - bit[31:26]: Exception Class (EC)
    ///   - bit[24:0]:  Instruction Specific Syndrome (ISS)
    ///   0 means no synchronous exception (e.g., watchdog timeout)
    pub esr_el1: u64,
    /// Fault Address Register (FAR_EL1)
    ///   - Data Abort: virtual address that caused the fault
    ///   - NULL pointer crash: 0x0
    pub far_el1: u64,
    /// Crash type classification for quick diagnosis
    ///   0=None, 1=DataAbort, 2=UndefinedInstruction, 3=InstructionAbort,
    ///   4=PcAlignmentFault, 5=SError
    pub crash_type: u8,
    /// Padding to keep struct 8-byte aligned
    pub _padding: [u8; 7],
}

/// Read the CPU register state from a target vCPU.
///
/// On AArch64 this reads the TrapFrame (gpr[31], sp_el0, elr, spsr),
/// plus ESR_EL2 and FAR_EL2 from the locked crash registers (方案B + 方案D).
fn read_vcpu_regs(vcpu: &crate::AxVCpuRef) -> CrashVcpuRegs {
    cfg_if::cfg_if! {
        if #[cfg(target_arch = "aarch64")] {
            let arch_vcpu = vcpu.get_arch_vcpu();

            // Read ESR_EL1 and FAR_EL1 from the TrapFrame, which were saved
            // by exception.S at the moment of VM exit.
            //
            // NOTE: The assembly saves ESR_EL1/FAR_EL1 (the Guest's original
            // exception registers) into the esr_el2/far_el2 struct fields.
            // We use ESR_EL1 because it retains its value across the EL2 trap,
            // while ESR_EL2 is overwritten on every VM exit. See exception.S.
            //
            // However, since subsequent VM exits can overwrite these fields,
            // we use the locked crash registers instead. These are captured by:
            //   方案B — the exception handler upon Data/Instruction Abort (first crash)
            //   方案D — the CrashFreezeGuest handler (fallback snapshot)
            let (esr, far) = arch_vcpu.ctx.crash_esr_far();

            // Classify crash type from ESR Exception Class
            let ec = (esr >> 26) & 0x3F;
            let crash_type = match ec {
                0x24 | 0x25 => 1,  // DataAbort
                0x22        => 2,  // UndefinedInstruction
                0x20 | 0x21 => 3,  // InstructionAbort
                0x23        => 4,  // PcAlignmentFault
                0x30        => 5,  // SError
                _           => 0,  // None
            };

            CrashVcpuRegs {
                gpr: arch_vcpu.ctx.gpr,
                sp_el0: arch_vcpu.ctx.sp_el0,
                elr_el1: arch_vcpu.ctx.elr,
                spsr_el1: arch_vcpu.ctx.spsr,
                esr_el1: esr,
                far_el1: far,
                crash_type,
                _padding: [0; 7],
            }
        } else {
            log::warn!("CrashReadGuestRegs: register dump not implemented for this architecture");
            CrashVcpuRegs {
                gpr: [0; 31],
                sp_el0: 0,
                elr_el1: 0,
                spsr_el1: 0,
                esr_el1: 0,
                far_el1: 0,
                crash_type: 0,
                _padding: [0; 7],
            }
        }
    }
}