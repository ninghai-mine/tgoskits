use core::arch::naked_asm;

// use super::entry::kernel_entry;
use crate::{
    arch::addrspace::{VMLINUX_LOAD_ADDRESS, to_phys},
    efi_stub::{efi_pe_entry, pe::*},
};

/// LoongArch64 kernel header implementing functionality similar to
/// Linux arch/loongarch/kernel/head.S _head section
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".head.text")]
pub unsafe extern "C" fn _head() -> ! {
    naked_asm!(
        // EFI header following Linux kernel format
        ".word {dos_signature}",        // "MZ", MS-DOS header
        ".org 0x8",
        ".dword _kernel_entry",         // Kernel entry point (physical address)
        ".dword _kernel_asize",         // Kernel image effective size
        ".quad {phys_link_kaddr}",      // PHYS_LINK_KADDR - Kernel image load offset
        ".org 0x38",                    // 0x20 ~ 0x37 reserved
        ".long {linux_pe_magic}",
        ".long 4f - _head",             // Offset to the PE header

        "4:",                           // pe_header
        // PE header
        ".long {image_nt_signature}",   // IMAGE_NT_SIGNATURE

        // COFF header
        ".short {file_machine}",        // IMAGE_FILE_MACHINE_LOONGARCH64
        ".short 2",                     // NumberOfSections
        ".long 0",                      // TimeDateStamp
        ".long 0",                      // PointerToSymbolTable
        ".long 0",                      // NumberOfSymbols
        ".short 2f - 1f",               // SizeOfOptionalHeader
        ".short {flags}",               // Characteristics (IMAGE_FILE_DEBUG_STRIPPED | IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_LINE_NUMS_STRIPPED)

        // Optional header
        "1:",
        ".short 0x020b",                // IMAGE_NT_OPTIONAL_HDR64_MAGIC
        ".byte 0x02",                   // MajorLinkerVersion
        ".byte 0x14",                   // MinorLinkerVersion
        ".long _etext - _stext",        // SizeOfCode
        ".long _kernel_rsize",          // SizeOfInitializedData
        ".long _kernel_vsize - _kernel_rsize", // SizeOfUninitializedData
        ".long efi_pe_entry - _head",   // AddressOfEntryPoint
        ".long _stext - _head",         // BaseOfCode

        // Extra header fields
        ".quad 0",                      // ImageBase
        ".long PAGE_SIZE",              // SectionAlignment (PECOFF_SEGMENT_ALIGN)
        ".long PECOFF_FILE_ALIGN",      // FileAlignment (PECOFF_FILE_ALIGN)
        ".short 0",                     // MajorOperatingSystemVersion
        ".short 0",                     // MinorOperatingSystemVersion
        ".short {major_image_version}",                     // MajorImageVersion
        ".short {minor_image_version}",                     // MinorImageVersion
        ".short 0",                     // MajorSubsystemVersion
        ".short 0",                     // MinorSubsystemVersion
        ".long 0",                      // Win32VersionValue

        ".long _end - _head",           // SizeOfImage

        ".long _stext - _head",         // SizeOfHeaders
        ".long 0",                      // CheckSum
        ".short {image_subsystem}",                    // IMAGE_SUBSYSTEM_EFI_APPLICATION
        ".short 0",                     // DllCharacteristics
        ".quad 0",                      // SizeOfStackReserve
        ".quad 0",                      // SizeOfStackCommit
        ".quad 0",                      // SizeOfHeapReserve
        ".quad 0",                      // SizeOfHeapCommit
        ".long 0",                      // LoaderFlags
        ".long (2f - .) / 8",           // NumberOfRvaAndSizes

        // Data directories
        ".quad 0",                      // ExportTable
        ".quad 0",                      // ImportTable
        ".quad 0",                      // ResourceTable
        ".quad 0",                      // ExceptionTable
        ".quad 0",                      // CertificationTable
        ".quad 0",                      // BaseRelocationTable

        "2:",
        // Section table
        ".ascii \".text\\0\\0\\0\"",
        ".long _etext - _stext",            // VirtualSize
        ".long _stext - _head",             // VirtualAddress
        ".long _etext - _stext",            // SizeOfRawData
        ".long _stext - _head",             // PointerToRawData

        ".long 0",                      // PointerToRelocations
        ".long 0",                      // PointerToLineNumbers
        ".short 0",                     // NumberOfRelocations
        ".short 0",                     // NumberOfLineNumbers
        ".long 0x60000020",             // Characteristics (IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_EXECUTE)

        // .data section
        ".ascii \".data\\0\\0\\0\"",
        ".long _kernel_vsize",          // VirtualSize
        ".long _etext - _head",         // VirtualAddress
        ".long _kernel_rsize",          // SizeOfRawData
        ".long _etext - _head",         // PointerToRawData

        ".long  0",                      // PointerToRelocations
        ".long  0",                      // PointerToLineNumbers
        ".short 0",                     // NumberOfRelocations
        ".short 0",                     // NumberOfLineNumbers
        ".long 0xc0000040",             // Characteristics (IMAGE_SCN_CNT_INITIALIZED_DATA | IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE)

        dos_signature = const IMAGE_DOS_SIGNATURE,
        linux_pe_magic = const LINUX_PE_MAGIC,
        phys_link_kaddr = const to_phys(VMLINUX_LOAD_ADDRESS),
        image_nt_signature = const IMAGE_NT_SIGNATURE,
        file_machine = const IMAGE_FILE_MACHINE_LOONGARCH64,
        flags = const IMAGE_FILE_DEBUG_STRIPPED | IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_LINE_NUMS_STRIPPED,
        major_image_version = const LINUX_EFISTUB_MAJOR_VERSION,
        minor_image_version = const LINUX_EFISTUB_MINOR_VERSION,
        image_subsystem = const IMAGE_SUBSYSTEM_EFI_APPLICATION,
    )
}

/// Kernel entry point called from EFI or directly
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kernel_entry() -> ! {
    naked_asm!(
        // Config direct window and set PG (from Linux head.S)
        "li.d        $t0, 0x8000000090000011",  // DMW0: 0x8000000000000000-0x8000ffffffffffff
        "csrwr       $t0, 0x180",               // LOONGARCH_CSR_DMWIN0
        "li.d        $t0, 0x9000000090000011",  // DMW1: 0x9000000000000000-0x9000ffffffffffff
        "csrwr       $t0, 0x181",               // LOONGARCH_CSR_DMWIN1

        // Perform early relocation if needed
        "bl          {early_relocate}",

        // Enable PG (paging) directly
        "li.w        $t0, 0xb0",               // PLV=0, IE=0, PG=1
        "csrwr       $t0, 0x0",                // LOONGARCH_CSR_CRMD
        "li.w        $t0, 0x04",               // PLV=0, PIE=1, PWE=0
        "csrwr       $t0, 0x1",                // LOONGARCH_CSR_PRMD
        "li.w        $t0, 0x00",               // FPE=0, SXE=0, ASXE=0, BTE=0
        "csrwr       $t0, 0x2",                // LOONGARCH_CSR_EUEN

        // Clear BSS
        "la.pcrel    $t0, __bss_start",
        "la.pcrel    $t1, __bss_stop",
        "beq         $t0, $t1, 3f",             // Skip if no BSS
        "2:",
        "st.d        $zero, $t0, 0",
        "addi.d      $t0, $t0, 8",
        "bne         $t0, $t1, 2b",

        "3:",
        // Save firmware arguments (from static variables set by EFI entry)
        "la.pcrel    $t0, {fw_arg0}",
        "ld.d        $a0, $t0, 0",              // Load efi_boot flag
        "la.pcrel    $t0, {fw_arg1}",
        "ld.d        $a1, $t0, 0",              // Load cmdline
        "la.pcrel    $t0, {fw_arg2}",
        "ld.d        $a2, $t0, 0",              // Load systable

        // Set up kernel stack (using init_thread_union)
        "la.pcrel    $sp, {init_stack}",
        "li.w        $t0, 0x4000",
        "add.d       $sp, $sp, $t0",         // Point to end of stack (stack grows down)

        // Jump to Rust kernel start
        "b           {rust_start}",

        early_relocate = sym super::relocate::efi_relocate,
        fw_arg0 = sym FW_ARG0,
        fw_arg1 = sym FW_ARG1,
        fw_arg2 = sym FW_ARG2,
        init_stack = sym INIT_THREAD_UNION,
        rust_start = sym rust_start_kernel,
    )
}

// Static storage for firmware arguments
#[unsafe(link_section = ".data")]
static mut FW_ARG0: usize = 0;

#[unsafe(link_section = ".data")]
static mut FW_ARG1: usize = 0;

#[unsafe(link_section = ".data")]
static mut FW_ARG2: usize = 0;

// Initial thread union for stack
#[unsafe(link_section = ".bss")]
static mut INIT_THREAD_UNION: [u8; 0x4000] = [0; 0x4000];

/// Rust kernel start function that will be called from assembly
#[unsafe(no_mangle)]
fn rust_start_kernel() -> ! {
    // Early debug - try to output something to indicate we reached here
    // This uses memory-mapped UART typically at 0x1fe001e0 for LoongArch
    unsafe {
        let uart_base = 0x1fe001e0 as *mut u8;
        let msg = b"KERNEL_START\n";
        for &byte in msg {
            while (uart_base.add(5).read_volatile() & 0x20) == 0 {}
            uart_base.write_volatile(byte);
        }
    }

    // Create a basic BootInfo structure
    // let mut boot_info = BootInfo::new();

    // Get EFI parameters from global variables
    let _efi_boot = unsafe { FW_ARG0 };
    let _cmdline_ptr = unsafe { FW_ARG1 };
    let _systable_ptr = unsafe { FW_ARG2 };

    // Should never reach here
    loop {
        unsafe {
            core::arch::asm!("nop");
        }
    }
}
