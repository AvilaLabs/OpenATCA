//! A split crate: only declarations, re-exports and attributes.
#![forbid(unsafe_code)]

pub mod core;
pub mod shell;

#[cfg(not(target_pointer_width = "64"))]
compile_error!("64-bit only");

pub use self::core::double;
pub use self::shell::load;

#[cfg(test)]
mod tests {
    fn t() { let _ = std::fs::read("x"); }
}
