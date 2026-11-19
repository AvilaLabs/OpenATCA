//! A split crate: only declarations, re-exports and attributes.
#![forbid(unsafe_code)]

pub mod core;
pub mod shell;

pub use core::double;
pub use shell::load;

#[cfg(test)]
mod tests {
    fn t() { let _ = std::fs::read("x"); }
}
