//! `rem_euclid` against the standard library's definition.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_math::rem_euclid;

/// xorshift64* (Vigna); the tests' own generator so no crate is needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / 9_007_199_254_740_992.0
    }
}

fn same(a: f64, b: f64) -> bool {
    if b.is_nan() {
        a.to_bits() == 0x7ff8_0000_0000_0000
    } else {
        a.to_bits() == b.to_bits()
    }
}

// REQ: OA-MATH-033
#[test]
fn equals_the_standard_library_on_two_million_inputs() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for i in 0..2_000_000u32 {
        let (a, b) = match i % 4 {
            // bearings and wraps, the shape the tracker uses
            0 => ((rng.unit() - 0.5) * 1440.0, 360.0),
            // any sign, moderate size
            1 => ((rng.unit() - 0.5) * 2e3, (rng.unit() - 0.5) * 50.0),
            // any bit pattern
            2 => (f64::from_bits(rng.next()), f64::from_bits(rng.next())),
            // wide range of exponents, either sign
            _ => (
                (rng.unit() - 0.5) * f64::powi(2.0, (rng.next() % 200) as i32 - 100),
                (rng.unit() - 0.5) * f64::powi(2.0, (rng.next() % 200) as i32 - 100),
            ),
        };
        assert!(
            same(rem_euclid(a, b), f64::rem_euclid(a, b)),
            "a = {a:e} b = {b:e}"
        );
    }
}

// REQ: OA-MATH-033
#[test]
fn edge_values_equal_the_standard_library() {
    let v = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        360.0,
        -360.0,
        1e-300,
        -1e-300,
        5e-324,
        -5e-324,
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        -f64::NAN,
    ];
    for &a in &v {
        for &b in &v {
            assert!(same(rem_euclid(a, b), f64::rem_euclid(a, b)), "{a:e} {b:e}");
        }
    }
    assert_eq!(rem_euclid(-1.0, 360.0), 359.0);
    assert_eq!(rem_euclid(370.0, 360.0), 10.0);
    assert_eq!(rem_euclid(-370.0, -360.0), 350.0);
}
