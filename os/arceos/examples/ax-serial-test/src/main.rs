#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use ax_std::println;

/// Select which crash type to trigger.
/// Change this constant and rebuild to test different crashes.
///   0 = null pointer dereference (Data Abort — Translation fault)
///   1 = undefined instruction  (Undefined Instruction)
///   2 = alignment fault        (Data Abort — Alignment fault)
///   3 = execute from zero      (Instruction Abort)
///   4 = no crash (normal exit, watchdog will catch hang)
const CRASH_TYPE: u64 = 3;

#[cfg(target_arch = "aarch64")]
unsafe fn trigger_null() -> ! {
    println!("[crash] type=0: null pointer dereference");
    unsafe { core::ptr::write_volatile(0x0 as *mut u64, 0xDEAD); }
    loop { core::hint::spin_loop(); }
}

#[cfg(target_arch = "aarch64")]
unsafe fn trigger_udef() -> ! {
    println!("[crash] type=1: undefined instruction");
    unsafe { core::arch::asm!("udf #0", options(noreturn)); }
}

#[cfg(target_arch = "aarch64")]
unsafe fn trigger_unaligned() -> ! {
    println!("[crash] type=2: alignment fault");
    // Create a misaligned pointer (0x8020_1001) and do an LDR that requires alignment
    let addr = 0x8020_1001usize as *mut u64;
    unsafe { core::ptr::write_volatile(addr, 0xDEAD); }
    loop { core::hint::spin_loop(); }
}

#[cfg(target_arch = "aarch64")]
unsafe fn trigger_instr_abort() -> ! {
    println!("[crash] type=3: instruction abort (branch to 0x0)");
    unsafe {
        core::arch::asm!(
            "mov x0, #0",
            "br x0",
            options(noreturn),
        );
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn trigger_none() -> ! {
    println!("[crash] type=4: no crash (watchdog will timeout)");
    loop { core::hint::spin_loop(); }
}

#[cfg(not(target_arch = "aarch64"))]
unsafe fn trigger_null() -> ! { loop {} }
#[cfg(not(target_arch = "aarch64"))]
unsafe fn trigger_udef() -> ! { loop {} }
#[cfg(not(target_arch = "aarch64"))]
unsafe fn trigger_unaligned() -> ! { loop {} }
#[cfg(not(target_arch = "aarch64"))]
unsafe fn trigger_instr_abort() -> ! { loop {} }
#[cfg(not(target_arch = "aarch64"))]
unsafe fn trigger_none() -> ! { loop {} }

#[unsafe(no_mangle)]
fn main() {
    println!("Hello from ArceOS VM guest! CRASH_TYPE={}", CRASH_TYPE);
    println!("Waiting 2s before crash...");

    // Give the monitor time to start polling
    ax_std::thread::sleep(core::time::Duration::from_secs(2));

    match CRASH_TYPE {
        0 => unsafe { trigger_null() },
        1 => unsafe { trigger_udef() },
        2 => unsafe { trigger_unaligned() },
        3 => unsafe { trigger_instr_abort() },
        _ => unsafe { trigger_none() },
    }
}
