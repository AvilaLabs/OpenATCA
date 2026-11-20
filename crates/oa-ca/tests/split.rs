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
