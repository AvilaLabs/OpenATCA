//! Cross-checks that need no reference table: identities over many pseudo-random inputs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_math::{acos, asin, atan, atan2, cbrt, cos, exp, hypot, ln, pow, powi, sin, sin_cos, tan};
use std::f64::consts::PI;

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
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / 9_007_199_254_740_992.0
    }
    fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
    /// Magnitude 2^e * [1, 2) with e uniform in [lo, hi], random sign.
    fn log_signed(&mut self, lo: i32, hi: i32) -> f64 {
        let span = (hi - lo + 1) as u64;
        let e = lo + (self.next() % span) as i32;
        let m = 1.0 + self.unit();
        let v = m * f64::from_bits(((e + 1023) as u64) << 52);
        if self.next() & 1 == 1 {
            -v
        } else {
            v
        }
    }
}

const N: usize = 200_000;

// REQ: OA-MATH-012
#[test]
fn sin_squared_plus_cos_squared_is_one() {
    let mut r = Rng(0x1234_5678_9abc_def1);
    for i in 0..N {
        let x = match i % 4 {
            0 => r.uniform(-10.0, 10.0),
            1 => r.log_signed(-30, 10),
            2 => r.log_signed(10, 1023),
            _ => r.uniform(-1e6, 1e6),
        };
        let (s, c) = sin_cos(x);
        let d = (s * s + c * c - 1.0).abs();
        assert!(d < 4.5e-16, "x = {x:e}: sin^2 + cos^2 - 1 = {d:e}");
        assert!(s.abs() <= 1.0 && c.abs() <= 1.0, "x = {x:e}");
    }
}

// REQ: OA-MATH-013
#[test]
fn sin_cos_pair_equals_separate_calls_bit_for_bit() {
    let mut r = Rng(0xdead_beef_0bad_f00d);
    for i in 0..N {
        let x = match i % 3 {
            0 => r.log_signed(-1074, 1023),
            1 => r.uniform(-100.0, 100.0),
            _ => f64::from_bits(r.next()),
        };
        let (s, c) = sin_cos(x);
        assert_eq!(s.to_bits(), sin(x).to_bits(), "sin at {x:e}");
        assert_eq!(c.to_bits(), cos(x).to_bits(), "cos at {x:e}");
    }
}

// REQ: OA-MATH-014
#[test]
fn atan2_quadrants_and_symmetries() {
    let mut r = Rng(0x0f0f_1234_5678_9abc);
    for i in 0..N {
        let (y, x) = if i % 2 == 0 {
            (r.log_signed(-1074, 1023), r.log_signed(-1074, 1023))
        } else {
            (r.log_signed(-10, 10), r.log_signed(-10, 10))
        };
        let a = atan2(y, x);
        // the result lies in the half-plane of y, and in the right half of it for the sign of x
        assert!(
            a.is_sign_negative() == y.is_sign_negative(),
            "sign at ({y:e}, {x:e})"
        );
        if x > 0.0 {
            assert!(a.abs() < PI / 2.0 + 1e-15, "({y:e}, {x:e}) -> {a:e}");
        } else {
            assert!(a.abs() > PI / 2.0 - 1e-15, "({y:e}, {x:e}) -> {a:e}");
        }
        // odd in y, bit for bit
        assert_eq!(
            atan2(-y, x).to_bits(),
            (-a).to_bits(),
            "oddness at ({y:e}, {x:e})"
        );
        // atan2(y, -x) = pi - atan2(y, x) for y > 0
        if y > 0.0 {
            let b = atan2(y, -x);
            assert!(
                (a + b - PI).abs() < 5e-16,
                "reflection at ({y:e}, {x:e}): {a:e} {b:e}"
            );
        }
        // against atan(y/x) where the quotient is a sane number
        if x > 0.0 && (y / x).abs() > 1e-200 && (y / x).abs() < 1e200 {
            let q = atan(y / x);
            let tol = 2.5e-16 * q.abs().max(1e-300);
            assert!(
                (a - q).abs() <= tol + 2.0 * f64::EPSILON * (y / x).abs() * q.abs().min(1.0),
                "({y:e}, {x:e})"
            );
        }
    }
}

// REQ: OA-MATH-015
#[test]
fn exp_ln_round_trip_and_inverse_functions() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for i in 0..N {
        let x = match i % 3 {
            0 => r.log_signed(-1000, 1000).abs(),
            1 => r.uniform(0.01, 100.0),
            _ => 1.0 + r.uniform(-1e-3, 1e-3),
        };
        let l = ln(x);
        let back = exp(l);
        let tol = l.abs() * 1.2e-16 + 3.3e-16;
        assert!((back / x - 1.0).abs() <= tol, "exp(ln({x:e})) = {back:e}");
    }
    for _ in 0..N {
        let t = r.uniform(-1.4, 1.4);
        let back = asin(sin(t));
        assert!(
            (back - t).abs() < 4e-16 * t.abs().max(1.0),
            "asin(sin({t:e})) = {back:e}"
        );
        let u = r.uniform(0.3, 2.8);
        let back = acos(cos(u));
        assert!((back - u).abs() < 1e-15, "acos(cos({u:e})) = {back:e}");
        let w = r.uniform(-1.5, 1.5);
        let back = atan(tan(w));
        assert!(
            (back - w).abs() < 6e-16 * w.abs().max(1.0),
            "atan(tan({w:e})) = {back:e}"
        );
        let v = r.log_signed(-300, 300);
        let c = cbrt(v);
        assert!(((c * c * c) / v - 1.0).abs() < 5e-16, "cbrt({v:e}) = {c:e}");
        let (p, q) = (r.log_signed(-300, 300), r.log_signed(-300, 300));
        let h = hypot(p, q);
        assert!(h >= p.abs() && h >= q.abs(), "hypot({p:e}, {q:e}) = {h:e}");
        let want = (p / h) * (p / h) + (q / h) * (q / h);
        assert!((want - 1.0).abs() < 1e-15, "hypot({p:e}, {q:e}) = {h:e}");
        let x = r.uniform(0.1, 50.0);
        let y = r.uniform(-8.0, 8.0);
        let z = pow(x, y) * pow(x, -y);
        assert!(
            (z - 1.0).abs() < 5e-16,
            "pow({x}, {y}) * pow({x}, -{y}) = {z:e}"
        );
    }
}

/// Distance in units in the last place between two finite doubles of the same sign.
fn ulps_apart(a: f64, b: f64) -> u64 {
    a.to_bits().abs_diff(b.to_bits())
}

// REQ: OA-MATH-028
#[test]
fn powi_agrees_with_pow_to_within_two_ulps() {
    let mut r = Rng(0x5555_aaaa_1357_9bdf);
    let mut compared = 0;
    for i in 0..N {
        let x = match i % 3 {
            0 => r.uniform(0.5, 2.0),
            1 => r.log_signed(-20, 20),
            _ => r.uniform(-3.0, 3.0),
        };
        let n = match i % 4 {
            0 => (r.next() % 41) as i32 - 20,
            1 => (r.next() % 2001) as i32 - 1000,
            2 => (r.next() % 20_001) as i32 - 10_000,
            _ => (r.next() % 9) as i32 - 4,
        };
        let (got, want) = (powi(x, n), pow(x, f64::from(n)));
        if want.is_nan() || got.is_nan() {
            assert!(got.is_nan() == want.is_nan(), "powi({x:e}, {n}) = {got:e}");
            continue;
        }
        // pow is faithful (under 1 ulp) and powi within a fraction of an ulp of the true value
        assert!(
            got.to_bits() >> 63 == want.to_bits() >> 63 && ulps_apart(got, want) <= 2,
            "powi({x:e}, {n}) = {got:e}, pow {want:e}"
        );
        compared += 1;
    }
    assert!(compared > N / 2);
}

// REQ: OA-MATH-016
#[test]
fn powi_is_exact_where_the_exact_result_is_representable() {
    // every power of two over the whole range, subnormals included, from either side
    for k in -1074..=1023i32 {
        let want = f64::from_bits(if k >= -1022 {
            ((k + 1023) as u64) << 52
        } else {
            1u64 << (k + 1074)
        });
        assert_eq!(powi(2.0, k).to_bits(), want.to_bits(), "2^{k}");
        assert_eq!(powi(0.5, -k).to_bits(), want.to_bits(), "0.5^{}", -k);
        assert_eq!(
            powi(-2.0, k).to_bits(),
            (want.to_bits() | (u64::from(k & 1 == 1) << 63)),
            "(-2)^{k}"
        );
    }
    // small cases whose exact result needs no rounding
    assert_eq!(powi(3.0, 20), 3_486_784_401.0);
    assert_eq!(powi(10.0, 15), 1e15);
    assert_eq!(powi(0.5, -10), 1024.0);
    assert_eq!(powi(-3.0, 5), -243.0);
    assert_eq!(powi(7.0, 1), 7.0);
    // pow agrees with the exactly representable integer powers
    for b in 2..=12 {
        for e in 0..=20 {
            let want = (b as f64).powi(0) * powi(b as f64, e);
            if want < 9.007e15 {
                assert_eq!(pow(b as f64, e as f64), want, "pow({b}, {e})");
            }
        }
    }
}

// REQ: OA-MATH-017
#[test]
fn tan_is_sin_over_cos_and_pow_matches_simple_roots() {
    let mut r = Rng(0xfeed_face_cafe_beef);
    for i in 0..N {
        let x = match i % 3 {
            0 => r.uniform(-3.0, 3.0),
            1 => r.log_signed(-20, 10),
            _ => r.uniform(-1e5, 1e5),
        };
        let (s, c) = sin_cos(x);
        let t = tan(x);
        assert!((t - s / c).abs() <= 5e-16 * t.abs(), "tan at {x:e}");
        let y = r.log_signed(-100, 100).abs();
        let sq = pow(y, 0.5);
        assert!(
            (sq / y.sqrt() - 1.0).abs() < 2.3e-16,
            "pow({y:e}, 0.5) = {sq:e}"
        );
        let sqr = pow(y, 2.0);
        assert!(
            (sqr / (y * y) - 1.0).abs() < 3.4e-16,
            "pow({y:e}, 2) = {sqr:e}"
        );
    }
}
