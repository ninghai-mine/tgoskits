extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use crate::capture::register::{self, CrashVcpuRegs};
use crate::capture::storage;
use crate::capture::memory;
use crate::monitor::event::CrashEvent;
use crate::recovery::analyzer;
use crate::recovery::report;
use serde::{Deserialize, Serialize};

/// Target VM configuration — should be set from VM config at startup.
const TARGET_VM_ID: u64 = 1;
const TARGET_VCPU_COUNT: u64 = 1;

/// Guest physical memory regions to dump (must match target-guest-memory.toml).
const MEMORY_REGIONS: &[(u64, usize)] = &[
    (0x8000_0000, 0x0800_0000), // 8 MB of Guest RAM
];

#[derive(Debug, Serialize, Deserialize)]
pub struct MemorySegment {
    pub base_gpa: u64,
    pub size: usize,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrashSnapshot {
    pub event: CrashEvent,
    /// Per-vCPU register states captured at crash time. (vcpu_id, regs)
    pub vcpu_regs: Vec<(u64, CrashVcpuRegs)>,
    /// Optional memory dump segments.
    pub memory_segments: Vec<MemorySegment>,
}

pub fn capture_snapshot(event: CrashEvent) {
    let timestamp = boot_timestamp();
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

    // Step 2: Dump guest physical memory via HVC #9.
    let memory_segments = match memory::dump_memory_regions(TARGET_VM_ID, MEMORY_REGIONS) {
        Ok(segments) => {
            let mut out = Vec::new();
            for (base, data) in &segments {
                let file_name = alloc::format!("memory_{:08x}_{}.bin", base, timestamp);
                let path = alloc::format!("/vmcore/{}", file_name);
                match ax_std::fs::write(&path, data) {
                    Ok(_) => {
                        ax_std::println!("[capture] memory dump saved: {} ({} bytes)", file_name, data.len());
                        out.push(MemorySegment {
                            base_gpa: *base,
                            size: data.len(),
                            path: file_name,
                        });
                    }
                    Err(e) => {
                        ax_std::println!("[capture] failed to write memory dump {}: {}", path, e);
                    }
                }
            }
            out
        }
        Err(e) => {
            ax_std::println!("[capture] memory dump failed: {}", e);
            Vec::new()
        }
    };

    let snapshot = CrashSnapshot {
        event,
        vcpu_regs,
        memory_segments,
    };

    ax_std::println!("[capture] snapshot captured");

    // Step 3: Save vmcore to persistent storage.
    if let Ok(vmcore_path) = storage::save_vmcore(&snapshot) {
        ax_std::println!("[capture] vmcore saved at: {}", vmcore_path);

        // Step 4: Load vmcore and run recovery analysis.
        if let Some(vmcore) = storage::load_vmcore(&vmcore_path) {
            ax_std::println!("[recovery] starting crash analysis...");

            let result = analyzer::analyze(
                &vmcore,
                &|_addr| { // Memory reader closure — looks up segments by address
                    // TODO: integrate with symbol table lookup
                    None
                },
                None,       // No symbol table available yet
            );

            // Print analysis summary to console.
            ax_std::println!("[recovery] analysis summary: {}", result.summary);
            for cause in &result.possible_causes {
                ax_std::println!("[recovery]   - {}", cause);
            }

            // Step 5: Save analysis reports (with _analysis suffix to avoid overwriting vmcore).
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

/// Monotonic counter used as timestamp for file names.
fn boot_timestamp() -> u64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    static TS: AtomicU64 = AtomicU64::new(0);
    TS.fetch_add(1, Ordering::Relaxed)
}