//! Linux kallsyms decoder — reads compressed kernel symbol table from
//! a frozen target VM's memory via HVC #9 and builds a searchable `SymbolTable`.
//!
//! # Data format (from kernel/kallsyms.c + scripts/kallsyms.c)
//!
//! The kernel stores symbols in a compressed format to save space.
//! Addresses are stored in `kallsyms_offsets[]` as s32 relative offsets
//! from `kallsyms_relative_base`. Names are token-compressed in
//! `kallsyms_names[]` and decompressed using `kallsyms_token_table[]`
//! and `kallsyms_token_index[]`.
//!
//! # Usage
//!
//! ```ignore
//! let sym = kallsyms::read_kallsyms(target_vm_id, &kallsyms_addrs)?;
//! ```
//!
//! Where `kallsyms_addrs` contains the GPAs of each kallsyms array,
//! obtained from vmlinux `nm` output at build time.

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::recovery::symbol::{SymbolInfo, SymbolTable};

// ---------------------------------------------------------------------------
// GPA configuration for kallsyms arrays
//
// These addresses must be filled in after compiling vmlinux:
//   aarch64-linux-gnu-nm vmlinux | grep -E 'kallsyms_'
// Then convert from VA to GPA using gva_to_gpa().
// ---------------------------------------------------------------------------

/// Guest Physical Addresses of kallsyms data structures in the target VM.
/// Populated after vmlinux compilation.
pub struct KallsymsAddrs {
    /// `kallsyms_num_syms` — u32 count of symbols
    pub num_syms_gpa: u64,
    /// `kallsyms_relative_base` — u64 base for address calculation
    pub relative_base_gpa: u64,
    /// `kallsyms_offsets` — s32 array of offsets (length = num_syms)
    pub offsets_gpa: u64,
    /// `kallsyms_names` — u8 array of compressed symbol names
    pub names_gpa: u64,
    /// `kallsyms_token_table` — char array of token strings
    pub token_table_gpa: u64,
    /// `kallsyms_token_index` — u16 array of token indices
    pub token_index_gpa: u64,
    /// `kallsyms_markers` — u32 array of markers (length = num_syms / 256 + 1)
    pub markers_gpa: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read and decode kallsyms from a frozen target VM's memory.
///
/// # Arguments
///
/// * `target_vm_id` — Frozen target VM ID
/// * `addrs` — Guest physical addresses of kallsyms arrays
/// * `read_fn` — Memory reader: `read_fn(gpa, buf) -> Result<bytes_read, err>`
///
/// # Returns
///
/// A `SymbolTable` with all kernel symbols loaded, or an error string.
pub fn read_kallsyms(
    target_vm_id: u64,
    addrs: &KallsymsAddrs,
    read_fn: &impl Fn(u64, u64, &mut [u8]) -> Result<usize, String>,
) -> Result<SymbolTable, String> {
    // Step 1: Read num_syms (u32)
    let num_syms = read_u32(target_vm_id, addrs.num_syms_gpa, read_fn)?;
    if num_syms == 0 || num_syms > 1_000_000 {
        return Err(format!("kallsyms_num_syms out of range: {}", num_syms));
    }

    // Step 2: Read relative_base (u64)
    let relative_base = read_u64(target_vm_id, addrs.relative_base_gpa, read_fn)?;

    // Step 3: Read markers (u32 array, num_syms/256 + 1 entries)
    let marker_count = (num_syms >> 8) + 1;
    let mut markers = alloc::vec![0u32; marker_count as usize];
    for i in 0..marker_count {
        let gpa = addrs.markers_gpa.wrapping_add(i as u64 * 4);
        let val = read_u32(target_vm_id, gpa, read_fn)?;
        markers[i as usize] = val;
    }

    // Step 4: Read offsets array (s32 array, num_syms entries)
    let offsets_size = num_syms as usize * 4;
    let mut offsets_raw = alloc::vec![0u8; offsets_size];
    read_fn(target_vm_id, addrs.offsets_gpa, &mut offsets_raw)?;

    // Step 5: Pre-scan names array to figure out total size needed
    // We'll read names on-demand per symbol using markers for acceleration

    // Step 6: Read token table and token index (these are small)
    //   token_index: 256 * u16 = 512 bytes
    let mut token_index = [0u16; 256];
    for i in 0..256 {
        let gpa = addrs.token_index_gpa.wrapping_add(i as u64 * 2);
        let val = read_u16(target_vm_id, gpa, read_fn)?;
        token_index[i] = val;
    }

    //   token_table: read up to 64KB (should be more than enough)
    let mut token_table_buf = alloc::vec![0u8; 65536];
    let token_table_len = read_fn(target_vm_id, addrs.token_table_gpa, &mut token_table_buf)?;

    // Step 7: Decode each symbol and build symbol table
    let mut symbols = Vec::with_capacity(num_syms as usize);
    let mut sym_offset: u32 = 0; // our tracking of position within names

    for i in 0..num_syms as usize {
        // Get address: relative_base + offsets[i] (as s32)
        let off_val = i32::from_le_bytes([
            offsets_raw[i * 4],
            offsets_raw[i * 4 + 1],
            offsets_raw[i * 4 + 2],
            offsets_raw[i * 4 + 3],
        ]);
        let addr = relative_base.wrapping_add(off_val as i64 as u64);

        // Get the name offset in kallsyms_names using markers + sequential scan
        let name_start = match get_symbol_offset(i, &markers, target_vm_id, addrs.names_gpa, read_fn) {
            Ok(off) => off,
            Err(_) => break, // corrupted, stop decoding
        };

        // Read compressed name from the name buffer
        // First read just the length byte(s)
        let mut len_byte = [0u8; 1];
        let _ = read_fn(target_vm_id, addrs.names_gpa.wrapping_add(name_start as u64), &mut len_byte)?;

        let (name_len, name_data_start) = if (len_byte[0] & 0x80) != 0 {
            // Big symbol: extra length byte
            let mut extra = [0u8; 1];
            let _ = read_fn(
                target_vm_id,
                addrs.names_gpa.wrapping_add(name_start as u64 + 1),
                &mut extra,
            )?;
            let total_len = ((len_byte[0] & 0x7F) as usize) | ((extra[0] as usize) << 7);
            (total_len, name_start + 2)
        } else {
            (len_byte[0] as usize, name_start + 1)
        };

        if name_len == 0 || name_len > 4096 {
            continue; // skip corrupted entry
        }

        // Read the compressed name data
        let mut name_data = alloc::vec![0u8; name_len];
        let _ = read_fn(
            target_vm_id,
            addrs.names_gpa.wrapping_add(name_data_start as u64),
            &mut name_data,
        )?;

        // Decompress
        let name = kallsyms_expand_symbol(&name_data, &token_table_buf[..token_table_len], &token_index);
        if name.is_empty() {
            continue;
        }

        symbols.push(SymbolInfo {
            name,
            addr,
            size: 0, // kallsyms doesn't store symbol sizes
        });
    }

    symbols.sort_by_key(|s| s.addr);
    symbols.dedup_by_key(|s| s.addr);

    ax_std::println!(
        "[kallsyms] loaded {} symbols from frozen target VM[{}]",
        symbols.len(),
        target_vm_id,
    );

    Ok(SymbolTable::from_sorted_symbols(
        symbols,
        0, // kallsyms addresses are absolute, no adjustment needed
    ))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the offset within `kallsyms_names` for the symbol at position `pos`.
///
/// Uses markers for acceleration: markers[pos >> 8] gives the offset in
/// names at the start of the marker group (every 256 symbols), then
/// scans forward `pos & 0xFF` symbols.
fn get_symbol_offset(
    pos: usize,
    markers: &[u32],
    target_vm_id: u64,
    names_gpa: u64,
    read_fn: &impl Fn(u64, u64, &mut [u8]) -> Result<usize, String>,
) -> Result<u32, String> {
    let marker_idx = pos >> 8;
    let marker_off = if marker_idx < markers.len() {
        markers[marker_idx]
    } else {
        0
    };

    let mut off = marker_off;
    let remaining = pos & 0xFF;
    let mut buf = [0u8; 3]; // max: 2 length bytes + 1 peek

    for _ in 0..remaining {
        // Read length byte
        let _ = read_fn(target_vm_id, names_gpa.wrapping_add(off as u64), &mut buf[..1])?;
        let len_byte = buf[0];
        let entry_len = if (len_byte & 0x80) != 0 {
            // Big symbol: 2 length bytes
            let _ = read_fn(target_vm_id, names_gpa.wrapping_add(off as u64 + 1), &mut buf[..1])?;
            let total = ((len_byte & 0x7F) as u32) | ((buf[0] as u32) << 7);
            off += 1 + 1 + total // len_byte + extra_len + data
        } else {
            off += 1 + len_byte as u32
        };
    }

    Ok(off)
}

/// Decompress a kallsyms name from its compressed token representation.
///
/// Each byte in `compressed` is an index into `token_index`, which gives
/// the offset into `token_table`. The token string is copied character by
/// character until null, skipping the first character (symbol type).
fn kallsyms_expand_symbol(
    compressed: &[u8],
    token_table: &[u8],
    token_index: &[u16; 256],
) -> String {
    let mut result = String::new();
    let mut skipped_first = false;

    for &code in compressed {
        let idx = code as usize;
        if idx >= token_index.len() {
            continue;
        }
        let table_off = token_index[idx] as usize;
        if table_off >= token_table.len() {
            continue;
        }

        // Copy token string until null
        let mut tptr = table_off;
        while tptr < token_table.len() {
            let c = token_table[tptr];
            if c == 0 {
                break;
            }
            if skipped_first {
                result.push(c as char);
            } else {
                skipped_first = true; // skip symbol type char
            }
            tptr += 1;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Low-level memory readers
// ---------------------------------------------------------------------------

fn read_u32(
    target_vm_id: u64,
    gpa: u64,
    read_fn: &impl Fn(u64, u64, &mut [u8]) -> Result<usize, String>,
) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    read_fn(target_vm_id, gpa, &mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(
    target_vm_id: u64,
    gpa: u64,
    read_fn: &impl Fn(u64, u64, &mut [u8]) -> Result<usize, String>,
) -> Result<u64, String> {
    let mut buf = [0u8; 8];
    read_fn(target_vm_id, gpa, &mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_u16(
    target_vm_id: u64,
    gpa: u64,
    read_fn: &impl Fn(u64, u64, &mut [u8]) -> Result<usize, String>,
) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    read_fn(target_vm_id, gpa, &mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock memory reader that simulates kallsyms data
    fn mock_read_fn_simple() -> impl Fn(u64, u64, &mut [u8]) -> Result<usize, String> {
        // Build a minimal mock kallsyms:
        // - 2 symbols: "panic" @ 0xffff_8000_8020_0100, "start_kernel" @ 0xffff_8000_8020_0000
        // - Simple token table (just store characters directly)
        // - No compression (each byte = token index for itself)
        |_vm_id: u64, gpa: u64, buf: &mut [u8]| -> Result<usize, String> {
            let base: u64 = 0x1000; // pretend GPA base
            let off = (gpa - base) as usize;
            let data: &[u8] = &[
                // 0x1000: kallsyms_num_syms = 2 (u32 LE)
                0x02, 0x00, 0x00, 0x00,
                // 0x1004: kallsyms_relative_base = 0xffff_8000_8020_0000 (u64 LE)
                0x00, 0x00, 0x20, 0x80, 0x00, 0x80, 0xff, 0xff,
                // 0x100C: kallsyms_offsets (s32 array, relative to base)
                0x00, 0x00, 0x00, 0x00, // offset 0 → addr = base + 0 = start_kernel
                0x00, 0x01, 0x00, 0x00, // offset 256 → addr = base + 256 = start_kernel + 256
                // 0x1014: kallsyms_names (compressed names)
                // symbol 0: "start_kernel"
                13, 0x73, 0x74, 0x61, 0x72, 0x74, 0x5F, 0x6B, 0x65, 0x72, 0x6E, 0x65, 0x6C, 0x00,
                // symbol 1: "panic"
                5, 0x70, 0x61, 0x6E, 0x69, 0x63, 0x00,
                // ... rest would be token table etc.
            ];
            let n = buf.len().min(data.len().saturating_sub(off));
            buf[..n].copy_from_slice(&data[off..off + n]);
            Ok(n)
        }
    }

    #[test]
    fn test_read_u32() {
        let read_fn = mock_read_fn_simple();
        let val = read_u32(1, 0x1000, &read_fn).unwrap();
        assert_eq!(val, 2);
    }

    #[test]
    fn test_read_u64() {
        let read_fn = mock_read_fn_simple();
        let val = read_u64(1, 0x1004, &read_fn).unwrap();
        assert_eq!(val, 0xffff_8000_8020_0000);
    }
}
