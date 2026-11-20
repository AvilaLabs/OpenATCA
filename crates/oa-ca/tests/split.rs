//! The split of the horizontal solver, the miss and the prediction (so that each part is proved
//! alone) is the same arithmetic in the same order: the unsplit code, kept here as it was, and
//! the split code agree bit for bit on a million deterministic inputs.
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

fn same_prediction(a: &Option<Prediction>, b: &Option<Prediction>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.ttl_s.to_bits() == b.ttl_s.to_bits()
                && a.now == b.now
                && a.h_miss_m.to_bits() == b.h_miss_m.to_bits()
                && a.v_miss_ft.to_bits() == b.v_miss_ft.to_bits()
        }
        _ => false,
    }
}

// REQ: OA-CA-018, OA-CA-028, OA-CA-029
#[test]
fn the_split_solver_miss_and_prediction_agree_bit_for_bit_with_the_unsplit_code() {
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
        assert_eq!(om.to_bits(), nm.to_bits(), "{rx} {ry} {vx} {vy} {t}");
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
                && same_f64(a.h_miss_m, b.h_miss_m)
                && same_f64(a.v_miss_ft, b.v_miss_ft)
        }
        _ => false,
    }
}

// REQ: OA-CA-018, OA-CA-028, OA-CA-029
#[test]
fn the_split_code_agrees_bit_for_bit_with_the_golden_vectors_of_the_original_code() {
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
            same_f64(want_miss, horizontal_miss(rx, ry, vx, vy, t)),
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
