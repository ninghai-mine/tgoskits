//! Kernel log buffer (printk/dmesg) capture module.
//!
//! Reads the target Guest's kernel log via the prb (printk ringbuffer)
//! introduced in Linux 5.10+. Uses the descriptor + info + data ring
//! layout to extract finalized log entries.
//!
//! Ringbuffer layout (aarch64, Linux 6.12):
//!
//!   printk_ringbuffer (at printk_rb_static):
//!     offset  0: desc_ring.count_bits       (u32)
//!     offset  8: desc_ring.descs            (*prb_desc)
//!     offset 16: desc_ring.infos            (*printk_info)
//!     offset 48: text_data_ring.size_bits   (u32)
//!     offset 56: text_data_ring.data        (*u8 → __log_buf)
//!
//!   prb_desc (24 bytes):
//!     offset  0: state_var (atomic_long_t)
//!     offset  8: text_blk_lpos.begin (unsigned long)
//!
//!   printk_info (88 bytes, aarch64):
//!     offset 16: text_len (u16)
//!
//!   Data ring entry: [u64 id][u8 text[text_len]]

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::capture::register;

/// Guest kernel linear mapping offset (GVA → GPA).
const PHYS_VIRT_OFFSET: u64 = 0xffff_0000_0000_0000;

/// Linux kernel image area base (from System.map _text).
/// Guest PA = VA - KERNEL_IMAGE_TEXT_VA + KERNEL_IMAGE_TEXT_PA
const KERNEL_IMAGE_TEXT_VA: u64 = 0xffff_8000_8000_0000;
const KERNEL_IMAGE_TEXT_PA: u64 = 0x8020_0000;

/// GPA of `printk_rb_static` symbol.
/// VA 0xffff800081298108 → PA = (VA - 0xffff800080000000) + 0x80200000 = 0x81498108
const PRINTK_RB_STATIC_GPA: u64 = 0x8149_8108;

/// Descriptor state: finalized (ready for reading).
const DESC_FINALIZED: u64 = 0x2;
const DESC_STATE_MASK: u64 = 0x3;

/// Maximum bytes per HVC #9 read.
const MAX_READ_SIZE: usize = 64 * 1024;

/// Size of printk_info on aarch64 (u64+u64+u16+u8+u8+u32+char[16]+char[48]).
const PRINTK_INFO_SIZE: usize = 88;
/// Size of prb_desc on aarch64 (state_var + text_blk_lpos + next).
const PRB_DESC_SIZE: usize = 24;
/// Text length field offset within printk_info.
const INFO_TEXT_LEN_OFF: usize = 16;
/// text_blk_lpos.begin offset within prb_desc.
const DESC_BEGIN_OFF: usize = 8;

/// Offset of key fields inside printk_ringbuffer.
const RB_DESC_RING_COUNT_BITS_OFF: usize = 0;
const RB_DESC_RING_DESCS_OFF: usize = 8;
const RB_DESC_RING_INFOS_OFF: usize = 16;
const RB_DATA_RING_SIZE_BITS_OFF: usize = 48;
const RB_DATA_RING_DATA_OFF: usize = 56;

/// How many descriptors to process per batch.
const DESC_BATCH: usize = 256;

pub struct KernelLogResult {
    pub raw_text: String,
    pub bytes_read: usize,
}

/// Read a u64 from a byte slice at the given offset (little-endian).
fn read_u64_le(buf: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buf[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

/// Read a u32 from a byte slice (little-endian).
fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&buf[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}

/// Read a u16 from a byte slice (little-endian).
fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&buf[offset..offset + 2]);
    u16::from_le_bytes(bytes)
}

/// Convert a Guest Virtual Address to a Guest Physical Address.
pub fn gva_to_gpa(gva: u64) -> u64 {
    if gva >= KERNEL_IMAGE_TEXT_VA && gva < KERNEL_IMAGE_TEXT_VA + 0x200_0000 {
        gva - KERNEL_IMAGE_TEXT_VA + KERNEL_IMAGE_TEXT_PA
    } else if gva >= PHYS_VIRT_OFFSET {
        gva.wrapping_sub(PHYS_VIRT_OFFSET)
    } else {
        gva
    }
}

/// Read a buffer from guest memory via HVC #9.
fn read_guest_buf(vm_id: u64, gpa: u64, size: usize) -> Result<Vec<u8>, String> {
    let read_size = size.min(MAX_READ_SIZE);
    let mut buf = vec![0u8; read_size];
    let n = register::read_guest_mem(vm_id, gpa, &mut buf)?;
    if n == 0 {
        return Err("read_guest_mem returned 0 bytes".to_string());
    }
    buf.truncate(n);
    Ok(buf)
}

/// Collect kernel log using prb ringbuffer parsing.
pub fn collect_kernel_log(
    target_vm_id: u64,
    _log_buf_gpa: Option<u64>,
    _max_size: usize,
) -> Result<KernelLogResult, String> {
    // 1. Read printk_ringbuffer struct (~88 bytes, but 80 is enough)
    let rb_buf = read_guest_buf(target_vm_id, PRINTK_RB_STATIC_GPA, 80)?;

    let desc_count_bits = read_u32_le(&rb_buf, RB_DESC_RING_COUNT_BITS_OFF);
    let descs_va = read_u64_le(&rb_buf, RB_DESC_RING_DESCS_OFF);
    let infos_va = read_u64_le(&rb_buf, RB_DESC_RING_INFOS_OFF);
    let data_size_bits = read_u32_le(&rb_buf, RB_DATA_RING_SIZE_BITS_OFF);
    let data_va = read_u64_le(&rb_buf, RB_DATA_RING_DATA_OFF);

    let num_descs = 1usize << desc_count_bits;
    let data_size = 1u64 << data_size_bits;
    let data_mask = data_size - 1;
    let descs_gpa = gva_to_gpa(descs_va);
    let infos_gpa = gva_to_gpa(infos_va);
    let data_gpa = gva_to_gpa(data_va);

    ax_std::println!(
        "[log] prb: {} descs, data_ring={:#x} (gpa={:#x}), __log_buf gpa={:#x}",
        num_descs, data_va, data_gpa, data_gpa,
    );

    // 2. Read the data ring buffer (entire buffer)
    let data_ring = read_guest_buf(target_vm_id, data_gpa, data_size as usize)?;

    // 3. Iterate descriptors in batches
    let mut text = String::new();
    let mut total_bytes_read = 0usize;

    for batch_start in (0..num_descs).step_by(DESC_BATCH) {
        let batch_end = (batch_start + DESC_BATCH).min(num_descs);
        let batch_count = batch_end - batch_start;

        // Read descs batch
        let descs_buf = read_guest_buf(
            target_vm_id,
            descs_gpa + (batch_start * PRB_DESC_SIZE) as u64,
            batch_count * PRB_DESC_SIZE,
        )?;

        // Read infos batch
        let infos_buf = read_guest_buf(
            target_vm_id,
            infos_gpa + (batch_start * PRINTK_INFO_SIZE) as u64,
            batch_count * PRINTK_INFO_SIZE,
        )?;

        total_bytes_read += descs_buf.len() + infos_buf.len();

        for i in 0..batch_count {
            let desc_off = i * PRB_DESC_SIZE;
            if desc_off + 16 > descs_buf.len() {
                break;
            }

            // Check descriptor state
            let state_var = read_u64_le(&descs_buf, desc_off);
            let state = (state_var >> 62) & DESC_STATE_MASK;

            if state != DESC_FINALIZED {
                continue;
            }

            // Read text_blk_lpos.begin
            let begin_lpos = read_u64_le(&descs_buf, desc_off + DESC_BEGIN_OFF);

            // EMPTY_LINE_LPOS and FAILED_LPOS are negative (high bits set)
            if begin_lpos as i64 == i64::MIN
                || begin_lpos == u64::MAX
                || begin_lpos >= (1u64 << 62)
            {
                continue; // empty or failed data block
            }

            // Read text_len from info
            let info_off = i * PRINTK_INFO_SIZE;
            if info_off + INFO_TEXT_LEN_OFF + 2 > infos_buf.len() {
                break;
            }
            let text_len = read_u16_le(&infos_buf, info_off + INFO_TEXT_LEN_OFF) as usize;
            if text_len == 0 {
                continue;
            }

            // Convert lpos to offset in data ring (skip 8-byte ID)
            let data_off = ((begin_lpos as u64) & data_mask) as usize + 8;

            if data_off + text_len > data_ring.len() {
                // Wraps around the ring
                let first_part = data_ring.len() - data_off;
                append_text(&data_ring[data_off..], &mut text);

                let remaining = text_len - first_part;
                if remaining > 0 && remaining <= data_ring.len() {
                    append_text(&data_ring[..remaining], &mut text);
                }
            } else {
                append_text(&data_ring[data_off..data_off + text_len], &mut text);
            }
        }
    }

    ax_std::println!(
        "[log] collected {} bytes of metadata, {} chars of log text",
        total_bytes_read,
        text.len(),
    );

    Ok(KernelLogResult {
        raw_text: text.clone(),
        bytes_read: total_bytes_read,
    })
}

/// Append printable bytes to the text buffer, replacing non-printable
/// characters (except newline/tab) with spaces.
fn append_text(data: &[u8], out: &mut String) {
    for &c in data {
        if c >= 0x20 && c <= 0x7e || c == b'\n' || c == b'\r' || c == b'\t' {
            out.push(c as char);
        }
        // skip other control chars and nulls
    }
}
