use alloc::boxed::Box;

use crate::hal::al::*;

pub fn init() {
    info!("Setting up MMU and page tables");

    let mut pt = memory::page_table_new();
    map_regions(&mut pt);
    let pt_addr = pt.addr();
    memory::set_kernel_page_table(pt_addr);
    memory::enable_paging();
}

fn map_regions(pt: &mut Box<dyn PageTable>) {
    for region in memory::memory_map() {
        let phys = PhysAddr::from(region.physical_start);
        let virt = VirtAddr::from(phys);
        pt.map(
            virt.raw().into(),
            phys.raw().into(),
            region.size_in_bytes,
            MemConfig {
                access: AccessFlags::READ | AccessFlags::WRITE | AccessFlags::EXECUTE,
                attrs: MemAttributes::Normal,
            },
            false,
        )
        .expect("Failed to map memory region");
    }
}
