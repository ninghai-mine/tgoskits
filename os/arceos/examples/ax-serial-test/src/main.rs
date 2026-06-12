#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use ax_std::println;

/// Make an HVC #0 call with a test code
/// x0 = hypercall number (0xCAFE = test), x1 = test value (0xDEAD)
#[cfg(target_arch = "aarch64")]
unsafe fn hvc_test() -> u64 {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") 0xCAFEu64 => result,
            in("x1") 0xDEADu64,
            options(nostack),
        );
    }
    result
}

#[cfg(not(target_arch = "aarch64"))]
unsafe fn hvc_test() -> u64 {
    u64::MAX
}

#[unsafe(no_mangle)]
fn main() {
    // HVC test: axvisor should log this
    let _hvc_result = unsafe { hvc_test() };
    
    // Print a message (uses earlycon with proper fixmap)
    println!("Hello from ArceOS VM guest!");

    // Trigger a real crash: null pointer dereference (Data Abort at EL1)
    println!("About to crash: writing to NULL...");
    unsafe {
        core::ptr::write_volatile(0x0 as *mut u64, 0xDEAD);
    }

    // Should never reach here
    #[allow(unreachable_code)]
    loop {
        core::hint::spin_loop();
    }
}
