extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::string::ToString;


use crate::capture::export;
use crate::capture::log;
use crate::capture::modules;
use crate::capture::register::{self, CrashVcpuRegs};
use crate::capture::storage;
use crate::capture::memory;
use crate::monitor::event::CrashEvent;
use crate::recovery::analyzer;
use crate::recovery::kallsyms::{self, KallsymsAddrs};
use crate::recovery::report;
use crate::recovery::symbol::SymbolTable;
use serde::{Deserialize, Serialize};

/// Target VM configuration — should be set from VM config at startup.
const TARGET_VM_ID: u64 = 1;
const TARGET_VCPU_COUNT: u64 = 1;

/// Guest physical memory regions to dump (must match target VM memory layout).
/// Linux kernel Image is loaded at the start of the VM's memory region.
/// 64 MiB covers: kernel image (~22 MiB), BSS, data, slab allocator, and
/// the common kernel stack region. Satisfies the "partial dump" requirement.
/// From AxVisor log: gpa=GPA:0x223600000, size=256 MiB.
const MEMORY_REGIONS: &[(u64, usize)] = &[
    // Two 64 MiB regions (instead of one 128 MiB) to avoid exceeding the
    // per-allocation limit of ArceOS's heap allocator.
    (0x223600000, 0x0400_0000),  // 64 MiB — lower half
    (0x227600000, 0x0400_0000),  // 64 MiB — upper half
];

/// Guest kernel linear mapping offset (GVA → GPA).
/// Linux ARM64 uses PAGE_OFFSET = 0xffff_0000_0000_0000 (48-bit VA).
/// So virtual address → physical: GPA = VA - PHYS_VIRT_OFFSET.
const PHYS_VIRT_OFFSET: u64 = 0xffff_0000_0000_0000;

/// Guest physical memory base address (from target VM config).
/// The target VM's RAM starts at GPA 0x8000_0000 with 256 MB.

/// Kernel image virtual address base (KIMAGE_VADDR).
const KERNEL_IMAGE_TEXT_VA: u64 = 0xffff_8000_8000_0000;

/// Path to the target kernel ELF (with symbol table) inside the monitor guest's filesystem.
/// Set to empty string to disable ELF symbol resolution (kallsyms will be used instead).
/// The ELF must be baked into the monitor-guest image at build time.
const KERNEL_ELF_PATH: &str = "";

/// Kernel base virtual address for symbol table lookups.
/// If the ELF has absolute virtual addresses (ET_EXEC linked at KERNEL_BASE_VADDR),
/// set this to 0 so that `addr - 0 = addr` matches `sym.st_value` directly.
/// If the ELF has relative offsets (PIE), set this to PHYS_VIRT_OFFSET + kernel_load_addr.
const KERNEL_BASE_ADDR: u64 = 0;

/// Kallsyms GPA configuration — uses locate for dynamic HPA_BASE.
fn get_kallsyms_addrs() -> KallsymsAddrs {
    use crate::capture::locate;
    KallsymsAddrs {
        num_syms_gpa:         locate::offset_to_hpa(locate::offsets::KALLSYMS_NUM_SYMS),
        relative_base_gpa:    locate::offset_to_hpa(locate::offsets::KALLSYMS_RELATIVE_BASE),
        offsets_gpa:          locate::offset_to_hpa(locate::offsets::KALLSYMS_OFFSETS),
        names_gpa:            locate::offset_to_hpa(locate::offsets::KALLSYMS_NAMES),
        token_table_gpa:      locate::offset_to_hpa(locate::offsets::KALLSYMS_TOKEN_TABLE),
        token_index_gpa:      locate::offset_to_hpa(locate::offsets::KALLSYMS_TOKEN_INDEX),
        markers_gpa:          locate::offset_to_hpa(locate::offsets::KALLSYMS_MARKERS),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Kernel log buffer text, if capture succeeded.
    pub kernel_log: Option<String>,
    /// Loaded kernel module information.
    pub modules: Vec<modules::ModuleInfo>,
}

pub fn capture_snapshot(event: CrashEvent) {
    let timestamp = boot_timestamp();
    ax_std::println!("[capture] start snapshot (event={:?})", event);

    // Locate the kernel dynamically so GPA calculations use the correct HPA_BASE.
    crate::capture::locate::locate_kernel(TARGET_VM_ID);
    let hpa_base = crate::capture::locate::get_hpa_base().unwrap_or(MEMORY_REGIONS[0].0);
    ax_std::println!("[capture] using HPA_BASE={:#x} (fallback={:#x})", hpa_base, MEMORY_REGIONS[0].0);

    // Ensure /vmcore/ directory exists for memory dump files.
    let _ = ax_std::fs::create_dir("/vmcore");

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
    // Step 3: Collect kernel log buffer.
    let kernel_log = match log::collect_kernel_log(TARGET_VM_ID, None, 64 * 1024) {
        Ok(result) => {
            ax_std::println!("[capture] kernel log collected: {} chars", result.raw_text.len());
            Some(result.raw_text)
        }
        Err(e) => {
            ax_std::println!("[capture] kernel log skipped: {}", e);
            None
        }
    };

    // Step 4: Load kallsyms + find page table for vmalloc translation.
    // We do this early so modules collection and analysis can use it.
    let (sym, _pgd_gpa) = 'load_sym: {
        let addrs = get_kallsyms_addrs();
        match kallsyms::read_kallsyms(TARGET_VM_ID, &addrs, &register::read_guest_mem) {
            Ok(table) => {
                ax_std::println!("[recovery] kallsyms loaded: {} symbols", table.len());
                let read_64 = |pa: u64| -> Option<u64> {
                    let mut buf = [0u8; 8];
                    register::read_guest_mem(TARGET_VM_ID, pa, &mut buf).ok()?;
                    Some(u64::from_le_bytes(buf))
                };
                let pgd = crate::recovery::page_table::find_pgd_pa(&table, &read_64);
                if let Ok(pa) = pgd {
                    ax_std::println!("[recovery] page table PGD @ GPA {:#x}", pa);
                } else if let Err(ref e) = pgd {
                    ax_std::println!("[recovery] page table unavailable: {}", e);
                }
                break 'load_sym (Some(table), pgd.ok());
            }
            Err(e) => ax_std::println!("[recovery] kallsyms unavailable: {}", e),
        }
        (None, None)
    };

    // Step 5: Collect loaded kernel modules (three strategies).
    let mut modules: Vec<modules::ModuleInfo> = Vec::new();

    // 5a: ELF header scan on dumped memory.
    let dump_refs: Vec<(u64, &[u8])> = memory_regions_data
        .iter()
        .map(|(base, data)| (*base, data.as_slice()))
        .collect();
    if let Ok(ref result) = modules::collect_modules(TARGET_VM_ID, None, &|_| None, &dump_refs) {
        for m in &result.modules {
            if !modules.iter().any(|x| x.name == m.name) {
                modules.push(m.clone());
            }
        }
    }

    // 5b: dmesg Call-trace extraction.
    // The kernel prints function+offset [module_name] in the Call trace.
    if let Some(ref log) = kernel_log {
        for line in log.lines() {
            // Look for " [module_name]" suffix in backtrace lines
            if let Some(start) = line.find(" [") {
                let bracket = &line[start..];
                if bracket.ends_with(']') {
                    let name = bracket[2..bracket.len() - 1].trim();
                    if !name.is_empty()
                        && !modules.iter().any(|m| m.name == name)
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    {
                        ax_std::println!("[capture] module '{}' found via dmesg", name);
                        modules.push(modules::ModuleInfo {
                            name: name.to_string(),
                            base_addr: 0,
                            size: 0,
                        });
                    }
                }
            }
        }
    }

    // 5c: kernel module list walk (needs GVA→GPA translation for vmalloc).
    if let Some(ref sym_table) = sym {
        // Helper: check if GPA is within a known valid guest-memory range.
        let is_valid_gpa = |gpa: u64| -> bool {
            // Primary mapping:  [0x80000000 .. 0x90000000)  (256 MB)
            // Identity mapping: [hpa_base .. hpa_base+256MB)
            (gpa >= 0x8000_0000 && gpa < 0x9000_0000)
            || (gpa >= hpa_base && gpa < hpa_base + 0x1000_0000)
        };

        // Helper: read 8 bytes from a raw GPA (no translation).
        let read_raw_gpa = |gpa: u64| -> Option<u64> {
            if !is_valid_gpa(gpa) {
                ax_std::println!("[capture] read_gva: rejecting invalid GPA {:#x}", gpa);
                return None;
            }
            let mut buf = [0u8; 8];
            register::read_guest_mem(TARGET_VM_ID, gpa, &mut buf).ok()?;
            Some(u64::from_le_bytes(buf))
        };

        let read_gva = |gva: u64| -> Option<u64> {
            ax_std::println!("[capture] read_gva(0x{:016x})", gva);

            // 1. Kernel image (first 32 MiB)
            if gva >= KERNEL_IMAGE_TEXT_VA
                && gva < KERNEL_IMAGE_TEXT_VA + 0x0200_0000
            {
                let gpa = gva.wrapping_sub(KERNEL_IMAGE_TEXT_VA)
                    .wrapping_add(hpa_base);
                ax_std::println!("[capture]   → kernel image GPA={:#x}", gpa);
                return read_raw_gpa(gpa);
            }
            // 2. Linear mapping: GVA - PAGE_OFFSET = GPA (identity-mapped view)
            if gva >= PHYS_VIRT_OFFSET && gva < 0xffff_8000_0000_0000u64 {
                let gpa = gva.wrapping_sub(PHYS_VIRT_OFFSET);
                ax_std::println!("[capture]   → linear map GPA={:#x}", gpa);
                return read_raw_gpa(gpa);
            }
            // 3. vmalloc → page table walk (requires PGD)
            if gva >= 0xffff_8000_0000_0000u64 {
                if let Some(pgd) = _pgd_gpa {
                    ax_std::println!("[capture]   → page table walk PGD={:#x}", pgd);
                    let gpa = crate::recovery::page_table::gva_to_gpa(
                        gva, pgd,
                        &|pa: u64| {
                            ax_std::println!("[capture]     pt_read(GPA={:#x})", pa);
                            let r = read_raw_gpa(pa);
                            ax_std::println!("[capture]     pt_read → {:?}", r);
                            r
                        },
                    );
                    match gpa {
                        Some(pa) => {
                            ax_std::println!("[capture]   → walk result GPA={:#x}", pa);
                            let data = read_raw_gpa(pa);
                            ax_std::println!("[capture]   → data={:?}", data);
                            return data;
                        }
                        None => {
                            ax_std::println!("[capture]   → walk FAILED (invalid vmalloc addr)");
                            return None;
                        }
                    }
                }
                ax_std::println!("[capture]   → no PGD, can't translate vmalloc");
                return None;
            }
            // 4. Fallback: raw GPA pass-through
            ax_std::println!("[capture]   → raw GPA={:#x}", gva);
            read_raw_gpa(gva)
        };
        let list_result = modules::collect_modules_via_list(sym_table, &read_gva);
        for m in list_result.modules {
            if let Some(existing) = modules.iter_mut().find(|x| x.name == m.name) {
                // Overwrite ELF scan result with list walk (has correct base_addr).
                existing.base_addr = m.base_addr;
                existing.size = m.size;
                ax_std::println!("[capture] module '{}' updated via list walk @ {:#x} ({} bytes)",
                    m.name, m.base_addr, m.size);
            } else {
                ax_std::println!("[capture] module '{}' via list walk @ {:#x} ({} bytes)",
                    m.name, m.base_addr, m.size);
                modules.push(m);
            }
        }
    }

    ax_std::println!("[capture] modules: {} total", modules.len());
    for m in &modules {
        ax_std::println!("  module: {} @ {:#x} ({} bytes)", m.name, m.base_addr, m.size);
    }

    // Detect double fault from register state.
    //
    // ARM64 exception levels: SPSR_EL1.M bits indicate the mode BEFORE the
    // exception was taken.
    //   M=5 (EL1h) → exception taken from normal kernel code
    //   M=4 (EL1t) → exception taken from exception handler → nested!
    //
    // Three signals are used (in priority order):
    //   A) SPSR.M == 4  — direct evidence of nested exception (rare:
    //      hypervisor must trap the *second* fault while Linux handled
    //      the first one internally).
    //   B) crash_type != 0 — hypervisor trapped a hardware exception
    //      (Data/Instruction Abort etc.).  If the kernel later called
    //      panic(), the panic was a *consequence* of the hardware fault.
    //      Reclassify as Exception.
    //   C) dmesg contains ESR/FAR — when the hypervisor does NOT trap
    //      EL1 exceptions (passthrough mode), crash_type is always 0.
    //      Fall back to parsing dmesg for ESR values that indicate a
    //      hardware abort.
    let actual_event = 'reclassify: {
        // Try register-based check first (signals A + B).
        if let Some((_, r)) = vcpu_regs.first() {
            let ec = (r.esr_el1 >> 26) & 0x3F;
            let el = r.spsr_el1 & 0xF;
            let is_abort = matches!(ec, 0x20 | 0x21 | 0x24 | 0x25);

            if is_abort {
                if el == 4 {
                    ax_std::println!(
                        "[capture] double fault detected (EC={:#x}, SPSR.M=4, crash_type={})",
                        ec, r.crash_type
                    );
                    break 'reclassify CrashEvent::DoubleFault;
                }
                if r.crash_type != 0 {
                    ax_std::println!(
                        "[capture] hardware exception (EC={:#x}, crash_type={}) — reclassifying as Exception",
                        ec, r.crash_type
                    );
                    break 'reclassify CrashEvent::Exception;
                }
            }
        }

        // Signal C: dmesg fallback — the hypervisor didn't trap the
        // exception, but Linux printed ESR/FAR in dmesg.
        if let Some(ref log_text) = kernel_log {
            let (dmesg_esr, _dmesg_far) = crate::recovery::analyzer::extract_esr_far_from_dmesg(log_text);
            if dmesg_esr != 0 {
                let ec = (dmesg_esr >> 26) & 0x3F;
                let is_abort = matches!(ec, 0x20 | 0x21 | 0x24 | 0x25);
                if is_abort {
                    ax_std::println!(
                        "[capture] hardware exception from dmesg (ESR={:#x}, EC={:#x}) — reclassifying as Exception",
                        dmesg_esr, ec
                    );
                    break 'reclassify CrashEvent::Exception;
                }
            }
        }

        event
    };

    let snapshot = CrashSnapshot {
        event: actual_event,
        vcpu_regs,
        memory_segments: memory_segments.clone(),
        kernel_log,
        modules,
    };

    ax_std::println!("[capture] snapshot captured");

    // Step 6: Save vmcore to persistent storage.
    if let Ok(vmcore_path) = storage::save_vmcore(&snapshot) {
        ax_std::println!("[capture] vmcore saved at: {}", vmcore_path);

        // Step 7: Load vmcore and run recovery analysis.
        if let Some(vmcore) = storage::load_vmcore(&vmcore_path) {
            ax_std::println!("[recovery] starting crash analysis...");

            // Load kallsyms symbol table from frozen target VM for function
            // name resolution and page-table walking.
            let sym = 'load: {
                let addrs = get_kallsyms_addrs();
                match kallsyms::read_kallsyms(
                        TARGET_VM_ID,
                        &addrs,
                        &register::read_guest_mem,
                    ) {
                        Ok(table) => {
                            ax_std::println!(
                                "[recovery] kallsyms loaded: {} symbols",
                                table.len(),
                            );
                            break 'load Some(table);
                        }
                        Err(e) => {
                            ax_std::println!("[recovery] kallsyms unavailable: {}", e);
                        }
                    }

                // Fall back to embedded ELF symbol file
                if !KERNEL_ELF_PATH.is_empty() {
                    match SymbolTable::from_kernel_elf(KERNEL_ELF_PATH, KERNEL_BASE_ADDR) {
                        Ok(table) => {
                            ax_std::println!(
                                "[recovery] symbol table loaded from ELF: {} symbols",
                                table.len(),
                            );
                            break 'load Some(table);
                        }
                        Err(e) => {
                            ax_std::println!("[recovery] ELF symbol table unavailable: {}", e);
                        }
                    }
                }

                None
            };

            // Try to find the kernel's master page table for vmalloc address
            // translation (needed for kernel stack unwinding, module list, etc.).
            let pgd_gpa = sym.as_ref().and_then(|s| {
                use crate::recovery::page_table;
                let read_64 = |pa: u64| -> Option<u64> {
                    let mut buf = [0u8; 8];
                    register::read_guest_mem(TARGET_VM_ID, pa, &mut buf).ok()?;
                    Some(u64::from_le_bytes(buf))
                };
                match page_table::find_pgd_pa(s, &read_64) {
                    Ok(pa) => {
                        ax_std::println!("[recovery] page table PGD @ GPA {:#x}", pa);
                        Some(pa)
                    }
                    Err(e) => {
                        ax_std::println!("[recovery] page table unavailable: {}", e);
                        None
                    }
                }
            });

            // Build a memory reader closure that translates GVA → GPA via:
            //   1. Kernel image formula (first 32 MiB — actual kernel image)
            //   2. Linear mapping formula
            //   3. Page table walk (vmalloc / module addresses)
            //   4. HVC #9 fallback
            // (GPA range check applied before any HVC #9 call.)
            const KERNEL_IMAGE_SIZE: u64 = 0x0200_0000; // 32 MiB

            // Helper: validate GPA and read 8 bytes from target VM memory.
            let read_mem_gpa = |gpa: u64| -> Option<u64> {
                // Must be in primary or identity-mapped RAM range.
                if !(gpa >= 0x8000_0000 && gpa < 0x9000_0000)
                    && !(gpa >= hpa_base && gpa < hpa_base + 0x1000_0000)
                {
                    ax_std::println!("[recovery] read_mem_gpa: rejecting invalid GPA {:#x}", gpa);
                    return None;
                }
                // Try dump segments first (faster, avoids HVC #9).
                if let Some((base, data)) = memory_regions_data
                    .iter()
                    .find(|(base, data)| gpa >= *base && gpa < *base + data.len() as u64)
                {
                    let offset = (gpa - *base) as usize;
                    if offset + 8 <= data.len() {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(&data[offset..offset + 8]);
                        return Some(u64::from_le_bytes(bytes));
                    }
                }
                // HVC #9 fallback
                let mut buf = [0u8; 8];
                register::read_guest_mem(TARGET_VM_ID, gpa, &mut buf).ok()?;
                Some(u64::from_le_bytes(buf))
            };

            let mem_reader = |addr: u64| -> Option<u64> {
                let gpa = if addr >= KERNEL_IMAGE_TEXT_VA
                    && addr < KERNEL_IMAGE_TEXT_VA + KERNEL_IMAGE_SIZE
                {
                    // Kernel image region (first 32 MiB)
                    addr.wrapping_sub(KERNEL_IMAGE_TEXT_VA)
                        .wrapping_add(hpa_base)
                } else if addr >= PHYS_VIRT_OFFSET
                    && addr < 0xffff_8000_0000_0000u64
                {
                    // Linear mapping: GVA - PAGE_OFFSET = GPA (identity-mapped view).
                    addr.wrapping_sub(PHYS_VIRT_OFFSET)
                } else if addr >= 0xffff_8000_0000_0000u64 && pgd_gpa.is_some()
                {
                    // vmalloc / module region → page table walk
                    let read_64 = |pa: u64| -> Option<u64> {
                        let mut buf = [0u8; 8];
                        register::read_guest_mem(TARGET_VM_ID, pa, &mut buf).ok()?;
                        Some(u64::from_le_bytes(buf))
                    };
                    crate::recovery::page_table::gva_to_gpa(addr, pgd_gpa.unwrap(), &read_64)?
                } else {
                    // Raw GPA
                    addr
                };
                // Read 8 bytes from the translated GPA (with validity check).
                read_mem_gpa(gpa)
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
            let (json_path, md_path) = match report::save_reports(&result, &report_base) {
                Ok(paths) => {
                    ax_std::println!("[recovery] analysis reports saved:");
                    ax_std::println!("  JSON: {}", paths.0);
                    ax_std::println!("  MD:   {}", paths.1);
                    paths
                }
                Err(e) => {
                    ax_std::println!("[recovery] failed to save reports: {}", e);
                    // Generate placeholder paths so downstream export still works
                    (alloc::format!("{}.json", report_base), alloc::format!("{}.md", report_base))
                }
            };

            // Step 6: Export all files to hypervisor storage via HVC #11.
            // Re-read saved files and send them to the hypervisor.
            let mut export_entries: Vec<(String, Vec<u8>)> = Vec::new();

            // 6a — vmcore JSON
            if let Ok(content) = ax_std::fs::read_to_string(&vmcore_path) {
                let fname = vmcore_path.rsplit('/').next().unwrap_or("vmcore.json");
                export_entries.push((fname.to_string(), content.into_bytes()));
            }

            // 6b — analysis reports (best-effort, skip if save_reports failed)
            for path in [&json_path, &md_path] {
                if let Ok(content) = ax_std::fs::read_to_string(path) {
                    let fname = path.rsplit('/').next().unwrap_or("report");
                    export_entries.push((fname.to_string(), content.into_bytes()));
                }
            }

            // 6c — memory dump binaries
            for seg in &memory_segments {
                let mem_path = alloc::format!("/vmcore/{}", seg.path);
                if let Ok(data) = ax_std::fs::read(&mem_path) {
                    let fname = seg.path.rsplit('/').next().unwrap_or("memory.bin");
                    export_entries.push((fname.to_string(), data));
                }
            }

            if !export_entries.is_empty() {
                let refs: Vec<(&str, &[u8])> = export_entries
                    .iter()
                    .map(|(n, d)| (n.as_str(), d.as_slice()))
                    .collect();
                export::export_files(&refs);
            } else {
                ax_std::println!("[export] no files to export");
            }

            // Step 7: Enter interactive crash analysis console.
            ax_std::println!("[console] entering interactive analysis shell...");
            crate::recovery::console::interactive_shell(
                &vmcore,
                &mem_reader,
                sym.as_ref(),
                &result,
            );
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