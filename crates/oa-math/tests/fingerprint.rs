//! Determinism fingerprint: one hash over the output bits of every function on a fixed
//! pseudo-random input sequence. The pinned value was produced on x86-64 and must be the same
//! on every IEEE-754 target; a different value means a result changed on some machine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_math::{acos, asin, atan, atan2, cbrt, cos, exp, hypot, ln, pow, powi, sin, sin_cos, tan};

/// Total number of function evaluations hashed.
pub const INPUTS: usize = 1_000_000;

/// The pinned fingerprint of the first `INPUTS` evaluations.
pub const PINNED: u64 = 0x9be8_ff0f_022d_36d3;

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
    /// A double drawn from one of several distributions: raw bits (every exponent, NaN and
    /// infinity included), small uniform, and log-uniform magnitudes of both signs.
    fn value(&mut self) -> f64 {
        let mode = self.next() % 8;
        match mode {
            0 => f64::from_bits(self.next()),
            1 | 2 => -4.0 + 8.0 * self.unit(),
            3 => -1.0 + 2.0 * self.unit(),
            4 => (self.unit() - 0.5) * 1500.0,
            _ => {
                let e = (self.next() % 121) as i32 - 60;
                let m = 1.0 + self.unit();
                let v = m * f64::from_bits(((e + 1023) as u64) << 52);
                if self.next() & 1 == 1 {
                    -v
                } else {
                    v
                }
            }
        }
    }
}

struct Fnv(u64);

impl Fnv {
    fn word(&mut self, w: u64) {
        for k in 0..8 {
            self.0 ^= (w >> (8 * k)) & 0xff;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn f(&mut self, x: f64) {
        self.word(x.to_bits());
    }
}

/// FNV-1a over the output bits of `INPUTS` evaluations cycling through all functions.
pub fn fingerprint() -> u64 {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    for i in 0..INPUTS {
        let x = rng.value();
        match i % 14 {
            0 => h.f(sin(x)),
            1 => h.f(cos(x)),
            2 => h.f(tan(x)),
            3 => h.f(atan(x)),
            4 => h.f(asin(x)),
            5 => h.f(acos(x)),
            6 => h.f(exp(x)),
            7 => h.f(ln(x)),
            8 => h.f(cbrt(x)),
            9 => {
                let (s, c) = sin_cos(x);
                h.f(s);
                h.f(c);
            }
            10 => h.f(atan2(x, rng.value())),
            11 => h.f(hypot(x, rng.value())),
            12 => {
                let y = rng.value();
                // keep y * ln x in a range that is not just overflow half the time
                let y = if rng.next() & 1 == 1 { y * 0.05 } else { y };
                h.f(pow(x, y));
            }
            _ => {
                let n = (rng.next() % 81) as i32 - 40;
                h.f(powi(x, n));
            }
        }
    }
    h.0
}

// REQ: OA-MATH-018
#[test]
fn output_bits_match_the_pinned_fingerprint() {
    let got = fingerprint();
    println!("fingerprint = {got:#018x}");
    assert_eq!(got, PINNED, "fingerprint changed: {got:#018x}");
}
