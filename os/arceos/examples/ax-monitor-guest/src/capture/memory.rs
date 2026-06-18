//! Target Guest physical memory dump via HVC #9 CrashReadGuestMem.
//!
//! Reads the target VM's RAM page by page and returns a contiguous dump
//! that can be stored in the vmcore for later analysis (unwind, symbols).

extern crate alloc;
use alloc::vec::Vec;
use alloc::format;
use crate::capture::register;

/// Describes a single contiguous memory region in the target VM.
#[derive(Debug, Clone)]
pub struct MemRegion {
    pub gpa: u64,
    pub size: u64,
}

/// Default memory regions for the target VM (128 MiB at 0x8000_0000).
/// In the future this should be read from the VM config or FDT.
const TARGET_MEM_REGIONS: &[MemRegion] = &[
    MemRegion { gpa: 0x8000_0000, size: 0x0800_0000 }, // 128 MiB
];

/// Page size for HVC #9 transfers.
const PAGE_SIZE: u64 = 4096;

/// Dump the target VM's memory regions into a flat byte vector.
///
/// Each page is fetched via one HVC #9 call. Returns `(regions, data)` where
/// `regions` describes the layout and `data` contains the raw bytes.
pub fn dump_target_memory(target_vm_id: u64) -> Result<(Vec<MemRegion>, Vec<u8>), String> {
    let mut all_data = Vec::new();
    let mut regions = Vec::new();

    for region in TARGET_MEM_REGIONS {
        ax_std::println!("[memory] dumping GPA {:#x}+{:#x} ...", region.gpa, region.size);
        let start = all_data.len();
        let mut gpa = region.gpa;
        let end = region.gpa + region.size;

        while gpa < end {
            let remaining = (end - gpa) as usize;
            let chunk_size = remaining.min(PAGE_SIZE as usize);
            let chunk_start = all_data.len();
            all_data.extend(core::iter::repeat(0u8).take(chunk_size));
            let buf = &mut all_data[chunk_start..];
            register::read_guest_mem(target_vm_id, gpa, buf)?;
            gpa += chunk_size as u64;
        }

        let actual_size = all_data.len() - start;
        regions.push(MemRegion {
            gpa: region.gpa,
            size: actual_size as u64,
        });
        ax_std::println!(
            "[memory] dumped region GPA {:#x}+{:#x} ({} pages)",
            region.gpa,
            actual_size,
            actual_size / PAGE_SIZE as usize,
        );
    }

    ax_std::println!(
        "[memory] total dump: {} bytes across {} regions",
        all_data.len(),
        regions.len()
    );
    Ok((regions, all_data))
}
