use crate::capture::snapshot::capture_snapshot;
use crate::monitor::event::CrashEvent;
use crate::monitor::state::{set_vm_state, VmState};

pub fn detect_panic(panic_signature: &str) {
    ax_std::println!("[monitor] panic detected: {}", panic_signature);

    set_vm_state(VmState::Panic);

    trigger_crash_event(CrashEvent::Panic);
}

pub fn trigger_crash_event(event: CrashEvent) {
    ax_std::println!("[monitor] crash event: {:?}", event);

    capture_snapshot(event);
}