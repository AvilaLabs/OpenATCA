//! The one NaN. Nothing here calls the platform maths library.

use crate::util::NAN;

/// `x`, except that every NaN becomes the one fixed NaN (`0x7ff8_0000_0000_0000`, positive,
/// quiet, no payload); every other value, `-0.0` and the infinities included, is returned
/// unchanged.
///
/// IEEE-754 fixes the result of an invalid operation (`0.0 / 0.0`, `inf - inf`, the square root
/// of a negative number) to be a NaN, not which one: the sign and payload differ between a
/// compile-time constant fold, an x86-64 instruction (which makes the negative "default" NaN)
/// and an aarch64 one (positive). A float whose bits feed a hash, a digest or a written output
/// goes through this first, so those bits are the same on every machine (B6).
#[inline]
pub fn canon_nan(x: f64) -> f64 {
    if x.is_nan() {
        NAN
    } else {
        x
    }
}

/// [`canon_nan`] for an `f32`: every NaN becomes `0x7fc0_0000`, every other value is unchanged.
#[inline]
pub fn canon_nan_f32(x: f32) -> f32 {
    if x.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else {
        x
    }
}
