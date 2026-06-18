//! Target Guest physical memory dump via HVC #9 CrashReadGuestMem.
//!
//! Uses `ax_hal::mem::virt_to_phys` for correct VA → GPA translation,
//! as the destination buffer must be a physical address visible to the
//! hypervisor's Stage-2 page tables.
extern crate alloc;



#[cfg(target_arch = "aarch64")]
use core::arch::asm;

use ax_hal::mem::{virt_to_phys, VirtAddr};

/// Maximum bytes per single HVC #9 call (must match hypervisor side).
const MAX_HVC_READ_SIZE: usize = 1024 * 1024; // 1 MB

// ---------------------------------------------------------------------------
// HVC call helper (same convention as register.rs)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn hvc_call(code: u64, x1: u64, x2: u64, x3: u64, x4: u64, x5: u64) -> u64 {
    let result: u64;
    unsafe {
        asm!("hvc #0",
             inout("x0") code => result,
             in("x1") x1, in("x2") x2, in("x3") x3,
             in("x4") x4, in("x5") x5,
             options(nostack));
    }
    result
}

#[cfg(not(target_arch = "aarch64"))]
fn hvc_call(_code: u64, _x1: u64, _x2: u64, _x3: u64, _x4: u64, _x5: u64) -> u64 {
    u64::MAX
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read a chunk of guest physical memory from a frozen target VM via HVC #9.
///
/// # Arguments
///
/// * `target_vm_id` — ID of the frozen target VM
/// * `target_gpa` — start guest physical address in the target VM
/// * `buffer` — caller-provided buffer to fill (length = requested bytes)
///
/// # Returns
///
/// Actual number of bytes read on success; the hypervisor may return fewer
/// bytes than requested when hitting an unmapped region.
pub fn read_guest_mem(target_vm_id: u64, target_gpa: u64, buffer: &mut [u8]) -> Result<usize, String> {
    let size = buffer.len();
    if size == 0 {
        return Ok(0);
    }
    if size > MAX_HVC_READ_SIZE {
        return Err(format!("read_guest_mem size {} exceeds max {}", size, MAX_HVC_READ_SIZE));
    }

    // Translate the virtual address of the buffer to a guest physical address
    // so the hypervisor can write into it via Stage-2 translation.
    let buf_addr = buffer.as_ptr() as usize;
    let vaddr = VirtAddr::from_usize(buf_addr);
    let buf_gpa = virt_to_phys(vaddr).as_usize() as u64;

    let ret = hvc_call(9, target_vm_id, target_gpa, buf_gpa, size as u64, 0);
    // Negative return = error, non-negative = actual bytes read.
    if (ret as i64) < 0 {
        Err(format!("CrashReadGuestMem failed on VM[{}] GPA={:#x} size={} ret={}",
                     target_vm_id, target_gpa, size, ret))
    } else {
        Ok(ret as usize)
    }
}

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

        // Read in chunks of at most MAX_HVC_READ_SIZE to avoid overwhelming the hypervisor.
        let chunk_size = MAX_HVC_READ_SIZE.min(size);
        let mut region_data = Vec::with_capacity(size);

        let mut offset = 0usize;
        while offset < size {
            let remaining = size - offset;
            let cur = chunk_size.min(remaining);
            let mut chunk_buf = alloc::vec![0u8; cur];

            let read = read_guest_mem(target_vm_id, base + offset as u64, &mut chunk_buf)?;
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
