use core::ptr::NonNull;

use some_serial::ns16550::Ns16550;
use some_serial::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kernel_entry() -> ! {
    unimplemented!()
}

// cosst 

pub(crate) fn efi_kernel_prepare() {
    println!("Preparing kernel entry...");

    let addr = 0x1FE001E0usize;

    let mut uart = Ns16550::new_mmio(NonNull::new(addr as _).unwrap(), 0, 1);
    let mut tx = uart.take_tx().unwrap();

    tx.write_byte(b'A');
    // tx.write_byte(b'\r');
//     tx.write_byte(b'\n');

    // let str = "Hello, UART Early Console!\r\n";

    // let bytes = str.as_bytes();

    // let mut buff = bytes;
    // while !buff.is_empty() {
    //     let n = tx.write_bytes(buff);
    //     buff = &buff[n..];
    // }

    unsafe {
        let ptr = addr as *mut u8;
        core::ptr::write_volatile(ptr, b'A');
        core::ptr::write_volatile(ptr, b'\r');
        core::ptr::write_volatile(ptr, b'\n');

        let ptr =( addr + 5 ) as *mut u8;
        ptr.read_volatile();


    }
}
