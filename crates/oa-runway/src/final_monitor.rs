//! Parallel-final monitor geometry and predicted-track evaluation. Core (RULESET B1, B2).
//!
//! This is an endpoint-derived engineering aid, not an adapted or authorized facility procedure.
//! The shell supplies built runway geometry or caller-sourced outer fixes and tracker predictions.
//! No raw plots, clock, files, display types, or allocation enter this core.

use crate::plane::{self, Geo, Local};
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
// The tracker plane rounds geographic coordinates to E-7 degrees; 1.5 m is the plane's stated
// maximum distance error across its supported extent. This conservative sum is the permitted
// discrepancy between an independently projected geographic position and the supplied local one.
const PROJECTION_CONSISTENCY_TOLERANCE_M: f64 = plane::PLANE_ERROR_BOUND_M + 0.01;

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
    /// Set by runway-derived adaptation. Direct endpoint-only geometry has no checked geographic
    /// frame and therefore cannot perform the input-coordinate consistency check.
    pub projection_origin: Option<Geo>,
    pub origin: Local,
    pub axis_east: f64,
    pub axis_north: f64,
    pub normal_east: f64,
    pub normal_north: f64,
    pub course_length_m: f64,
    pub ntz_half_width_m: f64,
    /// Largest R95 that leaves the adaptation-derived runway-to-NTZ clearance after the stated
    /// local-plane error guard. Larger values make the monitor unavailable rather than widen the
    /// warning region across an aircraft on its own runway centerline.
    pub max_r95_m: f64,
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
    if first.runway.plane_origin != second.runway.plane_origin {
        return Err(PrepareError::BadEndpoint);
    }
    let projection_origin = first.runway.plane_origin;
    let first =
        centerline_final_from_runway(first.runway_id, first.runway, first.end_name, extension_m)?;
    let second = centerline_final_from_runway(
        second.runway_id,
        second.runway,
        second.end_name,
        extension_m,
    )?;
    let mut pair = prepare_pair(first, second, config)?;
    pair.projection_origin = Some(projection_origin);
    Ok(pair)
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
    let minimum_separation = oa_math::min_f64(sep_outer.abs(), sep_threshold.abs());
    if ntz_half_width_m * 2.0 >= minimum_separation {
        return Err(PrepareError::ZoneConsumesSeparation);
    }
    let usable_clearance_m = minimum_separation * 0.5 - ntz_half_width_m;
    let max_r95_m = usable_clearance_m - plane::PLANE_ERROR_BOUND_M - 0.01;
    if !max_r95_m.is_finite() || max_r95_m < 0.0 {
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
        projection_origin: None,
        origin,
        axis_east,
        axis_north,
        normal_east,
        normal_north,
        course_length_m,
        ntz_half_width_m,
        max_r95_m,
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
    UnusableUncertainty,
    CoordinateMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct UpdateResult {
    pub examined: u16,
    pub alerts: u16,
    pub degraded: bool,
    pub due: bool,
    /// Elapsed time since the previous due evaluation. This is evaluation, not input, cadence.
    pub observed_interval_ns: u64,
    /// Time since the prior valid input frame, including frames that were not due for evaluation.
    pub observed_input_interval_ns: u64,
    /// Age of the oldest selected prediction source at this input frame.
    pub prediction_age_ns: u64,
    /// Whether the input-frame receipt gap exceeded the safety freshness bound.
    pub input_cadence_degraded: bool,
    /// Whether the selected predictions were older than the freshness limit.
    pub prediction_stale: bool,
}

// A7.1: report evaluation cadence separately from valid input cadence and prediction age.
const _: () = assert!(core::mem::size_of::<UpdateResult>() == 40);

/// Timing state is shell-owned in use; the transition is pure in explicit timestamps.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FinalMonitorState {
    /// Receipt time of the last valid input frame, even if it was not due for evaluation.
    pub last_input_frame_ns: u64,
    /// Receipt time of the last due evaluation attempt.
    pub last_evaluation_ns: u64,
    /// Bit 0 marks an input-frame time; bit 1 marks an evaluation time.
    pub clock_flags: u8,
    /// Reserved bytes are zero and retained for deterministic C layout.
    pub reserved: [u8; 7],
}

const _: () = assert!(core::mem::size_of::<FinalMonitorState>() == 24);

const HAS_INPUT_FRAME_TIME: u8 = 0b01;
const HAS_EVALUATION_TIME: u8 = 0b10;
const KNOWN_CLOCK_FLAGS: u8 = 0b11;

impl FinalMonitorState {
    pub const fn new() -> Self {
        Self {
            last_input_frame_ns: 0,
            last_evaluation_ns: 0,
            clock_flags: 0,
            reserved: [0; 7],
        }
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

// A7.2: stack-only validated timing values for one monitor call; size guard waived.
#[derive(Clone, Copy)]
struct ValidatedUpdateTiming {
    track_count: usize,
    observed_interval_ns: u64,
    observed_input_interval_ns: u64,
    prediction_age_ns: u64,
    due: bool,
    input_cadence_degraded: bool,
    prediction_stale: bool,
}

fn validated_projection_plane(pair: FinalPair) -> Result<Option<plane::Plane>, UpdateError> {
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
        || !pair.max_r95_m.is_finite()
        || !(0.0..=MAX_TRACK_R95_M).contains(&pair.max_r95_m)
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
    pair.projection_origin
        .map(|origin| plane::plane_new(origin.lat_deg, origin.lon_deg).ok_or(UpdateError::BadInput))
        .transpose()
}

fn validate_track_values(
    pair: FinalPair,
    projection_plane: Option<plane::Plane>,
    tracks: &PredictedTracks<'_>,
) -> Result<(), UpdateError> {
    if tracks.addresses.windows(2).any(|w| {
        w.first()
            .zip(w.get(1))
            .is_some_and(|(a, b)| *a == 0 || *a >= *b)
    }) || tracks
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
        if *r95 > pair.max_r95_m {
            return Err(UpdateError::UnusableUncertainty);
        }
        if let Some(projection_plane) = projection_plane {
            let Some(projected) = plane::to_local(
                &projection_plane,
                Geo {
                    lat_deg: f64::from(*lat) / 10_000_000.0,
                    lon_deg: f64::from(*lon) / 10_000_000.0,
                },
            ) else {
                return Err(UpdateError::CoordinateMismatch);
            };
            if oa_math::hypot(projected.east_m - *east, projected.north_m - *north)
                > PROJECTION_CONSISTENCY_TOLERANCE_M
            {
                return Err(UpdateError::CoordinateMismatch);
            }
        }
        if oa_math::hypot(*vx, *vy) > MAX_TRACK_SPEED_MPS {
            return Err(UpdateError::BadInput);
        }
    }
    Ok(())
}

fn validate_update_inputs(
    pair: FinalPair,
    state: &FinalMonitorState,
    now_ns: u64,
    prediction_report_ns: u64,
    tracks: &PredictedTracks<'_>,
) -> Result<ValidatedUpdateTiming, UpdateError> {
    let n = tracks.addresses.len();
    if n > MAX_TRACKS {
        return Err(UpdateError::Capacity);
    }
    if !valid_tracks(tracks) {
        return Err(UpdateError::BadInput);
    }
    if state.clock_flags & !KNOWN_CLOCK_FLAGS != 0
        || state.reserved != [0; 7]
        || (state.clock_flags & HAS_EVALUATION_TIME != 0
            && (state.clock_flags & HAS_INPUT_FRAME_TIME == 0
                || state.last_evaluation_ns > state.last_input_frame_ns))
    {
        return Err(UpdateError::BadInput);
    }
    let projection_plane = validated_projection_plane(pair)?;
    validate_track_values(pair, projection_plane, tracks)?;
    let has_input_frame = state.clock_flags & HAS_INPUT_FRAME_TIME != 0;
    let has_evaluation = state.clock_flags & HAS_EVALUATION_TIME != 0;
    if (has_input_frame && now_ns < state.last_input_frame_ns)
        || (has_evaluation && now_ns < state.last_evaluation_ns)
        || now_ns < prediction_report_ns
    {
        return Err(UpdateError::ClockRegression);
    }
    let observed_interval_ns = if has_evaluation {
        now_ns - state.last_evaluation_ns
    } else {
        0
    };
    let observed_input_interval_ns = if has_input_frame {
        now_ns - state.last_input_frame_ns
    } else {
        0
    };
    let due = !has_evaluation || observed_interval_ns >= pair.update_ns;
    let prediction_age_ns = now_ns.saturating_sub(prediction_report_ns);
    Ok(ValidatedUpdateTiming {
        track_count: n,
        observed_interval_ns,
        observed_input_interval_ns,
        prediction_age_ns,
        due,
        input_cadence_degraded: has_input_frame && observed_input_interval_ns > DEGRADED_AFTER_NS,
        prediction_stale: prediction_age_ns > DEGRADED_AFTER_NS,
    })
}

fn emit_candidates(
    pair: FinalPair,
    tracks: PredictedTracks<'_>,
    output: FinalAlertBuffer<'_>,
) -> Result<u16, UpdateError> {
    let n = tracks.addresses.len();
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
    Ok(count as u16)
}

fn result_from_timing(
    timing: ValidatedUpdateTiming,
    due: bool,
    examined: u16,
    alerts: u16,
) -> UpdateResult {
    UpdateResult {
        examined,
        alerts,
        degraded: timing.input_cadence_degraded || timing.prediction_stale,
        due,
        observed_interval_ns: timing.observed_interval_ns,
        observed_input_interval_ns: timing.observed_input_interval_ns,
        prediction_age_ns: timing.prediction_age_ns,
        input_cadence_degraded: timing.input_cadence_degraded,
        prediction_stale: timing.prediction_stale,
    }
}

fn record_input_frame(state: &mut FinalMonitorState, now_ns: u64) {
    state.last_input_frame_ns = now_ns;
    state.clock_flags |= HAS_INPUT_FRAME_TIME;
}

fn record_evaluation(state: &mut FinalMonitorState, now_ns: u64) {
    state.last_evaluation_ns = now_ns;
    state.clock_flags |= HAS_EVALUATION_TIME;
}

fn valid_output_shape(output: &FinalAlertBuffer<'_>) -> bool {
    output.addresses.len() == output.pair_ids.len()
        && output.addresses.len() == output.severity.len()
        && output.addresses.len() == output.lat_e7.len()
        && output.addresses.len() == output.lon_e7.len()
        && output.addresses.len() == output.seconds_to_entry_milli.len()
}

/// Evaluate one pair when due. Valid input-frame receipt cadence and prediction age determine
/// freshness; the separate due-evaluation interval is reported but does not stand in for either.
/// Input addresses must be sorted, unique and nonzero. At most 256 rows are accepted, with no
/// allocation and capacity checked before any output write. Invalid input or capacity refusal
/// leaves both clocks and output unchanged; a valid non-due frame advances only its receipt clock.
pub fn update(
    pair: FinalPair,
    state: &mut FinalMonitorState,
    now_ns: u64,
    prediction_report_ns: u64,
    tracks: PredictedTracks<'_>,
    output: FinalAlertBuffer<'_>,
) -> Result<UpdateResult, UpdateError> {
    if !valid_output_shape(&output) {
        return Err(UpdateError::BadInput);
    }
    let timing = validate_update_inputs(pair, state, now_ns, prediction_report_ns, &tracks)?;
    let degraded = timing.input_cadence_degraded || timing.prediction_stale;
    if !timing.due || degraded {
        record_input_frame(state, now_ns);
        if timing.due {
            record_evaluation(state, now_ns);
        }
        return Ok(result_from_timing(timing, timing.due, 0, 0));
    }
    let examined = u16::try_from(timing.track_count).map_err(|_| UpdateError::Capacity)?;
    let alerts = emit_candidates(pair, tracks, output)?;
    record_input_frame(state, now_ns);
    record_evaluation(state, now_ns);
    Ok(result_from_timing(timing, true, examined, alerts))
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

    fn oblique_pair() -> FinalPair {
        let course = |runway_id, outer_fix, threshold| FinalCourseInput {
            runway_id,
            end_name: *b"18L",
            outer_fix,
            threshold,
        };
        prepare_pair(
            course(
                11,
                Local {
                    east_m: -1_000.0,
                    north_m: -1_000.0,
                },
                Local {
                    east_m: -400.0,
                    north_m: -200.0,
                },
            ),
            course(
                12,
                Local {
                    east_m: 1_000.0,
                    north_m: -250.0,
                },
                Local {
                    east_m: 1_600.0,
                    north_m: 550.0,
                },
            ),
            DEFAULT_CONFIG,
        )
        .expect("independent oblique parallel geometry is valid")
    }

    fn empty_tracks() -> PredictedTracks<'static> {
        PredictedTracks {
            addresses: &[],
            east_m: &[],
            north_m: &[],
            velocity_east_mps: &[],
            velocity_north_mps: &[],
            r95_m: &[],
            lat_e7: &[],
            lon_e7: &[],
        }
    }

    fn output_shape_refuses(output: FinalAlertBuffer<'_>) {
        let mut state = FinalMonitorState::new();
        let result = update(pair(), &mut state, 1, 1, empty_tracks(), output);
        assert_eq!(result, Err(UpdateError::BadInput));
        assert_eq!(state, FinalMonitorState::new());
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

    #[derive(Clone, Copy)]
    struct TestTrack {
        now: u64,
        report: u64,
        east: f64,
        north: f64,
        vx: f64,
        vy: f64,
        r95: f64,
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
        evaluate_for(
            pair(),
            state,
            TestTrack {
                now,
                report,
                east,
                north,
                vx,
                vy,
                r95: 0.0,
            },
        )
        .unwrap_or_else(|_| panic!("well-formed prediction was refused"))
    }

    fn evaluate_for(
        pair: FinalPair,
        state: &mut FinalMonitorState,
        track: TestTrack,
    ) -> Result<(UpdateResult, Option<FinalSeverity>), UpdateError> {
        let addresses = [1234];
        let east_m = [track.east];
        let north_m = [track.north];
        let velocity_east_mps = [track.vx];
        let velocity_north_mps = [track.vy];
        let r95_m = [track.r95];
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
            pair,
            state,
            track.now,
            track.report,
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
        let result = result?;
        Ok((
            result,
            out_severity.first().copied().filter(|_| result.alerts > 0),
        ))
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
            last_input_frame_ns: 100,
            last_evaluation_ns: 100,
            clock_flags: HAS_INPUT_FRAME_TIME | HAS_EVALUATION_TIME,
            reserved: [0; 7],
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
        assert_eq!(state.last_input_frame_ns, 100);
        assert_eq!(state.last_evaluation_ns, 100);
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

    // REQ: OA-RWY-014, OA-RWY-015
    #[test]
    fn oblique_pair_preserves_independent_east_north_geometry_and_conflict() {
        // Expected values come directly from the chosen vectors: (600,800)/1000 and
        // perpendicular (-0.8,0.6). The offset vector (2000,750) has signed separation
        // -1150 m; no monitor output is used to calculate these expectations.
        let pair = oblique_pair();
        assert!((pair.axis_east - 0.6).abs() < 1.0e-12);
        assert!((pair.axis_north - 0.8).abs() < 1.0e-12);
        assert!((pair.normal_east + 0.8).abs() < 1.0e-12);
        assert!((pair.normal_north - 0.6).abs() < 1.0e-12);
        assert!((pair.origin.east_m - 300.0).abs() < 1.0e-12);
        assert!((pair.origin.north_m + 225.0).abs() < 1.0e-12);
        assert!((pair.course_length_m - 1_000.0).abs() < 1.0e-9);

        let mut state = FinalMonitorState::new();
        let offset = 500.0;
        let track = TestTrack {
            now: 1_000,
            report: 1_000,
            east: pair.origin.east_m + pair.normal_east * offset,
            north: pair.origin.north_m + pair.normal_north * offset,
            vx: -pair.normal_east * 100.0,
            vy: -pair.normal_north * 100.0,
            r95: 0.0,
        };
        let (result, severity) = evaluate_for(pair, &mut state, track)
            .expect("finite east/north prediction is accepted");
        assert_eq!(result.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Caution));
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn uncertainty_at_adaptation_limit_is_accepted_and_next_value_refused() {
        let pair = pair();
        let mut at_limit_state = FinalMonitorState::new();
        let at_limit = evaluate_for(
            pair,
            &mut at_limit_state,
            TestTrack {
                now: 1_000,
                report: 1_000,
                east: 0.0,
                north: 0.0,
                vx: 0.0,
                vy: 0.0,
                r95: pair.max_r95_m,
            },
        );
        let (accepted, severity) = at_limit.expect("uncertainty at the bound is admissible");
        assert_eq!(accepted.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Warning));

        let mut beyond_limit_state = FinalMonitorState::new();
        let beyond = evaluate_for(
            pair,
            &mut beyond_limit_state,
            TestTrack {
                now: 1_000,
                report: 1_000,
                east: 0.0,
                north: 0.0,
                vx: 0.0,
                vy: 0.0,
                r95: pair.max_r95_m + f64::EPSILON * pair.max_r95_m.max(1.0),
            },
        );
        assert_eq!(beyond, Err(UpdateError::UnusableUncertainty));
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn each_misaligned_output_column_is_refused_before_state_changes() {
        macro_rules! buffers {
            () => {
                (
                    [0_u32],
                    [pair().id],
                    [FinalSeverity::Warning],
                    [0_i32],
                    [0_i32],
                    [0_u32],
                )
            };
        }

        let (_a, mut p, mut s, mut lat, mut lon, mut time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut [],
            pair_ids: &mut p,
            severity: &mut s,
            lat_e7: &mut lat,
            lon_e7: &mut lon,
            seconds_to_entry_milli: &mut time,
        });

        let (mut a, _p, mut s, mut lat, mut lon, mut time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut a,
            pair_ids: &mut [],
            severity: &mut s,
            lat_e7: &mut lat,
            lon_e7: &mut lon,
            seconds_to_entry_milli: &mut time,
        });

        let (mut a, mut p, _s, mut lat, mut lon, mut time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut a,
            pair_ids: &mut p,
            severity: &mut [],
            lat_e7: &mut lat,
            lon_e7: &mut lon,
            seconds_to_entry_milli: &mut time,
        });

        let (mut a, mut p, mut s, _lat, mut lon, mut time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut a,
            pair_ids: &mut p,
            severity: &mut s,
            lat_e7: &mut [],
            lon_e7: &mut lon,
            seconds_to_entry_milli: &mut time,
        });

        let (mut a, mut p, mut s, mut lat, _lon, mut time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut a,
            pair_ids: &mut p,
            severity: &mut s,
            lat_e7: &mut lat,
            lon_e7: &mut [],
            seconds_to_entry_milli: &mut time,
        });

        let (mut a, mut p, mut s, mut lat, mut lon, _time) = buffers!();
        output_shape_refuses(FinalAlertBuffer {
            addresses: &mut a,
            pair_ids: &mut p,
            severity: &mut s,
            lat_e7: &mut lat,
            lon_e7: &mut lon,
            seconds_to_entry_milli: &mut [],
        });
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
        let (result, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -545.0, 86.6, 50.0);
        assert_eq!(result.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Caution));

        // Mutation killed: treat an exact 4.8 s source interval as stale or over cadence.
        // The closer starting point puts the modeled sample inside the zone at the 4.8 s boundary.
        let (crossing, severity) = evaluate(
            &mut state,
            4_800_001_000,
            4_800_001_000,
            86.6 * 4.8,
            -545.0 + 50.0 * 4.8,
            86.6,
            50.0,
        );
        assert!(!crossing.degraded);
        assert_eq!(crossing.observed_input_interval_ns, DEGRADED_AFTER_NS);
        assert_eq!(crossing.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Warning));
    }

    // REQ: OA-RWY-015, OA-RWY-016, OA-RWY-017
    #[test]
    fn five_second_input_gap_suppresses_geometrically_inside_warning() {
        // Mutation killed: publish a warning from a trajectory after a >4.8 s input gap.
        let mut state = FinalMonitorState::new();
        let (first, severity) = evaluate(&mut state, 1_000, 1_000, 0.0, -545.0, 86.6, 50.0);
        assert!(!first.degraded);
        assert_eq!(severity, Some(FinalSeverity::Caution));

        let (late, severity) = evaluate(
            &mut state,
            5_000_001_000,
            5_000_001_000,
            86.6 * 5.0,
            -545.0 + 50.0 * 5.0,
            86.6,
            50.0,
        );
        assert!(late.due);
        assert!(late.input_cadence_degraded);
        assert!(!late.prediction_stale);
        assert_eq!(late.observed_input_interval_ns, 5_000_000_000);
        assert_eq!(late.alerts, 0);
        assert_eq!(severity, None);
    }

    // REQ: OA-RWY-017
    #[test]
    fn stale_prediction_sets_degraded() {
        let mut state = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut state, 4_800_001_001, 1_000, 0.0, 0.0, 0.0, 0.0);
        assert!(result.degraded);
        assert!(result.due);
        assert_eq!(result.alerts, 0);
        assert_eq!(severity, None);
    }

    // REQ: OA-RWY-015, OA-RWY-017
    #[test]
    fn prediction_at_exact_freshness_limit_is_accepted_but_one_nanosecond_later_is_not() {
        let mut at_limit = FinalMonitorState::new();
        let (result, severity) = evaluate(&mut at_limit, 4_800_001_000, 1_000, 0.0, 0.0, 0.0, 0.0);
        assert!(!result.degraded);
        assert_eq!(result.alerts, 1);
        assert_eq!(severity, Some(FinalSeverity::Warning));

        let mut past_limit = FinalMonitorState::new();
        let (result, severity) =
            evaluate(&mut past_limit, 4_800_001_001, 1_000, 0.0, 0.0, 0.0, 0.0);
        assert!(result.degraded);
        assert_eq!(result.alerts, 0);
        assert_eq!(severity, None);
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn uncertainty_wider_than_adaptation_clearance_refuses_before_warning() {
        let pair = pair();
        assert!(pair.max_r95_m > 0.0);
        let mut state = FinalMonitorState::new();
        let result = evaluate_for(
            pair,
            &mut state,
            TestTrack {
                now: 1_000,
                report: 1_000,
                east: 0.0,
                north: 0.0,
                vx: 0.0,
                vy: 0.0,
                r95: pair.max_r95_m + 0.01,
            },
        );
        assert_eq!(result, Err(UpdateError::UnusableUncertainty));
    }

    // REQ: OA-RWY-015, OA-RWY-016
    #[test]
    fn a_one_hertz_caller_reports_its_actual_two_second_monitor_interval() {
        let mut configured = pair();
        configured.update_ns = 1_200_000_000;
        let mut state = FinalMonitorState::new();
        let (first, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 1_000_000_000,
                report: 1_000_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .unwrap();
        assert_eq!(first.observed_interval_ns, 0);
        let (second, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 2_000_000_000,
                report: 2_000_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .unwrap();
        assert!(!second.due);
        let (third, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 3_000_000_000,
                report: 3_000_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .unwrap();
        assert!(third.due);
        assert_eq!(third.observed_interval_ns, 2_000_000_000);
        assert_eq!(third.observed_input_interval_ns, 1_000_000_000);
        assert!(
            !third.degraded,
            "fresh one-second input frames are not stale"
        );
    }

    // REQ: OA-RWY-015, OA-RWY-017
    #[test]
    fn input_receipt_cadence_is_independent_from_evaluation_and_source_age() {
        // Mutation killed: reuse the last due-evaluation timestamp as the input receipt clock.
        let mut configured = pair();
        configured.update_ns = 4_800_000_000;
        let mut state = FinalMonitorState::new();
        for now in [1_000_000_000, 5_800_000_000, 10_600_000_000, 15_400_000_000] {
            let (result, _) = evaluate_for(
                configured,
                &mut state,
                TestTrack {
                    now,
                    report: now,
                    east: 0.0,
                    north: -550.0,
                    vx: 0.0,
                    vy: 0.0,
                    r95: 0.0,
                },
            )
            .expect("exact-period input frame accepted");
            assert!(!result.degraded);
            assert_eq!(result.prediction_age_ns, 0);
            if result.observed_input_interval_ns != 0 {
                assert_eq!(result.observed_input_interval_ns, 4_800_000_000);
                assert_eq!(result.observed_interval_ns, 4_800_000_000);
            }
        }

        let (late_fresh, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 20_201_000_000,
                report: 20_201_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .expect("fresh prediction with late frame remains an explicit result");
        assert!(late_fresh.input_cadence_degraded);
        assert!(!late_fresh.prediction_stale);
        assert_eq!(late_fresh.observed_input_interval_ns, 4_801_000_000);
        assert_eq!(late_fresh.prediction_age_ns, 0);

        let (at_age_limit, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 25_001_000_000,
                report: 20_201_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .expect("prediction exactly at freshness limit remains accepted");
        assert!(!at_age_limit.prediction_stale);
        assert!(!at_age_limit.input_cadence_degraded);
        assert_eq!(at_age_limit.prediction_age_ns, 4_800_000_000);

        let (old_source, _) = evaluate_for(
            configured,
            &mut state,
            TestTrack {
                now: 25_002_000_000,
                report: 20_201_000_000,
                east: 0.0,
                north: -550.0,
                vx: 0.0,
                vy: 0.0,
                r95: 0.0,
            },
        )
        .expect("old source is reported as stale independently");
        assert!(!old_source.input_cadence_degraded);
        assert!(old_source.prediction_stale);
        assert_eq!(old_source.prediction_age_ns, 4_801_000_000);
    }

    // REQ: OA-RWY-015, OA-RWY-017
    #[test]
    fn regressing_input_time_cannot_advance_either_clock_or_output() {
        // Mutation killed: advance a clock before validating frame-time monotonicity.
        let mut state = FinalMonitorState {
            last_input_frame_ns: 5_000_000_000,
            last_evaluation_ns: 3_000_000_000,
            clock_flags: HAS_INPUT_FRAME_TIME | HAS_EVALUATION_TIME,
            reserved: [0; 7],
        };
        let original_state = state;
        let addresses = [1234];
        let east = [0.0];
        let north = [-550.0];
        let velocity_east = [0.0];
        let velocity_north = [0.0];
        let r95 = [0.0];
        let lat = [100];
        let lon = [200];
        let mut output_addresses = [77];
        let mut pair_ids = [pair().id];
        let mut severity = [FinalSeverity::Warning];
        let mut output_lat = [303];
        let mut output_lon = [404];
        let mut entry_time = [505];
        let result = update(
            pair(),
            &mut state,
            4_000_000_000,
            4_000_000_000,
            PredictedTracks {
                addresses: &addresses,
                east_m: &east,
                north_m: &north,
                velocity_east_mps: &velocity_east,
                velocity_north_mps: &velocity_north,
                r95_m: &r95,
                lat_e7: &lat,
                lon_e7: &lon,
            },
            FinalAlertBuffer {
                addresses: &mut output_addresses,
                pair_ids: &mut pair_ids,
                severity: &mut severity,
                lat_e7: &mut output_lat,
                lon_e7: &mut output_lon,
                seconds_to_entry_milli: &mut entry_time,
            },
        );
        assert_eq!(result, Err(UpdateError::ClockRegression));
        assert_eq!(state, original_state);
        assert_eq!(output_addresses, [77]);
        assert_eq!(severity, [FinalSeverity::Warning]);
        assert_eq!(output_lat, [303]);
        assert_eq!(output_lon, [404]);
        assert_eq!(entry_time, [505]);
    }

    // REQ: OA-RWY-016
    #[test]
    fn runway_derived_pair_refuses_inconsistent_geographic_and_local_positions() {
        let (plane, first, second) = kord_parallel_runways();
        let pair = prepare_centerline_pair(
            RunwayFinalSelection {
                runway_id: 27_001,
                runway: &first,
                end_name: *b"27R",
            },
            RunwayFinalSelection {
                runway_id: 27_002,
                runway: &second,
                end_name: *b"27L",
            },
            DEFAULT_CENTERLINE_EXTENSION_M,
            DEFAULT_CONFIG,
        )
        .expect("pinned runway pair");
        let geo = Geo {
            lat_deg: 41.990,
            lon_deg: -87.910,
        };
        let local = crate::plane::to_local(&plane, geo).expect("point inside runway plane");
        let addresses = [1234];
        let east_m = [local.east_m + 20.0];
        let north_m = [local.north_m];
        let velocity_east_mps = [0.0];
        let velocity_north_mps = [0.0];
        let r95_m = [0.0];
        let lat_e7 = [(geo.lat_deg * 10_000_000.0) as i32];
        let lon_e7 = [(geo.lon_deg * 10_000_000.0) as i32];
        let mut out_addr = [0];
        let mut out_pair = [pair.id];
        let mut out_severity = [FinalSeverity::Caution];
        let mut out_lat = [0];
        let mut out_lon = [0];
        let mut out_time = [0];
        let result = update(
            pair,
            &mut FinalMonitorState::new(),
            1_000_000_000,
            1_000_000_000,
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
        assert_eq!(result, Err(UpdateError::CoordinateMismatch));
    }

    // REQ: OA-RWY-018
    #[test]
    fn close_parallel_finals_are_refused() {
        let result = prepare_pair(input(1, -400.0), input(2, 400.0), DEFAULT_CONFIG);
        assert_eq!(result, Err(PrepareError::TooClose));
    }

    // REQ: OA-RWY-015
    #[test]
    fn nonunique_or_zero_track_addresses_are_refused_atomically() {
        for addresses in [[1234, 1234], [0, 1234], [1235, 1234]] {
            let mut state = FinalMonitorState::new();
            let east = [0.0, 0.0];
            let north = [-550.0, -550.0];
            let velocity_east = [0.0, 0.0];
            let velocity_north = [0.0, 0.0];
            let r95 = [0.0, 0.0];
            let lat = [100, 101];
            let lon = [200, 201];
            let mut out_addresses = [77, 78];
            let mut out_pair_ids = [pair().id; 2];
            let mut out_severity = [FinalSeverity::Warning; 2];
            let mut out_lat = [303, 304];
            let mut out_lon = [404, 405];
            let mut out_entry = [505, 506];
            let result = update(
                pair(),
                &mut state,
                1,
                1,
                PredictedTracks {
                    addresses: &addresses,
                    east_m: &east,
                    north_m: &north,
                    velocity_east_mps: &velocity_east,
                    velocity_north_mps: &velocity_north,
                    r95_m: &r95,
                    lat_e7: &lat,
                    lon_e7: &lon,
                },
                FinalAlertBuffer {
                    addresses: &mut out_addresses,
                    pair_ids: &mut out_pair_ids,
                    severity: &mut out_severity,
                    lat_e7: &mut out_lat,
                    lon_e7: &mut out_lon,
                    seconds_to_entry_milli: &mut out_entry,
                },
            );
            assert_eq!(result, Err(UpdateError::BadInput));
            assert_eq!(state, FinalMonitorState::new());
            assert_eq!(out_addresses, [77, 78]);
            assert_eq!(out_severity, [FinalSeverity::Warning; 2]);
            assert_eq!(out_lat, [303, 304]);
            assert_eq!(out_lon, [404, 405]);
            assert_eq!(out_entry, [505, 506]);
        }
    }

    // REQ: OA-RWY-015, OA-RWY-017
    #[test]
    fn inconsistent_public_clock_state_is_refused_without_advancing() {
        let invalid_states = [
            FinalMonitorState {
                clock_flags: KNOWN_CLOCK_FLAGS | 0x80,
                ..FinalMonitorState::new()
            },
            FinalMonitorState {
                last_input_frame_ns: 10,
                last_evaluation_ns: 11,
                clock_flags: KNOWN_CLOCK_FLAGS,
                ..FinalMonitorState::new()
            },
            FinalMonitorState {
                last_evaluation_ns: 10,
                clock_flags: HAS_EVALUATION_TIME,
                ..FinalMonitorState::new()
            },
            FinalMonitorState {
                reserved: [1, 0, 0, 0, 0, 0, 0],
                ..FinalMonitorState::new()
            },
        ];
        for mut state in invalid_states {
            let original = state;
            let result = evaluate_for(
                pair(),
                &mut state,
                TestTrack {
                    now: 20,
                    report: 20,
                    east: 0.0,
                    north: 0.0,
                    vx: 0.0,
                    vy: 0.0,
                    r95: 0.0,
                },
            );
            assert_eq!(result, Err(UpdateError::BadInput));
            assert_eq!(state, original);
        }
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

    // REQ: OA-RWY-016
    #[test]
    fn nonfinite_pair_uncertainty_limit_refuses_atomically_at_update() {
        // Mutation killed: omit max_r95_m validation so NaN/Infinity disable the clearance cap.
        for invalid_limit in [f64::NAN, f64::INFINITY] {
            let mut invalid_pair = pair();
            invalid_pair.max_r95_m = invalid_limit;
            assert_bad_input_is_atomic(invalid_pair, 0.0, 0.0, 0.0, 0.0);
        }
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
            last_input_frame_ns: 100,
            last_evaluation_ns: 100,
            clock_flags: HAS_INPUT_FRAME_TIME | HAS_EVALUATION_TIME,
            reserved: [0; 7],
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
        assert_eq!(state.last_input_frame_ns, 100);
        assert_eq!(state.last_evaluation_ns, 100);
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
        assert_eq!(state.last_input_frame_ns, 100);
        assert_eq!(state.last_evaluation_ns, 100);
        assert_eq!(out_addr, [77]);
        assert_eq!(out_pair, [sentinel_pair]);
        assert_eq!(out_severity, [FinalSeverity::Warning]);
        assert_eq!(out_lat, [303]);
        assert_eq!(out_lon, [404]);
        assert_eq!(out_time, [505]);
    }
}
