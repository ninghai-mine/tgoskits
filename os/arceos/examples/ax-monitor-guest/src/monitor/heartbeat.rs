use core::sync::atomic::{AtomicU64, Ordering};

static HEARTBEAT: AtomicU64 = AtomicU64::new(0);

/// Target kernel `jiffies` symbol information.
/// VA from System.map: jiffies = jiffies_64 = 0xffff8000813d79c0
/// GPA formula: GVA - KIMAGE_VADDR + KERNEL_LOAD_PA
///   KIMAGE_VADDR = 0xffff_8000_8000_0000
///   KERNEL_LOAD_PA = 0x223_6000_000 (MEMORY_REGIONS[0].0)
const JIFFIES_VA: u64 = 0xffff_8000_813d_79c0;
const KIMAGE_VADDR: u64 = 0xffff_8000_8000_0000;
const KERNEL_LOAD_PA: u64 = 0x2_2360_0000;
const JIFFIES_GPA: u64 = JIFFIES_VA - KIMAGE_VADDR + KERNEL_LOAD_PA;

pub fn update_heartbeat() {
    HEARTBEAT.fetch_add(1, Ordering::SeqCst);
}

pub fn get_heartbeat() -> u64 {
    HEARTBEAT.load(Ordering::SeqCst)
}

/// Read `jiffies` (64-bit timer tick counter) from the target VM.
///
/// Returns `None` if the HVC #9 read fails (e.g., target VM is not yet
/// started or the GPA is invalid).
pub fn read_target_jiffies(target_vm_id: u64) -> Option<u64> {
    let mut buf = [0u8; 8];
    crate::capture::register::read_guest_mem(target_vm_id, JIFFIES_GPA, &mut buf).ok()?;
    Some(u64::from_le_bytes(buf))
}