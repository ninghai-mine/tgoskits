use core::arch::naked_asm;

use super::switch_to_elx;

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kernel_entry(_fdt_addr: usize) -> ! {
    naked_asm!(
        asm_sym_addr!(x8, "{fdt}"),
        "str  x0, [x8]",

        asm_sym_addr!(x8, "__cpu0_stack_top"),
        "mov sp, x8",

        "bl {switch_to_elx}",
        fdt = sym crate::fdt::FDT_ADDR,
        switch_to_elx = sym switch_to_elx,
    )
}

pub fn el_entry() -> ! {
    super::relocate::apply();
    crate::fdt::setup_earlycon();
    println!("Hello, Somehal on AArch64!");

    loop {}
}
