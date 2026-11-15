//! Tests of the fast paths against the double-double paths (the test hooks `*_slow` and
//! `*_fast` of each module). Compiled for tests only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::atan::{acos_fast, acos_slow, asin_fast, asin_slow, atan2_fast, atan2_slow};
use crate::atan::{atan_fast, atan_slow};
use crate::exp::{exp_fast, exp_slow};
use crate::log::{ln_fast, ln_slow};
use crate::pow::{pow_fast, pow_slow};
use crate::root::{cbrt_fast, cbrt_slow, hypot_fast, hypot_slow};
use crate::trig::{cos_fast, cos_slow, sin_cos_fast, sin_cos_slow, sin_fast, sin_slow};
use crate::trig::{tan_fast, tan_slow};

/// Number of random inputs of the bit-for-bit comparison per function.
const N_EQUAL: usize = 1_000_000;
/// Number of random inputs of the acceptance-rate measurement per function.
const N_RATE: usize = 300_000;

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
    fn uni(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    /// A magnitude 2^e (1 + unit) with e uniform in `lo..=hi`, of either sign when `signed`.
    fn logu(&mut self, lo: i32, hi: i32, signed: bool) -> f64 {
        let e = lo + (self.next() & 0xffff) as i32 * (hi - lo + 1) / 65536;
        let v = (1.0 + self.unit()) * f64::from_bits(((e + 1023) as u64) << 52);
        if signed && self.next() & 1 == 1 {
            -v
        } else {
            v
        }
    }
    fn raw(&mut self) -> f64 {
        f64::from_bits(self.next())
    }
    /// The distribution of the fingerprint test: raw bits, small uniform, log-uniform.
    fn wide(&mut self) -> f64 {
        match self.next() & 7 {
            0 => self.raw(),
            1 | 2 => self.uni(-4.0, 4.0),
            3 => self.uni(-1.0, 1.0),
            4 => self.uni(-750.0, 750.0),
            _ => self.logu(-60, 60, true),
        }
    }
}

/// One function under test: the public function, the fast path (None when it declines) and the
/// double-double path, all returning result bits (`[low word, 0]` for one result), and the
/// generators of typical inputs and wide inputs.
struct Fun {
    name: &'static str,
    arity: usize,
    public: fn(f64, f64) -> [u64; 2],
    fast: fn(f64, f64) -> Option<[u64; 2]>,
    slow: fn(f64, f64) -> [u64; 2],
    typical: fn(&mut Rng) -> (f64, f64),
    wide: fn(&mut Rng) -> (f64, f64),
    reference: &'static [u8],
}

fn one(v: f64) -> [u64; 2] {
    [v.to_bits(), 0]
}

fn one_opt(v: Option<f64>) -> Option<[u64; 2]> {
    v.map(one)
}

fn wide1(r: &mut Rng) -> (f64, f64) {
    (r.wide(), 0.0)
}

/// Arguments of the trigonometric functions: the wide distribution, and half the time
/// magnitudes up to the Cody-Waite limit 2^19 (the most multiples of pi/2 to subtract).
fn trig_wide(r: &mut Rng) -> (f64, f64) {
    if r.next() & 1 == 0 {
        (r.logu(-26, 19, true), 0.0)
    } else {
        (r.wide(), 0.0)
    }
}

fn wide2(r: &mut Rng) -> (f64, f64) {
    (r.wide(), r.wide())
}

const SIN: Fun = Fun {
    name: "sin",
    arity: 1,
    public: |x, _| one(crate::sin(x)),
    fast: |x, _| one_opt(sin_fast(x)),
    slow: |x, _| one(sin_slow(x)),
    typical: |r| (r.uni(-100.0, 100.0), 0.0),
    wide: trig_wide,
    reference: include_bytes!("../tests/data/sin.bin"),
};
const COS: Fun = Fun {
    name: "cos",
    arity: 1,
    public: |x, _| one(crate::cos(x)),
    fast: |x, _| one_opt(cos_fast(x)),
    slow: |x, _| one(cos_slow(x)),
    typical: |r| (r.uni(-100.0, 100.0), 0.0),
    wide: trig_wide,
    reference: include_bytes!("../tests/data/cos.bin"),
};
const SIN_COS: Fun = Fun {
    name: "sin_cos",
    arity: 1,
    public: |x, _| {
        let (s, c) = crate::sin_cos(x);
        [s.to_bits(), c.to_bits()]
    },
    fast: |x, _| sin_cos_fast(x).map(|(s, c)| [s.to_bits(), c.to_bits()]),
    slow: |x, _| {
        let (s, c) = sin_cos_slow(x);
        [s.to_bits(), c.to_bits()]
    },
    typical: |r| (r.uni(-100.0, 100.0), 0.0),
    wide: trig_wide,
    reference: include_bytes!("../tests/data/sin.bin"),
};
const TAN: Fun = Fun {
    name: "tan",
    arity: 1,
    public: |x, _| one(crate::tan(x)),
    fast: |x, _| one_opt(tan_fast(x)),
    slow: |x, _| one(tan_slow(x)),
    typical: |r| (r.uni(-100.0, 100.0), 0.0),
    wide: trig_wide,
    reference: include_bytes!("../tests/data/tan.bin"),
};
const ATAN: Fun = Fun {
    name: "atan",
    arity: 1,
    public: |x, _| one(crate::atan(x)),
    fast: |x, _| one_opt(atan_fast(x)),
    slow: |x, _| one(atan_slow(x)),
    typical: |r| (r.uni(-10.0, 10.0), 0.0),
    wide: wide1,
    reference: include_bytes!("../tests/data/atan.bin"),
};
const ATAN2: Fun = Fun {
    name: "atan2",
    arity: 2,
    public: |y, x| one(crate::atan2(y, x)),
    fast: |y, x| one_opt(atan2_fast(y, x)),
    slow: |y, x| one(atan2_slow(y, x)),
    typical: |r| (r.uni(-10.0, 10.0), r.uni(-10.0, 10.0)),
    wide: wide2,
    reference: include_bytes!("../tests/data/atan2.bin"),
};
const ASIN: Fun = Fun {
    name: "asin",
    arity: 1,
    public: |x, _| one(crate::asin(x)),
    fast: |x, _| one_opt(asin_fast(x)),
    slow: |x, _| one(asin_slow(x)),
    typical: |r| (r.uni(-1.0, 1.0), 0.0),
    wide: |r| {
        let v = r.wide();
        (if r.next() & 1 == 0 { v } else { 1.0 - v * 1e-3 }, 0.0)
    },
    reference: include_bytes!("../tests/data/asin.bin"),
};
const ACOS: Fun = Fun {
    name: "acos",
    arity: 1,
    public: |x, _| one(crate::acos(x)),
    fast: |x, _| one_opt(acos_fast(x)),
    slow: |x, _| one(acos_slow(x)),
    typical: |r| (r.uni(-1.0, 1.0), 0.0),
    wide: |r| {
        let v = r.wide();
        (if r.next() & 1 == 0 { v } else { 1.0 - v * 1e-3 }, 0.0)
    },
    reference: include_bytes!("../tests/data/acos.bin"),
};
const EXP: Fun = Fun {
    name: "exp",
    arity: 1,
    public: |x, _| one(crate::exp(x)),
    fast: |x, _| one_opt(exp_fast(x)),
    slow: |x, _| one(exp_slow(x)),
    typical: |r| (r.uni(-700.0, 700.0), 0.0),
    wide: wide1,
    reference: include_bytes!("../tests/data/exp.bin"),
};
const LN: Fun = Fun {
    name: "ln",
    arity: 1,
    public: |x, _| one(crate::ln(x)),
    fast: |x, _| one_opt(ln_fast(x)),
    slow: |x, _| one(ln_slow(x)),
    typical: |r| (r.uni(1e-3, 1e3), 0.0),
    wide: |r| {
        // all magnitudes, and half the time close to 1 (the series in the table remainder)
        if r.next() & 1 == 0 {
            (r.wide().abs(), 0.0)
        } else {
            (1.0 + r.logu(-45, -2, true), 0.0)
        }
    },
    reference: include_bytes!("../tests/data/ln.bin"),
};
const POW: Fun = Fun {
    name: "pow",
    arity: 2,
    public: |x, y| one(crate::pow(x, y)),
    fast: |x, y| one_opt(pow_fast(x, y)),
    slow: |x, y| one(pow_slow(x, y)),
    typical: |r| (r.uni(0.1, 100.0), r.uni(-5.0, 5.0)),
    wide: |r| {
        // y ln x up to the overflow limit, either sign of x (integer y then and again)
        let x = r.logu(-40, 40, false);
        let lx = crate::ln(x);
        let z = r.uni(-760.0, 760.0);
        let y = z / lx;
        let y = if r.next() & 3 == 0 {
            (y as i64) as f64
        } else {
            y
        };
        (if r.next() & 3 == 0 { -x } else { x }, y)
    },
    reference: include_bytes!("../tests/data/pow.bin"),
};
const HYPOT: Fun = Fun {
    name: "hypot",
    arity: 2,
    public: |x, y| one(crate::hypot(x, y)),
    fast: |x, y| one_opt(hypot_fast(x, y)),
    slow: |x, y| one(hypot_slow(x, y)),
    typical: |r| (r.uni(-100.0, 100.0), r.uni(-100.0, 100.0)),
    wide: wide2,
    reference: include_bytes!("../tests/data/hypot.bin"),
};
const CBRT: Fun = Fun {
    name: "cbrt",
    arity: 1,
    public: |x, _| one(crate::cbrt(x)),
    fast: |x, _| one_opt(cbrt_fast(x)),
    slow: |x, _| one(cbrt_slow(x)),
    typical: |r| (r.uni(-1e6, 1e6), 0.0),
    wide: wide1,
    reference: include_bytes!("../tests/data/cbrt.bin"),
};

/// Hand-picked inputs, tried in every position of every argument.
const EDGE: [f64; 28] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    -0.5,
    2.0,
    3.0,
    4.0,
    -2.0,
    10.0,
    1e-9,
    7.450_580_596_923_828e-9,
    f64::MIN_POSITIVE,
    5e-324,
    1e300,
    f64::MAX,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    std::f64::consts::FRAC_PI_2,
    std::f64::consts::PI,
    std::f64::consts::FRAC_PI_4,
    524_288.0,
    8.0,
    27.0,
    0.9999999999999999,
    1.0000000000000002,
];

/// The public function equals the double-double path, and so does the fast path whenever it
/// answers, on `n` inputs: the edge list, then a mix of typical, wide and exact-result inputs.
/// Returns how many times the fast path answered.
fn compare(f: &Fun, n: usize, seed: u64) -> usize {
    let mut rng = Rng(seed);
    let mut taken = 0usize;
    let mut check = |x: f64, y: f64| {
        let want = (f.slow)(x, y);
        assert_eq!(
            (f.public)(x, y),
            want,
            "{}: public differs from the slow path at ({x:e}, {y:e})",
            f.name
        );
        if let Some(got) = (f.fast)(x, y) {
            taken += 1;
            assert_eq!(
                got, want,
                "{}: fast differs from the slow path at ({x:e}, {y:e})",
                f.name
            );
        }
    };
    for a in EDGE {
        if f.arity == 1 {
            check(a, 0.0);
        } else {
            for b in EDGE {
                check(a, b);
            }
        }
    }
    for i in 0..n {
        let (x, y) = match i & 7 {
            0 => (f.wide)(&mut rng),
            1 => {
                // small integers and exact cases: (3, 4), (2, 10), 27 ...
                let a = (rng.next() & 63) as f64 - 31.0;
                let b = (rng.next() & 15) as f64 - 5.0;
                if f.arity == 1 {
                    (a * a * a, 0.0)
                } else {
                    (a, b)
                }
            }
            _ => (f.typical)(&mut rng),
        };
        check(x, y);
    }
    taken
}

/// Fraction of the committed reference inputs and of `N_RATE` typical random inputs on which
/// the fast path answers.
fn rates(f: &Fun) -> (f64, f64) {
    let words: Vec<u64> = f
        .reference
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    let rec = f.arity + 2;
    let (mut total, mut taken) = (0usize, 0usize);
    for w in words.chunks_exact(rec) {
        let x = f64::from_bits(w[0]);
        let y = if f.arity == 2 {
            f64::from_bits(w[1])
        } else {
            0.0
        };
        total += 1;
        if (f.fast)(x, y).is_some() {
            taken += 1;
        }
    }
    let by_ref = taken as f64 / total as f64;
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut taken = 0usize;
    for _ in 0..N_RATE {
        let (x, y) = (f.typical)(&mut rng);
        if (f.fast)(x, y).is_some() {
            taken += 1;
        }
    }
    let by_random = taken as f64 / N_RATE as f64;
    println!(
        "{:8} fast path taken: {:.4} of {total} reference inputs, {:.4} of {N_RATE} typical random inputs",
        f.name, by_ref, by_random
    );
    (by_ref, by_random)
}

fn equal(f: &Fun) {
    let taken = compare(f, N_EQUAL, 0x9e37_79b9_7f4a_7c15 ^ f.name.len() as u64);
    println!("{:8} {taken} of {N_EQUAL} fast", f.name);
    assert!(
        taken > N_EQUAL / 4,
        "{}: fast path almost never taken",
        f.name
    );
}

fn rate(f: &Fun, min_ref: f64, min_random: f64) {
    let (a, b) = rates(f);
    assert!(
        a >= min_ref,
        "{}: reference rate {a} below {min_ref}",
        f.name
    );
    assert!(
        b >= min_random,
        "{}: random rate {b} below {min_random}",
        f.name
    );
}

// REQ: OA-MATH-021
#[test]
fn sin_fast_equals_slow() {
    equal(&SIN);
}
// REQ: OA-MATH-021
#[test]
fn cos_fast_equals_slow() {
    equal(&COS);
}
// REQ: OA-MATH-021
#[test]
fn sin_cos_fast_equals_slow() {
    equal(&SIN_COS);
}
// REQ: OA-MATH-021
#[test]
fn tan_fast_equals_slow() {
    equal(&TAN);
}
// REQ: OA-MATH-021
#[test]
fn atan_fast_equals_slow() {
    equal(&ATAN);
}
// REQ: OA-MATH-021
#[test]
fn atan2_fast_equals_slow() {
    equal(&ATAN2);
}
// REQ: OA-MATH-021
#[test]
fn asin_fast_equals_slow() {
    equal(&ASIN);
}
// REQ: OA-MATH-021
#[test]
fn acos_fast_equals_slow() {
    equal(&ACOS);
}
// REQ: OA-MATH-021
#[test]
fn exp_fast_equals_slow() {
    equal(&EXP);
}
// REQ: OA-MATH-021
#[test]
fn ln_fast_equals_slow() {
    equal(&LN);
}
// REQ: OA-MATH-021
#[test]
fn pow_fast_equals_slow() {
    equal(&POW);
}
// REQ: OA-MATH-021
#[test]
fn hypot_fast_equals_slow() {
    equal(&HYPOT);
}
// REQ: OA-MATH-021
#[test]
fn cbrt_fast_equals_slow() {
    equal(&CBRT);
}

// REQ: OA-MATH-022
#[test]
fn sin_fast_rate() {
    rate(&SIN, 0.4, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn cos_fast_rate() {
    rate(&COS, 0.4, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn sin_cos_fast_rate() {
    rate(&SIN_COS, 0.4, 0.94);
}
// REQ: OA-MATH-022
#[test]
fn tan_fast_rate() {
    rate(&TAN, 0.38, 0.94);
}
// REQ: OA-MATH-022
#[test]
fn atan_fast_rate() {
    rate(&ATAN, 0.68, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn atan2_fast_rate() {
    rate(&ATAN2, 0.72, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn asin_fast_rate() {
    rate(&ASIN, 0.82, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn acos_fast_rate() {
    rate(&ACOS, 0.92, 0.97);
}
// REQ: OA-MATH-022
#[test]
fn exp_fast_rate() {
    rate(&EXP, 0.74, 0.95);
}
// REQ: OA-MATH-022
#[test]
fn ln_fast_rate() {
    rate(&LN, 0.95, 0.99);
}
// REQ: OA-MATH-022
#[test]
fn pow_fast_rate() {
    rate(&POW, 0.72, 0.91);
}
// REQ: OA-MATH-022
#[test]
fn hypot_fast_rate() {
    rate(&HYPOT, 0.49, 0.99);
}
// REQ: OA-MATH-022
#[test]
fn cbrt_fast_rate() {
    rate(&CBRT, 0.97, 0.99);
}
