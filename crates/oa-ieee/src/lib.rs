#![forbid(unsafe_code)]
//! oa-ieee: the exactly specified IEEE-754 operations that `core` lacks on stable Rust 1.89.
//!
//! `f64::sqrt`, `floor`, `ceil`, `trunc` and `round` exist only in `std` on stable 1.89, and
//! `core::arch` intrinsics cannot be called without `unsafe`. The crates oa-math and oa-geo are
//! `no_std` so that the compiler itself refuses file, network, clock, thread and hash-map use in
//! them; they reach these five operations through this crate instead.
//!
//! This is the only `std` code under the core crates, and it is deliberately tiny. Every function
//! here is correctly rounded or exact by IEEE-754 (square root is required to be correctly
//! rounded; the four rounding functions are exact), so every conforming machine returns the
//! same bits for every result that is not a NaN, and RULESET B6 holds for those. A NaN result
//! (the square root of a negative number) is required to be a NaN but not which one: its sign
//! and payload differ between a compile-time fold, x86-64 and aarch64, and the functions here
//! pass whatever the machine made. Code that hashes or writes the bits of a float passes it
//! through `oa_math::canon_nan` first. Nothing else belongs here: no `mul_add` (the maths crate
//! avoids fused multiply-add on purpose) and nothing transcendental. A test pins the list of
//! public functions.
//!
//! Public API (pinned by `tests/api.rs`):
//!
//! * [`sqrt`]
//! * [`floor`]
//! * [`ceil`]
//! * [`trunc`]
//! * [`round`]

// The audited fixed-width wrapper is portable to wasm32-unknown-unknown (RULESET B10).
#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("core supports native 64-bit targets and wasm32-unknown-unknown only");

/// Square root, correctly rounded. `sqrt(-0.0) = -0.0`; negative arguments give a NaN whose sign
/// and payload depend on the machine (see the crate docs).
#[inline]
pub fn sqrt(v: f64) -> f64 {
    v.sqrt()
}

/// Largest integer not above `v`, exact.
#[inline]
pub fn floor(v: f64) -> f64 {
    v.floor()
}

/// Smallest integer not below `v`, exact.
#[inline]
pub fn ceil(v: f64) -> f64 {
    v.ceil()
}

/// `v` with the fractional part dropped (toward zero), exact.
#[inline]
pub fn trunc(v: f64) -> f64 {
    v.trunc()
}

/// Nearest integer, ties away from zero (as `f64::round`), exact.
#[inline]
pub fn round(v: f64) -> f64 {
    v.round()
}
