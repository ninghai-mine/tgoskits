#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod capture;
mod monitor;
mod recovery;

use ax_std::println;
use capture::register;

const TARGET_VM_ID: u64 = 1;

#[unsafe(no_mangle)]
fn main() {
    println!("[monitor-guest] ArceOS Monitor Guest Start");

    monitor::watchdog::start_watchdog();

    // Poll target VM status until it crashes (Stopped).
    // The target will crash by writing to NULL, which triggers a Data Abort
    // at EL1. The hypervisor detects this and sets VM status to Stopped.
    println!("[monitor-guest] waiting for target VM[{}] to crash...", TARGET_VM_ID);
    loop {
        if register::poll_crash_status(TARGET_VM_ID) {
            println!("[monitor-guest] target VM[{}] has crashed!", TARGET_VM_ID);
            break;
        }
        monitor::heartbeat::update_heartbeat();
        ax_std::thread::sleep(core::time::Duration::from_millis(200));
    }

    // Target has crashed, capture snapshot
    monitor::panic::detect_panic("kernel panic: null pointer dereference");

    loop {
        ax_std::thread::sleep(core::time::Duration::from_secs(1));
    }
}