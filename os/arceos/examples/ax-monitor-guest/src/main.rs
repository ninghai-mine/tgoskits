#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod capture;
mod monitor;
mod recovery;

use ax_std::println;

#[unsafe(no_mangle)]
fn main() {
    println!("[monitor-guest] ArceOS Monitor Guest Start");

    monitor::watchdog::start_watchdog();

    for i in 0..3 {
        println!("[monitor-guest] heartbeat {}", i);
        monitor::heartbeat::update_heartbeat();
        ax_std::thread::sleep(core::time::Duration::from_secs(1));
    }

    monitor::panic::detect_panic("kernel panic: null pointer dereference");

    loop {
        ax_std::thread::sleep(core::time::Duration::from_secs(1));
    }
}