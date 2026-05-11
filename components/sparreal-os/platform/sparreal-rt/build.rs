fn main() {
    let hv = std::env::var("CARGO_FEATURE_HV").is_ok();
    let uspace = std::env::var("CARGO_FEATURE_USPACE").is_ok();

    println!("cargo::rustc-check-cfg=cfg(uspace)");
    println!("cargo::rustc-check-cfg=cfg(hv)");

    if hv {
        println!("cargo:rustc-cfg=hv");
    } else if uspace {
        println!("cargo:rustc-cfg=uspace");
    }
}
