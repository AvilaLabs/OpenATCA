//! Every line below is a violation (line numbers are asserted).
use std::fs;
use std::{io, time::Instant};
pub fn a() {
    let _m: HashMap<u8, u8> = Default::default();
    println!("x");
    let _ = crate::shell::go();
    let _ = std
        ::env::args();
    let _ = fx_shell::read("x");
    let _ = fx_cleansplit::double(1);
    let _ = fx_cleansplit::core::double(1);
}
thread_local! { static X: u8 = 0; }
static mut N: u8 = 0;
// std::net in a comment is fine
pub const OK: &str = "std::net";
