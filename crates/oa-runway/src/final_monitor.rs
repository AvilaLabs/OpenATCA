//! Parallel-final monitor geometry and predicted-track evaluation. Core (RULESET B1, B2).
//!
//! This is an endpoint-derived engineering aid, not an adapted or authorized facility procedure.
//! The shell supplies built runway geometry or caller-sourced outer fixes and tracker predictions.
//! No raw plots, clock, files, display types, or allocation enter this core.

use crate::plane::Local;
use crate::runway::{EndName, Runway};

pub const FT_TO_M: f64 = 0.3048;
pub const DEFAULT_NTZ_WIDTH_FT: f64 = 2_000.0;
pub const DEFAULT_UPDATE_NS: u64 = 1_200_000_000;
pub const DEFAULT_LOOKAHEAD_NS: u64 = 10_000_000_000;
pub const DEFAULT_MINIMUM_SEPARATION_FT: f64 = 3_100.0;
pub const DEGRADED_AFTER_NS: u64 = 4_800_000_000;
// A7.1: 256 bounds each pass; update performs a sorted-key check, full numeric validation,
// candidate evaluation and output emission (at most 1,024 row visits). Candidate scratch is 2 KiB
// at the cap; no row can create more than one alert. Raising the bound requires measured
// end-to-end monitor capacity evidence.
pub const MAX_TRACKS: usize = 256;
pub const DEFAULT_CENTERLINE_EXTENSION_M: f64 = 10_000.0;
const MAX_LOOKAHEAD_NS: u64 = 60_000_000_000;
const MAX_UPDATE_NS: u64 = 4_800_000_000;
// The course length is at most the distance between two endpoints each bounded to a 20 km
// local frame by finite_point.
const MAX_COURSE_LENGTH_M: f64 = 40_000.0;
// Derived from prepare_pair's existing 20,000 ft maximum NTZ width.
const MAX_NTZ_HALF_WIDTH_M: f64 = 3_048.0;
// Mirrors the accepted ADS-B reported-speed ceiling (4,088 kt); do not use the lower 360 m/s
// tracker candidate-pairing gate as a predicted-track limit.
const MAX_TRACK_SPEED_MPS: f64 = 4_088.0 * 0.514_444_444_444;
// Prediction positions and the runway origin share the same bounded 20 km local frame.
const MAX_TRACK_R95_M: f64 = 20_000.0;

/// Cold adaptation values; defaults are a development profile, not universal minima.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinalMonitorConfig {
    pub ntz_width_ft: f64,
    pub lookahead_ns: u64,
    pub update_ns: u64,
    pub minimum_separation_ft: f64,
}

/// Defaults required for this engineering profile.
pub const DEFAULT_CONFIG: FinalMonitorConfig = FinalMonitorConfig {
    ntz_width_ft: DEFAULT_NTZ_WIDTH_FT,
    lookahead_ns: DEFAULT_LOOKAHEAD_NS,
    update_ns: DEFAULT_UPDATE_NS,
    minimum_separation_ft: DEFAULT_MINIMUM_SEPARATION_FT,
};

/// A final segment described by an outer endpoint and a landing threshold. Source identity remains
/// with the caller; use [`centerline_final_from_runway`] when constructing a development segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinalCourseInput {
    pub runway_id: u32,
    pub end_name: [u8; 3],
    pub outer_fix: Local,
    pub threshold: Local,
}

/// Stable public identifier for one ordered pair; no internal array index crosses the boundary.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalPairId {
    pub first_runway_id: u32,
    pub first_end_name: [u8; 3],
    pub second_runway_id: u32,
    pub second_end_name: [u8; 3],
}

/// Prepared geometry for two parallel finals. Cold adaptation record.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinalPair {
    pub id: FinalPairId,
    pub origin: Local,
    pub axis_east: f64,
    pub axis_north: f64,
    pub normal_east: f64,
    pub normal_north: f64,
    pub course_length_m: f64,
    pub ntz_half_width_m: f64,
    pub lookahead_ns: u64,
    pub update_ns: u64,
}

/// A selected runway end for the centerline adapter. Cold caller input (RULESET A7.2).
#[derive(Clone, Copy, Debug)]
pub struct RunwayFinalSelection<'a> {
    pub runway_id: u32,
    pub runway: &'a Runway,
    pub end_name: EndName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareError {
    BadConfig,
    SameRunway,
    BadEndpoint,
    CourseNotParallel,
    TooClose,
    ZoneConsumesSeparation,
    RunwayEndMissing,
}

/// Derive a final-course segment from a built runway's published end and axis.
/// The extension is a geometry bound for monitoring; it is not a published FAF or procedure fix.
pub fn centerline_final_from_runway(
    runway_id: u32,
    runway: &Runway,
    end_name: EndName,
    extension_m: f64,
) -> Result<FinalCourseInput, PrepareError> {
    if runway_id == 0 || !extension_m.is_finite() || !(100.0..=16_000.0).contains(&extension_m) {
        return Err(PrepareError::BadEndpoint);
    }
    let end = runway
        .ends
        .iter()
        .find(|end| end.name == end_name)
        .ok_or(PrepareError::RunwayEndMissing)?;
    if !runway.ux.is_finite() || !runway.uy.is_finite() {
        return Err(PrepareError::BadEndpoint);
    }
    let first_is_target = runway
        .ends
        .first()
        .is_some_and(|first| first.name == end_name);
    let (inbound_east, inbound_north) = if first_is_target {
        (runway.ux, runway.uy)
    } else {
        (-runway.ux, -runway.uy)
    };
    let outer_fix = Local {
        east_m: end.threshold_local.east_m - inbound_east * extension_m,
        north_m: end.threshold_local.north_m - inbound_north * extension_m,
    };
    if !finite_point(outer_fix) {
        return Err(PrepareError::BadEndpoint);
    }
    Ok(FinalCourseInput {
        runway_id,
        end_name,
        outer_fix,
        threshold: end.threshold_local,
    })
}

/// Adapt two built runway ends into the pair geometry. This is the production-facing integration
/// hook when no charted CIFP final fix is supplied; its outer endpoint is explicitly derived from
/// the runway centerline and is never labeled as a procedure fix.
pub fn prepare_centerline_pair(
    first: RunwayFinalSelection<'_>,
    second: RunwayFinalSelection<'_>,
    extension_m: f64,
    config: FinalMonitorConfig,
) -> Result<FinalPair, PrepareError> {
    let first =
        centerline_final_from_runway(first.runway_id, first.runway, first.end_name, extension_m)?;
    let second = centerline_final_from_runway(
        second.runway_id,
        second.runway,
        second.end_name,
        extension_m,
    )?;
    prepare_pair(first, second, config)
}

fn finite_point(p: Local) -> bool {
    p.east_m.is_finite() && p.north_m.is_finite() && length(p.east_m, p.north_m) <= 20_000.0
}

fn length(x: f64, y: f64) -> f64 {
    oa_math::hypot(x, y)
}

/// Prepare the midpoint NTZ from two caller-supplied endpoint pairs. This checks geometry, not
/// source authenticity or procedure authorization. `first`/`second` order is retained as a stable
/// ID; inbound courses must be within 3 degrees.
pub fn prepare_pair(
    first: FinalCourseInput,
    second: FinalCourseInput,
    config: FinalMonitorConfig,
) -> Result<FinalPair, PrepareError> {
    if first.runway_id == 0 || second.runway_id == 0 || first.runway_id == second.runway_id {
        return Err(PrepareError::SameRunway);
    }
    if !config.ntz_width_ft.is_finite()
        || config.ntz_width_ft <= 0.0
        || config.ntz_width_ft > 20_000.0
        || !config.minimum_separation_ft.is_finite()
        || config.minimum_separation_ft <= 0.0
        || config.lookahead_ns == 0
        || config.lookahead_ns > 60_000_000_000
        || config.update_ns == 0
        || config.update_ns > 4_800_000_000
    {
        return Err(PrepareError::BadConfig);
    }
    if !finite_point(first.outer_fix)
        || !finite_point(first.threshold)
        || !finite_point(second.outer_fix)
        || !finite_point(second.threshold)
    {
        return Err(PrepareError::BadEndpoint);
    }
    let fdx = first.threshold.east_m - first.outer_fix.east_m;
    let fdy = first.threshold.north_m - first.outer_fix.north_m;
    let sdx = second.threshold.east_m - second.outer_fix.east_m;
    let sdy = second.threshold.north_m - second.outer_fix.north_m;
    let fl = length(fdx, fdy);
    let sl = length(sdx, sdy);
    if !fl.is_finite() || !sl.is_finite() || fl < 1.0 || sl < 1.0 {
        return Err(PrepareError::BadEndpoint);
    }
    let fax = fdx / fl;
    let fay = fdy / fl;
    let sax = sdx / sl;
    let say = sdy / sl;
    // Axes are inbound, outer fix to threshold. Dot product is cos(angle); this bound is 3 deg.
    if fax * sax + fay * say < 0.9986295347545738 {
        return Err(PrepareError::CourseNotParallel);
    }
    let ax = fax + sax;
    let ay = fay + say;
    let al = length(ax, ay);
    let axis_east = ax / al;
    let axis_north = ay / al;
    let normal_east = -axis_north;
    let normal_north = axis_east;
    let sep_outer = (second.outer_fix.east_m - first.outer_fix.east_m) * normal_east
        + (second.outer_fix.north_m - first.outer_fix.north_m) * normal_north;
    let sep_threshold = (second.threshold.east_m - first.threshold.east_m) * normal_east
        + (second.threshold.north_m - first.threshold.north_m) * normal_north;
    let min_sep = config.minimum_separation_ft * FT_TO_M;
    if !sep_outer.is_finite() || !sep_threshold.is_finite() || sep_outer * sep_threshold <= 0.0 {
        return Err(PrepareError::CourseNotParallel);
    }
    if sep_outer.abs() < min_sep || sep_threshold.abs() < min_sep {
        return Err(PrepareError::TooClose);
    }
    let ntz_half_width_m = config.ntz_width_ft * FT_TO_M * 0.5;
    if ntz_half_width_m * 2.0 >= oa_math::min_f64(sep_outer.abs(), sep_threshold.abs()) {
        return Err(PrepareError::ZoneConsumesSeparation);
    }
    let origin = Local {
        east_m: (first.outer_fix.east_m
            + second.outer_fix.east_m
            + first.threshold.east_m
            + second.threshold.east_m)
            * 0.25,
        north_m: (first.outer_fix.north_m
            + second.outer_fix.north_m
            + first.threshold.north_m
            + second.threshold.north_m)
            * 0.25,
    };
    let course_length_m = (fl + sl) * 0.5;
    if !finite_point(origin) || !course_length_m.is_finite() {
        return Err(PrepareError::BadEndpoint);
    }
    Ok(FinalPair {
        id: FinalPairId {
            first_runway_id: first.runway_id,
            first_end_name: first.end_name,
            second_runway_id: second.runway_id,
            second_end_name: second.end_name,
        },
        origin,
        axis_east,
        axis_north,
        normal_east,
        normal_north,
        course_length_m,
        ntz_half_width_m,
        lookahead_ns: config.lookahead_ns,
        update_ns: config.update_ns,
    })
}

/// Predicted track columns, each with one value per tracker prediction.
#[repr(C)]
pub struct PredictedTracks<'a> {
    pub addresses: &'a [u32],
    pub east_m: &'a [f64],
    pub north_m: &'a [f64],
    pub velocity_east_mps: &'a [f64],
    pub velocity_north_mps: &'a [f64],
    pub r95_m: &'a [f64],
    pub lat_e7: &'a [i32],
    pub lon_e7: &'a [i32],
}

const _: () = assert!(core::mem::size_of::<PredictedTracks<'static>>() == 128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FinalSeverity {
    Caution = 0,
    Warning = 1,
}

/// Caller-owned structure-of-arrays output; capacity is checked before any field is written.
#[repr(C)]
pub struct FinalAlertBuffer<'a> {
    pub addresses: &'a mut [u32],
    pub pair_ids: &'a mut [FinalPairId],
    pub severity: &'a mut [FinalSeverity],
    pub lat_e7: &'a mut [i32],
    pub lon_e7: &'a mut [i32],
    pub seconds_to_entry_milli: &'a mut [u32],
}

const _: () = assert!(core::mem::size_of::<FinalAlertBuffer<'static>>() == 96);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateError {
    BadInput,
    Capacity,
    ClockRegression,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct UpdateResult {
    pub examined: u16,
    pub alerts: u16,
    pub degraded: bool,
    pub due: bool,
}

const _: () = assert!(core::mem::size_of::<UpdateResult>() == 6);

/// Timing state is shell-owned in use; the transition is pure in explicit timestamps.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalMonitorState {
    pub last_update_ns: u64,
}

const _: () = assert!(core::mem::size_of::<FinalMonitorState>() == 8);

impl FinalMonitorState {
    pub const fn new() -> Self {
        Self { last_update_ns: 0 }
    }
}

impl Default for FinalMonitorState {
    fn default() -> Self {
        Self::new()
    }
}

fn valid_tracks(t: &PredictedTracks<'_>) -> bool {
    let n = t.addresses.len();
    n <= MAX_TRACKS
        && t.east_m.len() == n
        && t.north_m.len() == n
        && t.velocity_east_mps.len() == n
        && t.velocity_north_mps.len() == n
        && t.r95_m.len() == n
        && t.lat_e7.len() == n
        && t.lon_e7.len() == n
}

/// Evaluate one pair on a due tracker-prediction update. Input addresses must be sorted, unique,
/// and nonzero. At most 256 rows are accepted; validation, candidate evaluation and emission are
/// linear, with no allocation and at most one output row per input. Capacity for all possible rows
/// is checked before any output write. Unknown, malformed, or numerically unbounded predictions
/// refuse the whole call; no partial output. A non-due call leaves state and output unchanged.
pub fn update(
    pair: FinalPair,
    state: &mut FinalMonitorState,
    now_ns: u64,
    prediction_report_ns: u64,
    tracks: PredictedTracks<'_>,
    output: FinalAlertBuffer<'_>,
) -> Result<UpdateResult, UpdateError> {
    let n = tracks.addresses.len();
    if n > MAX_TRACKS {
        return Err(UpdateError::Capacity);
    }
    if !valid_tracks(&tracks)
        || output.addresses.len() != output.pair_ids.len()
        || output.addresses.len() != output.severity.len()
        || output.addresses.len() != output.lat_e7.len()
        || output.addresses.len() != output.lon_e7.len()
        || output.addresses.len() != output.seconds_to_entry_milli.len()
    {
        return Err(UpdateError::BadInput);
    }
    if pair.id.first_runway_id == 0
        || pair.id.second_runway_id == 0
        || pair.id.first_runway_id == pair.id.second_runway_id
        || !finite_point(pair.origin)
        || !pair.axis_east.is_finite()
        || !pair.axis_north.is_finite()
        || !pair.normal_east.is_finite()
        || !pair.normal_north.is_finite()
        || pair.axis_east.abs() > 1.0
        || pair.axis_north.abs() > 1.0
        || pair.normal_east.abs() > 1.0
        || pair.normal_north.abs() > 1.0
        || !pair.course_length_m.is_finite()
        || !(1.0..=MAX_COURSE_LENGTH_M).contains(&pair.course_length_m)
        || !pair.ntz_half_width_m.is_finite()
        || !(f64::MIN_POSITIVE..=MAX_NTZ_HALF_WIDTH_M).contains(&pair.ntz_half_width_m)
        || !(1..=MAX_LOOKAHEAD_NS).contains(&pair.lookahead_ns)
        || !(1..=MAX_UPDATE_NS).contains(&pair.update_ns)
    {
        return Err(UpdateError::BadInput);
    }
    let axis_norm_sq = pair.axis_east * pair.axis_east + pair.axis_north * pair.axis_north;
    let normal_norm_sq =
        pair.normal_east * pair.normal_east + pair.normal_north * pair.normal_north;
    let axis_normal_dot = pair.axis_east * pair.normal_east + pair.axis_north * pair.normal_north;
    if (axis_norm_sq - 1.0).abs() > 1.0e-9
        || (normal_norm_sq - 1.0).abs() > 1.0e-9
        || axis_normal_dot.abs() > 1.0e-9
    {
        return Err(UpdateError::BadInput);
    }
    if tracks.addresses.windows(2).any(|w| {
        w.first()
            .zip(w.get(1))
            .is_some_and(|(a, b)| *a == 0 || *a >= *b)
    }) {
        return Err(UpdateError::BadInput);
    }
    if tracks
        .addresses
        .first()
        .is_some_and(|address| *address == 0)
    {
        return Err(UpdateError::BadInput);
    }
    for ((((((east, north), vx), vy), r95), lat), lon) in tracks
        .east_m
        .iter()
        .zip(tracks.north_m)
        .zip(tracks.velocity_east_mps)
        .zip(tracks.velocity_north_mps)
        .zip(tracks.r95_m)
        .zip(tracks.lat_e7)
        .zip(tracks.lon_e7)
    {
        if !east.is_finite()
            || !north.is_finite()
            || !vx.is_finite()
            || !vy.is_finite()
            || vx.abs() > MAX_TRACK_SPEED_MPS
            || vy.abs() > MAX_TRACK_SPEED_MPS
            || !r95.is_finite()
            || !(0.0..=MAX_TRACK_R95_M).contains(r95)
            || east.abs() > 20_000.0
            || north.abs() > 20_000.0
            || !(-900_000_000..=900_000_000).contains(lat)
            || !(-1_800_000_000..=1_800_000_000).contains(lon)
        {
            return Err(UpdateError::BadInput);
        }
        if oa_math::hypot(*vx, *vy) > MAX_TRACK_SPEED_MPS {
            return Err(UpdateError::BadInput);
        }
    }
    if now_ns < state.last_update_ns || now_ns < prediction_report_ns {
        return Err(UpdateError::ClockRegression);
    }
    let due =
        state.last_update_ns == 0 || now_ns.saturating_sub(state.last_update_ns) >= pair.update_ns;
    let degraded = now_ns.saturating_sub(prediction_report_ns) >= DEGRADED_AFTER_NS;
    if !due {
        return Ok(UpdateResult {
            examined: 0,
            alerts: 0,
            degraded,
            due: false,
        });
    }
    if output.addresses.len() < n {
        return Err(UpdateError::Capacity);
    }
    let mut candidates = [AlertCandidate::EMPTY; MAX_TRACKS];
    for (index, ((((&east, &north), &vx), &vy), &r95)) in tracks
        .east_m
        .iter()
        .zip(tracks.north_m)
        .zip(tracks.velocity_east_mps)
        .zip(tracks.velocity_north_mps)
        .zip(tracks.r95_m)
        .enumerate()
    {
        let dx = east - pair.origin.east_m;
        let dy = north - pair.origin.north_m;
        let along = dx * pair.axis_east + dy * pair.axis_north;
        let cross = dx * pair.normal_east + dy * pair.normal_north;
        let along_v = vx * pair.axis_east + vy * pair.axis_north;
        let cross_v = vx * pair.normal_east + vy * pair.normal_north;
        let half = pair.ntz_half_width_m + r95 + 1.5;
        let along_low = -pair.course_length_m * 0.5;
        let along_high = pair.course_length_m * 0.5;
        let on_final_segment = along >= along_low && along <= along_high;
        let candidate = if on_final_segment && cross.abs() <= half {
            Some(AlertCandidate {
                present: true,
                severity: FinalSeverity::Warning,
                seconds_to_entry_milli: 0,
            })
        } else if on_final_segment && cross * cross_v < 0.0 {
            let seconds = (cross.abs() - half) / cross_v.abs();
            let t_ns = seconds * 1_000_000_000.0;
            let future_along = along + along_v * seconds;
            if t_ns.is_finite()
                && t_ns >= 0.0
                && t_ns <= pair.lookahead_ns as f64
                && future_along >= along_low
                && future_along <= along_high
            {
                let millis = seconds * 1_000.0;
                if !millis.is_finite() || !(0.0..=u32::MAX as f64).contains(&millis) {
                    return Err(UpdateError::BadInput);
                }
                let Ok(seconds_to_entry_milli) = u32::try_from(millis as u64) else {
                    return Err(UpdateError::BadInput);
                };
                Some(AlertCandidate {
                    present: true,
                    severity: FinalSeverity::Caution,
                    seconds_to_entry_milli,
                })
            } else {
                None
            }
        } else {
            None
        };
        if let Some(candidate) = candidate {
            let Some(slot) = candidates.get_mut(index) else {
                return Err(UpdateError::Capacity);
            };
            *slot = candidate;
        }
    }
    let mut count = 0usize;
    for (index, &address) in tracks.addresses.iter().enumerate() {
        let Some(candidate) = candidates.get(index).copied() else {
            return Err(UpdateError::Capacity);
        };
        if !candidate.present {
            continue;
        }
        let Some((&lat, &lon)) = tracks.lat_e7.get(index).zip(tracks.lon_e7.get(index)) else {
            return Err(UpdateError::BadInput);
        };
        let (
            Some(out_address),
            Some(out_pair),
            Some(out_severity),
            Some(out_lat),
            Some(out_lon),
            Some(out_time),
        ) = (
            output.addresses.get_mut(count),
            output.pair_ids.get_mut(count),
            output.severity.get_mut(count),
            output.lat_e7.get_mut(count),
            output.lon_e7.get_mut(count),
            output.seconds_to_entry_milli.get_mut(count),
        )
        else {
            return Err(UpdateError::Capacity);
        };
        *out_address = address;
        *out_pair = pair.id;
        *out_severity = candidate.severity;
        *out_lat = lat;
        *out_lon = lon;
        *out_time = candidate.seconds_to_entry_milli;
        count += 1;
    }
    state.last_update_ns = now_ns;
    Ok(UpdateResult {
        examined: n as u16,
        alerts: count as u16,
        degraded,
        due: true,
    })
}

// A7.1: one transient fixed-width result per accepted track; no heap growth.
#[derive(Clone, Copy)]
#[repr(C)]
struct AlertCandidate {
    present: bool,
    severity: FinalSeverity,
    seconds_to_entry_milli: u32,
}

const _: () = assert!(core::mem::size_of::<AlertCandidate>() == 8);
const _: () = assert!(core::mem::size_of::<[AlertCandidate; MAX_TRACKS]>() == 2_048);

impl AlertCandidate {
    const EMPTY: Self = Self {
        present: false,
        severity: FinalSeverity::Caution,
        seconds_to_entry_milli: 0,
    };
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used
    )]

    use super::*;
    use crate::plane::{plane_new, Geo};
    use crate::runway::{build_runway, DeclaredFt, EndInput, RunwayInput};

    fn input(runway_id: u32, y: f64) -> FinalCourseInput {
        FinalCourseInput {
            runway_id,
            end_name: *b"27L",
            outer_fix: Local {
                east_m: -5_000.0,
                north_m: y,
            },
            threshold: Local {
                east_m: 5_000.0,
                north_m: y,
            },
        }
    }

    fn pair() -> FinalPair {
        match prepare_pair(input(1, -550.0), input(2, 550.0), DEFAULT_CONFIG) {
            Ok(pair) => pair,
            Err(_) => panic!("synthetic parallel finals rejected"),
        }
    }

    fn kord_end(name: &'static str, alignment: f64, lat: f64, lon: f64) -> EndInput<'static> {
        EndInput {
            name,
            true_alignment_deg: Some(alignment),
            position: Some(Geo {
                lat_deg: lat,
                lon_deg: lon,
            }),
            displaced_position: None,
            displaced_len_ft: None,
            elev_ft: None,
            displaced_elev_ft: None,
            declared: DeclaredFt::default(),
            cifp_threshold: None,
        }
    }

    fn kord_parallel_runways() -> (crate::plane::Plane, Runway, Runway) {
        // Exact 2026-10-29 NASR APT_RWY_END values from the repository's public KORD fixture.
        let plane = plane_new(41.97694027, -87.90814972).expect("KORD reference plane");
        let runway_27r = build_runway(
            &plane,
            &RunwayInput {
                length_ft: Some(7_500.0),
                width_ft: Some(150.0),
                a: kord_end("27R", 270.0, 42.0028308, -87.89908325),
                b: kord_end("09L", 90.0, 42.00283205, -87.92667608),
            },
        )
        .expect("KORD 09L/27R public runway geometry");
        let runway_27l = build_runway(
            &plane,
            &RunwayInput {
                length_ft: Some(11_260.0),
                width_ft: Some(150.0),
                a: kord_end("27L", 270.0, 41.98390033, -87.89015488),
                b: kord_end("09R", 90.0, 41.98389366, -87.93156908),
            },
        )
        .expect("KORD 09R/27L public runway geometry");
        (plane, runway_27r, runway_27l)
    }

    fn evaluate(
        state: &mut FinalMonitorState,
        now: u64,
        report: u64,
        east: f64,
        north: f64,
        vx: f64,
        vy: f64,
    ) -> (UpdateResult, Option<FinalSeverity>) {
        let addresses = [1234];
        let east_m = [east];
        let north_m = [north];
        let velocity_east_mps = [vx];
        let velocity_north_mps = [vy];
        let r95_m = [0.0];
        let lat_e7 = [100];
        let lon_e7 = [200];
        let mut out_addr = [0];
        let mut out_pair = [FinalPairId {
            first_runway_id: 0,
            first_end_name: [0; 3],
            second_runway_id: 0,
            second_end_name: [0; 3],
        }];
        let mut out_severity = [FinalSeverity::Caution];
        let mut out_lat = [0];
        let mut out_lon = [0];
        let mut out_time = [0];
        let result = update(
            pair(),
            state,
            now,
            report,
            PredictedTracks {
                addresses: &addresses,
                east_m: &east_m,
                north_m: &north_m,
                velocity_east_mps: &velocity_east_mps,
                velocity_north_mps: &velocity_north_mps,
                r95_m: &r95_m,
                lat_e7: &lat_e7,
                lon_e7: &lon_e7,
            },
            FinalAlertBuffer {
                addresses: &mut out_addr,
                pair_ids: &mut out_pair,
                severity: &mut out_severity,
                lat_e7: &mut out_lat,
                lon_e7: &mut out_lon,
                seconds_to_entry_milli: &mut out_time,
            },
        );
        let result = match result {
            Ok(result) => result,
            Err(_) => panic!("well-formed prediction was refused"),
        };
        (
            result,
            out_severity.first().copied().filter(|_| result.alerts > 0),
        )
    }

    fn assert_bad_input_is_atomic(pair: FinalPair, vx: f64, vy: f64, r95: f64, north: f64) {
        let addresses = [1234];
        let east_m = [0.0];
        let north_m = [north];
        let velocity_east_mps = [vx];
        let velocity_north_mps = [vy];
        let r95_m = [r95];
        let lat_e7 = [100];
        let lon_e7 = [200];
        let mut state = FinalMonitorState {
            last_update_ns: 100,
        };
        let mut out_addr = [77];
        let sentinel_pair = FinalPairId {
            first_runway_id: 7,
            first_end_name: *b"07L",
            second_runway_id: 8,
            second_end_name: *b"08R",
        };
        let mut out_pair = [sentinel_pair];
        let mut out_severity = [FinalSeverity::Warning];
        let mut out_lat = [303];
        let mut out_lon = [404];
        let mut out_time = [505];
        let result = update(
            pair,
            &mut state,
            1_200_000_100,
            1_200_000_100,
            PredictedTracks {
                addresses: &addresses,
                east_m: &east_m,
                north_m: &north_m,
                velocity_east_mps: &velocity_east_mps,
                velocity_north_mps: &velocity_north_mps,
                r95_m: &r95_m,
                lat_e7: &lat_e7,
                lon_e7: &lon_e7,
            },
            FinalAlertBuffer {
                addresses: &mut out_addr,
                pair_ids: &mut out_pair,
                severity: &mut out_severity,
                lat_e7: &mut out_lat,
                lon_e7: &mut out_lon,
                seconds_to_entry_milli: &mut out_time,
            },
        );
        assert_eq!(result, Err(UpdateError::BadInput));
        assert_eq!(state.last_update_ns, 100);
        assert_eq!(out_addr, [77]);
        assert_eq!(out_pair, [sentinel_pair]);
        assert_eq!(out_severity, [FinalSeverity::Warning]);
        assert_eq!(out_lat, [303]);
        assert_eq!(out_lon, [404]);
        assert_eq!(out_time, [505]);
    }

    // REQ: OA-RWY-014
    #[test]
    fn pair_uses_endpoints_and_midway_ntz_default() {
        let pair = pair();
        assert_eq!(pair.id.first_runway_id, 1);
        assert_eq!(pair.id.second_runway_id, 2);
        assert_eq!(pair.origin.east_m, 0.0);
        assert_eq!(pair.origin.north_m, 0.0);
        assert_eq!(pair.ntz_half_width_m, 304.8);
        assert_eq!(pair.course_length_m, 10_000.0);
    }

    // REQ: OA-RWY-014
    #[test]
    fn kord_parallel_pair_uses_public_runway_geometry() {
        let (_plane, runway_27r, runway_27l) = kord_parallel_runways();
        // Source pins: NASR APT_RWY_END.csv sha256 84f3185a567ca9d4703f07d47d62b6b3e9f8d2ed7ac9ef3ae62ddb30ae3c2d2b;
        // CIFP fixture sha256 c551bd277a8d2cb92d71090373ce12a4798332b8d87705d4fe57367e0df67eb9.
        // CIFP PG coordinates `N42001019W087535670` (27R) and `N41590204W087532456` (27L)
        // cross-check the NASR 2026-10-29 runway endpoints used by the builder. No published FAF
        // is substituted: this fixture exercises the explicit centerline-extension adaptation.
        let threshold_27r = runway_27r
            .ends
            .iter()
            .find(|end| end.name == *b"27R")
            .expect("27R end");
        let threshold_27l = runway_27l
            .ends
            .iter()
            .find(|end| end.name == *b"27L")
            .expect("27L end");
        assert!((threshold_27r.threshold.lat_deg - 42.0028305556).abs() < 0.000001);
        assert!((threshold_27r.threshold.lon_deg + 87.8990833333).abs() < 0.000001);
        assert!((threshold_27l.threshold.lat_deg - 41.9839).abs() < 0.000001);
        assert!((threshold_27l.threshold.lon_deg + 87.8901555556).abs() < 0.000001);
        let pair = prepare_centerline_pair(
            RunwayFinalSelection {
                runway_id: 27_001,
                runway: &runway_27r,
                end_name: *b"27R",
            },
            RunwayFinalSelection {
                runway_id: 27_002,
                runway: &runway_27l,
                end_name: *b"27L",
            },
            DEFAULT_CENTERLINE_EXTENSION_M,
            DEFAULT_CONFIG,
        )
        .expect("source-pinned KORD 27L/27R centerlines form a valid pair");
        assert_eq!(pair.id.first_runway_id, 27_001);
        assert_eq!(pair.id.second_runway_id, 27_002);
        assert!((pair.course_length_m - DEFAULT_CENTERLINE_EXTENSION_M).abs() < 0.01);
        assert!(pair.axis_east < -0.99);
        assert!(pair.axis_north.abs() < 0.01);
        assert!(pair.ntz_half_width_m > 300.0);
    }

    // REQ: OA-RWY-015
    #[test]
    fn on_course_clear() {
        let mut state = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -550.0, 100.0, 0.0);
        assert_eq!(result.alerts, 0);
        assert_eq!(severity, None);
    }

    // REQ: OA-RWY-015
    #[test]
    fn drift_caution_then_warning() {
        let mut state = FinalMonitorState::new();
        let (first, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -550.0, 100.0, 50.0);
        assert_eq!(first.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Caution));
        let (second, severity) = evaluate(
            &mut state,
            1_200_001_000,
            1_200_001_000,
            0.0,
            -250.0,
            100.0,
            0.0,
        );
        assert_eq!(second.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Warning));
    }

    // REQ: OA-RWY-015
    #[test]
    fn a_750_mps_predicted_track_remains_inside_the_accepted_speed_domain() {
        let mut state = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -550.0, 0.0, 750.0);
        assert_eq!(result.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Caution));
    }

    // REQ: OA-RWY-016
    #[test]
    fn thirty_degree_blunder_caution_within_prediction() {
        let mut state = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -550.0, 86.6, 50.0);
        assert_eq!(result.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Caution));

        // At 50 m/s lateral speed, this predicted trajectory enters the 306.3 m half-width
        // after about 4.9 s. Feed the corresponding modeled sample on the next due update.
        let (crossing, severity) = evaluate(
            &mut state,
            5_000_001_000,
            5_000_001_000,
            86.6 * 5.0,
            -550.0 + 50.0 * 5.0,
            86.6,
            50.0,
        );
        assert_eq!(crossing.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Warning));
    }

    // REQ: OA-RWY-017
    #[test]
    fn stale_prediction_sets_degraded() {
        let mut state = FinalMonitorState::new();
        let (result, _) = evaluate(&mut state, 4_800_001_000, 1_000, 0.0, -550.0, 100.0, 0.0);
        assert!(result.degraded);
        assert!(result.due);
    }

    // REQ: OA-RWY-018
    #[test]
    fn close_parallel_finals_are_refused() {
        let result = prepare_pair(input(1, -400.0), input(2, 400.0), DEFAULT_CONFIG);
        assert_eq!(result, Err(PrepareError::TooClose));
    }

    // REQ: OA-RWY-019
    #[test]
    fn warning_is_limited_to_the_final_segment_endpoints() {
        for east in [-5_000.0, 5_000.0] {
            let mut state = FinalMonitorState::new();
            let (result, severity) = evaluate(&mut state, 1_000, 1_000, east, 0.0, 0.0, 0.0);
            assert_eq!(result.alerts, 1);
            assert_eq!(severity, Some(FinalSeverity::Warning));
        }
        for east in [-5_000.1, 5_000.1] {
            let mut state = FinalMonitorState::new();
            let (result, severity) = evaluate(&mut state, 1_000, 1_000, east, 0.0, 0.0, 0.0);
            assert_eq!(result.alerts, 0);
            assert_eq!(severity, None);
        }
    }

    // REQ: OA-RWY-019
    #[test]
    fn predicted_zone_entry_beyond_final_endpoint_is_clear() {
        let mut state = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut state, 1_000, 1_000, 4_900.0, -550.0, 100.0, 50.0);
        assert_eq!(result.alerts, 0);
        assert_eq!(severity, None);
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn finite_extreme_pair_and_prediction_values_refuse_without_false_clear_or_mutation() {
        let mut invalid = pair();
        invalid.origin.east_m = f64::MAX;
        assert_bad_input_is_atomic(invalid, 100.0, 50.0, 0.0, -550.0);

        let mut invalid = pair();
        invalid.axis_east = f64::MAX;
        assert_bad_input_is_atomic(invalid, 100.0, 50.0, 0.0, -550.0);

        let mut invalid = pair();
        invalid.course_length_m = f64::MAX;
        assert_bad_input_is_atomic(invalid, 100.0, 50.0, 0.0, -550.0);

        let mut invalid = pair();
        invalid.ntz_half_width_m = f64::MAX;
        assert_bad_input_is_atomic(invalid, 100.0, 50.0, 0.0, -550.0);

        assert_bad_input_is_atomic(pair(), f64::MAX, 50.0, 0.0, -550.0);
        assert_bad_input_is_atomic(pair(), 100.0, f64::MAX, 0.0, -550.0);
        assert_bad_input_is_atomic(pair(), 100.0, 50.0, f64::MAX, -550.0);
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn excessive_lookahead_cannot_saturate_a_caller_visible_entry_time() {
        let mut invalid = pair();
        invalid.lookahead_ns = u64::MAX;
        // This slow crossing would exceed u32 milliseconds if the caller-controlled lookahead
        // were accepted. The invalid adaptation must refuse before touching state or output.
        assert_bad_input_is_atomic(invalid, 0.0, 1.0e-9, 0.0, -20_000.0);
    }

    // REQ: OA-RWY-015
    #[test]
    fn track_and_output_caps_refuse_atomically() {
        let mut state = FinalMonitorState {
            last_update_ns: 100,
        };
        let addresses = [1, 2];
        let east_m = [0.0, 0.0];
        let north_m = [-550.0, -550.0];
        let velocity_east_mps = [100.0, 100.0];
        let velocity_north_mps = [50.0, 50.0];
        let r95_m = [0.0, 0.0];
        let lat_e7 = [100, 101];
        let lon_e7 = [200, 201];
        let mut out_addr = [77];
        let sentinel_pair = FinalPairId {
            first_runway_id: 7,
            first_end_name: *b"07L",
            second_runway_id: 8,
            second_end_name: *b"08R",
        };
        let mut out_pair = [sentinel_pair];
        let mut out_severity = [FinalSeverity::Warning];
        let mut out_lat = [303];
        let mut out_lon = [404];
        let mut out_time = [505];
        let result = update(
            pair(),
            &mut state,
            1_200_000_100,
            1_200_000_100,
            PredictedTracks {
                addresses: &addresses,
                east_m: &east_m,
                north_m: &north_m,
                velocity_east_mps: &velocity_east_mps,
                velocity_north_mps: &velocity_north_mps,
                r95_m: &r95_m,
                lat_e7: &lat_e7,
                lon_e7: &lon_e7,
            },
            FinalAlertBuffer {
                addresses: &mut out_addr,
                pair_ids: &mut out_pair,
                severity: &mut out_severity,
                lat_e7: &mut out_lat,
                lon_e7: &mut out_lon,
                seconds_to_entry_milli: &mut out_time,
            },
        );
        assert_eq!(result, Err(UpdateError::Capacity));
        assert_eq!(state.last_update_ns, 100);
        assert_eq!(out_addr, [77]);
        assert_eq!(out_pair, [sentinel_pair]);
        assert_eq!(out_severity, [FinalSeverity::Warning]);
        assert_eq!(out_lat, [303]);
        assert_eq!(out_lon, [404]);
        assert_eq!(out_time, [505]);

        let addresses: [u32; MAX_TRACKS + 1] = core::array::from_fn(|index| {
            u32::try_from(index + 1).expect("address count is bounded by MAX_TRACKS")
        });
        let empty: [f64; 0] = [];
        let empty_i32: [i32; 0] = [];
        let result = update(
            pair(),
            &mut state,
            2_400_000_100,
            2_400_000_100,
            PredictedTracks {
                addresses: &addresses,
                east_m: &empty,
                north_m: &empty,
                velocity_east_mps: &empty,
                velocity_north_mps: &empty,
                r95_m: &empty,
                lat_e7: &empty_i32,
                lon_e7: &empty_i32,
            },
            FinalAlertBuffer {
                addresses: &mut out_addr,
                pair_ids: &mut out_pair,
                severity: &mut out_severity,
                lat_e7: &mut out_lat,
                lon_e7: &mut out_lon,
                seconds_to_entry_milli: &mut out_time,
            },
        );
        assert_eq!(result, Err(UpdateError::Capacity));
        assert_eq!(state.last_update_ns, 100);
        assert_eq!(out_addr, [77]);
        assert_eq!(out_pair, [sentinel_pair]);
        assert_eq!(out_severity, [FinalSeverity::Warning]);
        assert_eq!(out_lat, [303]);
        assert_eq!(out_lon, [404]);
        assert_eq!(out_time, [505]);
    }
}
