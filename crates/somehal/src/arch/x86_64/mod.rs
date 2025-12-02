use crate::ArchTrait;

mod paging;

pub struct Arch;

impl ArchTrait for Arch {
    type PT = paging::Table;

    fn kernel_code() -> &'static [u8] {
        todo!()
    }

    fn post_allocator() {
        todo!()
    }

    fn per_cpu_trap_init(is_primary: bool) {
        todo!()
    }

    fn _pa(vaddr: *const u8) -> usize {
        todo!()
    }

    fn _va(paddr: usize) -> *mut u8 {
        todo!()
    }

    fn _io(paddr: usize) -> *mut u8 {
        todo!()
    }

    fn ioremap(paddr: usize, size: usize) -> *mut u8 {
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

    fn systimer_irq() -> usize {
        todo!()
    }

    fn shutdown() -> ! {
        todo!()
    }

    fn systimer_enable() {
        todo!()
    }

    fn systimer_disable() {
        todo!()
    }

    fn systimer_set_interval(ticks: usize) {
        todo!()
    }

    fn systimer_ack() {
        todo!()
    }

    fn systimer_freq() -> usize {
        todo!()
    }

    fn systimer_tick() -> usize {
        todo!()
    }

    fn irq_all_is_enabled() -> bool {
        todo!()
    }

    fn irq_all_set_enable(enable: bool) {
        todo!()
    }

    fn irq_is_enabled(irq: crate::irq::SoftIrqId) -> bool {
        todo!()
    }

    fn irq_set_enable(irq: crate::irq::SoftIrqId, enable: bool) {
        todo!()
    }
}
