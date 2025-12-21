use core::{hint::spin_loop, panic::PanicInfo};

#[allow(dead_code)]
#[cfg_attr(not(any(windows, unix)), panic_handler)]
fn panic(info: &PanicInfo) -> ! {
    error!("Panicked: {info}");
    loop {
        // Infinite loop to halt the system
        spin_loop();
    }
}
