extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::memory::{self, MemRegion};
use crate::capture::register::{self, CrashVcpuRegs};
use crate::capture::storage;
use crate::monitor::event::CrashEvent;
use crate::recovery::analyzer;
use crate::recovery::report;
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

    // Step 2: Dump target VM memory via HVC #9 (best-effort, may fail).
    match memory::dump_target_memory(TARGET_VM_ID) {
        Ok((_regions, data)) => {
            ax_std::println!("[capture] memory dump: {} bytes", data.len());
        }
        Err(e) => {
            ax_std::println!("[capture] memory dump skipped: {}", e);
        }
    }

    // Step 3: Save vmcore to persistent storage.
    if let Ok(vmcore_path) = storage::save_vmcore(&snapshot) {
        ax_std::println!("[capture] vmcore saved at: {}", vmcore_path);

        // Step 3: Load vmcore and run recovery analysis.
        if let Some(vmcore) = storage::load_vmcore(&vmcore_path) {
            ax_std::println!("[recovery] starting crash analysis...");

            let result = analyzer::analyze(
                &vmcore,
                &|_| None, // No memory dump available yet
                None,       // No symbol table available yet
            );

            // Print analysis summary to console.
            ax_std::println!("[recovery] analysis summary: {}", result.summary);
            for cause in &result.possible_causes {
                ax_std::println!("[recovery]   - {}", cause);
            }

            // Step 4: Save analysis reports (with _analysis suffix to avoid overwriting vmcore).
            let report_base = alloc::format!("{}_analysis", vmcore_path.trim_end_matches(".json"));
            match report::save_reports(&result, &report_base) {
                Ok((json_path, md_path)) => {
                    ax_std::println!("[recovery] analysis reports saved:");
                    ax_std::println!("  JSON: {}", json_path);
                    ax_std::println!("  MD:   {}", md_path);
                }
                Err(e) => {
                    ax_std::println!("[recovery] failed to save reports: {}", e);
                }
            }
        } else {
            ax_std::println!("[recovery] failed to load vmcore for analysis");
        }
    } else {
        ax_std::println!("[capture] failed to save vmcore");
    }
}