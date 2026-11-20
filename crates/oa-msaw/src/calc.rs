//! The arithmetic at the core of the net: the predicted vertical path, the floor at a point and
//! the decision that a predicted altitude is below it. Core (pure). The proofs at the end hold
//! for the bounds stated on [`crate::MAX_LOOK_AHEAD_S`], [`crate::MAX_ABS_ALT_FT`] and
//! [`crate::MAX_ABS_VRATE_FPM`].

use crate::types::{
    Corridor, CorridorHit, FloorKind, FloorSample, Grade, MsawConfig, SamplePoint, CK_DEPARTURE,
    CK_RUNWAY, FLOOR_NONE, KB_APPROACH, KB_DEPARTURE, KB_RUNWAY, MAX_ABS_ALT_FT, MAX_ABS_LAT_DEG,
    MAX_CLEARANCE_FT, MAX_FLOOR_FT, MAX_RADIUS_M,
};

/// Metres per degree of latitude (mean radius 6,371,008.8 m).
pub(crate) const M_PER_DEG: f64 = 111_194.926_644_558_74;
pub(crate) const RAD_PER_DEG: f64 = 0.017_453_292_519_943_295;
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

/// The alert floor at a point and what set it. `hit` is what the corridors say of the point (`None`
/// outside every corridor whose whole query circle holds it, see [`corridor_hit_at`]). `None`
/// when the point has no terrain or obstacle coverage (rule 1): no floor is invented.
///
/// Outside the corridors the floor is the higher of terrain plus its clearance (the point's own
/// where an area sets one, else the configuration's) and the obstacle top plus its clearance; the
/// chart altitude is not a floor anywhere. Inside a corridor it is the higher of the surface, if the
/// corridor has one, and terrain or the obstacle top plus the corridor's clearance.
pub fn floor_of(
    s: &FloorSample,
    hit: Option<CorridorHit>,
    cfg: &MsawConfig,
) -> Option<(i32, FloorKind)> {
    if s.terrain_ft == FLOOR_NONE || !s.obstacle_covered {
        return None;
    }
    // A sample that [`with_airport_area`] has resolved holds its area's clearance and no ceiling:
    // the clearance then applies to obstacles as it does to terrain.
    let in_area = s.apt_clearance_ft != FLOOR_NONE && s.apt_ceiling_ft == FLOOR_NONE;
    let (tc, oc) = match hit {
        Some(h) => (h.clearance_ft, h.clearance_ft),
        None => (
            if s.gtm_clearance_ft == FLOOR_NONE {
                cfg.terrain_clearance_ft
            } else {
                s.gtm_clearance_ft
            },
            if in_area {
                s.apt_clearance_ft
            } else {
                cfg.obstacle_clearance_ft
            },
        ),
    };
    let mut best = (s.terrain_ft.saturating_add(tc), FloorKind::Terrain);
    if s.obstacle_ft != FLOOR_NONE {
        let o = s.obstacle_ft.saturating_add(oc);
        if o > best.0 {
            best = (o, FloorKind::Obstacle);
        }
    }
    if let Some(a) = hit.and_then(|h| h.surface_ft) {
        if a > best.0 {
            best = (a, FloorKind::Procedure);
        }
    }
    // A clearance the adaptation sets for an area (not the airport area's) is a lower bound inside
    // a corridor too: a deliberate site setting is never undercut.
    if hit.is_some() && s.gtm_clearance_ft != FLOOR_NONE && !in_area {
        let g = s.terrain_ft.saturating_add(s.gtm_clearance_ft);
        if g > best.0 {
            best = (g, FloorKind::Terrain);
        }
    }
    Some(best)
}

/// The sample with the airport terminal area's clearance in place of the general one: where an area
/// holds the point, no `gtm_clearance` line does, and the lowest predicted altitude of the span is
/// below the area's ceiling. Never an inhibit: the floor stays terrain and obstacles plus the
/// area's clearance, the same for both. The returned sample marks the area as applied by dropping
/// its ceiling.
pub fn with_airport_area(s: &FloorSample, low_ft: i32) -> FloorSample {
    if s.gtm_clearance_ft == FLOOR_NONE
        && s.apt_clearance_ft != FLOOR_NONE
        && s.apt_ceiling_ft != FLOOR_NONE
        && low_ft < s.apt_ceiling_ft
    {
        FloorSample {
            gtm_clearance_ft: s.apt_clearance_ft,
            apt_ceiling_ft: FLOOR_NONE,
            ..*s
        }
    } else {
        *s
    }
}

/// True if a track's altitude is too old to be taken as it is: older than `max_age_s`. An age that
/// is not known (NaN) is taken as fresh (and counted by the cycle); infinity is stale.
pub fn altitude_stale(age_s: f32, max_age_s: f64) -> bool {
    f64::from(age_s) > max_age_s
}

/// How a track's altitude is used, from its age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltUse {
    /// Judged as it is.
    Fresh,
    /// Stale within the hold: judged at an extrapolated altitude, and said so.
    Extrapolated,
    /// Past the hold: not monitored, and said so, never read as clear.
    Lost,
}

/// The use of an altitude of age `age_s` (NaN: unknown, fresh) with the limits `max_s` and `hold_s`.
pub fn altitude_use(age_s: f32, max_s: f64, hold_s: f64) -> AltUse {
    let a = f64::from(age_s);
    if a > hold_s {
        AltUse::Lost
    } else if a > max_s {
        AltUse::Extrapolated
    } else {
        AltUse::Fresh
    }
}

/// The altitude a track is judged at after `age_s` seconds without a new measurement: the altitude
/// carried on at its vertical rate if that is a descent, never a climb. An unknown rate (NaN) is
/// carried on as a descent of [`UNKNOWN_RATE_DESCENT_FPM`], the conservative side.
pub fn extrapolated_alt_ft(alt_ft: f64, vrate_fpm: f64, age_s: f64) -> f64 {
    let vr = if vrate_fpm.is_nan() {
        -UNKNOWN_RATE_DESCENT_FPM
    } else {
        vrate_fpm
    };
    oa_math::min_f64(alt_ft, predicted_alt_ft(alt_ft, vr, age_s))
}

/// The descent a stale altitude of unknown rate is carried on at, feet per minute.
pub const UNKNOWN_RATE_DESCENT_FPM: f64 = 1000.0;

/// How far from the pavement edge a point may be and still be over the runway, metres.
pub const PAVEMENT_MARGIN_M: f64 = 60.0;
/// The vertical rate above which a track is climbing and no landing, feet per minute.
pub const CLIMB_FPM: f64 = 200.0;
/// An altitude change over 10 s within which a track is level, feet.
pub const LEVEL_FT: f64 = 50.0;

/// True if a track can be landing: not climbing. Its rate is at most [`CLIMB_FPM`], or it has
/// stayed within [`LEVEL_FT`] over the last 10 s; a rate that is not known and no level evidence
/// is not a landing.
pub fn not_climbing(vrate_known: bool, vrate_fpm: f64, change_10s_ft: f64) -> bool {
    (vrate_known && vrate_fpm <= CLIMB_FPM) || (-LEVEL_FT..=LEVEL_FT).contains(&change_10s_ft)
}

/// The runway elevation at a point, feet (rounded down), if the point itself is over the pavement
/// of the runway strip `c`: along the runway between its threshold and its far end, and within the
/// pavement half-width plus [`PAVEMENT_MARGIN_M`] of the centreline. The strip's wider margin gives
/// no landing. The elevation is interpolated along the runway between the elevations of its two
/// ends.
pub fn runway_elev_at(c: &Corridor, lat_e7: i32, lon_e7: i32) -> Option<i32> {
    if c.kind != CK_RUNWAY {
        return None;
    }
    let thr_lat = f64::from(c.thr_lat_e7) * 1.0e-7;
    let cos_lat = oa_math::cos(thr_lat * RAD_PER_DEG);
    let (sin_b, cos_b) = oa_math::sin_cos(c.out_bearing_deg * RAD_PER_DEG);
    let dy = f64::from(lat_e7.saturating_sub(c.thr_lat_e7)) * 1.0e-7 * M_PER_DEG;
    let dx = f64::from(lon_e7.saturating_sub(c.thr_lon_e7)) * 1.0e-7 * M_PER_DEG * cos_lat;
    let along = dx * sin_b + dy * cos_b;
    let signed = dx * cos_b - dy * sin_b;
    let across = if signed < 0.0 { -signed } else { signed };
    let run = c.length_m - c.land_from_m;
    if !(along.is_finite() && across.is_finite() && run > 0.0)
        || along < c.land_from_m
        || along > c.length_m
        || across > c.pave_half_m + PAVEMENT_MARGIN_M
    {
        return None;
    }
    let f = (along - c.land_from_m) / run;
    let e = f64::from(c.thr_elev_ft) + (f64::from(c.far_elev_ft) - f64::from(c.thr_elev_ft)) * f;
    Some(alt_floor_ft(e))
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

/// What corridor `c` says of a query circle at (`lat_e7`, `lon_e7`) of `radius_m`: `Some` only if
/// the whole circle lies inside the corridor, then the corridor's clearance and its surface at
/// the circle's centre along the centreline, rounded up. An approach surface is the procedure's
/// own path, the threshold crossing height over the threshold elevation rising at the published
/// angle, lowered by the allowance; a runway surface is the threshold elevation; a departure
/// corridor has none. The circle is inside when its centre is at least
/// `radius_m` from every one of the four edges.
pub fn corridor_hit_ft(
    c: &Corridor,
    lat_e7: i32,
    lon_e7: i32,
    radius_m: f64,
    cfg: &MsawConfig,
) -> Option<CorridorHit> {
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
    Some(match c.kind {
        CK_DEPARTURE => CorridorHit {
            surface_ft: None,
            clearance_ft: cfg.departure_min_clearance_ft,
            kinds: KB_DEPARTURE,
        },
        // The runway strip: an aircraft on final is necessarily close to the ground, so no
        // clearance is added over terrain or obstacles and the surface is the runway elevation
        // less the allowance. A landing is judged on the point (see [`runway_elev_at`]), not here.
        CK_RUNWAY => CorridorHit {
            surface_ft: Some(c.thr_elev_ft.saturating_sub(cfg.approach_allowance_ft)),
            clearance_ft: 0,
            kinds: KB_RUNWAY,
        },
        _ => {
            // The procedure's own path: the threshold crossing height over the threshold elevation,
            // rising at the published angle, less the allowance, at the centre of the circle (the allowance
            // is the margin for the along-track uncertainty).
            let slope = oa_math::tan(c.path_angle_deg * RAD_PER_DEG);
            let path = f64::from(c.thr_elev_ft) + f64::from(c.tch_ft) + slope * along / FT_M
                - f64::from(cfg.approach_allowance_ft);
            let surface = alt_ceil_ft(path);
            if along + radius_m <= FINAL_ZONE_M {
                // The last 1 NM of the box: no clearance over terrain or obstacles.
                CorridorHit {
                    surface_ft: Some(surface),
                    clearance_ft: 0,
                    kinds: KB_APPROACH,
                }
            } else {
                CorridorHit {
                    surface_ft: Some(surface),
                    clearance_ft: cfg.approach_min_clearance_ft,
                    kinds: KB_APPROACH,
                }
            }
        }
    })
}

/// The last stretch of a capture box before the threshold, metres (1 NM), where the floor takes no
/// clearance over terrain or obstacles.
pub const FINAL_ZONE_M: f64 = 1852.0;
/// Metres per foot.
const FT_M: f64 = 0.3048;

/// The union of two hits: the higher surface and the higher clearance.
pub(crate) fn hit_union(a: Option<CorridorHit>, b: CorridorHit) -> CorridorHit {
    match a {
        None => b,
        Some(a) => CorridorHit {
            surface_ft: match (a.surface_ft, b.surface_ft) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, None) => x,
                (None, y) => y,
            },
            clearance_ft: a.clearance_ft.max(b.clearance_ft),
            kinds: a.kinds | b.kinds,
        },
    }
}

/// What the corridors of `cfg` say of a point: the union of the hits of every corridor that
/// holds the point's whole query circle, `None` if none does. Looks at every corridor; the net
/// uses [`crate::corridor_hit_at`], which looks at those whose cell holds the point and gives the
/// same answer (tested against this).
pub fn corridor_hit_linear(cfg: &MsawConfig, p: &SamplePoint) -> Option<CorridorHit> {
    let mut best: Option<CorridorHit> = None;
    for c in cfg.corridors.iter().take(cfg.n_corridors) {
        if let Some(h) = corridor_hit_ft(c, p.lat_e7, p.lon_e7, f64::from(p.radius_m), cfg) {
            best = Some(hit_union(best, h));
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
    use crate::types::{CK_APPROACH, MAX_ABS_VRATE_FPM, MAX_CORRIDORS, MAX_LOOK_AHEAD_S};

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
            departure_min_clearance_ft: amc,
            velocity_uncertainty_mps: 0.0,
            max_altitude_age_s: 12.0,
            stale_hold_s: 30.0,
            altitude_loss_degrade_share: 0.5,
            terminal_lookahead_s: 15.0,
            corridors: [Corridor {
                kind: CK_APPROACH,
                thr_lat_e7: 0,
                thr_lon_e7: 0,
                out_bearing_deg: 0.0,
                length_m: 1.0,
                near_half_width_m: 1.0,
                far_half_width_m: 1.0,
                thr_elev_ft: 0,
                faf_alt_ft: 0,
                land_from_m: 0.0,
                pave_half_m: 0.0,
                far_elev_ft: 0,
                path_angle_deg: 3.0,
                tch_ft: 50,
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
        let in_corridor: bool = kani::any();
        let hit = if in_corridor {
            Some(CorridorHit {
                surface_ft: if use_surface { Some(a) } else { None },
                clearance_ft: amc,
                kinds: 0,
            })
        } else {
            None
        };
        // The second sample is at least as high in every input, none without coverage.
        kani::assume(t1 != FLOOR_NONE && m1 != FLOOR_NONE);
        kani::assume(t1 <= t2 && m1 <= m2);
        kani::assume(o1 == FLOOR_NONE || (o2 != FLOOR_NONE && o1 <= o2));
        // The same area clearance (or none) at both points, the chart altitude not a floor.
        let g: i32 = kani::any();
        kani::assume(g == FLOOR_NONE || (g >= 0 && g <= MAX_CLEARANCE_FT));
        let cfg = cfg_with(tc, oc, amc);
        let s1 = FloorSample {
            terrain_ft: t1,
            obstacle_ft: o1,
            obstacle_covered: true,
            mva_ft: m1,
            gtm_clearance_ft: g,
            apt_clearance_ft: FLOOR_NONE,
            apt_ceiling_ft: FLOOR_NONE,
        };
        let s2 = FloorSample {
            terrain_ft: t2,
            obstacle_ft: o2,
            obstacle_covered: true,
            mva_ft: m2,
            gtm_clearance_ft: g,
            apt_clearance_ft: FLOOR_NONE,
            apt_ceiling_ft: FLOOR_NONE,
        };
        if let (Some((f1, _)), Some((f2, _))) = (floor_of(&s1, hit, &cfg), floor_of(&s2, hit, &cfg))
        {
            assert!(f1 <= f2);
            if violates(alt, f1) {
                assert!(violates(alt, f2));
            }
        } else {
            assert!(false);
        }
    }

    // The domain proved: any finite altitude within MAX_ABS_ALT_FT, any finite vertical rate
    // within MAX_ABS_VRATE_FPM, any span inside [0, MAX_LOOK_AHEAD_S], and any floor the net can
    // compute, which is MAX_FLOOR_FT in magnitude (terrain held to a million feet plus the largest
    // clearance).
    // REQ: OA-MSAW-022
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
            gtm_clearance_ft: FLOOR_NONE,
            apt_clearance_ft: FLOOR_NONE,
            apt_ceiling_ft: FLOOR_NONE,
        };
        let hit = CorridorHit {
            surface_ft: Some(a),
            clearance_ft: amc,
            kinds: 0,
        };
        if let Some((f, _)) = floor_of(&s, Some(hit), &cfg) {
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

    // REQ: OA-MSAW-059
    #[kani::proof]
    fn the_airport_area_floor_is_never_below_terrain_or_obstacles_plus_the_area_clearance() {
        let (t, o, ac, ak, low, tc): (i32, i32, i32, i32, i32, i32) = (
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
            kani::any(),
        );
        kani::assume(ac >= 0 && ac <= MAX_CLEARANCE_FT);
        kani::assume(tc >= 0 && tc <= MAX_CLEARANCE_FT);
        // Inside the area (below its ceiling) with no explicit gtm_clearance.
        kani::assume(ak != FLOOR_NONE && low < ak);
        let s = FloorSample {
            terrain_ft: t,
            obstacle_ft: o,
            obstacle_covered: true,
            mva_ft: FLOOR_NONE,
            gtm_clearance_ft: FLOOR_NONE,
            apt_clearance_ft: ac,
            apt_ceiling_ft: ak,
        };
        let cfg = cfg_with(tc, 0, 0);
        if let Some((f, _)) = floor_of(&with_airport_area(&s, low), None, &cfg) {
            // The floor is an area object, never an inhibit: it holds terrain plus the area's
            // clearance and the obstacle top plus the same clearance.
            assert!(f >= t.saturating_add(ac));
            if o != FLOOR_NONE {
                assert!(f >= o.saturating_add(ac));
            }
        } else {
            assert!(t == FLOOR_NONE);
        }
    }

    // The plan-level property (a row past the hold is never planned) did not finish in 10 minutes
    // under Kani (msaw_plan with its allocation); it is a test (OA-MSAW-065). What is proved is the
    // classification msaw_plan switches on.
    // REQ: OA-MSAW-065
    #[kani::proof]
    fn an_altitude_older_than_the_hold_is_classed_lost_and_an_extrapolation_never_climbs() {
        let (age, max, hold): (f32, f64, f64) = (kani::any(), kani::any(), kani::any());
        kani::assume(max.is_finite() && max > 0.0 && hold.is_finite() && hold >= max);
        let u = altitude_use(age, max, hold);
        if !age.is_nan() && f64::from(age) > hold {
            assert!(u == AltUse::Lost);
        }
        if age.is_nan() || f64::from(age) <= max {
            assert!(u == AltUse::Fresh);
        }
        // A descent is carried on, a climb never: the extrapolated altitude is not above the altitude.
        let (alt, vr, a): (f64, f64, f64) = (kani::any(), kani::any(), kani::any());
        kani::assume(alt.is_finite() && alt >= -MAX_ABS_ALT_FT && alt <= MAX_ABS_ALT_FT);
        kani::assume(vr.is_nan() || (vr.is_finite() && vr.abs() <= MAX_ABS_VRATE_FPM));
        kani::assume(a.is_finite() && a >= 0.0 && a <= MAX_LOOK_AHEAD_S);
        assert!(extrapolated_alt_ft(alt, vr, a) <= alt);
    }
}
