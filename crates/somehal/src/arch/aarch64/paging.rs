use core::arch::asm;

use num_align::NumAlign;
use page_table_generic::{GB, MapConfig, PageTable, PageTableEntry};

use crate::{
    arch::{
        elx::{Pte, PteFlags, Table, set_table, setup_sctlr, setup_table_regs},
        entry::mmu_entry,
    },
    consts::VMLINUX_LOAD_ADDRESS,
    mem::{MB, kernel_vcode_offset, page_size, ram::Ram},
};

static BOOT_TABLE: spin::Once<PageTable<Table, Ram>> = spin::Once::new();

pub fn enable_mmu() -> ! {
    println!("Mapping early memory regions...");

    let k_start = crate::mem::kernel_range().start;

    let mut table = PageTable::<Table, _>::new(Ram).unwrap();

    let start = k_start.align_down(GB);
    let size = GB;
    let mut pte = Pte::new_valid();
    pte.update_flags(|f| {
        f.insert(PteFlags::SHAREABLE | PteFlags::INNER);
    });
    pte.set_mair_idx(1);

    pr_range!("Kernel", start, size);

    println!(
        "Mapping kernel identity: vaddr={:#x}, paddr={:#x}, size={:#x}",
        start, start, size
    );
    table
        .map(&MapConfig {
            vaddr: start.into(),
            paddr: start.into(),
            size,
            pte,
            allow_huge: true,
            flush: false,
        })
        .unwrap();

    let code_start = crate::kernel_code().as_ptr() as usize;
    let code_end = (crate::kernel_code().as_ptr_range().end as usize).align_up(2 * MB);
    let size = code_end - code_start;

    pr_range!("Kernel Code", code_start, size);

    table
        .map(&MapConfig {
            vaddr: VMLINUX_LOAD_ADDRESS.into(),
            paddr: code_start.into(),
            size,
            pte,
            allow_huge: true,
            flush: false,
        })
        .unwrap();

    let debug_base = unsafe { crate::console::DEBUG_BASE };
    if debug_base != 0 {
        let start = debug_base.align_down(page_size());
        let size = page_size();
        let mut pte = Pte::new_valid();
        pte.set_mair_idx(0);

        pr_range!("Debug UART", start, size);

        table
            .map(&MapConfig {
                vaddr: start.into(),
                paddr: start.into(),
                size,
                pte,
                allow_huge: true,
                flush: false,
            })
            .unwrap();
    }

    println!("Page table entries analysis:");
    for (i, pte_info) in table.walk(0.into(), usize::MAX.into()).enumerate() {
        if i < 10 {
            // Limit output to first 10 entries to avoid spam
            let paddr = pte_info.pte.paddr();
            let level = pte_info.level;
            let vaddr = pte_info.vaddr;
            let flags = pte_info.pte.as_flags();
            println!(
                "  PTE[{}]: vaddr={:#x}, paddr={:#x}, level={}, valid={}, huge={}, flags_bits={:#x}",
                i,
                vaddr.raw(),
                paddr.raw(),
                level,
                pte_info.pte.valid(),
                pte_info.pte.is_huge(),
                flags.bits()
            );
        }
    }

    let tb_addr = table.root_paddr();
    BOOT_TABLE.call_once(|| table);
    println!("Boot page table at physical address: {:#x}", tb_addr);

    // Use physical address to avoid virtual address mapping issues
    let mmu_entry_phys = mmu_entry as usize;
    println!("MMU Entry point at physical address: {:#x}", mmu_entry_phys);
    setup_table_regs();
    set_table(tb_addr.into());

    println!("Enabling MMU...");
    setup_sctlr();
    println!("MMU enabled.");

    // Jump to mmu_entry using physical address
    unsafe {
        asm!(
            "
            mov x8, {0}
            br x8
        ",
            in(reg) mmu_entry_phys,
            options(noreturn)
        )
    }
}
