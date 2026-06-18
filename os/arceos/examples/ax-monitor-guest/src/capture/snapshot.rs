extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;


use crate::capture::register::{self, CrashVcpuRegs};
use crate::capture::storage;
use crate::capture::memory;
use crate::monitor::event::CrashEvent;
use crate::recovery::analyzer;
use crate::recovery::report;
use crate::recovery::symbol::SymbolTable;
use serde::{Deserialize, Serialize};

/// Target VM configuration — should be set from VM config at startup.
const TARGET_VM_ID: u64 = 1;
const TARGET_VCPU_COUNT: u64 = 1;

/// Guest physical memory regions to dump (must match target-guest-memory.toml).
const MEMORY_REGIONS: &[(u64, usize)] = &[
    (0x8000_0000, 0x0800_0000), // 8 MB of Guest RAM
];

/// Guest kernel linear mapping offset (GVA → GPA).
/// StarryOS/ArceOS AArch64 uses KERNEL_ASPACE_BASE = 0xffff_8000_0000_0000,
/// so the kernel virtual address is: GPA + PHYS_VIRT_OFFSET.
const PHYS_VIRT_OFFSET: u64 = 0xffff_8000_0000_0000;

/// Path to the target kernel ELF (with symbol table) inside the monitor guest's filesystem.
/// Set to empty string to disable symbol resolution.
/// The ELF must be baked into the monitor-guest image at build time.
const KERNEL_ELF_PATH: &str = "";

/// Kernel base virtual address for symbol table lookups.
/// If the ELF has absolute virtual addresses (ET_EXEC linked at KERNEL_BASE_VADDR),
/// set this to 0 so that `addr - 0 = addr` matches `sym.st_value` directly.
/// If the ELF has relative offsets (PIE), set this to PHYS_VIRT_OFFSET + kernel_load_addr.
const KERNEL_BASE_ADDR: u64 = 0;

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
    // Keep both the metadata (for vmcore JSON) and the raw data (for analysis closure).
    let memory_segments: Vec<MemorySegment>;
    let memory_regions_data: Vec<(u64, Vec<u8>)> = match memory::dump_memory_regions(TARGET_VM_ID, MEMORY_REGIONS) {
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
            memory_segments = out;
            segments
        }
        Err(e) => {
            ax_std::println!("[capture] memory dump failed: {}", e);
            memory_segments = Vec::new();
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

            // Build a memory reader closure that translates Guest Virtual Address
            // → Guest Physical Address and looks up data in the dumped segments.
            let mem_reader = |addr: u64| -> Option<u64> {
                // GVA → GPA translation (linear mapping)
                let gpa = if addr >= PHYS_VIRT_OFFSET {
                    addr.wrapping_sub(PHYS_VIRT_OFFSET)
                } else {
                    addr
                };
                // Find which memory segment contains this GPA
                let (base, data) = memory_regions_data
                    .iter()
                    .find(|(base, data)| gpa >= *base && gpa < *base + data.len() as u64)?;
                let offset = (gpa - *base) as usize;
                if offset + 8 > data.len() {
                    return None;
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[offset..offset + 8]);
                Some(u64::from_le_bytes(bytes))
            };

            // Try to load kernel ELF symbol table for function name resolution.
            // Gracefully degrades to None if ELF is not available.
            let sym = if !KERNEL_ELF_PATH.is_empty() {
                match SymbolTable::from_kernel_elf(KERNEL_ELF_PATH, KERNEL_BASE_ADDR) {
                    Ok(table) => {
                        ax_std::println!("[recovery] symbol table loaded: {} symbols", table.len());
                        Some(table)
                    }
                    Err(e) => {
                        ax_std::println!("[recovery] symbol table unavailable: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let result = analyzer::analyze(
                &vmcore,
                &mem_reader,
                sym.as_ref(),
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