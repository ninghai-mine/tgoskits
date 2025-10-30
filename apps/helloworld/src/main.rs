#![no_std]
#![no_main]

use sparreal_rt::somehal;
extern crate alloc;
extern crate sparreal_rt;

fn main() {
    unsafe { somehal::arch::efi_relocate() };
}
