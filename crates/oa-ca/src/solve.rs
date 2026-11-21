//! Straight-line relative motion: the loss intervals and the miss distances. Core (pure).
//!
//! Two aircraft at relative position `r` (metres, east and north of the first) with relative
//! velocity `v` (metres per second) are in horizontal loss at time `t` while `|r + v t| < H`.
//! That is a quadratic in `t`, solved by its discriminant `H^2 a - cross(r, v)^2` (`a = v.v`) and
//! the time of closest approach `-(r.v) / a`. The discriminant does cancel near tangency, where the
//! perpendicular miss `|cross| / sqrt(a)` is close to `H`: its rounding error is a few units of
//! `2^-52 H^2 a`, so the half-width of the pass, `sqrt(disc) / a`, is off by at most about
//! `3e-8 H / sqrt(a)` seconds, a distance of about `3e-8 H` (0.03 m at the largest `H`, well inside
//! the 1 m tolerance of the tests), and which side of zero a tiny discriminant falls on decides only
//! a pass of that size: a pair inside now is kept current either way (`step_of`), and the edges are
//! strict in any case.
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

/// What the horizontal solver has once the discriminant is known: the answer where no square root
/// is needed, or the quantities the root step takes.
// A7.2: transient, one per pair evaluation; size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HorizontalStep {
    /// The span (or none) without a square root.
    Done(Option<Span>),
    /// The pair needs `sqrt(disc)`: `disc > 0`, `a` the squared relative speed (at least the
    /// stationary threshold squared), `tc` the time of closest approach.
    Root {
        now: bool,
        tc: f64,
        a: f64,
        disc: f64,
    },
}

/// The first step of [`horizontal_span`]: everything up to the square root of the discriminant.
/// The values it works from are computed apart (the squared speed, the discriminant, the time of
/// closest approach) and the decision on them is [`step_of`], so that the arithmetic is proved apart and the
/// decision, which is a few comparisons, is tested over random inputs (`tests/split.rs`).
pub fn horizontal_step(rx: f64, ry: f64, vx: f64, vy: f64, h: f64, t_max: f64) -> HorizontalStep {
    let hh = h * h;
    let now = rx * rx + ry * ry < hh;
    let a = vx * vx + vy * vy;
    if a < STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS {
        // A speed taken as zero: the other two values are not used (and `tc` would divide by it).
        return step_of(now, a, 0.0, 0.0, t_max);
    }
    step_of(
        now,
        a,
        discriminant(rx, ry, vx, vy, hh, a),
        approach_time(rx, ry, vx, vy, a),
        t_max,
    )
}

/// The discriminant `H^2 a - cross(r, v)^2` of the horizontal loss quadratic.
fn discriminant(rx: f64, ry: f64, vx: f64, vy: f64, hh: f64, a: f64) -> f64 {
    let cross = rx * vy - ry * vx;
    hh * a - cross * cross
}

/// The unclamped time of closest approach `-(r.v) / a`.
fn approach_time(rx: f64, ry: f64, vx: f64, vy: f64, a: f64) -> f64 {
    -(rx * vx + ry * vy) / a
}

/// The decision of [`horizontal_step`] from `now`, the squared speed `a`, the discriminant and the
/// time of closest approach `tc` (the proofs of the arithmetic around it take each of the last
/// three under the contract proved for the function that computes it; `step_of` itself is
/// tested, not proved).
pub fn step_of(now: bool, a: f64, disc: f64, tc: f64, t_max: f64) -> HorizontalStep {
    if a < STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS {
        return HorizontalStep::Done(if now {
            Some(Span {
                lo: 0.0,
                hi: t_max,
                now,
            })
        } else {
            None
        });
    }
    if disc <= 0.0 {
        // Never inside; or inside now by less than the rounding of the discriminant, in which
        // case the pair is kept as current and leaves at the closest approach.
        return HorizontalStep::Done(if now {
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
        });
    }
    HorizontalStep::Root { now, tc, a, disc }
}

/// The last step of [`horizontal_span`]: the span from the square root of the discriminant,
/// `root = sqrt(disc)`. It takes `root` as a parameter so that it is proved apart from the square
/// root (the contract it is proved for is in the proofs module).
pub fn span_from_root(now: bool, tc: f64, a: f64, t_max: f64, root: f64) -> Option<Span> {
    let w = root / a;
    let t1 = tc - w;
    let t2 = tc + w;
    let lo = if now || t1 < 0.0 { 0.0 } else { t1 };
    let hi = if t2 > t_max {
        t_max
    } else if now && t2 < 0.0 {
        // Inside now: the pair leaves at `t2 >= 0` in exact arithmetic; a rounded `t2` just
        // below zero (a tiny discriminant) must not turn a current loss into no loss.
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

/// The loss interval of the horizontal separation, or `None` if the pair is never inside the
/// limit `h` within `[0, t_max]`. `(rx, ry)` is the relative position, `(vx, vy)` the relative
/// velocity.
pub fn horizontal_span(rx: f64, ry: f64, vx: f64, vy: f64, h: f64, t_max: f64) -> Option<Span> {
    match horizontal_step(rx, ry, vx, vy, h, t_max) {
        HorizontalStep::Done(s) => s,
        HorizontalStep::Root { now, tc, a, disc } => {
            span_from_root(now, tc, a, t_max, oa_ieee::sqrt(disc))
        }
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
    let hi = if t2 > t_max {
        t_max
    } else if now && t2 < 0.0 {
        // Inside now: the pair leaves at `t2 >= 0` in exact arithmetic; a rounded `t2` just
        // below zero (a tiny discriminant) must not turn a current loss into no loss.
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

/// The time in `[0, t_max]` of the closest horizontal approach for straight-line motion, seconds
/// (zero for a relative speed taken as zero).
pub fn closest_time(rx: f64, ry: f64, vx: f64, vy: f64, t_max: f64) -> f64 {
    let a = vx * vx + vy * vy;
    if a < STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS {
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
    }
}

/// The relative position, metres, at the time of [`closest_time`].
pub fn closest_point(rx: f64, ry: f64, vx: f64, vy: f64, t_max: f64) -> (f64, f64) {
    let t = closest_time(rx, ry, vx, vy, t_max);
    (rx + vx * t, ry + vy * t)
}

/// The smallest horizontal separation over `[0, t_max]` for straight-line motion, metres.
///
/// The square root of `x * x + y * y` at the closest point. For the stated input bounds the point
/// is finite with each coordinate at most 5e7 m (proved), so the sum is at most 5e15 and cannot
/// overflow; the correctly rounded square root of a finite value that is not negative is finite
/// and not negative.
pub fn horizontal_miss(rx: f64, ry: f64, vx: f64, vy: f64, t_max: f64) -> f64 {
    let (x, y) = closest_point(rx, ry, vx, vy, t_max);
    oa_ieee::sqrt(x * x + y * y)
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
    Some(prediction_of(
        &m,
        horizontal_miss(rx, ry, vx, vy, t_max),
        match vert {
            Some(z) => vertical_miss(z.dz_ft, z.dvz_fps, t_max),
            None => f64::NAN,
        },
    ))
}

/// The prediction of a conflict span `m` and the two smallest separations: the part of
/// [`predict`] after the span and the misses are known, split off so that the spans and misses
/// are proved apart from it; it only copies its inputs and is tested, not proved.
pub fn prediction_of(m: &Span, h_miss_m: f64, v_miss_ft: f64) -> Prediction {
    Prediction {
        ttl_s: if m.now { 0.0 } else { m.lo },
        now: m.now,
        h_miss_m,
        v_miss_ft,
    }
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

    // The proofs that follow split the horizontal solver and the miss at the square root, which
    // the proofs of the arithmetic around them take as a parameter. A composition proof is the
    // whole property (the step, the contract, the last step) only because the production code is
    // exactly that composition (the decision `step_of` and the final `prediction_of` are tested,
    // not proved): `horizontal_span` is `horizontal_step`, then `span_from_root` of
    // `oa_ieee::sqrt(disc)`; `horizontal_miss` is `closest_point`, then `oa_ieee::sqrt` of the sum
    // of squares; `predict` is `conflict_span`, `horizontal_miss` and `vertical_miss`, then
    // `prediction_of` (tested bit for bit against the unsplit code in `tests`).

    /// Bounds of what the first step hands the root step, for the stated input bounds: the
    /// squared relative speed at most 2 (4e3)^2 and the discriminant at most H^2 a, both with
    /// room for rounding; the time of closest approach at most 2 (1e6)(4e3) / (5e-5)^2.
    const A_MAX: f64 = 3.3e7;
    const DISC_MAX: f64 = 4.0e19;
    const TC_MAX: f64 = 4.0e18;
    /// The contract on the square root: for a finite `disc` in `(0, DISC_MAX]`, `root` is finite,
    /// not negative and at most this (sqrt of `DISC_MAX`, with room).
    const ROOT_MAX: f64 = 7.0e9;
    /// The bound on the horizontal coordinates of the closest point: 1e6 + 4e3 * 1e4, with room.
    const POINT_MAX: f64 = 5.0e7;

    // REQ: OA-CA-028
    #[kani::proof]
    fn the_squared_speed_and_the_discriminant_of_bounded_inputs_are_within_their_bounds() {
        let (rx, ry, vx, vy, h, _t) = horizontal_inputs();
        let hh = h * h;
        let a = vx * vx + vy * vy;
        assert!(a >= 0.0 && a <= A_MAX);
        let d = discriminant(rx, ry, vx, vy, hh, a);
        assert!(d.is_finite());
        // The upper bound that the root contract needs (a negative one is no root step).
        assert!(d <= DISC_MAX);
    }

    // REQ: OA-CA-028
    #[kani::proof]
    fn the_approach_time_of_bounded_inputs_is_finite_and_bounded_when_the_speed_is_not_zero() {
        let (rx, ry, vx, vy, _h, _t) = horizontal_inputs();
        let a = vx * vx + vy * vy;
        kani::assume(a >= STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS);
        assert!(within(approach_time(rx, ry, vx, vy, a), TC_MAX));
    }

    // REQ: OA-CA-028
    #[kani::proof]
    fn the_span_of_a_current_pair_from_any_root_within_the_contract_is_current() {
        let (tc, a, t, root): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(within(tc, TC_MAX));
        kani::assume(a >= STATIONARY_SPEED_MPS * STATIONARY_SPEED_MPS && a <= A_MAX);
        kani::assume(t > 0.0 && t <= MAX_LOOK_AHEAD_S);
        // The root contract: finite, not negative, at most ROOT_MAX.
        kani::assume(root >= 0.0 && root <= ROOT_MAX);
        let s = span_from_root(true, tc, a, t, root);
        assert!(s.is_some());
        if let Some(s) = s {
            assert!(s.now && s.lo == 0.0 && 0.0 <= s.hi && s.hi <= t);
        }
    }

    // REQ: OA-CA-028
    #[kani::proof]
    fn the_square_root_of_a_bounded_discriminant_meets_the_root_contract() {
        let disc: f64 = kani::any();
        kani::assume(disc.is_finite() && disc > 0.0 && disc <= DISC_MAX);
        let root = oa_ieee::sqrt(disc);
        assert!(root >= 0.0 && root <= ROOT_MAX);
    }

    // REQ: OA-CA-028
    #[kani::proof]
    fn a_pair_inside_the_vertical_limit_now_has_a_span() {
        let (dz, dvz, v, t): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        kani::assume(within(dz, MAX_ALT_FT) && within(dvz, MAX_ABS_VRATE_FPS));
        kani::assume(v > 0.0 && v <= MAX_ALT_FT);
        kani::assume(t > 0.0 && t <= MAX_LOOK_AHEAD_S);
        if dz < v && -dz < v {
            let s = vertical_span(dz, dvz, v, t);
            assert!(s.is_some());
            if let Some(s) = s {
                assert!(s.now && s.lo == 0.0 && 0.0 <= s.hi && s.hi <= t);
            }
        }
    }

    // The horizontal miss is the square root of the sum of squares of the closest point. The chain
    // is proved in two parts: the closest point is finite with each coordinate at most POINT_MAX
    // (this proof), and the miss of any such point is finite and not negative (the proof of
    // `the_horizontal_miss_is_finite_and_not_negative`, over the real function).
    //
    // REQ: OA-CA-029
    #[kani::proof]
    fn the_closest_point_is_finite_and_bounded() {
        let (rx, ry, vx, vy, _h, t) = horizontal_inputs();
        let (x, y) = closest_point(rx, ry, vx, vy, t);
        assert!(within(x, POINT_MAX) && within(y, POINT_MAX));
        let ct = closest_time(rx, ry, vx, vy, t);
        assert!(ct >= 0.0 && ct <= t);
    }

    // REQ: OA-CA-029
    #[kani::proof]
    fn the_horizontal_miss_is_finite_and_not_negative() {
        // The real `horizontal_miss`, under the inputs of the closest-point proof (the whole
        // chain in one harness, not a lemma over a free point).
        let (rx, ry, vx, vy, _h, t) = horizontal_inputs();
        let m = horizontal_miss(rx, ry, vx, vy, t);
        assert!(m.is_finite() && m >= 0.0);
    }

    // REQ: OA-CA-029
    #[kani::proof]
    fn a_conflict_span_of_any_ordered_horizontal_span_is_ordered_and_finite() {
        // Any ordered horizontal span inside the look-ahead (the solver's own are such: proved
        // in `horizontal_span_is_finite_and_ordered_for_bounded_inputs`), with the vertical part
        // known or unknown: the conflict span `combine_spans` gives is finite, ordered, inside the
        // look-ahead.
        let t: f64 = kani::any();
        kani::assume(t > 0.0 && t <= MAX_LOOK_AHEAD_S);
        let (lo, hi): (f64, f64) = (kani::any(), kani::any());
        kani::assume(0.0 <= lo && lo <= hi && hi <= t);
        let hs = if kani::any() {
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
        // The vertical part known, and unknown (the horizontal span is then the test).
        let known: bool = kani::any();
        let vert = if known { Some(&z) } else { None };
        kani::cover!(hs.is_none(), "no horizontal span");
        kani::cover!(!known, "an unknown vertical part");
        let m = combine_spans(hs, vert, t);
        if let Some(m) = m {
            kani::cover!(
                known && hs.is_some(),
                "a conflict of a horizontal and a vertical span"
            );
            assert!(m.lo.is_finite() && m.hi.is_finite());
            assert!(0.0 <= m.lo && m.lo <= m.hi && m.hi <= t);
        }
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
            kani::cover!(true, "a conflict with the altitude known");
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
