use std::{fs, io::Write, path::PathBuf};

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rustc-link-search={}", out_dir.display());

    println!("cargo::rustc-check-cfg=cfg(efi)");

    if std::env::var("CARGO_FEATURE_EFI").is_ok() {
        println!("cargo:rustc-cfg=efi");
    }

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let uspace = std::env::var("CARGO_FEATURE_USPACE").is_ok();

    let mut build = Build {
        arch: Arch::from(arch.as_str()),
        out_dir,
        kernel_load_vaddr: 0x200000,
        uspace,
    };

    build.prepare();
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Arch {
    #[default]
    Loongarch64,
}

impl From<&str> for Arch {
    fn from(s: &str) -> Self {
        match s {
            "loongarch64" => Arch::Loongarch64,
            _ => panic!("Unsupported architecture: {}", s),
        }
    }
}

#[derive(Default)]
struct Build {
    arch: Arch,
    out_dir: PathBuf,
    kernel_load_vaddr: u64,
    uspace: bool,
}

impl Build {
    const LD_NAME: &'static str = "somehal.x";

    fn prepare(&mut self) {
        match self.arch {
            Arch::Loongarch64 => self.prepare_loongarch64(),
        }
    }

    fn prepare_loongarch64(&mut self) {
        let ld_src = "src/arch/loongarch64/link.ld";

        // if self.uspace {
        self.kernel_load_vaddr = 0x9000000000200000;
        // }

        let kernel_load_vaddr = self.kernel_load_vaddr as usize;

        let ld = include_str!("src/arch/loongarch64/link.ld")
            .replace("${kernel_load_vaddr}", &format!("{kernel_load_vaddr:#x}"));

        println!("cargo:rerun-if-changed={ld_src}");
        println!("cargo:rustc-cfg=efi");

        let ld_dst = self.out_dir.join(Self::LD_NAME);

        fs::write(ld_dst, ld).unwrap();

        let defines = quote::quote! {
            pub const VMLINUX_LOAD_ADDRESS: usize = #kernel_load_vaddr;
        };
        let syntax_tree = syn::parse2(defines).unwrap();
        let formatted = prettyplease::unparse(&syntax_tree);
        let mut out_file = fs::File::create(self.out_dir.join("defines.rs")).unwrap();
        out_file.write_all(formatted.as_bytes()).unwrap();
    }
}
