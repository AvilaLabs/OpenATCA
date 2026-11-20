//! The arithmetic at the core of the net: the predicted vertical path, the floor at a point and
//! the decision that a predicted altitude is below it. Core (pure). The proofs at the end hold
//! for the bounds stated on [`crate::MAX_LOOK_AHEAD_S`], [`crate::MAX_ABS_ALT_FT`] and
//! [`crate::MAX_ABS_VRATE_FPM`].

use crate::types::{
    FloorKind, FloorSample, Grade, MsawConfig, SamplePoint, FLOOR_NONE, MAX_ABS_ALT_FT,
    MAX_ABS_LAT_DEG, MAX_CLEARANCE_FT, MAX_RADIUS_M,
};
use oa_safety::geom::polygon_contains;

/// Metres per degree of latitude (mean radius 6,371,008.8 m).
const M_PER_DEG: f64 = 111_194.926_644_558_74;
const RAD_PER_DEG: f64 = 0.017_453_292_519_943_295;
/// Feet and whole numbers beyond this are held to it before they become an `i32`.
const I32_HOLD: f64 = 1.0e9;

/// The altitude predicted at `t_s` seconds, in a straight line at the track's vertical rate:
/// feet. For the bounds above the result is finite (proved).
pub fn predicted_alt_ft(alt_ft: f64, vrate_fpm: f64, t_s: f64) -> f64 {
    alt_ft + vrate_fpm / 60.0 * t_s
}

/// The lowest the straight-line altitude gets over `[t0_s, t1_s]`: the altitude is linear, so it
/// is at one end.
pub fn lowest_alt_ft(alt_ft: f64, vrate_fpm: f64, t0_s: f64, t1_s: f64) -> f64 {
    oa_math::min_f64(
        predicted_alt_ft(alt_ft, vrate_fpm, t0_s),
        predicted_alt_ft(alt_ft, vrate_fpm, t1_s),
    )
}

/// Whole feet rounded down (a predicted altitude is conservative when it is low), held to
/// `+-1e9`. A NaN gives the lowest value, so it can only raise an alert.
// E8: the value is held within the i32 range first, so the cast does not truncate.
#[allow(clippy::cast_possible_truncation)]
pub fn alt_floor_ft(x: f64) -> i32 {
    if x.is_nan() {
        return i32::MIN;
    }
    oa_math::clamp_f64(oa_ieee::floor(x), -I32_HOLD, I32_HOLD) as i32
}

/// A degree as 1e-7 degrees, rounded, held to the range of the coordinate.
// E8: the value is held within the i32 range first, so the cast does not truncate.
#[allow(clippy::cast_possible_truncation)]
pub fn deg_to_e7(x: f64) -> i32 {
    oa_math::clamp_f64(oa_ieee::round(x * 1.0e7), -1.8e9, 1.8e9) as i32
}

/// The floor at a point and what set it. `proc_ft` is the highest published altitude of the
/// procedure areas that contain the point (`None` outside them all). `None` when the point has
/// no terrain or chart coverage (rule 1): no floor is invented.
///
/// The normal floor is the higher of terrain plus its clearance, the obstacle top plus its
/// clearance and the chart altitude. In a procedure area the floor is the published altitude when
/// that is lower than the normal floor: a lower floor in that area only, and never a higher one.
/// (A published procedure altitude already protects its path from terrain and obstacles, which is
/// why the clearances do not apply under it.)
pub fn floor_of(
    s: &FloorSample,
    proc_ft: Option<i32>,
    cfg: &MsawConfig,
) -> Option<(i32, FloorKind)> {
    if s.terrain_ft == FLOOR_NONE || s.mva_ft == FLOOR_NONE {
        return None;
    }
    let mut best = (
        s.terrain_ft.saturating_add(cfg.terrain_clearance_ft),
        FloorKind::Terrain,
    );
    if s.obstacle_ft != FLOOR_NONE {
        let o = s.obstacle_ft.saturating_add(cfg.obstacle_clearance_ft);
        if o > best.0 {
            best = (o, FloorKind::Obstacle);
        }
    }
    if s.mva_ft > best.0 {
        best = (s.mva_ft, FloorKind::Mva);
    }
    match proc_ft {
        Some(p) if p < best.0 => Some((p, FloorKind::Procedure)),
        _ => Some(best),
    }
}

/// True if a predicted altitude is below the floor: a violation. Monotone: a higher floor never
/// removes one (proved).
pub fn violates(alt_ft: i32, floor_ft: i32) -> bool {
    alt_ft < floor_ft
}

/// The highest published altitude of the procedure areas of `cfg` that contain the point.
pub fn proc_floor_at(cfg: &MsawConfig, lat_e7: i32, lon_e7: i32) -> Option<i32> {
    let mut best: Option<i32> = None;
    for p in cfg.procs.iter().take(cfg.n_procs) {
        if polygon_contains(&p.area, lat_e7, lon_e7) {
            best = Some(best.map_or(p.floor_ft, |b| b.max(p.floor_ft)));
        }
    }
    best
}

/// The time of the sample point `k` of `n` over `look_ahead_s`, seconds.
pub fn sample_time_s(k: usize, n: usize, look_ahead_s: f64) -> f64 {
    let steps = n.saturating_sub(1);
    if steps == 0 {
        return 0.0;
    }
    look_ahead_s * (k as f64) / (steps as f64)
}

/// The time span the point `k` stands for: half a step either side of its time, held to the
/// look-ahead. The point's floor is judged against the lowest altitude of the span.
pub fn sample_span_s(k: usize, n: usize, look_ahead_s: f64) -> (f64, f64) {
    let steps = n.saturating_sub(1);
    if steps == 0 {
        return (0.0, look_ahead_s);
    }
    let half = look_ahead_s / (steps as f64) / 2.0;
    let t = sample_time_s(k, n, look_ahead_s);
    (
        oa_math::max_f64(0.0, t - half),
        oa_math::min_f64(look_ahead_s, t + half),
    )
}

/// The sample points of a straight-line path: `out` is filled with `cfg.samples` points (fewer
/// if `out` is shorter) and the count is returned. `None` if the row cannot be projected (a
/// position beyond the latitude of a flat scale, a velocity that is not finite). The radius of a
/// point is the 95 % radius plus half the distance to the next point.
pub fn sample_points(t: &PathStart, cfg: &MsawConfig, out: &mut [SamplePoint]) -> Option<usize> {
    if !(t.lat_deg.is_finite()
        && t.lon_deg.is_finite()
        && t.lat_deg.abs() <= MAX_ABS_LAT_DEG
        && t.lon_deg.abs() <= 180.0
        && t.ve.is_finite()
        && t.vn.is_finite())
    {
        return None;
    }
    let n = cfg.samples.min(out.len());
    let cos_lat = oa_math::cos(t.lat_deg * RAD_PER_DEG);
    if cos_lat.is_nan() || cos_lat <= 0.0 {
        return None;
    }
    let speed = oa_math::hypot(t.ve, t.vn);
    let step_s = if cfg.samples > 1 {
        cfg.look_ahead_s / ((cfg.samples - 1) as f64)
    } else {
        0.0
    };
    let radius = oa_math::min_f64(t.r95_m + speed * step_s / 2.0, MAX_RADIUS_M);
    for (k, slot) in out.iter_mut().take(n).enumerate() {
        let ts = sample_time_s(k, cfg.samples, cfg.look_ahead_s);
        let lat = t.lat_deg + t.vn * ts / M_PER_DEG;
        let lon = t.lon_deg + t.ve * ts / (M_PER_DEG * cos_lat);
        *slot = SamplePoint {
            lat_e7: deg_to_e7(lat),
            lon_e7: deg_to_e7(lon),
            radius_m: radius as f32,
        };
    }
    Some(n)
}

/// A track's start of path in the units the arithmetic uses (the cycle fills it from the row;
/// public so that a test can project a path).
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathStart {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// East and north velocity, metres per second.
    pub ve: f64,
    pub vn: f64,
    /// The 95 % radius already held to the cap, metres.
    pub r95_m: f64,
}

/// The time at which a straight-line altitude first falls below `floor_ft` within
/// `[lo_s, hi_s]`, given that its lowest value there is below it: `lo_s` if it is already below
/// at `lo_s`, else the crossing, held to the span. Always inside the span.
pub fn violation_time_s(alt_ft: f64, vrate_fpm: f64, floor_ft: i32, lo_s: f64, hi_s: f64) -> f64 {
    let at_lo = predicted_alt_ft(alt_ft, vrate_fpm, lo_s);
    let f = f64::from(floor_ft);
    if at_lo < f || vrate_fpm >= 0.0 {
        return lo_s;
    }
    let fps = -vrate_fpm / 60.0;
    // A rate that rounds to nothing has no crossing to find: the earliest time, the urgent
    // side (the proof found the division 0/0 for a denormal rate).
    if fps.is_nan() || fps <= 1.0e-9 {
        return lo_s;
    }
    let t = lo_s + (at_lo - f) / fps;
    oa_math::clamp_f64(t, lo_s, hi_s)
}

/// The grade of a violation `ttv_s` seconds ahead: a warning within `warning_s`, else a caution.
pub fn grade_of(ttv_s: f64, warning_s: f64) -> Grade {
    if ttv_s <= warning_s {
        Grade::Warning
    } else {
        Grade::Caution
    }
}

/// The largest clearance or floor magnitude the configuration may state is in `i32` range with
/// room for the sum with a terrain elevation.
const _: () = assert!(MAX_CLEARANCE_FT < 1_000_000);
const _: () = assert!(MAX_ABS_ALT_FT < 1.0e9);

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;
    use crate::types::{ProcArea, MAX_ABS_VRATE_FPM, MAX_LOOK_AHEAD_S, MAX_PROCS};
    use oa_safety::geom::POLYGON_EMPTY;

    fn cfg_with(tc: i32, oc: i32) -> MsawConfig {
        MsawConfig {
            max_tracks: 1,
            max_open: 1,
            track_budget: 1,
            look_ahead_s: 60.0,
            warning_s: 30.0,
            samples: 2,
            hysteresis_ns: 1,
            terrain_clearance_ft: tc,
            obstacle_clearance_ft: oc,
            max_speed_mps: 300.0,
            r95_cap_m: 1000.0,
            procs: [ProcArea {
                area: POLYGON_EMPTY,
                floor_ft: 0,
            }; MAX_PROCS],
            n_procs: 0,
        }
    }

    // REQ: OA-MSAW-020
    #[kani::proof]
    fn predicted_altitude_is_finite_inside_the_look_ahead_for_bounded_inputs() {
        let (alt, vr, t): (f64, f64, f64) = (kani::any(), kani::any(), kani::any());
        kani::assume(alt.is_finite() && alt >= -MAX_ABS_ALT_FT && alt <= MAX_ABS_ALT_FT);
        kani::assume(vr.is_finite() && vr >= -MAX_ABS_VRATE_FPM && vr <= MAX_ABS_VRATE_FPM);
        kani::assume(t >= 0.0 && t <= MAX_LOOK_AHEAD_S);
        assert!(predicted_alt_ft(alt, vr, t).is_finite());
    }

    // REQ: OA-MSAW-021
    #[kani::proof]
    fn a_higher_floor_never_removes_an_alert() {
        let (alt, t1, t2, o1, o2, m1, m2): (i32, i32, i32, i32, i32, i32, i32) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        let (tc, oc): (i32, i32) = (kani::any(), kani::any());
        kani::assume(tc >= 0 && tc <= MAX_CLEARANCE_FT);
        kani::assume(oc >= 0 && oc <= MAX_CLEARANCE_FT);
        let p: i32 = kani::any();
        let use_proc: bool = kani::any();
        let proc_ft = if use_proc { Some(p) } else { None };
        // The second sample is at least as high in every input, none without coverage.
        kani::assume(t1 != FLOOR_NONE && m1 != FLOOR_NONE);
        kani::assume(t1 <= t2 && m1 <= m2);
        kani::assume(o1 == FLOOR_NONE || (o2 != FLOOR_NONE && o1 <= o2));
        let cfg = cfg_with(tc, oc);
        let s1 = FloorSample {
            terrain_ft: t1,
            obstacle_ft: o1,
            mva_ft: m1,
        };
        let s2 = FloorSample {
            terrain_ft: t2,
            obstacle_ft: o2,
            mva_ft: m2,
        };
        if let (Some((f1, _)), Some((f2, _))) =
            (floor_of(&s1, proc_ft, &cfg), floor_of(&s2, proc_ft, &cfg))
        {
            assert!(f1 <= f2);
            if violates(alt, f1) {
                assert!(violates(alt, f2));
            }
        } else {
            assert!(false);
        }
    }

    // REQ: OA-MSAW-022
    #[kani::proof]
    fn a_violation_time_stays_inside_its_span_for_bounded_inputs() {
        let (alt, vr, lo, hi): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        let floor: i32 = kani::any();
        kani::assume(alt.is_finite() && alt >= -MAX_ABS_ALT_FT && alt <= MAX_ABS_ALT_FT);
        kani::assume(vr.is_finite() && vr >= -MAX_ABS_VRATE_FPM && vr <= MAX_ABS_VRATE_FPM);
        kani::assume(lo >= 0.0 && lo <= hi && hi <= MAX_LOOK_AHEAD_S);
        kani::assume(floor >= -MAX_CLEARANCE_FT && floor <= MAX_CLEARANCE_FT);
        let t = violation_time_s(alt, vr, floor, lo, hi);
        assert!(t >= lo && t <= hi);
    }
}
