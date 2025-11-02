use acpi::{AcpiTables, Handler};
use core::ffi::c_void;

/// RSDP存储
#[unsafe(link_section = ".data")]
static mut RSDP: usize = 0;

/// 设置RSDP地址
pub(crate) fn set_rsdp(addr: *const c_void) {
    unsafe {
        RSDP = addr as usize;
    }
}

/// 获取RSDP地址
fn rsdp() -> *const c_void {
    unsafe { RSDP as _ }
}

pub fn tables<T: Handler>(h: T) -> Result<AcpiTables<T>, acpi::AcpiError> {
    unsafe { ::acpi::AcpiTables::from_rsdp(h, rsdp() as usize) }
}
