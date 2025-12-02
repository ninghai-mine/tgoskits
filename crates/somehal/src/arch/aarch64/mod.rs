#[macro_use]
mod _macros;

#[cfg(feature = "hv")]
#[path = "el2/mod.rs"]
mod elx;

#[cfg(not(feature = "hv"))]
#[path = "el1/mod.rs"]
mod elx;

mod context;
mod entry;
mod head;
mod paging;
mod relocate;
mod trap;

use elx::*;

use crate::ArchTrait;

pub struct Arch;

impl ArchTrait for Arch {
    type PT = paging::Generic;

    fn post_allocator() {}

    fn kernel_code() -> &'static [u8] {
        let start = ext_sym_addr!(_head);
        let end = ext_sym_addr!(__kernel_code_end);
        let size = end - start;
        unsafe { core::slice::from_raw_parts(start as *const u8, size) }
    }

    fn _pa(vaddr: *const u8) -> usize {
        vaddr as usize - crate::consts::KERNEL_LINER_OFFSET
    }

    fn _va(paddr: usize) -> *mut u8 {
        (paddr + crate::consts::KERNEL_LINER_OFFSET) as *mut u8
    }

    fn ioremap(paddr: usize, size: usize) -> *mut u8 {
        if crate::mem::is_mmu_enabled() {
            todo!()
        } else {
            paddr as *mut u8
        }
    }

    fn _io(paddr: usize) -> *mut u8 {
        Self::_va(paddr)
    }

    fn per_cpu_trap_init(is_primary: bool) {
        todo!()
    }

    fn systimer_irq() -> usize {
        todo!()
    }

    fn systimer_enable() {}

    fn systimer_disable() {}

    fn systimer_set_interval(_ticks: u64) {}

    fn systimer_ack() {}

    fn systimer_freq() -> u64 {
        todo!()
    }

    fn systimer_tick() -> u64 {
        todo!()
    }

    fn shutdown() -> ! {
        todo!()
    }

    fn irq_all_is_enabled() -> bool {
        todo!()
    }

    fn irq_all_set_enable(enable: bool) {
        todo!()
    }

    fn set_kernel_page_table<A: page_table_generic::FrameAllocator>(
        pt: page_table_generic::PageTable<Self::PT, A>,
    ) {
        todo!()
    }

    fn get_kernel_page_table<A: page_table_generic::FrameAllocator>()
    -> page_table_generic::PageTable<Self::PT, A> {
        todo!()
    }

    fn kernel_page_table_paddr_asid() -> (usize, usize) {
        todo!()
    }

    fn set_kernel_page_table_paddr_asid(paddr: usize, asid: usize) {
        todo!()
    }

    fn irq_is_enabled(crate::irq: crate::irq::SoftIrqId) -> bool {
        todo!()
    }

    fn irq_set_enable(crate::irq: crate::irq::SoftIrqId, enable: bool) {
        todo!()
    }
}
