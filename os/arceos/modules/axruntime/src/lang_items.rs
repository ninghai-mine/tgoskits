// Copyright 2025 The Axvisor Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    match axpanic::enter_panic(current_cpu_id()) {
        axpanic::PanicDisposition::Primary => panic_primary(info),
        // Once panic ownership is established, recursive and cross-CPU panic
        // entries must avoid the full print/backtrace path and terminate the
        // system instead of halting one CPU and risking test timeouts.
        axpanic::PanicDisposition::Recursive | axpanic::PanicDisposition::Concurrent => {
            panic_shutdown()
        }
    }
}

fn panic_primary(info: &PanicInfo) -> ! {
    let _oops_guard = axpanic::enter_oops();
    panic_message(info);
    panic_backtrace();
    panic_shutdown()
}

fn panic_message(info: &PanicInfo) {
    ax_println!("{}", info);
}

fn panic_backtrace() {
    if should_print_panic_backtrace() {
        ax_println!("{}", axbacktrace::Backtrace::capture().kind("panic"));
    }
}

fn should_print_panic_backtrace() -> bool {
    axpanic::should_emit_panic_backtrace()
}

fn panic_shutdown() -> ! {
    // Read the hardware ESR_EL1/FAR_EL1 set by the original exception.
    // These persist from when the fault (e.g., Data Abort) was taken
    // at EL1, through the trap handler, until here — unless a nested
    // exception overwrote them.
    let esr: u64;
    let far: u64;
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mrs {0}, esr_el1", out(reg) esr);
        core::arch::asm!("mrs {0}, far_el1", out(reg) far);
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        esr = 0;
        far = 0;
    }

    // Notify the hypervisor (if present) via GuestPanic HVC #13.
    // Pass ESR/FAR as arguments so the hypervisor can capture them
    // even if `mrs esr_el1` at EL2 returns a different value.
    //
    // When no hypervisor is present, HVC at EL1 triggers an exception
    // (recursive panic → system_off).  Either way we reach shutdown.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("hvc #0",
            in("x0") 13u64,
            in("x1") esr,
            in("x2") far,
            options(noreturn));
    }

    ax_hal::power::system_off()
}

fn current_cpu_id() -> usize {
    #[cfg(feature = "smp")]
    {
        ax_hal::percpu::this_cpu_id()
    }

    #[cfg(not(feature = "smp"))]
    {
        0
    }
}
