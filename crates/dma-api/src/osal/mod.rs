cfg_if::cfg_if! {
    if #[cfg(target_arch = "aarch64")] {
        #[path = "aarch64.rs"]
        pub mod arch;
    } else{
        #[path = "nop.rs"]
        pub mod arch;
    }
}
