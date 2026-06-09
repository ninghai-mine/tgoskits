extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::register::{self, CrashVcpuRegs};
use crate::capture::storage;
use crate::monitor::event::CrashEvent;
use serde::{Deserialize, Serialize};

/// Target VM configuration — should be set from VM config at startup.
const TARGET_VM_ID: u64 = 1;
const TARGET_VCPU_COUNT: u64 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct CrashSnapshot {
    pub event: CrashEvent,
    /// Per-vCPU register states captured at crash time. (vcpu_id, regs)
    pub vcpu_regs: Vec<(u64, CrashVcpuRegs)>,
}

pub fn capture_snapshot(event: CrashEvent) {
    ax_std::println!("[capture] start snapshot (event={:?})", event);

    // Step 1: Freeze the target VM and read all vCPU registers via HVC.
    let vcpu_regs = match register::freeze_and_read_all(TARGET_VM_ID, TARGET_VCPU_COUNT) {
        Ok(regs) => {
            ax_std::println!("[capture] captured {} vCPU register sets", regs.len());
            for (id, r) in &regs {
                ax_std::println!(
                    "  VCpu[{}] PC={:#018x} SP={:#018x}",
                    id, r.elr_el1, r.sp_el0
                );
            }
            regs
        }
        Err(e) => {
            ax_std::println!("[capture] register capture failed: {}", e);
            ax_std::println!("[capture] falling back to simulated register data");
            vec![(0, CrashVcpuRegs::default())]
        }
    };

    let snapshot = CrashSnapshot {
        event,
        vcpu_regs,
    };

    ax_std::println!("[capture] snapshot captured");

    if let Err(e) = storage::save_vmcore(&snapshot) {
        ax_std::println!("[capture] failed to save vmcore: {}", e);
    }
}