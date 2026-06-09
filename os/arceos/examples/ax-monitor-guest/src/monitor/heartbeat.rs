use core::sync::atomic::{AtomicU64, Ordering};

static HEARTBEAT: AtomicU64 = AtomicU64::new(0);

pub fn update_heartbeat() {
    HEARTBEAT.fetch_add(1, Ordering::SeqCst);
}

pub fn get_heartbeat() -> u64 {
    HEARTBEAT.load(Ordering::SeqCst)
}