use ax_std::thread;
use core::time::Duration;

use crate::monitor::event::CrashEvent;
use crate::monitor::heartbeat::get_heartbeat;
use crate::monitor::panic::trigger_crash_event;
use crate::monitor::state::{set_vm_state, VmState};

const WATCHDOG_TIMEOUT_SECS: u64 = 5;

pub fn start_watchdog() {
    thread::spawn(move || {
        let mut last_heartbeat = get_heartbeat();

        loop {
            thread::sleep(Duration::from_secs(WATCHDOG_TIMEOUT_SECS));

            let current = get_heartbeat();

            if current == last_heartbeat {
                ax_std::println!("[watchdog] Guest hang detected");

                set_vm_state(VmState::Hang);

                trigger_crash_event(CrashEvent::WatchdogTimeout);

                break;
            }

            last_heartbeat = current;
        }
    });
}