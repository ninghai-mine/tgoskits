#![no_std]
#![no_main]
#![feature(used_with_arg)]

#[bare_test::tests]
mod tests {

    use core::hint::spin_loop;

    use bare_test::*;

    #[test]
    fn test2() {
        println!("test2 hello");
    }

    #[test]
    #[timeout = 100]
    fn test3() {
        println!("test3 hello");
    }
}
