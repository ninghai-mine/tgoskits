#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use ax_std::println;

/// Write a character to the PL011 UART at MMIO 0x9000000
unsafe fn uart_putc(c: u8) {
    let uart_base = 0x900_0000 as *mut u8;
    // PL011: wait until TX FIFO not full (flag register bit 5)
    let fr = uart_base.add(0x18) as *const u32;
    while (*fr) & (1 << 5) != 0 {}
    // Write data to DR register
    core::ptr::write_volatile(uart_base as *mut u32, c as u32);
}

unsafe fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        uart_putc(b);
    }
}

#[unsafe(no_mangle)]
fn main() {
    // Try direct UART write first
    unsafe { uart_puts("UART: Hello from ArceOS VM guest!\n"); }

    // Then try ax_std println
    println!("STDOUT: Hello from ArceOS VM guest!");

    // Panic to see if panic message appears
    panic!("TEST PANIC - if you see this, panic works!");
}
