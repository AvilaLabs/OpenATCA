//! The split of `hypot` (the bit ordering, the double-double core and its root, each proved
//! alone) is the same arithmetic in the same order: the code as it was before the split, kept
//! here, and the code now agree bit for bit on a million deterministic inputs. Compiled for tests
//! only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::dd;
use crate::root::{hypot, hypot_fast, hypot_slow};
use crate::util::{abs, exponent, mul_pow2, pow2, scale_round, NAN};

fn old_hypot_fast(x: f64, y: f64) -> Option<f64> {
    use crate::root::{hypot_fast_plain_for_test, hypot_fast_scaled_for_test};
    let (bx, by) = (abs(x).to_bits(), abs(y).to_bits());
    let (hi_bits, lo_bits) = if by > bx { (by, bx) } else { (bx, by) };
    let (big, small) = (f64::from_bits(hi_bits), f64::from_bits(lo_bits));
    if (1.0e-114..1.0e150).contains(&big) {
        if small < big * 8.673_617_379_884_035e-19 {
            return None;
        }
        return hypot_fast_plain_for_test(big, small);
    }
    hypot_fast_scaled_for_test(x, y)
}

fn old_hypot_slow(x: f64, y: f64) -> f64 {
    if x.is_infinite() || y.is_infinite() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    let (ax, ay) = (abs(x), abs(y));
    let (big, small) = if ay > ax { (ay, ax) } else { (ax, ay) };
    if small == 0.0 {
        return big;
    }
    if small < big * 8.673_617_379_884_035e-19 {
        return big;
    }
    let e = exponent(big);
    let a = mul_pow2(big, -e);
    let b = mul_pow2(small, -e);
    let sum = dd::add(dd::two_prod(a, a), dd::two_prod(b, b));
    scale_round(dd::sqrt(sum), e)
}

fn old_hypot(x: f64, y: f64) -> f64 {
    match old_hypot_fast(x, y) {
        Some(r) => r,
        None => old_hypot_slow(x, y),
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Uniform in `0..n` (n below 2^32).
    fn below(&mut self, n: u64) -> u64 {
        ((self.next() >> 32) * n) >> 32
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
    /// A value with the edges `hypot` branches on: zero, subnormals, the range limits of the fast
    /// paths (1e-290, 1e-114, 1e150, 1e290), the stated domain edge 5e7, ratios of 2^-60, and
    /// random magnitudes and bit patterns (NaN and infinity included).
    fn val(&mut self) -> f64 {
        let e = self.below(2200) as i32 - 1100;
        let m = 1.0 + (self.next() >> 12) as f64 / (1u64 << 52) as f64;
        let v = match self.below(16) {
            0 => 0.0,
            1 => -0.0,
            2 => 5.0e7,
            3 => f64::from_bits(self.below(4096) + 1),
            4 => 1.0e-114,
            5 => 1.0e150,
            6 => 1.0e-290,
            7 => 1.0e290,
            8 => f64::from_bits(self.next()),
            9 => self.unit() * 5.0e7,
            _ => m * pow2(e),
        };
        if self.next() & 1 == 0 {
            -v
        } else {
            v
        }
    }
}

// REQ: OA-MATH-005, OA-MATH-038
#[test]
fn the_split_hypot_agrees_bit_for_bit_with_the_unsplit_code() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut fast_some, mut fast_none) = (0u32, 0u32);
    for i in 0..1_000_000u32 {
        let x = r.val();
        // Every fourth pair is a ratio near 2^-60 or 1 of the first.
        let y = match i & 3 {
            0 => x * 8.673_617_379_884_035e-19 * (1.0 + r.unit() * 1e-3),
            1 => x * (1.0 + r.unit() * 1e-9),
            _ => r.val(),
        };
        for (p, q) in [(x, y), (y, x)] {
            let (a, b) = (old_hypot_fast(p, q), hypot_fast(p, q));
            assert_eq!(a.map(f64::to_bits), b.map(f64::to_bits), "{p:e} {q:e}");
            if a.is_some() {
                fast_some += 1;
            } else {
                fast_none += 1;
            }
            assert_eq!(
                old_hypot_slow(p, q).to_bits(),
                hypot_slow(p, q).to_bits(),
                "{p:e} {q:e}"
            );
            assert_eq!(
                old_hypot(p, q).to_bits(),
                hypot(p, q).to_bits(),
                "{p:e} {q:e}"
            );
        }
    }
    assert!(
        fast_some > 100_000 && fast_none > 1_000,
        "{fast_some} {fast_none}"
    );
}
