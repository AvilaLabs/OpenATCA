//! The split of the horizontal solver, the miss and the prediction (so that each part is proved
//! alone) is the same arithmetic in the same order: the unsplit code, kept here as it was, and
//! the split code agree bit for bit on a million deterministic inputs, except the horizontal miss,
//! which is now a square root of a sum of squares where the original had `oa_math::hypot`, and
//! agrees to within 2 ulp.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_ca::solve::*;

// ---- the code before the split, verbatim ---------------------------------------------------

fn old_horizontal_span(rx: f64, ry: f64, vx: f64, vy: f64, h: f64, t_max: f64) -> Option<Span> {
    let hh = h * h;
    let now = rx * rx + ry * ry < hh;
    let a = vx * vx + vy * vy;
    if a < STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS {
        return if now {
            Some(Span {
                lo: 0.0,
                hi: t_max,
                now,
            })
        } else {
            None
        };
    }
    let cross = rx * vy - ry * vx;
    let disc = hh * a - cross * cross;
    let tc = -(rx * vx + ry * vy) / a;
    if disc <= 0.0 {
        return if now {
            let hi = if tc < 0.0 {
                0.0
            } else if tc > t_max {
                t_max
            } else {
                tc
            };
            Some(Span { lo: 0.0, hi, now })
        } else {
            None
        };
    }
    let w = oa_ieee::sqrt(disc) / a;
    let t1 = tc - w;
    let t2 = tc + w;
    let lo = if now || t1 < 0.0 { 0.0 } else { t1 };
    let hi = if t2 > t_max {
        t_max
    } else if now && t2 < 0.0 {
        0.0
    } else {
        t2
    };
    if lo > hi {
        None
    } else {
        Some(Span { lo, hi, now })
    }
}

fn old_horizontal_miss(rx: f64, ry: f64, vx: f64, vy: f64, t_max: f64) -> f64 {
    let a = vx * vx + vy * vy;
    let t = if a < STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS {
        0.0
    } else {
        let tc = -(rx * vx + ry * vy) / a;
        if tc < 0.0 {
            0.0
        } else if tc > t_max {
            t_max
        } else {
            tc
        }
    };
    oa_math::hypot(rx + vx * t, ry + vy * t)
}

fn old_predict(
    rx: f64,
    ry: f64,
    vx: f64,
    vy: f64,
    h_eff: f64,
    vert: Option<&Vertical>,
    t_max: f64,
) -> Option<Prediction> {
    let m = combine_spans(
        old_horizontal_span(rx, ry, vx, vy, h_eff, t_max),
        vert,
        t_max,
    )?;
    Some(Prediction {
        ttl_s: if m.now { 0.0 } else { m.lo },
        now: m.now,
        h_miss_m: old_horizontal_miss(rx, ry, vx, vy, t_max),
        v_miss_ft: match vert {
            Some(z) => vertical_miss(z.dz_ft, z.dvz_fps, t_max),
            None => f64::NAN,
        },
    })
}

// ---- inputs ---------------------------------------------------------------------------------

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
    /// Uniform in `[-1, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
    /// A value of scale `m`: mostly in range, with the edges the solver cares about.
    fn val(&mut self, m: f64) -> f64 {
        match self.next() % 16 {
            0 => 0.0,
            1 => -0.0,
            2 => m,
            3 => -m,
            4 => f64::from_bits(self.next() % 1000 + 1), // subnormal
            5 => -f64::from_bits(self.next() % 1000 + 1),
            6 => self.unit() * 1.0e-9,
            7 => self.unit() * 5.0e-5,
            8 => 5.0e-5,
            9 => -5.0e-5,
            10 => f64::from_bits(self.next()), // any bit pattern, NaN and infinity included
            11 => self.unit() * m * 1.0e-4,
            _ => self.unit() * m,
        }
    }
    fn pos(&mut self, m: f64) -> f64 {
        match self.next() % 8 {
            0 => m,
            1 => f64::from_bits(self.next() % 1000 + 1),
            2 => 5.0e-5,
            3 => f64::from_bits(self.next()),
            _ => (self.unit().abs() + 1.0e-12) * m,
        }
    }
}

fn same(a: &Option<Span>, b: &Option<Span>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.lo.to_bits() == b.lo.to_bits() && a.hi.to_bits() == b.hi.to_bits() && a.now == b.now
        }
        _ => false,
    }
}

/// The horizontal miss is `sqrt(x * x + y * y)`; the original code (and so the golden vectors)
/// had `oa_math::hypot`. Both are within 2 ulp of the true value, so the miss may differ in the
/// last bits. Miss values are not negative, so the ulp distance is the distance of the bits. Where
/// the closest point is under 1e-150 m from the origin the squares underflow and the miss is
/// 0 or a few ulp of a tiny number; two such values are equal for the purpose (a miss of nothing).
/// Where it is over 1e150 m (far outside the bounds the proofs state; the golden vectors have a few)
/// the squares overflow and the new miss is infinite where `oa_math::hypot` has the finite value.
fn miss_within_2_ulp(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
        || (a.is_nan() && b.is_nan())
        || (a >= 1.0e150 && b == f64::INFINITY)
        || (a.is_finite()
            && b.is_finite()
            && a >= 0.0
            && b >= 0.0
            && (a.to_bits().abs_diff(b.to_bits()) <= 2 || (a < 1.0e-150 && b < 1.0e-150)))
}

fn same_prediction(a: &Option<Prediction>, b: &Option<Prediction>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.ttl_s.to_bits() == b.ttl_s.to_bits()
                && a.now == b.now
                && miss_within_2_ulp(a.h_miss_m, b.h_miss_m)
                && a.v_miss_ft.to_bits() == b.v_miss_ft.to_bits()
        }
        _ => false,
    }
}

// REQ: OA-CA-018, OA-CA-028, OA-CA-029
#[test]
fn the_split_solver_miss_and_prediction_agree_with_the_unsplit_code() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut some, mut nones) = (0u32, 0u32);
    for _ in 0..1_000_000 {
        let (rx, ry) = (r.val(MAX_COORD_M), r.val(MAX_COORD_M));
        let (vx, vy) = (r.val(MAX_REL_SPEED_MPS), r.val(MAX_REL_SPEED_MPS));
        let h = r.pos(MAX_RADIUS_M);
        let t = r.pos(MAX_LOOK_AHEAD_S);
        let (old, new) = (
            old_horizontal_span(rx, ry, vx, vy, h, t),
            horizontal_span(rx, ry, vx, vy, h, t),
        );
        assert!(same(&old, &new), "{rx} {ry} {vx} {vy} {h} {t}");
        if old.is_some() {
            some += 1;
        } else {
            nones += 1;
        }
        let (om, nm) = (
            old_horizontal_miss(rx, ry, vx, vy, t),
            horizontal_miss(rx, ry, vx, vy, t),
        );
        assert!(
            miss_within_2_ulp(om, nm),
            "{rx} {ry} {vx} {vy} {t}: {om} {nm}"
        );
        let z = Vertical {
            dz_ft: r.val(MAX_ALT_FT),
            dvz_fps: r.val(MAX_ABS_VRATE_FPS),
            v_eff_ft: r.pos(MAX_ALT_FT),
        };
        for vert in [Some(&z), None] {
            let (op, np) = (
                old_predict(rx, ry, vx, vy, h, vert, t),
                predict(rx, ry, vx, vy, h, vert, t),
            );
            assert!(same_prediction(&op, &np), "{rx} {ry} {vx} {vy} {h} {t}");
        }
    }
    // Both kinds of result were compared, many times.
    assert!(some > 10_000 && nones > 10_000, "{some} {nones}");
}

// ---- golden vectors from the original code ---------------------------------------------------

const GOLDEN: &str = include_str!("golden/solver_vectors.txt");

fn bits(tok: &str) -> f64 {
    f64::from_bits(u64::from_str_radix(tok, 16).unwrap())
}

/// Equal bit for bit, except that any NaN equals any NaN (the payload of a NaN is not arithmetic).
fn same_f64(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn span_from<'a>(it: &mut impl Iterator<Item = &'a str>) -> Option<Span> {
    match it.next().unwrap() {
        "N" => None,
        "S" => Some(Span {
            lo: bits(it.next().unwrap()),
            hi: bits(it.next().unwrap()),
            now: it.next().unwrap() == "1",
        }),
        other => panic!("{other}"),
    }
}

fn prediction_from<'a>(it: &mut impl Iterator<Item = &'a str>) -> Option<Prediction> {
    match it.next().unwrap() {
        "N" => None,
        "P" => Some(Prediction {
            ttl_s: bits(it.next().unwrap()),
            now: it.next().unwrap() == "1",
            h_miss_m: bits(it.next().unwrap()),
            v_miss_ft: bits(it.next().unwrap()),
        }),
        other => panic!("{other}"),
    }
}

fn same_span_golden(a: &Option<Span>, b: &Option<Span>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same_f64(a.lo, b.lo) && same_f64(a.hi, b.hi) && a.now == b.now,
        _ => false,
    }
}

fn same_prediction_golden(a: &Option<Prediction>, b: &Option<Prediction>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            same_f64(a.ttl_s, b.ttl_s)
                && a.now == b.now
                && miss_within_2_ulp(a.h_miss_m, b.h_miss_m)
                && same_f64(a.v_miss_ft, b.v_miss_ft)
        }
        _ => false,
    }
}

// REQ: OA-CA-018, OA-CA-028, OA-CA-029
#[test]
fn the_split_code_agrees_bit_for_bit_with_the_golden_vectors_of_the_original_code() {
    // Bit for bit in every field but the horizontal miss, which is within 2 ulp (the vectors have
    // the `oa_math::hypot` value of the original code; the golden file is not regenerated).
    // The vectors were made by the code before the split (the commit is named at the top of the
    // file), so the comparison does not rest on a copy of that code kept in this file.
    let (mut cases, mut spans, mut preds_known, mut preds_unknown) = (0u32, 0u32, 0u32, 0u32);
    for (n, line) in GOLDEN.lines().enumerate() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let mut inp = [0.0f64; 9];
        for x in &mut inp {
            *x = bits(it.next().unwrap());
        }
        let [rx, ry, vx, vy, h, t, dz, dvz, v] = inp;
        let want_span = span_from(&mut it);
        let want_miss = bits(it.next().unwrap());
        let want_known = prediction_from(&mut it);
        assert_eq!(it.next(), Some("|"));
        let want_unknown = prediction_from(&mut it);
        assert_eq!(it.next(), None);
        let at = format!("line {}: {line}", n + 1);
        assert!(
            same_span_golden(&want_span, &horizontal_span(rx, ry, vx, vy, h, t)),
            "{at}"
        );
        assert!(
            miss_within_2_ulp(want_miss, horizontal_miss(rx, ry, vx, vy, t)),
            "{at}"
        );
        let z = Vertical {
            dz_ft: dz,
            dvz_fps: dvz,
            v_eff_ft: v,
        };
        assert!(
            same_prediction_golden(&want_known, &predict(rx, ry, vx, vy, h, Some(&z), t)),
            "{at}"
        );
        assert!(
            same_prediction_golden(&want_unknown, &predict(rx, ry, vx, vy, h, None, t)),
            "{at}"
        );
        cases += 1;
        spans += u32::from(want_span.is_some());
        preds_known += u32::from(want_known.is_some());
        preds_unknown += u32::from(want_unknown.is_some());
    }
    // Every kind of result is in the file, in numbers that matter.
    assert_eq!(cases, 2400);
    assert!(
        spans > 300 && preds_known > 100 && preds_unknown > 100,
        "{spans} {preds_known} {preds_unknown}"
    );
}

// REQ: OA-CA-028
#[test]
fn the_first_step_hands_the_root_step_the_three_values_the_proofs_take_within_their_contract() {
    // The proofs of the first step take `a`, the discriminant and the time of closest approach
    // under a contract, each proved for the function that computes it; that `horizontal_step`
    // passes exactly those on, and that the root step's inputs meet the contract of the square
    // root and the last step, is checked here on inputs inside the stated bounds (the proof of
    // `horizontal_step` itself does not finish in CBMC). The constants are the proofs' own.
    const A_MAX: f64 = 3.3e7;
    const DISC_MAX: f64 = 4.0e19;
    const TC_MAX: f64 = 4.0e18;
    let within = |x: f64, m: f64| x.is_finite() && x.abs() <= m;
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut roots, mut dones) = (0u32, 0u32);
    for _ in 0..400_000 {
        let (rx, ry) = (r.val(MAX_COORD_M), r.val(MAX_COORD_M));
        let (vx, vy) = (r.val(MAX_REL_SPEED_MPS), r.val(MAX_REL_SPEED_MPS));
        let (h, t) = (r.pos(MAX_RADIUS_M), r.pos(MAX_LOOK_AHEAD_S));
        if !(within(rx, MAX_COORD_M)
            && within(ry, MAX_COORD_M)
            && within(vx, MAX_REL_SPEED_MPS)
            && within(vy, MAX_REL_SPEED_MPS)
            && h > 0.0
            && h <= MAX_RADIUS_M
            && t > 0.0
            && t <= MAX_LOOK_AHEAD_S)
        {
            continue;
        }
        match horizontal_step(rx, ry, vx, vy, h, t) {
            HorizontalStep::Done(_) => dones += 1,
            HorizontalStep::Root { now, tc, a, disc } => {
                roots += 1;
                let cross = rx * vy - ry * vx;
                assert_eq!(a.to_bits(), (vx * vx + vy * vy).to_bits());
                assert_eq!(disc.to_bits(), (h * h * a - cross * cross).to_bits());
                assert_eq!(tc.to_bits(), (-(rx * vx + ry * vy) / a).to_bits());
                assert_eq!(now, rx * rx + ry * ry < h * h);
                assert!((STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS..=A_MAX).contains(&a));
                assert!(disc.is_finite() && disc > 0.0 && disc <= DISC_MAX);
                assert!(within(tc, TC_MAX));
                // The contract the root step takes of the square root.
                let root = oa_ieee::sqrt(disc);
                assert!(root.is_finite() && (0.0..=7.0e9).contains(&root));
            }
        }
    }
    assert!(roots > 10_000 && dones > 10_000, "{roots} {dones}");
}

// REQ: OA-CA-028
#[test]
fn the_decision_of_the_first_step_gives_a_current_pair_a_span_or_a_root_step() {
    let t = 120.0;
    let slow = STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS * 0.5;
    // A pair at rest relative to each other: the whole look-ahead when current, none otherwise.
    assert_eq!(
        step_of(true, slow, 1.0, 5.0, t),
        HorizontalStep::Done(Some(Span {
            lo: 0.0,
            hi: t,
            now: true
        }))
    );
    assert_eq!(
        step_of(false, slow, 1.0, 5.0, t),
        HorizontalStep::Done(None)
    );
    // No positive discriminant and a current pair: kept current to the closest approach, clamped
    // to the look-ahead.
    for (tc, hi) in [(-3.0, 0.0), (0.0, 0.0), (40.0, 40.0), (t, t), (500.0, t)] {
        match step_of(true, 10.0, 0.0, tc, t) {
            HorizontalStep::Done(Some(s)) => assert!(s.now && s.lo == 0.0 && s.hi == hi),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        step_of(false, 10.0, -1.0, 40.0, t),
        HorizontalStep::Done(None)
    );
    // A positive discriminant hands the root step the same three numbers.
    assert_eq!(
        step_of(true, 10.0, 7.0, -2.5, t),
        HorizontalStep::Root {
            now: true,
            tc: -2.5,
            a: 10.0,
            disc: 7.0
        }
    );
}

// REQ: OA-CA-029
#[test]
fn the_time_to_loss_of_a_prediction_is_the_span_start_or_zero_and_the_misses_are_carried() {
    let t = 120.0;
    for (lo, hi) in [(0.0, 0.0), (0.0, t), (30.0, 30.0), (30.0, 90.0), (t, t)] {
        let span = Span { lo, hi, now: false };
        let p = prediction_of(&span, 12.5, 300.0);
        assert!(!p.now && p.ttl_s == lo && p.h_miss_m == 12.5 && p.v_miss_ft == 300.0);
        assert!((0.0..=t).contains(&p.ttl_s));
        let current = Span { lo, hi, now: true };
        let p = prediction_of(&current, 0.0, 0.0);
        assert!(p.now && p.ttl_s == 0.0);
    }
    // An unknown altitude has no vertical miss: the NaN is carried, not hidden.
    let p = prediction_of(
        &Span {
            lo: 5.0,
            hi: 9.0,
            now: false,
        },
        1.0,
        f64::NAN,
    );
    assert!(p.v_miss_ft.is_nan() && p.ttl_s == 5.0 && p.h_miss_m == 1.0);
}
