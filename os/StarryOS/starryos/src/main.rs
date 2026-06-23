#![no_std]
#![no_main]
#![doc = include_str!("../../README.md")]

extern crate alloc;

use alloc::{borrow::ToOwned, vec::Vec};

pub const CMDLINE: &[&str] = &["/bin/sh", "-c", include_str!("init.sh")];

#[unsafe(no_mangle)]
fn main() {
    // Intentional crash for testing crash monitor:
    // Issue HVC #13 (GuestPanic) to notify the hypervisor.
    // The hypervisor locks crash registers from the HVC TrapFrame
    // and sets VM status to Stopped, so PollCrashStatus detects it.
    unsafe {
        core::arch::asm!("hvc #0", in("x0") 13u64);
    }

    let args = CMDLINE
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let envs = [];

    starry_kernel::entry::init(&args, &envs);
}
