//! Kernel log buffer (printk/dmesg) capture module.
//!
//! Reads the target Guest's kernel log ring buffer from guest physical memory
//! via HVC #9, and extracts readable text.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::capture::register;

/// Guest kernel linear mapping offset (GVA → GPA).
/// Linux ARM64 uses PAGE_OFFSET = 0xffff_0000_0000_0000 (48-bit VA).
const PHYS_VIRT_OFFSET: u64 = 0xffff_0000_0000_0000;

/// Linux kernel image virtual base (from System.map _text).
/// Used to convert kernel-image-area addresses (BSS, data) to GPA.
/// PA = VA - KERNEL_IMAGE_TEXT_VA + KERNEL_IMAGE_TEXT_PA
const KERNEL_IMAGE_TEXT_VA: u64 = 0xffff_8000_8000_0000;
const KERNEL_IMAGE_TEXT_PA: u64 = 0x2_2340_0000;

/// Default kernel log buffer GPA.
/// __log_buf VA=0xffff8000814d8000 → PA=0x2241d8000
/// (verified via /proc/iomem: Kernel code 0x223400000-0x224cfffff)
const DEFAULT_LOG_BUF_GPA: Option<u64> = Some(0x2_241d_8000);

/// Maximum bytes to read from the log ring buffer in one HVC #9 call.
const MAX_READ_SIZE: usize = 64 * 1024; // 64 KB

/// Result of a kernel log collection.
pub struct KernelLogResult {
    /// Raw log text (may include non-printable characters).
    pub raw_text: String,
    /// Actual number of bytes read from the ring buffer.
    pub bytes_read: usize,
}

/// Convert a Guest Virtual Address (GVA) to a Guest Physical Address (GPA)
/// using the linear mapping offset or kernel image offset.
pub fn gva_to_gpa(gva: u64) -> u64 {
    // Kernel image area (0xffff8000_xxxxxxxx)
    if gva >= KERNEL_IMAGE_TEXT_VA && gva < KERNEL_IMAGE_TEXT_VA + 0x200_0000 {
        gva - KERNEL_IMAGE_TEXT_VA + KERNEL_IMAGE_TEXT_PA
    }
    // Linear map (0xffff_0000_0000_0000)
    else if gva >= PHYS_VIRT_OFFSET {
        gva.wrapping_sub(PHYS_VIRT_OFFSET)
    } else {
        gva
    }
}

/// Collect the kernel log buffer from a frozen target VM.
///
/// # Arguments
///
/// * `target_vm_id` — Frozen target VM ID
/// * `log_buf_gpa`  — Physical address of the log ring buffer.
///                    `None` uses the default fallback address.
/// * `max_size`     — Maximum bytes to read (clamped to 64 KB).
pub fn collect_kernel_log(
    target_vm_id: u64,
    log_buf_gpa: Option<u64>,
    max_size: usize,
) -> Result<KernelLogResult, String> {
    let gpa = log_buf_gpa.or(DEFAULT_LOG_BUF_GPA).ok_or_else(|| {
        "kernel log buffer address not provided and no default available".to_string()
    })?;

    let read_size = max_size.min(MAX_READ_SIZE);
    let mut buffer = vec![0u8; read_size];

    let bytes_read = register::read_guest_mem(target_vm_id, gpa, &mut buffer)?;
    if bytes_read == 0 {
        return Err("read_guest_mem returned 0 bytes from log buffer".to_string());
    }

    buffer.truncate(bytes_read);
    let raw_text = extract_printk_buffer(&buffer)?;

    ax_std::println!(
        "[log] collected {} bytes of kernel log ({} chars extracted)",
        bytes_read,
        raw_text.len(),
    );

    Ok(KernelLogResult {
        raw_text,
        bytes_read,
    })
}

/// Extract readable text from a raw printk ring buffer dump.
///
/// Skips zero-filled regions and non-printable control characters
/// (preserving newlines and carriage returns), then finds the actual
/// head/tail positions assuming a simple ring buffer layout.
fn extract_printk_buffer(raw: &[u8]) -> Result<String, String> {
    if raw.is_empty() {
        return Ok(String::new());
    }

    // Try to find the last null terminator (start of valid log data),
    // then collect from there to the end.
    let mut start = 0usize;
    for i in (0..raw.len()).rev() {
        if raw[i] == 0x00 {
            // Found a null byte — data before it may be stale.
            // If we find a printable sequence after this, start there.
            let mut has_content = false;
            for j in i + 1..raw.len() {
                if raw[j] >= 0x20 && raw[j] <= 0x7e || raw[j] == b'\n' || raw[j] == b'\r' || raw[j] == b'\t' {
                    has_content = true;
                    break;
                }
            }
            if has_content {
                start = i + 1;
                break;
            }
        }
    }

    // Also scan from start: skip leading zeros and non-printable bytes.
    while start < raw.len() && (raw[start] == 0x00 || raw[start] < 0x20 && raw[start] != b'\n' && raw[start] != b'\r' && raw[start] != b'\t') {
        start += 1;
    }

    // Collect printable characters, preserving newlines.
    let mut text = String::new();
    let mut i = start;
    while i < raw.len() {
        let c = raw[i];
        if c == 0x00 {
            // Reached end of valid data
            break;
        }
        if c >= 0x20 && c <= 0x7e || c == b'\n' || c == b'\r' || c == b'\t' {
            text.push(c as char);
        } else if c < 0x20 {
            // Skip other control characters
        }
        i += 1;
    }

    if text.is_empty() {
        // Fallback: take everything that isn't null
        for &c in &raw[start..] {
            if c == 0x00 { break; }
            if c >= 0x20 && c <= 0x7e || c == b'\n' || c == b'\r' || c == b'\t' {
                text.push(c as char);
            }
        }
    }

    Ok(text)
}
