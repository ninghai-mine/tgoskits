#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

mod capture;
mod monitor;
mod recovery;

use ax_std::println;
use capture::register;
use monitor::state::{set_vm_state, VmState, get_vm_state};
use monitor::event::CrashEvent;
use monitor::panic::trigger_crash_event;

const TARGET_VM_ID: u64 = 1;

// Hang detection constants
/// How many 200ms ticks between jiffies reads (5 seconds).
const JIFFIES_POLL_INTERVAL: u64 = 25;
/// How many 200ms ticks of no jiffies change → declare hang (30 seconds).
const HANG_TIMEOUT_TICKS: u64 = 150;

#[unsafe(no_mangle)]
fn main() {
    println!("[monitor-guest] ArceOS Monitor Guest Start");

    monitor::watchdog::start_watchdog();

    // Poll target VM status until it crashes (Stopped) or we detect a hang.
    println!("[monitor-guest] waiting for target VM[{}] to crash...", TARGET_VM_ID);

    let mut prev_jiffies: Option<u64> = None;
    let mut hang_ticks: u64 = 0;
    let mut poll_ticks: u64 = 0;

    loop {
        if register::poll_crash_status(TARGET_VM_ID) {
            println!("[monitor-guest] target VM[{}] has crashed!", TARGET_VM_ID);
            break;
        }

        // Early exit if another thread already detected a hang
        if get_vm_state() == VmState::Hang {
            println!("[monitor-guest] hang already detected by watchdog, capturing...");
            break;
        }

        monitor::heartbeat::update_heartbeat();

        // Periodically read target VM's jiffies to detect hangs
        poll_ticks += 1;
        if poll_ticks >= JIFFIES_POLL_INTERVAL {
            poll_ticks = 0;
            match monitor::heartbeat::read_target_jiffies(TARGET_VM_ID) {
                Some(j) => {
                    if let Some(prev) = prev_jiffies {
                        if j == prev {
                            hang_ticks += JIFFIES_POLL_INTERVAL;
                            if hang_ticks >= HANG_TIMEOUT_TICKS {
                                println!("[monitor-guest] TARGET VM HANG DETECTED: jiffies unchanged for {} ticks", hang_ticks);
                                set_vm_state(VmState::Hang);
                                trigger_crash_event(CrashEvent::WatchdogTimeout);
                                break;
                            }
                        } else {
                            hang_ticks = 0;  // reset — target is alive
                        }
                    }
                    prev_jiffies = Some(j);
                }
                None => {
                    // read_guest_mem can fail if target VM is not fully booted
                    // or if HVC #9 encounters an error.  Reset state on failure
                    // to avoid false positives during boot.
                    if prev_jiffies.is_some() {
                        println!("[monitor-guest] warning: failed to read target jiffies");
                    }
                    prev_jiffies = None;
                    hang_ticks = 0;
                }
            }
        }

        ax_std::thread::sleep(core::time::Duration::from_millis(200));
    }

    // If we detected a hang, the capture already happened in trigger_crash_event.
    // If poll_crash_status returned true, fall through to normal capture.
    if get_vm_state() != VmState::Hang {
        // Target has crashed, capture snapshot
        monitor::panic::detect_panic("kernel panic: null pointer dereference");

        loop {
            ax_std::thread::sleep(core::time::Duration::from_secs(1));
        }
    }
}