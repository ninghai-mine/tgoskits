use loongArch64::register::{ecfg, eentry, estat, tlbrentry};

use crate::{arch::cache::local_flush_icache_range, mem::StaticCell};

const VECSIZE: usize = 0x200;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vector([u8; VECSIZE]);

// 等效于 C: long exception_handlers[VECSIZE * 128 / sizeof(long)] __aligned(SZ_64K);
// 在 64 位系统中，sizeof(long) = 8，所以数组大小为 VECSIZE * 128 / 8 = VECSIZE * 16
#[repr(C, align(65536))] // 65536 = 64KB 对齐
struct ExceptionHandlers([Vector; 128]);

impl ExceptionHandlers {
    const fn new() -> Self {
        Self([Vector([0; VECSIZE]); 128])
    }
}

static EXCEPTION_HANDLERS: StaticCell<ExceptionHandlers> =
    StaticCell::new(Some(ExceptionHandlers::new()));

fn eentry_addr() -> usize {
    EXCEPTION_HANDLERS.0.as_ptr() as usize
}

fn tlbrentry_addr() -> usize {
    eentry_addr() + 80 * VECSIZE
}

pub fn per_cpu_trap_init(is_primary: bool) {
    setup_vint_size();
    configure_exception_vector();

    if is_primary {
        for i in 0..64 {
            set_handler(i, handle_reserved);
        }
        for i in 64..=64 + 14 {
            set_handler(i, handle_int);
        }

        local_flush_icache_range(eentry_addr(), eentry_addr() + 0x400);
    }
}

fn setup_vint_size() {
    let n = 64 - (VECSIZE / 4).leading_zeros() - 1;
    ecfg::set_vs(n as _);
}

/// 配置异常向量
/// 等效于 C 的 configure_exception_vector()
fn configure_exception_vector() {
    eentry::set_eentry(eentry_addr());
    tlbrentry::set_tlbrentry(tlbrentry_addr());
}

fn set_handler(idx: usize, handler: fn()) {
    unsafe {
        let src = core::slice::from_raw_parts(handler as *const u8, VECSIZE);
        EXCEPTION_HANDLERS.update(|vec| {
            let dst = &mut vec.0[idx].0[..];
            dst.copy_from_slice(src);

            local_flush_icache_range(
                dst.as_ptr_range().start as usize,
                dst.as_ptr_range().end as usize,
            );
        });
    }
}

fn handle_reserved() {}

fn handle_int() {}

/// 处理向量中断
/// 等效于 C 的 do_vint()
fn do_vint() {
    // unsigned int estat = read_csr_estat() & CSR_ESTAT_IS;
    let mut estat = estat::read().is();

    // while ((hwirq = ffs(estat)))
    // ffs (find first set) 返回第一个被设置的位的位置（1-based）
    while estat != 0 {
        // 找到第一个设置的位（从低位开始，0-based）
        let hwirq = estat.trailing_zeros() + 1;

        // estat &= ~BIT(hwirq - 1);
        // 清除已处理的位
        estat &= !(1 << (hwirq - 1));

        handle_irq(hwirq - 1);
    }
}

fn handle_irq(hwirq: u32) {
    // 处理中断的具体实现
}
