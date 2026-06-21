//! Export crash dump files from the monitor guest to the hypervisor's
//! storage via HVC #11 (CrashSaveFile).
//!
//! After a crash snapshot is captured and analysed, this module sends
//! the vmcore JSON, analysis reports, and memory dumps to the hypervisor,
//! which writes them to its filesystem (requires axvisor built with
//! `--features fs` for persistent block-device-backed storage).

extern crate alloc;
use alloc::format;
use alloc::string::String;

use ax_hal::mem::{virt_to_phys, VirtAddr};

use super::hvc::hvc_call;

/// Maximum single-file size we attempt to export.
/// The hypervisor side is expected to handle reasonable sizes.
const MAX_EXPORT_SIZE: usize = 8 * 1024 * 1024; // 8 MB

/// Save a file to the hypervisor's `/vmcore/` directory via HVC #11 CrashSaveFile.
///
/// # Arguments
///
/// * `filename` — File name only (not path), e.g. `"vmcore_1_Panic.json"`.
/// * `data`     — File content bytes.
///
/// # Errors
///
/// Returns an error description if the hypercall fails or the file is
/// too large.
pub fn save_file_to_hypervisor(filename: &str, data: &[u8]) -> Result<(), String> {
    if data.is_empty() {
        return Ok(());
    }
    if data.len() > MAX_EXPORT_SIZE {
        return Err(format!(
            "file '{}' too large ({} > {} bytes)",
            filename,
            data.len(),
            MAX_EXPORT_SIZE,
        ));
    }
    // Reject path separators — hypervisor writes into a fixed directory.
    if filename.contains('/') || filename.contains('\\') {
        return Err(format!("filename must not contain path separators: '{}'", filename));
    }

    let name_bytes = filename.as_bytes();
    let name_gpa =
        virt_to_phys(VirtAddr::from_usize(name_bytes.as_ptr() as usize)).as_usize() as u64;
    let data_gpa =
        virt_to_phys(VirtAddr::from_usize(data.as_ptr() as usize)).as_usize() as u64;
    let data_len = data.len() as u64;

    let ret = hvc_call(11, name_gpa, data_gpa, data_len, 0, 0);
    if (ret as i64) < 0 {
        Err(format!("CrashSaveFile failed for '{}', ret={}", filename, ret))
    } else {
        ax_std::println!(
            "[export] saved '{}' to hypervisor ({} bytes)",
            filename,
            data.len(),
        );
        Ok(())
    }
}

/// Export a list of named file entries to the hypervisor.
///
/// Each entry is a `(filename, data)` pair.  All files are sent
/// sequentially; failures are logged but do not abort the remaining
/// entries.
pub fn export_files(files: &[(&str, &[u8])]) {
    let total = files.len();
    let mut ok = 0usize;
    for (name, data) in files {
        match save_file_to_hypervisor(name, data) {
            Ok(()) => ok += 1,
            Err(e) => ax_std::println!("[export] FAILED: {}  {}", name, e),
        }
    }
    ax_std::println!("[export] {}/{} files exported to hypervisor", ok, total);
}
