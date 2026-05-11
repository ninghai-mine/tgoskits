#![no_main]
#![cfg_attr(not(any(windows, unix)), no_std)]
#![cfg(not(any(windows, unix)))]

use core::{sync::atomic::AtomicBool, time::Duration};

use log::info;
use sparreal_rt::os::time::one_shot_after;

extern crate alloc;
#[macro_use]
extern crate sparreal_rt;

#[sparreal_rt::entry]
fn main() {
    info!("Hello, world!");

    static TEST_IRQ: AtomicBool = AtomicBool::new(false);

    one_shot_after(Duration::from_millis(200), || {
        TEST_IRQ.store(true, core::sync::atomic::Ordering::SeqCst);
    })
    .unwrap();

    // 等待中断触发
    println!("Waiting for timer interrupt...");
    loop {
        if TEST_IRQ.load(core::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }

    println!("All tests passed!");
}
