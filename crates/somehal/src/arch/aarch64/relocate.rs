use core::sync::atomic::AtomicBool;

use crate::arch::head::_head;

// 从链接器脚本导入符号
unsafe extern "C" {
    fn __rela_dyn_begin();
    fn __rela_dyn_end();
}

// AArch64 重定位类型常量
const R_AARCH64_RELATIVE: u32 = 1027;
/// 计算加载偏移量 (实际地址 - 链接地址)
fn get_load_offset() -> i64 {
    sym_addr!(_head) as i64
}

/// 应用 .rela.dyn 重定位
pub fn apply() {
    #[unsafe(link_section = ".data")]
    static INIT: AtomicBool = AtomicBool::new(false);

    if INIT.swap(true, core::sync::atomic::Ordering::Relaxed) {
        return;
    }

    unsafe {
        crate::elf::apply_reloc(
            get_load_offset(),
            sym_addr!(__rela_dyn_begin) as _,
            sym_addr!(__rela_dyn_end) as _,
            R_AARCH64_RELATIVE,
        );
    }
}
