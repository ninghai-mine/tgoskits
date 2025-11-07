#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;

#[macro_use]
mod console;

#[cfg(target_arch = "loongarch64")]
#[path = "arch/loongarch64/mod.rs"]
pub mod arch;

#[cfg(target_arch = "aarch64")]
#[path = "arch/aarch64/mod.rs"]
pub mod arch;

pub(crate) mod fdt;

mod acpi;
mod cmdline;
#[cfg(efi)]
mod efi_stub;
mod elf;
mod mem;

trait ArchTrait {
    fn post_allocator();
}

pub fn post_allocator() {
    // arch::Arch::post_allocator();
}
