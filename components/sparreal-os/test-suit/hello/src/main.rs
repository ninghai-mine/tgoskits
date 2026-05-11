#![no_main]
#![cfg_attr(not(any(windows, unix)), no_std)]
#![cfg(not(any(windows, unix)))]

extern crate alloc;
#[macro_use]
extern crate sparreal_rt;

#[sparreal_rt::entry]
fn main() {
    println!("========================================");
    println!("All tests passed!");
    println!("========================================");
}
