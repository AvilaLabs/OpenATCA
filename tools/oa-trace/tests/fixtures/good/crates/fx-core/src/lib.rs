//! Pure. Mentions std::fs, HashMap, println! and crate::shell only in comments and strings.
/* block comment: std::io::stdin() and dbg!(x), /* nested HashSet */ still comment */

#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("64-bit targets only");

pub const TEXT: &str = "std::fs::read and HashMap and println!(x) and crate::shell::go";
pub const RAW: &str = r#"thread_local! static mut std::time"#;
pub const CH: char = '"';
pub const LIFETIME_OK: &str = "quote ' and \" inside";

pub fn add<'a>(a: &'a i64, b: i64) -> i64 {
    a + b
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::collections::HashMap;

    #[test]
    fn t() {
        println!("{:?}", HashMap::<u8, u8>::new());
        let _ = fs::read("x");
    }
}

#[cfg(test)]
mod ext_tests;
