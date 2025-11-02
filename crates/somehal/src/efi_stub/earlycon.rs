use core::{cell::UnsafeCell, ptr::NonNull};

use acpi::{AcpiError, Handler, PhysicalMapping, sdt::spcr::Spcr};
use some_serial::ns16550::Ns16550;

use crate::console::Con;

use super::acpi_handle::AcpiHandle;

pub(crate) fn setup_earlycon() -> Result<(), AcpiError> {
    let tb = crate::acpi::tables(AcpiHandle)?;

    for spsr in tb.find_tables::<Spcr>() {
        println!("Found {:?}", spsr.interface_type());
        println!("  Base address: {:#x?}", spsr.base_address());

        if deal_with_spsr(&spsr).is_some() {
            println!("Early console setup complete.");
            break;
        }
    }

    Ok(())
}

fn deal_with_spsr(spsr: &PhysicalMapping<impl Handler, Spcr>) -> Option<()> {
    println!("Found {:?}", spsr.interface_type());
    let base_address = match spsr.base_address()? {
        Ok(addr) => addr,
        Err(e) => {
            println!("Failed to get base address: {:?}", e);
            return None;
        }
    };
    println!("  Base address: {:#x?}", base_address.address);
    println!("  Baud rate: {:?}", spsr.baud_rate());
    println!("  Clock frequency: {:?}", spsr.uart_clock_frequency());

    let mut clock = 0;
    if let Some(freq) = spsr.uart_clock_frequency() {
        clock = freq.into();
    }

    match spsr.interface_type() {
        acpi::sdt::spcr::SpcrInterfaceType::Full16550
        | acpi::sdt::spcr::SpcrInterfaceType::Generic16550 => {
            let mut uart = Ns16550::new_mmio(
                NonNull::new(base_address.address as _).unwrap(),
                clock,
                base_address.access_size as _,
            );
            let tx = uart.take_tx().unwrap();
            set_sender(tx);
        }
        t => {
            println!("Unsupported SPCR interface type `{t:?}` for early console.");
            return None;
        }
    }

    unsafe { crate::console::set_out(&SENDER) };

    Some(())
}

fn set_sender(sender: some_serial::Sender) {
    unsafe {
        *SENDER.0.get() = Some(sender);
    }
}

#[unsafe(link_section = ".data")]
static SENDER: SenderCell = SenderCell(UnsafeCell::new(None));

struct SenderCell(UnsafeCell<Option<some_serial::Sender>>);

unsafe impl Sync for SenderCell {}

impl Con for SenderCell {
    fn write_str(&self, s: &str) {
        unsafe {
            if let Some(ref mut sender) = *self.0.get() {
                let bytes = s.as_bytes();
                let mut buff = bytes;
                while !buff.is_empty() {
                    let n = sender.write_bytes(buff);
                    buff = &buff[n..];
                }
            }
        }
    }
}
