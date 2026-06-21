//! Target Guest physical memory dump via HVC #9 CrashReadGuestMem.
//!
//! Reads the target VM's RAM page by page and returns a contiguous dump
//! that can be stored in the vmcore for later analysis (unwind, symbols).

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capture::register;

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
