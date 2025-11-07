use core::fmt::Write;

pub fn _print(args: core::fmt::Arguments) {
    let _ = ConFmt {}.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\r\n"));
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!("{}{}", core::format_args!($($arg)*), "\r\n")));
}

struct ConFmt {}

impl Write for ConFmt {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        con().write_str(s);
        Ok(())
    }
}

fn con() -> &'static dyn Con {
    unsafe { CON }
}

pub(crate) trait Con: Send + Sync {
    fn write_str(&self, s: &str);
}

struct NoCon;
impl Con for NoCon {
    fn write_str(&self, _s: &str) {
        // Do nothing
    }
}

static mut CON: &dyn Con = &NoCon;

pub(crate) unsafe fn set_out(v: &'static dyn Con) {
    unsafe {
        CON = v;
    }
}

pub fn setup_earlycon() {
    let _ = earlycon_form_cmdline();
}

fn earlycon_form_cmdline() -> Option<()> {
    let cmdline = crate::cmdline::cmdline()?;
    let val = crate::cmdline::var("earlycon")?;

    


    None
}
