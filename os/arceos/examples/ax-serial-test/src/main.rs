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

/// Direct UART write (crude, no FIFO check)
unsafe fn uart_write(s: &str) {
    let uart = 0x900_0000 as *mut u32;
    for &b in s.as_bytes() {
        unsafe {
            core::ptr::write_volatile(uart, b as u32);
        }
    }
}

#[unsafe(no_mangle)]
fn main() {
    // Test 1: HVC call - axvisor should log this
    let hvc_result = unsafe { hvc_test() };
    
    // Test 2: Direct UART write (no busy wait)
    unsafe { uart_write("!\n"); }
    
    // Test 3: ax_std println
    println!("Hello from ArceOS VM guest! HVC result={}", hvc_result);

    loop {}
}
