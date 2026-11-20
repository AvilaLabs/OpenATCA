//! The horizontal miss is `oa_ieee::sqrt(x * x + y * y)` (OA-CA-029). It agrees with
//! `oa_math::hypot` to within 2 ulp over the whole range the proofs bound the closest point to.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::float_arithmetic
)]

const POINT_MAX: f64 = 5.0e7;
/// Above this the squares and their sum are normal numbers.
const UNDERFLOW_FREE: f64 = 1.0e-150;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in [-1, 1].
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

fn new_form(x: f64, y: f64) -> f64 {
    oa_ieee::sqrt(x * x + y * y)
}

fn ulps(a: f64, b: f64) -> u64 {
    a.to_bits().abs_diff(b.to_bits())
}

fn check(x: f64, y: f64) {
    let (n, o) = (new_form(x, y), oa_math::hypot(x, y));
    assert!(n.is_finite() && n >= 0.0, "{x} {y} {n}");
    if x.abs().max(y.abs()) >= UNDERFLOW_FREE {
        assert!(ulps(n, o) <= 2, "{x} {y}: {n} vs {o}");
    } else {
        // The squares underflow (a point under 1e-150 m from the origin: a miss of nothing), so
        // the relative error is lost, and the absolute error is below 1e-150 m.
        assert!((n - o).abs() <= 1e-150, "{x} {y}: {n} vs {o}");
    }
}

// REQ: OA-CA-029
#[test]
fn the_new_horizontal_miss_agrees_with_hypot_within_two_ulp() {
    let mut r = Rng(0x1234_5678_9abc_def0);
    for _ in 0..2_000_000 {
        // Uniform over the bound, and log-uniform scales, so that small and large mix.
        let (a, b) = (r.unit(), r.unit());
        check(a * POINT_MAX, b * POINT_MAX);
        let (sa, sb) = (r.unit().abs() * 40.0, r.unit().abs() * 40.0);
        check(
            a * POINT_MAX * 10f64.powf(-sa / 5.0),
            b * POINT_MAX * 10f64.powf(-sb / 5.0),
        );
    }
}

// REQ: OA-CA-029
#[test]
fn the_new_horizontal_miss_agrees_with_hypot_on_the_edges() {
    let tiny = [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        1e-300,
        1e-200,
        1e-160,
        1e-10,
        1e-3,
        1.0,
        3.0,
        4.0,
    ];
    let big = [
        POINT_MAX,
        -POINT_MAX,
        POINT_MAX * (1.0 - f64::EPSILON),
        4.9e7,
        3.5e7,
        1e7,
    ];
    for &a in tiny.iter().chain(&big) {
        for &b in tiny.iter().chain(&big) {
            check(a, b);
            check(-a, b);
        }
    }
    // The corners and the neighbours of the bound, bit by bit.
    for k in 0..200u64 {
        let below = f64::from_bits(POINT_MAX.to_bits() - k);
        check(below, POINT_MAX);
        check(below, below);
        check(f64::from_bits(k), below);
    }
    assert_eq!(new_form(3.0, 4.0), 5.0);
    assert_eq!(new_form(0.0, 0.0), 0.0);
    // Even at the corner, no overflow.
    assert!(new_form(POINT_MAX, POINT_MAX).is_finite());
}
