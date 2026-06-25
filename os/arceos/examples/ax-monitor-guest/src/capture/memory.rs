//! Target Guest physical memory dump via HVC #9 CrashReadGuestMem.
//!
//! Reads the target VM's RAM in 1 MiB chunks and returns the data
//! as in-memory `Vec<(base_gpa, data)>`.  The caller persists it
//! to files afterwards.  Region size is capped at 64 MiB to avoid
//! exhausting the monitor guest's heap.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capture::register;

/// Maximum bytes per single HVC #9 call (must match hypervisor side).
const MAX_HVC_READ_SIZE: usize = 1024 * 1024; // 1 MiB

/// Dump one or more contiguous memory regions from a frozen target VM.
///
/// Each region is specified as a `(base_gpa, size_in_bytes)` tuple.
/// The resulting memory is returned as `Vec<(base_gpa, data)>`.
pub fn dump_memory_regions(target_vm_id: u64, regions: &[(u64, usize)]) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let mut dump = Vec::new();
    for &(base, size) in regions {
        if size == 0 {
            continue;
        }
        ax_std::println!("[memory] dumping VM[{}] region GPA={:#x} size={}",
                         target_vm_id, base, size);
        let chunk_size = MAX_HVC_READ_SIZE.min(size);
        let max_buf = size.min(64 * 1024 * 1024); // cap at 64 MiB
        let mut region_data = Vec::with_capacity(max_buf);

        let mut offset = 0usize;
        while offset < size && region_data.len() < max_buf {
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
                ax_std::println!("[memory]  ... {}/{} MiB dumped",
                                 offset / 1024 / 1024, size / 1024 / 1024);
            }
        }
        if !region_data.is_empty() {
            ax_std::println!("[memory]  region done: {} bytes", region_data.len());
            dump.push((base, region_data));
        }
    }
    Ok(dump)
}
