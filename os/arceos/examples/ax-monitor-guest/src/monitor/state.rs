use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Running = 0,
    Panic = 1,
    Hang = 2,
    Dead = 3,
}

static VM_STATE: AtomicU8 = AtomicU8::new(VmState::Running as u8);

pub fn set_vm_state(state: VmState) {
    VM_STATE.store(state as u8, Ordering::SeqCst);
}

pub fn get_vm_state() -> VmState {
    match VM_STATE.load(Ordering::SeqCst) {
        0 => VmState::Running,
        1 => VmState::Panic,
        2 => VmState::Hang,
        3 => VmState::Dead,
        _ => VmState::Dead,
    }
}