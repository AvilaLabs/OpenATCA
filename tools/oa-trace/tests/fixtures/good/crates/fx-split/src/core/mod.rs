//! Core of the split crate. May name core crates and the `core` module of a split crate.
pub fn double(x: i64) -> i64 {
    fx_core::add(&x, x)
}
