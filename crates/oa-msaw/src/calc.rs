//! The arithmetic at the core of the net: the predicted vertical path, the floor at a point and
//! the decision that a predicted altitude is below it. Core (pure). The proofs at the end hold
//! for the bounds stated on [`crate::MAX_LOOK_AHEAD_S`], [`crate::MAX_ABS_ALT_FT`] and
//! [`crate::MAX_ABS_VRATE_FPM`].

use crate::types::{
    Corridor, FloorKind, FloorSample, Grade, MsawConfig, SamplePoint, FLOOR_NONE, MAX_ABS_ALT_FT,
    MAX_ABS_LAT_DEG, MAX_CLEARANCE_FT, MAX_FLOOR_FT, MAX_RADIUS_M,
};

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

/// The floor at a point and what set it. `surface_ft` is the approach surface of the corridor the
/// point's whole query circle lies inside (`None` outside every corridor, see
/// [`corridor_surface_at`]). `None` when the point has no coverage (rule 1): no floor is invented.
///
/// Outside the corridors the floor is the higher of terrain plus its clearance, the obstacle top
/// plus its clearance and the chart altitude. Inside one the chart altitude does not apply and
/// the floor is the higher of the surface and terrain or the obstacle top plus the approach
/// clearance: terrain and obstacles protect an approach always. Terrain, the obstacle data and
/// (outside a corridor only) the chart must all be there.
pub fn floor_of(
    s: &FloorSample,
    surface_ft: Option<i32>,
    cfg: &MsawConfig,
) -> Option<(i32, FloorKind)> {
    if s.terrain_ft == FLOOR_NONE || !s.obstacle_covered {
        return None;
    }
    let (tc, oc) = match surface_ft {
        Some(_) => (cfg.approach_min_clearance_ft, cfg.approach_min_clearance_ft),
        None => (cfg.terrain_clearance_ft, cfg.obstacle_clearance_ft),
    };
    let mut best = (s.terrain_ft.saturating_add(tc), FloorKind::Terrain);
    if s.obstacle_ft != FLOOR_NONE {
        let o = s.obstacle_ft.saturating_add(oc);
        if o > best.0 {
            best = (o, FloorKind::Obstacle);
        }
    }
    match surface_ft {
        Some(a) => {
            if a > best.0 {
                best = (a, FloorKind::Procedure);
            }
        }
        None => {
            if s.mva_ft == FLOOR_NONE {
                return None;
            }
            if s.mva_ft > best.0 {
                best = (s.mva_ft, FloorKind::Mva);
            }
        }
    }
    Some(best)
}

/// True if a predicted altitude is below the floor: a violation. Monotone: a higher floor never
/// removes one (proved).
pub fn violates(alt_ft: i32, floor_ft: i32) -> bool {
    alt_ft < floor_ft
}

/// Whole feet rounded up, held to `+-1e9` (a surface is conservative when it is high).
// E8: the value is held within the i32 range first, so the cast does not truncate.
#[allow(clippy::cast_possible_truncation)]
fn alt_ceil_ft(x: f64) -> i32 {
    if x.is_nan() {
        return i32::MAX;
    }
    oa_math::clamp_f64(oa_ieee::ceil(x), -I32_HOLD, I32_HOLD) as i32
}

/// The approach surface of corridor `c` for a query circle at (`lat_e7`, `lon_e7`) of `radius_m`:
/// `Some` only if the whole circle lies inside the corridor, then the surface at the circle's far
/// side along the centreline (the highest it reaches), rounded up. The surface is a straight line
/// from the threshold elevation at the threshold to the published altitude at the fix lowered by
/// `allowance_ft` at the far end. The circle is inside when its centre is at least `radius_m`
/// from every one of the four edges.
pub fn corridor_surface_ft(
    c: &Corridor,
    lat_e7: i32,
    lon_e7: i32,
    radius_m: f64,
    allowance_ft: i32,
) -> Option<i32> {
    let thr_lat = f64::from(c.thr_lat_e7) * 1.0e-7;
    let cos_lat = oa_math::cos(thr_lat * RAD_PER_DEG);
    let (sin_b, cos_b) = oa_math::sin_cos(c.out_bearing_deg * RAD_PER_DEG);
    let dy = f64::from(lat_e7.saturating_sub(c.thr_lat_e7)) * 1.0e-7 * M_PER_DEG;
    let dx = f64::from(lon_e7.saturating_sub(c.thr_lon_e7)) * 1.0e-7 * M_PER_DEG * cos_lat;
    let along = dx * sin_b + dy * cos_b;
    let across = dx * cos_b - dy * sin_b;
    let cross = if across < 0.0 { -across } else { across };
    if !(along.is_finite() && cross.is_finite() && radius_m >= 0.0) {
        return None;
    }
    if along < radius_m || c.length_m - along < radius_m {
        return None;
    }
    let k = (c.far_half_width_m - c.near_half_width_m) / c.length_m;
    let half = c.near_half_width_m + k * along;
    let side = (half - cross) / oa_ieee::sqrt(1.0 + k * k);
    if side.is_nan() || side < radius_m {
        return None;
    }
    let far = oa_math::min_f64(along + radius_m, c.length_m);
    let top = f64::from(c.faf_alt_ft) - f64::from(allowance_ft);
    let base = f64::from(c.thr_elev_ft);
    Some(alt_ceil_ft(base + (top - base) * far / c.length_m))
}

/// The approach surface at a point: the highest surface of the corridors of `cfg` that hold the
/// point's whole query circle, `None` if no corridor does.
pub fn corridor_surface_at(cfg: &MsawConfig, p: &SamplePoint) -> Option<i32> {
    let mut best: Option<i32> = None;
    for c in cfg.corridors.iter().take(cfg.n_corridors) {
        if let Some(v) = corridor_surface_ft(
            c,
            p.lat_e7,
            p.lon_e7,
            f64::from(p.radius_m),
            cfg.approach_allowance_ft,
        ) {
            best = Some(best.map_or(v, |b| b.max(v)));
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
/// point is the 95 % radius plus the velocity uncertainty times the prediction time of the point,
/// plus half the distance to the next point.
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
    for (k, slot) in out.iter_mut().take(n).enumerate() {
        let ts = sample_time_s(k, cfg.samples, cfg.look_ahead_s);
        let radius = oa_math::min_f64(
            t.r95_m + cfg.velocity_uncertainty_mps * ts + speed * step_s / 2.0,
            MAX_RADIUS_M,
        );
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
const _: () = assert!(MAX_FLOOR_FT == 1_000_000 + MAX_CLEARANCE_FT);
const _: () = assert!(MAX_ABS_ALT_FT < 1.0e9);

#[cfg(kani)]
mod proofs {
    //! Kani proof harnesses (docs/VERIFICATION.md). Compiled only under `cargo kani`.
    use super::*;
    use crate::types::{MAX_ABS_VRATE_FPM, MAX_CORRIDORS, MAX_LOOK_AHEAD_S};

    fn cfg_with(tc: i32, oc: i32, amc: i32) -> MsawConfig {
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
            approach_allowance_ft: 0,
            approach_min_clearance_ft: amc,
            velocity_uncertainty_mps: 0.0,
            corridors: [Corridor {
                thr_lat_e7: 0,
                thr_lon_e7: 0,
                out_bearing_deg: 0.0,
                length_m: 1.0,
                near_half_width_m: 1.0,
                far_half_width_m: 1.0,
                thr_elev_ft: 0,
                faf_alt_ft: 0,
            }; MAX_CORRIDORS],
            n_corridors: 0,
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
        let (tc, oc, amc): (i32, i32, i32) = (kani::any(), kani::any(), kani::any());
        kani::assume(tc >= 0 && tc <= MAX_CLEARANCE_FT);
        kani::assume(oc >= 0 && oc <= MAX_CLEARANCE_FT);
        kani::assume(amc >= 0 && amc <= MAX_CLEARANCE_FT);
        // The same corridor surface (or none) at both points: the corridor and the normal floor
        // are both covered, the surface is a value the geometry gives and is held fixed.
        let a: i32 = kani::any();
        let use_surface: bool = kani::any();
        let surface = if use_surface { Some(a) } else { None };
        // The second sample is at least as high in every input, none without coverage.
        kani::assume(t1 != FLOOR_NONE && m1 != FLOOR_NONE);
        kani::assume(t1 <= t2 && m1 <= m2);
        kani::assume(o1 == FLOOR_NONE || (o2 != FLOOR_NONE && o1 <= o2));
        let cfg = cfg_with(tc, oc, amc);
        let s1 = FloorSample {
            terrain_ft: t1,
            obstacle_ft: o1,
            obstacle_covered: true,
            mva_ft: m1,
        };
        let s2 = FloorSample {
            terrain_ft: t2,
            obstacle_ft: o2,
            obstacle_covered: true,
            mva_ft: m2,
        };
        if let (Some((f1, _)), Some((f2, _))) =
            (floor_of(&s1, surface, &cfg), floor_of(&s2, surface, &cfg))
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
    // The domain proved: any finite altitude within MAX_ABS_ALT_FT, any finite vertical rate
    // within MAX_ABS_VRATE_FPM, any span inside [0, MAX_LOOK_AHEAD_S], and any floor the net can
    // compute, which is MAX_FLOOR_FT in magnitude (terrain held to a million feet plus the largest
    // clearance).
    #[kani::proof]
    fn a_violation_time_stays_inside_its_span_for_bounded_inputs() {
        let (alt, vr, lo, hi): (f64, f64, f64, f64) =
            (kani::any(), kani::any(), kani::any(), kani::any());
        let floor: i32 = kani::any();
        kani::assume(alt.is_finite() && alt >= -MAX_ABS_ALT_FT && alt <= MAX_ABS_ALT_FT);
        kani::assume(vr.is_finite() && vr >= -MAX_ABS_VRATE_FPM && vr <= MAX_ABS_VRATE_FPM);
        kani::assume(lo >= 0.0 && lo <= hi && hi <= MAX_LOOK_AHEAD_S);
        kani::assume(floor >= -MAX_FLOOR_FT && floor <= MAX_FLOOR_FT);
        let t = violation_time_s(alt, vr, floor, lo, hi);
        assert!(t >= lo && t <= hi);
    }

    // REQ: OA-MSAW-043
    #[kani::proof]
    fn the_corridor_floor_is_never_below_terrain_or_obstacles_plus_the_approach_clearance() {
        let (t, o, m, a, amc): (i32, i32, i32, i32, i32) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        let covered: bool = kani::any();
        kani::assume(amc >= 0 && amc <= MAX_CLEARANCE_FT);
        let cfg = cfg_with(0, 0, amc);
        let s = FloorSample {
            terrain_ft: t,
            obstacle_ft: o,
            obstacle_covered: covered,
            mva_ft: m,
        };
        if let Some((f, _)) = floor_of(&s, Some(a), &cfg) {
            // An answer needs terrain and the obstacle data, and then it holds the floor above
            // both (the chart altitude is not a condition inside a corridor).
            assert!(t != FLOOR_NONE && covered);
            assert!(f >= t.saturating_add(amc));
            assert!(f >= a);
            if o != FLOOR_NONE {
                assert!(f >= o.saturating_add(amc));
            }
        } else {
            assert!(t == FLOOR_NONE || !covered);
        }
    }
}
