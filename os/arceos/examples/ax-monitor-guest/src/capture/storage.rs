//! Crash dump persistent storage (ArceOS: ax_std::fs + serde-json-core).

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::capture::snapshot::CrashSnapshot;
use serde::{Deserialize, Serialize};

const VMCORE_VERSION: &str = "1.0";
const VMCORE_DIR: &str = "/vmcore";

#[derive(Debug, Serialize, Deserialize)]
pub struct VmcoreFile {
    pub vmcore_version: String,
    pub timestamp: String,
    pub target_vm_id: u64,
    pub crash_event: String,
    pub vcpu_count: usize,
    pub registers: Vec<VcpuRegsEntry>,
    pub memory_dump_offset: Option<u64>,
    pub kernel_log: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VcpuRegsEntry {
    pub vcpu_id: u64,
    pub gpr: [u64; 31],
    pub sp_el0: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
    pub crash_type: u8,
}

pub fn save_vmcore(snapshot: &CrashSnapshot) -> Result<String, String> {
    // Ignore error if directory already exists
    let _ = ax_std::fs::create_dir(VMCORE_DIR);
    let timestamp = boot_timestamp();
    let event_name = format!("{:?}", snapshot.event);
    let file_name = format!("vmcore_{}_{}.json", timestamp, event_name);
    let file_path = format!("{}/{}", VMCORE_DIR, file_name);

    let vmcore = VmcoreFile {
        vmcore_version: VMCORE_VERSION.to_string(), timestamp, target_vm_id: 1,
        crash_event: event_name, vcpu_count: snapshot.vcpu_regs.len(),
        registers: snapshot.vcpu_regs.iter().map(|(id, r)| VcpuRegsEntry {
            vcpu_id: *id, gpr: r.gpr, sp_el0: r.sp_el0, elr_el1: r.elr_el1, spsr_el1: r.spsr_el1,
            esr_el1: r.esr_el1, far_el1: r.far_el1, crash_type: r.crash_type,
        }).collect(),
        memory_dump_offset: None, kernel_log: None,
    };

    let json = serde_json_core::to_string::<_, 4096>(&vmcore).map_err(|e| format!("serialization: {}", e))?;
    ax_std::fs::write(&file_path, json.as_bytes()).map_err(|e| format!("write: {}", e))?;
    ax_std::println!("[storage] vmcore saved: {} ({} bytes)", file_path, json.len());
    Ok(file_path)
}

pub fn load_vmcore(file_path: &str) -> Option<VmcoreFile> {
    let content = ax_std::fs::read_to_string(file_path).ok()?;
    serde_json_core::from_str::<VmcoreFile>(&content).ok().map(|(v, _)| v)
}

fn boot_timestamp() -> String {
    use core::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{:08}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_roundtrip() {
        let vmcore = VmcoreFile { vmcore_version: "1.0".into(), timestamp: "t".into(), target_vm_id: 1,
            crash_event: "Panic".into(), vcpu_count: 1,
            registers: vec![VcpuRegsEntry { vcpu_id: 0, gpr: [0x42; 31], sp_el0: 0x1000,
                elr_el1: 0xffff000008000000, spsr_el1: 0x3c5,
                esr_el1: 0x96000044, far_el1: 0, crash_type: 1 }],
            memory_dump_offset: None, kernel_log: None };
        let json = serde_json_core::to_string::<_, 4096>(&vmcore).unwrap();
        let (parsed, _): (VmcoreFile, _) = serde_json_core::from_str(&json).unwrap();
        assert_eq!(parsed.registers[0].elr_el1, 0xffff000008000000);
    }
}
