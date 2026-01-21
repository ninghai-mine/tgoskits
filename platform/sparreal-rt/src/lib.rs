#![no_std]
#![no_main]
#![cfg(not(any(windows, unix)))]

extern crate alloc;
extern crate somehal;

use somehal::KernelOp;
pub use sparreal_kernel::entry;
pub use sparreal_kernel::*;

mod hal_impl;

#[somehal::entry(Kernel)]
fn main() -> ! {
    sparreal_kernel::run_kernel()
}

pub struct Kernel;

impl KernelOp for Kernel {
    fn ioremap(&self, paddr: usize, size: usize) -> somehal::PagingResult<*mut u8> {
        sparreal_kernel::os::mem::ioremap(paddr.into(), size).map(|addr| addr.raw() as *mut u8)
    }
}
