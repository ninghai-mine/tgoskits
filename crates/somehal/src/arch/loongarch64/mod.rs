#[macro_use]
mod _macros;

mod addrspace;
mod cache;
mod context;
pub(crate) mod entry;
mod head;
pub mod paging;
mod register;
mod relocate;
mod trap;

use loongArch64::{
    register::{crmd, tcfg, ticlr},
    time::{Time, get_timer_freq},
};
use page_table_generic::{FrameAllocator, PageTable};
pub use paging::Entry as Pte;
pub use relocate::relocate;

use crate::{ArchTrait, arch::register::irq::TI, irq::SoftIrqId};

const MIN_TICKS: usize = 4;

pub type PT<A> = page_table_generic::PageTable<paging::Generic, A>;

pub struct Arch;

impl ArchTrait for Arch {
    type PT = paging::Generic;

    fn kernel_code() -> &'static [u8] {
        let start = ext_sym_addr!(_head);
        let end = ext_sym_addr!(__kernel_code_end);
        unsafe { core::slice::from_raw_parts(start as *const u8, end - start) }
    }

    fn post_allocator() {}

    fn _pa(vaddr: *const u8) -> usize {
        addrspace::to_phys(vaddr as usize)
    }

    fn _va(paddr: usize) -> *mut u8 {
        addrspace::to_cache(paddr) as *mut u8
    }

    fn ioremap(paddr: usize, _size: usize) -> *mut u8 {
        Self::_io(paddr)
    }

    fn _io(paddr: usize) -> *mut u8 {
        addrspace::to_uncache(paddr) as *mut u8
    }

    fn per_cpu_trap_init(is_primary: bool) {
        trap::per_cpu_trap_init(is_primary);
    }

    fn systimer_irq() -> usize {
        TI as _
    }

    fn systimer_enable() {
        tcfg::set_en(true);
    }

    fn systimer_disable() {
        tcfg::set_en(false);
    }

    fn systimer_set_interval(ticks: usize) {
        let ticks = ticks.max(MIN_TICKS);
        // Ensure the value is aligned to a multiple of 4 as required by TCFG
        let ticks = (ticks + 3) & !3;
        let ticks = ticks.min(usize::MAX);

        // 先禁用定时器
        tcfg::set_en(false);
        // 设置单次模式
        tcfg::set_periodic(false);
        // 设置初始值
        tcfg::set_init_val(ticks);
        // 清除可能存在的中断
        ticlr::clear_timer_interrupt();
        // 不在这里 enable，让调用者通过 systimer_enable() 来使能
    }

    fn systimer_ack() {
        ticlr::clear_timer_interrupt();
    }

    fn systimer_freq() -> usize {
        get_timer_freq()
    }

    fn systimer_tick() -> usize {
        Time::read()
    }

    fn shutdown() -> ! {
        loop {
            unsafe { loongArch64::asm::idle() };
        }
    }

    fn irq_all_is_enabled() -> bool {
        crmd::read().ie()
    }

    fn irq_all_set_enable(enable: bool) {
        crmd::set_ie(enable);
    }

    fn irq_is_enabled(irq: SoftIrqId) -> bool {
        use loongArch64::register::ecfg::{self, LineBasedInterrupt};

        match irq.kind() {
            trap::IrqKind::Private(hwirq) => {
                // 对于 CPU 本地中断，检查 ECFG.LIE 对应位
                // ECFG.LIE 位 0-12 对应中断 0-12 (SWI0-1, HWI0-7, PCOV, TI, IPI)
                let lie = ecfg::read().lie();
                let mask = LineBasedInterrupt::from_bits_retain(1 << hwirq);
                lie.contains(mask)
            }
            trap::IrqKind::External(_hwirq) => {
                // 外部中断需要通过级联中断控制器来检查
                // 目前暂不支持，返回 false
                false
            }
        }
    }

    fn irq_set_enable(irq: SoftIrqId, enable: bool) {
        use loongArch64::register::ecfg::{self, LineBasedInterrupt};

        match irq.kind() {
            trap::IrqKind::Private(hwirq) => {
                // 对于 CPU 本地中断，设置 ECFG.LIE 对应位
                // 参考 Linux: set_csr_ecfg(ECFGF(d->hwirq)) / clear_csr_ecfg(ECFGF(d->hwirq))
                let current_lie = ecfg::read().lie();
                let mask = LineBasedInterrupt::from_bits_retain(1 << hwirq);
                let new_lie = if enable {
                    current_lie | mask
                } else {
                    current_lie - mask
                };
                ecfg::set_lie(new_lie);
            }
            trap::IrqKind::External(_hwirq) => {
                // 外部中断需要通过级联中断控制器来设置
                // 目前暂不支持
            }
        }
    }

    fn set_kernel_page_table<A: FrameAllocator>(pt: PageTable<Self::PT, A>) {
        // 获取页表的根物理地址
        let root_paddr = pt.root_paddr().raw();

        // 设置 PGDH (高地址空间页表基地址，用于内核空间)
        paging::write_csr_pgdh(root_paddr as u64);

        // 刷新 TLB
        paging::local_flush_tlb_all();

        // 将页表存储起来以便后续获取
        // 注意：这里我们使用 forget 来防止页表被释放
        core::mem::forget(pt);
    }

    fn get_kernel_page_table<A: FrameAllocator>() -> PageTable<Self::PT, A> {
        // 从 PGDH 寄存器读取当前内核页表的根地址
        let root_paddr = paging::read_csr_pgdh() as usize;

        // 创建 PageTableRef，然后包装成 PageTable
        // 注意：这里假设 A 实现了 Default trait
        let allocator: A = unsafe { core::mem::zeroed() };
        let pt_ref = page_table_generic::PageTableRef::<Self::PT, A>::from_paddr(
            root_paddr.into(),
            allocator,
        );

        // 手动构造 PageTable
        // SAFETY: PageTable 只是 PageTableRef 的包装，内存布局兼容
        unsafe {
            let pt = core::mem::ManuallyDrop::new(pt_ref);
            core::ptr::read(&pt as *const _ as *const PageTable<Self::PT, A>)
        }
    }

    fn create_page_table<A: FrameAllocator>(allocator: A) -> PageTable<Self::PT, A> {
        PageTable::<Self::PT, A>::new(allocator).unwrap()
    }

    fn kernel_page_table_paddr_asid() -> (usize, usize) {
        let pt_paddr = paging::read_csr_pgdh() as usize;
        let asid = (paging::read_csr_asid() & 0x3FF) as usize; // ASID 为低 10 位
        (pt_paddr, asid)
    }

    fn set_kernel_page_table_paddr_asid(paddr: usize, asid: usize) {
        // 设置 PGDH (高地址空间页表基地址，用于内核空间)
        paging::write_csr_pgdh(paddr as u64);

        // 设置 ASID
        paging::write_csr_asid(asid as u64);

        // 刷新 TLB
        paging::local_flush_tlb_all();
    }
}

// 导出公开的页表相关函数供外部使用
pub use paging::{
    local_flush_tlb_all, read_csr_asid, read_csr_pgdh, write_csr_asid, write_csr_pgdh,
};
