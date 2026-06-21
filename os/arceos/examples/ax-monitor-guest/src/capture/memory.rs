//! Target Guest physical memory dump via HVC #9 CrashReadGuestMem.
//!
//! Reads the target VM's RAM page by page and returns a contiguous dump
//! that can be stored in the vmcore for later analysis (unwind, symbols).

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capture::register;

/// Describes a single contiguous memory region in the target VM.
#[derive(Debug, Clone)]
pub struct MemRegion {
    pub gpa: u64,
    pub size: u64,
}

/// Default memory regions for the target VM (kernel image at 0x8020_0000).
/// In the future this should be read from the VM config or FDT.
const TARGET_MEM_REGIONS: &[MemRegion] = &[
    MemRegion { gpa: 0x8020_0000, size: 0x0010_0000 }, // 1 MiB kernel image
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

/// Maximum bytes per single HVC #9 call (must match hypervisor side).
const MAX_HVC_READ_SIZE: usize = 1024 * 1024; // 1 MB

/// Dump one or more contiguous memory regions from a frozen target VM.
///
/// Each region is specified as a `(base_gpa, size_in_bytes)` tuple.
/// The resulting memory is returned as `Vec<(base_gpa, data)>`.
/// This is the entry point called by `snapshot.rs`.
pub fn dump_memory_regions(target_vm_id: u64, regions: &[(u64, usize)]) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let mut dump = Vec::new();
    for &(base, size) in regions {
        if size == 0 {
            continue;
        }
        ax_std::println!("[memory] dumping VM[{}] region GPA={:#x} size={}",
                         target_vm_id, base, size);

        let chunk_size = MAX_HVC_READ_SIZE.min(size);
        let mut region_data = Vec::with_capacity(size);

        let mut offset = 0usize;
        while offset < size {
            let remaining = size - offset;
            let cur = chunk_size.min(remaining);
            let mut chunk_buf = alloc::vec![0u8; cur];
            let read = register::read_guest_mem(target_vm_id, base + offset as u64, &mut chunk_buf)?;
            if read == 0 {
                ax_std::println!("[memory]  hit unmapped GPA={:#x}, stopping region dump",
                                 base + offset as u64);
                break;
            }
            region_data.extend_from_slice(&chunk_buf[..read]);
            offset += read;

            if offset % (1024 * 1024) == 0 {
                ax_std::println!("[memory]  ... {}/{} MB dumped", offset / 1024 / 1024, size / 1024 / 1024);
            }
        }

        if !region_data.is_empty() {
            ax_std::println!("[memory]  region done: {} bytes read", region_data.len());
            dump.push((base, region_data));
        }
    }
    Ok(dump)
}
