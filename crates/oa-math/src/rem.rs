//! The Euclidean remainder of two doubles. Nothing here calls the platform maths library.

use crate::util::{abs, NAN};

/// The Euclidean remainder of `a` by `b`: the value in `[0, |b|)` that differs from `a` by a
/// multiple of `b`. Defined exactly as `f64::rem_euclid` is in the standard library
/// (`let r = a % b; if r < 0.0 { r + b.abs() } else { r }`), and bit for bit equal to it for
/// every input but a NaN result, which here has the one fixed bit pattern of the crate.
///
/// `%` on doubles is `fmod`: IEEE-754 requires the result to be the exact remainder, so every
/// machine returns the same value. The one rounding is the addition `r + |b|` for a negative `r`,
/// and that is correctly rounded. (For a tiny negative `a` the sum can round up to `|b|` itself,
/// as it does in the standard library.)
pub fn rem_euclid(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r.is_nan() {
        NAN
    } else if r < 0.0 {
        r + abs(b)
    } else {
        r
    }
}
