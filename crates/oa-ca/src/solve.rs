//! Straight-line relative motion: the loss intervals and the miss distances. Core (pure).
//!
//! Two aircraft at relative position `r` (metres, east and north of the first) with relative
//! velocity `v` (metres per second) are in horizontal loss at time `t` while `|r + v t| < H`.
//! That is a quadratic in `t`; it is solved without cancellation near the middle of the pass by
//! the perpendicular miss `|cross(r, v)| / |v|` and the time of closest approach `-(r.v) / (v.v)`.
//! Vertically, relative altitude `dz` (feet) and rate `dvz` (feet per second) are in loss while
//! `|dz + dvz t| < V`, which is linear. A pair is in conflict when the two loss intervals meet
//! inside `[0, T]`, `T` the look-ahead.
//!
//! Stated bounds (the proofs in this file hold for them, and the cycle refuses what exceeds
//! them before it calls here): components of `r` at most [`MAX_COORD_M`], of `v` at most
//! [`MAX_REL_SPEED_MPS`], `H` in `(0, MAX_RADIUS_M]`, `T` in `(0, MAX_LOOK_AHEAD_S]`, `|dz|` and
//! `V` at most [`MAX_ALT_FT`], `|dvz|` at most [`MAX_ABS_VRATE_FPS`]. A relative speed under
//! [`STATIONARY_SPEED_MPS`] (a relative drift of under half a metre in the longest look-ahead, inside the 1 m of the proofs' tolerance) is
//! taken as zero: the pair is inside for the whole look-ahead or never. The same for a relative
//! vertical rate under [`LEVEL_RATE_FPS`]. Every division is by a quantity at least the threshold,
//! so no result is infinite or NaN for inputs within the bounds.
//!
//! Edges are strict, as in `oa-encounter`: a separation equal to the minimum is not inside it.

/// Largest component of the relative position, metres.
pub const MAX_COORD_M: f64 = 1.0e6;
/// Largest component of the relative velocity, metres per second.
pub const MAX_REL_SPEED_MPS: f64 = 4.0e3;
/// Largest horizontal separation limit, metres.
pub const MAX_RADIUS_M: f64 = 1.0e6;
/// Longest look-ahead, seconds.
pub const MAX_LOOK_AHEAD_S: f64 = 1.0e4;
/// Largest relative altitude and vertical limit, feet.
pub const MAX_ALT_FT: f64 = 1.0e6;
/// Largest relative vertical rate, feet per second.
pub const MAX_ABS_VRATE_FPS: f64 = 2.0e3;
/// A relative speed under this, metres per second, is taken as zero.
pub const STATIONARY_SPEED_MPS: f64 = 5.0e-5;
/// A relative vertical rate under this, feet per second, is taken as zero.
pub const LEVEL_RATE_FPS: f64 = 5.0e-5;

/// A loss interval inside the look-ahead, seconds from now, `0 <= lo <= hi <= T`.
// A7.2: transient, one per pair evaluation; size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub lo: f64,
    pub hi: f64,
    /// The pair is inside the limit now (`t = 0`, strictly).
    pub now: bool,
}

/// The loss interval of the horizontal separation, or `None` if the pair is never inside the
/// limit `h` within `[0, t_max]`. `(rx, ry)` is the relative position, `(vx, vy)` the relative
/// velocity.
pub fn horizontal_span(rx: f64, ry: f64, vx: f64, vy: f64, h: f64, t_max: f64) -> Option<Span> {
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
        // Never inside; or inside now by less than the rounding of the discriminant, in which
        // case the pair is kept as current and leaves at the closest approach.
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
    let hi = if t2 > t_max { t_max } else { t2 };
    if lo > hi {
        None
    } else {
        Some(Span { lo, hi, now })
    }
}

/// The loss interval of the vertical separation `|dz + dvz t| < v`, or `None` if never inside
/// within `[0, t_max]`. `dvz` is feet per second.
pub fn vertical_span(dz: f64, dvz: f64, v: f64, t_max: f64) -> Option<Span> {
    let now = dz < v && -dz < v;
    if dvz < LEVEL_RATE_FPS && -dvz < LEVEL_RATE_FPS {
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
    let ta = (-v - dz) / dvz;
    let tb = (v - dz) / dvz;
    let (t1, t2) = if dvz > 0.0 { (ta, tb) } else { (tb, ta) };
    let lo = if now || t1 < 0.0 { 0.0 } else { t1 };
    let hi = if t2 > t_max { t_max } else { t2 };
    if lo > hi {
        None
    } else {
        Some(Span { lo, hi, now })
    }
}

/// The intersection of two loss intervals, or `None`: a conflict needs both inside now, or an
/// overlap of positive length.
pub fn meet(h: &Span, v: &Span) -> Option<Span> {
    let lo = if h.lo > v.lo { h.lo } else { v.lo };
    let hi = if h.hi < v.hi { h.hi } else { v.hi };
    let now = h.now && v.now;
    if now || lo < hi {
        Some(Span {
            lo,
            hi: if hi < lo { lo } else { hi },
            now,
        })
    } else {
        None
    }
}

/// The smallest horizontal separation over `[0, t_max]` for straight-line motion, metres.
pub fn horizontal_miss(rx: f64, ry: f64, vx: f64, vy: f64, t_max: f64) -> f64 {
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

/// The smallest vertical separation over `[0, t_max]` for straight-line motion, feet.
pub fn vertical_miss(dz: f64, dvz: f64, t_max: f64) -> f64 {
    if dvz < LEVEL_RATE_FPS && -dvz < LEVEL_RATE_FPS {
        return dz.abs();
    }
    let ze = dz + dvz * t_max;
    if (dz >= 0.0) != (ze >= 0.0) {
        0.0
    } else {
        let (a, b) = (dz.abs(), ze.abs());
        if a < b {
            a
        } else {
            b
        }
    }
}

/// The vertical part of a pair: relative altitude, relative rate (feet per second) and the
/// effective limit, feet.
// A7.2: transient, one per pair evaluation; size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertical {
    pub dz_ft: f64,
    pub dvz_fps: f64,
    pub v_eff_ft: f64,
}

/// What the prediction says of a pair in conflict.
// A7.2: transient, one per pair evaluation; size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    /// Seconds from now to the start of the loss; zero when it is current.
    pub ttl_s: f64,
    /// The pair is inside both limits now.
    pub now: bool,
    /// Smallest predicted horizontal separation over the look-ahead, metres.
    pub h_miss_m: f64,
    /// Smallest predicted vertical separation over the look-ahead, feet; NaN when the altitude
    /// of either aircraft is unknown.
    pub v_miss_ft: f64,
}

/// The span of a pair's loss given its horizontal span `hs`: the horizontal span itself when
/// `vert` is `None` (an altitude unknown: the test is horizontal only), else its overlap with the
/// vertical loss interval. An unknown altitude never removes a conflict (proved:
/// `an_unknown_altitude_never_removes_a_conflict`).
pub fn combine_spans(hs: Option<Span>, vert: Option<&Vertical>, t_max: f64) -> Option<Span> {
    let hs = hs?;
    let vs = match vert {
        Some(z) => vertical_span(z.dz_ft, z.dvz_fps, z.v_eff_ft, t_max)?,
        None => Span {
            lo: 0.0,
            hi: t_max,
            now: true,
        },
    };
    meet(&hs, &vs)
}

/// The span of the pair's loss inside `[0, t_max]`, or `None` if no loss is predicted.
pub fn conflict_span(
    rx: f64,
    ry: f64,
    vx: f64,
    vy: f64,
    h_eff: f64,
    vert: Option<&Vertical>,
    t_max: f64,
) -> Option<Span> {
    combine_spans(horizontal_span(rx, ry, vx, vy, h_eff, t_max), vert, t_max)
}

/// The prediction for a pair, or `None` if no loss is predicted within `[0, t_max]`: the span of
/// [`conflict_span`] with the smallest horizontal and vertical separations over the look-ahead.
pub fn predict(
    rx: f64,
    ry: f64,
    vx: f64,
    vy: f64,
    h_eff: f64,
    vert: Option<&Vertical>,
    t_max: f64,
) -> Option<Prediction> {
    let m = conflict_span(rx, ry, vx, vy, h_eff, vert, t_max)?;
    Some(Prediction {
        ttl_s: if m.now { 0.0 } else { m.lo },
        now: m.now,
        h_miss_m: horizontal_miss(rx, ry, vx, vy, t_max),
        v_miss_ft: match vert {
            Some(z) => vertical_miss(z.dz_ft, z.dvz_fps, t_max),
            None => f64::NAN,
        },
    })
}

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;

    fn within(x: f64, m: f64) -> bool {
        x.is_finite() && x >= -m && x <= m
    }

    /// Every input of the horizontal solver inside the stated bounds.
    fn horizontal_inputs() -> (f64, f64, f64, f64, f64, f64) {
        let (rx, ry, vx, vy): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        let (h, t): (f64, f64) = (kani::any(), kani::any());
        kani::assume(within(rx, MAX_COORD_M) && within(ry, MAX_COORD_M));
        kani::assume(within(vx, MAX_REL_SPEED_MPS) && within(vy, MAX_REL_SPEED_MPS));
        kani::assume(h > 0.0 && h <= MAX_RADIUS_M);
        kani::assume(t > 0.0 && t <= MAX_LOOK_AHEAD_S);
        (rx, ry, vx, vy, h, t)
    }

    // REQ: OA-CA-018
    #[kani::proof]
    fn horizontal_span_is_finite_and_ordered_for_bounded_inputs() {
        let (rx, ry, vx, vy, h, t) = horizontal_inputs();
        if let Some(s) = horizontal_span(rx, ry, vx, vy, h, t) {
            // No NaN or infinity came out of any division, square root or product, and the
            // interval is ordered inside the look-ahead.
            assert!(s.lo.is_finite() && s.hi.is_finite());
            assert!(0.0 <= s.lo && s.lo <= s.hi && s.hi <= t);
        }
    }

    // REQ: OA-CA-019
    #[kani::proof]
    fn vertical_span_is_finite_and_ordered_for_bounded_inputs() {
        let (dz, dvz, v, t): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(within(dz, MAX_ALT_FT) && within(dvz, MAX_ABS_VRATE_FPS));
        kani::assume(v > 0.0 && v <= MAX_ALT_FT);
        kani::assume(t > 0.0 && t <= MAX_LOOK_AHEAD_S);
        if let Some(s) = vertical_span(dz, dvz, v, t) {
            assert!(s.lo.is_finite() && s.hi.is_finite());
            assert!(0.0 <= s.lo && s.lo <= s.hi && s.hi <= t);
        }
        // The vertical miss is finite and not negative.
        let m = vertical_miss(dz, dvz, t);
        assert!(m.is_finite() && m >= 0.0);
    }

    // REQ: OA-CA-021
    #[kani::proof]
    fn an_unknown_altitude_never_removes_a_conflict() {
        // Any horizontal result inside the look-ahead (not only those the solver can give).
        let t_max: f64 = kani::any();
        kani::assume(t_max > 0.0 && t_max <= MAX_LOOK_AHEAD_S);
        let hs = if kani::any() {
            let (lo, hi): (f64, f64) = (kani::any(), kani::any());
            kani::assume(0.0 <= lo && lo <= hi && hi <= t_max);
            Some(Span {
                lo,
                hi,
                now: kani::any(),
            })
        } else {
            None
        };
        let (dz, dvz, v): (f64, f64, f64) = (kani::any(), kani::any(), kani::any());
        kani::assume(within(dz, MAX_ALT_FT) && within(dvz, MAX_ABS_VRATE_FPS));
        kani::assume(v > 0.0 && v <= MAX_ALT_FT);
        let z = Vertical {
            dz_ft: dz,
            dvz_fps: dvz,
            v_eff_ft: v,
        };
        let known = combine_spans(hs, Some(&z), t_max);
        let unknown = combine_spans(hs, None, t_max);
        if let Some(k) = known {
            // A conflict with the altitude known is a conflict, no later and no less current,
            // with the altitude unknown; and it is the horizontal conflict itself.
            let u = unknown.unwrap();
            assert!(u.lo <= k.lo && k.hi <= u.hi);
            assert!(!k.now || u.now);
        }
        if let Some(h) = hs.filter(|h| h.now || h.lo < h.hi) {
            // With the altitude unknown the conflict is the horizontal span, whole (a span of no
            // length that is not current is no conflict, with the altitude known or not).
            let u = unknown.unwrap();
            assert!(u.lo == h.lo && u.hi == h.hi && u.now == h.now);
        }
    }
}
