//! Shared HVC hypercall helper for the monitor guest.
//!
//! Provides a single `hvc_call()` used by all capture modules
//! (`register`, `memory`, `export`) to issue HVC #0 hypercalls to AxVisor.
//!
//! # Convention (AArch64)
//!
//! - `x0` = hypercall code
//! - `x1` – `x5` = arguments
//! - `x0` (return) = result (negative = error)

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn hvc_call(code: u64, x1: u64, x2: u64, x3: u64, x4: u64, x5: u64) -> u64 {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") code => result,
            in("x1") x1,
            in("x2") x2,
            in("x3") x3,
            in("x4") x4,
            in("x5") x5,
            options(nostack),
        );
    }
    result
}

#[cfg(not(target_arch = "aarch64"))]
pub fn hvc_call(_code: u64, _x1: u64, _x2: u64, _x3: u64, _x4: u64, _x5: u64) -> u64 {
    u64::MAX
}
