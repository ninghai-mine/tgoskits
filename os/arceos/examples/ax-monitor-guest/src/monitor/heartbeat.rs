use core::sync::atomic::{AtomicU64, Ordering};

static HEARTBEAT: AtomicU64 = AtomicU64::new(0);

/// Hardcoded jiffies offset within the kernel image (VA - KIMAGE_VADDR).
/// From System.map: jiffies = 0xffff8000813d79c0, KIMAGE_VADDR = 0xffff800080000000.
const JIFFIES_OFFSET: u64 = 0x13d79c0;
const FALLBACK_HPA_BASE: u64 = 0x2_2360_0000;

pub fn update_heartbeat() {
    HEARTBEAT.fetch_add(1, Ordering::SeqCst);
}

pub fn get_heartbeat() -> u64 {
    HEARTBEAT.load(Ordering::SeqCst)
}

/// Read `jiffies` (64-bit timer tick counter) from the target VM.
///
/// Uses dynamically-located HPA_BASE so the GPA is correct even after
/// axvisor restarts.  Returns `None` if HPA_BASE hasn't been located yet
/// or the HVC #9 read fails — in that case hang detection is suppressed
/// (no false positives).
pub fn read_target_jiffies(target_vm_id: u64) -> Option<u64> {
    let hpa_base = crate::capture::locate::get_hpa_base()
        .unwrap_or(FALLBACK_HPA_BASE);
    let gpa = hpa_base + JIFFIES_OFFSET;
    let mut buf = [0u8; 8];
    crate::capture::register::read_guest_mem(target_vm_id, gpa, &mut buf).ok()?;
    Some(u64::from_le_bytes(buf))
}