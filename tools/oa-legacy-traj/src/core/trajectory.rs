//! Shared issue-time trajectory baseline (pure core, B1/B2/B4/B7).
//!
//! This is a deliberately limited constant-groundspeed/constant-vertical-rate model. It is not
//! aircraft-performance or wind qualified, does not model forward uncertainty, and never claims
//! a confidence bound. All times and state are caller supplied. Inputs and outputs are values;
//! there are no callbacks and no allocation.

use crate::core::cross::great_circle_m;
use crate::core::types::Place;

/// One converted route is bounded to 256 points (15,360 bytes); longer snapshots are refused.
/// This is an offline baseline envelope, not a claim about every published route.
pub const MAX_ROUTE_POINTS: usize = 256;
/// Evidence admits at most 256 rows (24,576 bytes) and 32,640 pair comparisons; larger sets are
/// refused rather than treated as a claim about all traffic volumes.
pub const MAX_REVISION_EVIDENCE: usize = 256;
pub const HORIZON_COUNT: usize = 3;
pub const HORIZON_5_MIN_MS: i64 = 300_000;
pub const HORIZON_10_MIN_MS: i64 = 600_000;
pub const HORIZON_20_MIN_MS: i64 = 1_200_000;
pub const HORIZON_MS: [i64; HORIZON_COUNT] =
    [HORIZON_5_MIN_MS, HORIZON_10_MIN_MS, HORIZON_20_MIN_MS];
/// Fixed ID-byte comparisons made by route identity validation and exact ETA matching.
pub const FIX_IDENTITY_BYTE_WORK: usize = 12 + 4 + 12;
pub const FLIGHT_KEY_IDENTITY_BYTE_WORK: usize = 16 + 4 + 4 + 12;
pub const ROUTE_BINDING_HASH_BYTE_WORK: usize = 32;
pub const OWNER_UNIT_ID_BYTE_WORK: usize = 8;
pub const ROUTE_POINT_RESERVED_BYTE_WORK: usize = 1;
/// Independent altitude and speed channel tags validated once per route point.
pub const ROUTE_CONSTRAINT_CHANNEL_WORK: usize = 2;
pub const ROUTE_PROVENANCE_RESERVED_BYTE_WORK: usize = 4;
pub const CURSOR_RESERVED_BYTE_WORK: usize = 5;
pub const OWNER_RESERVED_BYTE_WORK: usize = 7;
pub const CLEARANCE_RESERVED_BYTE_WORK: usize = 3;
pub const REVISION_RESERVED_BYTE_WORK: usize = 4;
/// Checked before scanning: cursor search, suffix validation, route walks, ETA search, bounded
/// identity/reserved-byte checks, plus one cursor and flight-key check.
pub const MAX_FORECAST_WORK: usize = MAX_ROUTE_POINTS
    * (HORIZON_COUNT
        + 3
        + 3 * FIX_IDENTITY_BYTE_WORK
        + ROUTE_POINT_RESERVED_BYTE_WORK
        + ROUTE_CONSTRAINT_CHANNEL_WORK)
    + ROUTE_PROVENANCE_RESERVED_BYTE_WORK
    + CURSOR_RESERVED_BYTE_WORK
    + OWNER_RESERVED_BYTE_WORK
    + CLEARANCE_RESERVED_BYTE_WORK
    + 2 * FIX_IDENTITY_BYTE_WORK
    + 3 * FLIGHT_KEY_IDENTITY_BYTE_WORK
    + 2 * ROUTE_BINDING_HASH_BYTE_WORK
    + OWNER_UNIT_ID_BYTE_WORK;
/// Pair comparisons needed to detect same-source payload conflicts at evidence capacity.
pub const MAX_EVIDENCE_COMPARISONS: usize = MAX_REVISION_EVIDENCE * (MAX_REVISION_EVIDENCE - 1) / 2;
/// Each evidence row's fixed identity/revision bytes and each pairwise same-source comparison.
pub const EVIDENCE_REVISION_BYTE_WORK: usize =
    FLIGHT_KEY_IDENTITY_BYTE_WORK + 8 + 8 + 8 + 32 + REVISION_RESERVED_BYTE_WORK;
pub const FLIGHT_CENTRE_MATCH_BYTE_WORK: usize = 16 + 4 + 4;
pub const MAX_EVIDENCE_SCAN_WORK: usize = MAX_REVISION_EVIDENCE
    * (4 + FLIGHT_KEY_IDENTITY_BYTE_WORK
        + FLIGHT_CENTRE_MATCH_BYTE_WORK
        + 2 * EVIDENCE_REVISION_BYTE_WORK)
    + MAX_EVIDENCE_COMPARISONS * EVIDENCE_REVISION_BYTE_WORK
    + FLIGHT_KEY_IDENTITY_BYTE_WORK;
pub const NO_ALTITUDE_FT: i32 = i32::MIN;
pub const NO_SPEED_MMPS: u32 = u32::MAX;
pub const NO_VERTICAL_RATE_FPM: i32 = i32::MIN;
pub const NO_POSITION_E7: i32 = i32::MIN;

/// Full stable flight identity. GUFI and SFDPS plan identity stay lossless across the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FlightKey {
    /// All 128 GUFI bits in network byte order; bytes avoid u128's 16-byte tail padding.
    pub gufi: [u8; 16],
    pub aircraft_address: u32,
    pub centre: [u8; 4],
    pub plan_id: [u8; 12],
}
const _: () = assert!(core::mem::size_of::<FlightKey>() == 36);

/// Fixed-width, length-tagged route identifier; the unused tail must be zero-filled by adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RouteIdentifier {
    pub len: u8,
    pub bytes: [u8; 12],
}
const _: () = assert!(core::mem::size_of::<RouteIdentifier>() == 13);

/// The complete identity of a converted route point, including region and scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct FixIdentity {
    pub ident: RouteIdentifier,
    pub region: [u8; 4],
    pub scope: RouteIdentifier,
    pub class: u8,
}
const _: () = assert!(core::mem::size_of::<FixIdentity>() == 31);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PathGeometry {
    GreatCircleLeg = 0,
    DirectTo = 1,
    VectorsGap = 2,
    Unsupported = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SpeedReference {
    GroundSpeed = 0,
    IndicatedAirspeed = 1,
    Unknown = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AltitudeConstraintKind {
    None = 0,
    Minimum = 1,
    Maximum = 2,
    Window = 3,
    AtAltitude = 4,
    Unsupported = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SpeedConstraintKind {
    None = 0,
    Minimum = 1,
    Maximum = 2,
    Window = 3,
    Unsupported = 255,
}

/// One normalized route point. Coordinates are degrees × 1e7; constraints use feet and mm/s.
/// The two constraint channels describe the populated bounds independently, so a fix can carry
/// altitude and speed restrictions together. Unsupported forms remain explicit and degrade the
/// relevant output instead of being silently ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RoutePoint {
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub min_altitude_ft: i32,
    pub max_altitude_ft: i32,
    pub min_speed_mmps: u32,
    pub max_speed_mmps: u32,
    pub fix: FixIdentity,
    pub altitude_constraint_kind: AltitudeConstraintKind,
    pub speed_constraint_kind: SpeedConstraintKind,
    pub geometry: PathGeometry,
    pub speed_reference: SpeedReference,
    /// A7: explicit zero bytes complete the 60-byte aligned record.
    pub reserved: [u8; 1],
}
// A7: 24 numeric + 31 identity + 4 enum + 1 explicit alignment byte = 60; A3: the bounded
// per-flight snapshot is capped at 256 records (15,360 bytes), with longer routes refused.
const _: () = assert!(core::mem::size_of::<RoutePoint>() == 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RouteProvenance {
    pub source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    pub payload_sha256: [u8; 32],
    pub key: FlightKey,
    /// A7: explicit zero bytes complete the 96-byte aligned record.
    pub reserved: [u8; 4],
}
// A7: 24 times + 32 hash + 36 key + 4 explicit alignment bytes = 96.
const _: () = assert!(core::mem::size_of::<RouteProvenance>() == 96);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct ActiveRouteCursor {
    pub source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    pub route_payload_sha256: [u8; 32],
    pub key: FlightKey,
    pub next_fix: FixIdentity,
    /// A7: explicit zero bytes complete the 128-byte aligned record.
    pub reserved: [u8; 5],
}
// A7: 24 times + 32 hash + 36 key + 31 fix + 5 explicit alignment bytes = 128.
const _: () = assert!(core::mem::size_of::<ActiveRouteCursor>() == 128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct OwnerProvenance {
    pub source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    pub owner_unit: [u8; 8],
    pub owner_state: OwnerState,
    /// A7: explicit zero bytes complete the 40-byte aligned record.
    pub reserved: [u8; 7],
}
// A7: 24 times + 8 unit + 1 state + 7 explicit alignment bytes = 40.
const _: () = assert!(core::mem::size_of::<OwnerProvenance>() == 40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OwnerState {
    Known = 0,
    Unknown = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ClearanceForm {
    Absent = 0,
    Exact = 1,
    Window = 2,
    AltitudeAtFix = 3,
    Block = 4,
    Vfr = 5,
    Unsupported = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Clearance {
    pub source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    /// Inclusive published interval. Use `NO_ALTITUDE_FT` for an absent side.
    pub min_altitude_ft: i32,
    pub max_altitude_ft: i32,
    /// Altitude-at-fix carried by the current clearance, otherwise `NO_ALTITUDE_FT`.
    pub altitude_at_fix_ft: i32,
    pub form: ClearanceForm,
    /// A7: explicit zero bytes complete the 40-byte aligned record.
    pub reserved: [u8; 3],
}
// A7: 24 times + 12 altitudes + 1 form + 3 explicit alignment bytes = 40.
const _: () = assert!(core::mem::size_of::<Clearance>() == 40);

/// Frozen shell flight state. `groundspeed_mmps` is a measured ground speed, not an airspeed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct CurrentTrack {
    /// Emitted tracker state epoch; coordinates and velocities are already predicted to this time.
    pub state_ms: i64,
    /// Time of the newest actual sensor measurement supporting this state.
    pub newest_source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    /// Positive aircraft identity from surveillance, independent of any flight-plan join.
    pub aircraft_address: u32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub vertical_rate_fpm: i32,
    pub groundspeed_mmps: u32,
}
const _: () = assert!(core::mem::size_of::<CurrentTrack>() == 56);

/// An optional exact route-point key at which an ETA is requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct EtaTarget {
    // A6: this is one cold issue-request value, not a collection field.
    pub present: bool,
    pub fix: FixIdentity,
}
const _: () = assert!(core::mem::size_of::<EtaTarget>() == 32);

/// A7.2: cold borrowed one-shot input value, neither stored nor iterated as a collection. Caller
/// passes the already selected source revision and frozen owner state; this function chooses no
/// route and mutates no flight state.
pub struct TrajectoryIssueInput<'a> {
    pub key: FlightKey,
    pub route: RouteProvenance,
    pub cutoff_ms: i64,
    /// Maximum permitted age of the frozen track report at cutoff; explicit caller policy.
    pub max_track_age_ms: i64,
    pub owner: OwnerProvenance,
    /// Explicit active suffix binding; no inference from proximity or ETA target.
    pub route_cursor: Option<ActiveRouteCursor>,
    pub track: CurrentTrack,
    pub clearance: Clearance,
    pub eta_target: EtaTarget,
    pub points: &'a [RoutePoint],
}
// A7.2: cold, borrowed one-shot input value; it is neither stored nor iterated as a collection.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Refusal {
    None = 0,
    RouteTooLong = 1,
    WorkLimit = 2,
    RouteUnavailableAtCutoff = 3,
    SourceAfterAvailability = 4,
    InvalidCoordinate = 5,
    NoGroundspeed = 6,
    RouteHasNoEnd = 7,
    UnsupportedGeometry = 8,
    UnsupportedConstraint = 9,
    InfeasibleAltitudeWindow = 10,
    InfeasibleSpeedWindow = 11,
    TimeOverflow = 12,
    InvalidTrack = 13,
    StaleTrack = 14,
    FutureTrack = 15,
    ClearanceUnavailableAtCutoff = 16,
    OwnerUnavailableAtCutoff = 17,
    UnsupportedClearanceForm = 18,
    NoAltitude = 19,
    AltitudeOverflow = 20,
    InvalidFlightIdentity = 21,
    InvalidFixIdentity = 22,
    EtaTargetNotFound = 23,
    ActiveLegUnknown = 24,
    AmbiguousActiveLeg = 25,
    CursorUnavailableAtCutoff = 26,
    RouteIdentityMismatch = 27,
    CursorIdentityMismatch = 28,
    TrackIdentityMismatch = 29,
    AmbiguousEtaTarget = 30,
    InvalidReservedFields = 31,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Availability {
    Available = 0,
    Unavailable = 1,
    Degraded = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct HorizonOutput {
    pub horizon_ms: i64,
    pub predicted_time_ms: i64,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub altitude_ft: i32,
    pub groundspeed_mmps: u32,
    pub flags: u32,
    pub available: Availability,
    pub refusal: Refusal,
    /// A7: explicit zero bytes complete the 40-byte aligned record.
    pub reserved: [u8; 2],
}
// A7: 16 time + 20 numeric + 2 enums + 2 explicit alignment bytes = 40.
const _: () = assert!(core::mem::size_of::<HorizonOutput>() == 40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct EtaOutput {
    pub eta_ms: i64,
    pub fix: FixIdentity,
    pub available: Availability,
    pub refusal: Refusal,
    /// A7: explicit zero bytes complete the 48-byte aligned record.
    pub reserved: [u8; 7],
}
// A7: 8 time + 31 fix + 2 enums + 7 explicit alignment bytes = 48.
const _: () = assert!(core::mem::size_of::<EtaOutput>() == 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct TrajectoryIssueOutput {
    pub key: FlightKey,
    pub examined_route_points: u16,
    pub model_flags: u16,
    pub prior_route_points_ignored: u16,
    /// A7: explicit zero bytes complete the first 48-byte aligned block.
    pub reserved: [u8; 6],
    pub route: RouteProvenance,
    pub cutoff_ms: i64,
    pub horizons: [HorizonOutput; HORIZON_COUNT],
    pub eta: EtaOutput,
    pub over_capacity_unexamined: u64,
}
// A7: 36 key + 6 counters + 6 explicit bytes + 96 route + 8 cutoff + 120 horizons
// + 48 ETA + 8 overflow count = 328. A3: exactly three named horizons, no variable collection.
const _: () = assert!(core::mem::size_of::<TrajectoryIssueOutput>() == 328);

/// Flags state precisely what this baseline omits or degrades.
pub const MODEL_CONSTANT_GROUNDSPEED: u16 = 1;
pub const MODEL_CONSTANT_VERTICAL_RATE: u16 = 2;
pub const MODEL_WIND_NOT_MODELLED: u16 = 4;
pub const MODEL_PERFORMANCE_NOT_MODELLED: u16 = 8;
pub const MODEL_FORWARD_UNCERTAINTY_NOT_MODELLED: u16 = 16;
pub const MODEL_OWNER_UNKNOWN: u16 = 32;
pub const MODEL_CLEARANCE_UNAVAILABLE: u16 = 64;
pub const MODEL_ROUTE_CONSTRAINT_UNSUPPORTED: u16 = 128;
pub const MODEL_ALTITUDE_UNAVAILABLE: u16 = 256;
pub const OUTPUT_CAPTURED_ALTITUDE: u32 = 1;
pub const OUTPUT_CONSTRAINT_CHECKED: u32 = 2;
pub const OUTPUT_VECTORS_GAP: u32 = 4;
pub const OUTPUT_OWNER_UNKNOWN: u32 = 8;
pub const OUTPUT_CLEARANCE_UNAVAILABLE: u32 = 16;
pub const OUTPUT_CONSTRAINT_UNSUPPORTED: u32 = 32;
pub const OUTPUT_SPEED_CONSTRAINT_VIOLATED: u32 = 64;

const LAT_E7_LIMIT: i32 = 900_000_000;
const LON_E7_LIMIT: i32 = 1_800_000_000;
const EARTH_RADIUS_M: f64 = 6_371_008.8;
const RAD_TO_E7_DEG: f64 = 572_957_795.1308232;

fn unavailable_horizon(horizon_ms: i64, refusal: Refusal) -> HorizonOutput {
    HorizonOutput {
        horizon_ms,
        available: Availability::Unavailable,
        refusal,
        lat_e7: NO_POSITION_E7,
        lon_e7: NO_POSITION_E7,
        altitude_ft: NO_ALTITUDE_FT,
        groundspeed_mmps: NO_SPEED_MMPS,
        predicted_time_ms: 0,
        flags: 0,
        reserved: [0; 2],
    }
}

fn coord_ok(lat_e7: i32, lon_e7: i32) -> bool {
    (-LAT_E7_LIMIT..=LAT_E7_LIMIT).contains(&lat_e7)
        && (-LON_E7_LIMIT..=LON_E7_LIMIT).contains(&lon_e7)
}

fn same_fix(a: FixIdentity, b: FixIdentity) -> bool {
    a.ident.len == b.ident.len
        && a.ident.bytes == b.ident.bytes
        && a.region == b.region
        && a.scope.len == b.scope.len
        && a.scope.bytes == b.scope.bytes
        && a.class == b.class
}

fn identifier_ok(id: RouteIdentifier) -> bool {
    if id.len == 0 || id.len > 12 {
        return false;
    }
    for (i, byte) in id.bytes.iter().enumerate() {
        if i < usize::from(id.len) {
            if !byte.is_ascii_graphic() {
                return false;
            }
        } else if *byte != 0 {
            return false;
        }
    }
    true
}

fn optional_identifier_ok(id: RouteIdentifier) -> bool {
    if id.len == 0 {
        id.bytes.iter().all(|byte| *byte == 0)
    } else {
        identifier_ok(id)
    }
}

fn padded_identifier_ok(bytes: &[u8]) -> bool {
    let mut present = false;
    let mut padding = false;
    // Called only with the fixed 4-, 8-, and 12-byte fields of this module.
    for byte in bytes {
        if *byte == 0 {
            padding = true;
        } else if padding || !byte.is_ascii_graphic() {
            return false;
        } else {
            present = true;
        }
    }
    present
}

fn reserved_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn fix_identity_ok(fix: FixIdentity) -> bool {
    identifier_ok(fix.ident)
        && optional_identifier_ok(fix.scope)
        && padded_identifier_ok(&fix.region)
        && fix.class != 0
}

fn flight_key_ok(key: FlightKey) -> bool {
    key.gufi != [0; 16]
        && key.aircraft_address != 0
        && key.aircraft_address <= 0x01ff_ffff
        && padded_identifier_ok(&key.centre)
        && padded_identifier_ok(&key.plan_id)
}

fn owner_unit_ok(owner: OwnerProvenance) -> bool {
    padded_identifier_ok(&owner.owner_unit)
}

fn round_e7(value: f64) -> Option<i32> {
    if value < i32::MIN as f64 || value > i32::MAX as f64 {
        return None;
    }
    let rounded = if value >= 0.0 {
        value + 0.5
    } else {
        value - 0.5
    };
    if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
        None
    } else {
        Some(rounded as i32)
    }
}

fn interpolate_great_circle(
    start: Place,
    end: Place,
    start_e7: (i32, i32),
    end_e7: (i32, i32),
    ratio: f64,
    distance_m: f64,
) -> Option<(i32, i32)> {
    if distance_m == 0.0 || ratio <= 0.0 {
        return Some(start_e7);
    }
    if ratio >= 1.0 {
        return Some(end_e7);
    }
    let phi1 = start.lat_deg * (core::f64::consts::PI / 180.0);
    let phi2 = end.lat_deg * (core::f64::consts::PI / 180.0);
    let lambda1 = start.lon_deg * (core::f64::consts::PI / 180.0);
    let lambda2 = end.lon_deg * (core::f64::consts::PI / 180.0);
    let delta = distance_m / EARTH_RADIUS_M;
    let sin_delta = oa_math::sin(delta);
    if (-1e-12..=1e-12).contains(&sin_delta) {
        return None;
    }
    let a = oa_math::sin((1.0 - ratio) * delta) / sin_delta;
    let b = oa_math::sin(ratio * delta) / sin_delta;
    let x = a * oa_math::cos(phi1) * oa_math::cos(lambda1)
        + b * oa_math::cos(phi2) * oa_math::cos(lambda2);
    let y = a * oa_math::cos(phi1) * oa_math::sin(lambda1)
        + b * oa_math::cos(phi2) * oa_math::sin(lambda2);
    let z = a * oa_math::sin(phi1) + b * oa_math::sin(phi2);
    let lat = oa_math::atan2(z, oa_math::hypot(x, y)) * RAD_TO_E7_DEG;
    let lon = oa_math::atan2(y, x) * RAD_TO_E7_DEG;
    if lat >= -(LAT_E7_LIMIT as f64)
        && lat <= LAT_E7_LIMIT as f64
        && lon >= -(LON_E7_LIMIT as f64)
        && lon <= LON_E7_LIMIT as f64
    {
        match (round_e7(lat), round_e7(lon)) {
            (Some(lat_i), Some(lon_i)) => Some((lat_i, lon_i)),
            _ => None,
        }
    } else {
        None
    }
}

fn output_unavailable(input: &TrajectoryIssueInput<'_>, refusal: Refusal) -> TrajectoryIssueOutput {
    TrajectoryIssueOutput {
        key: input.key,
        route: input.route,
        cutoff_ms: input.cutoff_ms,
        horizons: [
            unavailable_horizon(HORIZON_5_MIN_MS, refusal),
            unavailable_horizon(HORIZON_10_MIN_MS, refusal),
            unavailable_horizon(HORIZON_20_MIN_MS, refusal),
        ],
        eta: EtaOutput {
            available: Availability::Unavailable,
            refusal,
            fix: input.eta_target.fix,
            eta_ms: 0,
            reserved: [0; 7],
        },
        examined_route_points: 0,
        model_flags: MODEL_WIND_NOT_MODELLED
            | MODEL_PERFORMANCE_NOT_MODELLED
            | MODEL_FORWARD_UNCERTAINTY_NOT_MODELLED,
        prior_route_points_ignored: 0,
        over_capacity_unexamined: if refusal == Refusal::RouteTooLong {
            input.points.len() as u64
        } else {
            0
        },
        reserved: [0; 6],
    }
}

fn output_route_refusal(
    input: &TrajectoryIssueInput<'_>,
    refusal: Refusal,
    examined: usize,
    prior_ignored: usize,
) -> TrajectoryIssueOutput {
    let mut output = output_unavailable(input, refusal);
    output.examined_route_points = examined as u16;
    output.prior_route_points_ignored = prior_ignored as u16;
    output
}

/// Produce the shared bounded baseline forecast. Route scans are capped at 256 points and the
/// declared work budget is checked before conversion/walking. Route-end predictions are explicitly
/// refused when the horizon extends beyond the last published point.
pub fn issue_trajectory(input: &TrajectoryIssueInput<'_>) -> TrajectoryIssueOutput {
    if input.points.len() > MAX_ROUTE_POINTS {
        return output_unavailable(input, Refusal::RouteTooLong);
    }
    let planned_work = input
        .points
        .len()
        .saturating_mul(
            HORIZON_COUNT
                + 3
                + 3 * FIX_IDENTITY_BYTE_WORK
                + ROUTE_POINT_RESERVED_BYTE_WORK
                + ROUTE_CONSTRAINT_CHANNEL_WORK,
        )
        .saturating_add(
            2 * FIX_IDENTITY_BYTE_WORK
                + 3 * FLIGHT_KEY_IDENTITY_BYTE_WORK
                + 2 * ROUTE_BINDING_HASH_BYTE_WORK
                + OWNER_UNIT_ID_BYTE_WORK
                + ROUTE_PROVENANCE_RESERVED_BYTE_WORK
                + CURSOR_RESERVED_BYTE_WORK
                + OWNER_RESERVED_BYTE_WORK
                + CLEARANCE_RESERVED_BYTE_WORK,
        );
    if planned_work > MAX_FORECAST_WORK {
        return output_unavailable(input, Refusal::WorkLimit);
    }
    if input.route.source_ms > input.route.available_ms {
        return output_unavailable(input, Refusal::SourceAfterAvailability);
    }
    if input.route.available_ms > input.cutoff_ms {
        return output_unavailable(input, Refusal::RouteUnavailableAtCutoff);
    }
    if input.route.key != input.key {
        return output_unavailable(input, Refusal::RouteIdentityMismatch);
    }
    if !reserved_zero(&input.route.reserved) {
        return output_unavailable(input, Refusal::InvalidReservedFields);
    }
    if input.track.aircraft_address != input.key.aircraft_address {
        return output_unavailable(input, Refusal::TrackIdentityMismatch);
    }
    if input.track.available_ms > input.cutoff_ms || input.track.state_ms > input.cutoff_ms {
        return output_unavailable(input, Refusal::FutureTrack);
    }
    if input.track.newest_source_ms > input.track.state_ms
        || input.track.state_ms > input.track.available_ms
    {
        return output_unavailable(input, Refusal::FutureTrack);
    }
    if input.max_track_age_ms < 0 {
        return output_unavailable(input, Refusal::StaleTrack);
    }
    let track_age_ms = match input.cutoff_ms.checked_sub(input.track.newest_source_ms) {
        Some(value) => value,
        None => return output_unavailable(input, Refusal::TimeOverflow),
    };
    if track_age_ms > input.max_track_age_ms {
        return output_unavailable(input, Refusal::StaleTrack);
    }
    let clearance_absent = input.clearance.form == ClearanceForm::Absent
        && input.clearance.source_ms == 0
        && input.clearance.available_ms == 0
        && input.clearance.message_id == 0;
    let clearance_evidence_valid = clearance_absent
        || (input.clearance.available_ms <= input.cutoff_ms
            && input.clearance.source_ms <= input.clearance.available_ms);
    let clearance_supported = (input.clearance.form == ClearanceForm::Exact
        && input.clearance.min_altitude_ft != NO_ALTITUDE_FT
        && input.clearance.min_altitude_ft == input.clearance.max_altitude_ft)
        || input.clearance.form == ClearanceForm::Window;
    let owner_valid = input.owner.owner_state == OwnerState::Unknown
        || (owner_unit_ok(input.owner)
            && input.owner.available_ms <= input.cutoff_ms
            && input.owner.source_ms <= input.owner.available_ms);
    let owner_evidence_valid = input.owner.available_ms <= input.cutoff_ms
        && input.owner.source_ms <= input.owner.available_ms;
    if owner_evidence_valid && !reserved_zero(&input.owner.reserved) {
        return output_unavailable(input, Refusal::InvalidReservedFields);
    }
    if clearance_evidence_valid && !reserved_zero(&input.clearance.reserved) {
        return output_unavailable(input, Refusal::InvalidReservedFields);
    }
    if !coord_ok(input.track.lat_e7, input.track.lon_e7) {
        return output_unavailable(input, Refusal::InvalidTrack);
    }
    if !flight_key_ok(input.key) {
        return output_unavailable(input, Refusal::InvalidFlightIdentity);
    }
    let cursor = match input.route_cursor {
        Some(value) => value,
        None => return output_unavailable(input, Refusal::ActiveLegUnknown),
    };
    if cursor.source_ms > cursor.available_ms || cursor.available_ms > input.cutoff_ms {
        return output_unavailable(input, Refusal::CursorUnavailableAtCutoff);
    }
    if cursor.key != input.key || cursor.route_payload_sha256 != input.route.payload_sha256 {
        return output_unavailable(input, Refusal::CursorIdentityMismatch);
    }
    if !reserved_zero(&cursor.reserved) {
        return output_unavailable(input, Refusal::InvalidReservedFields);
    }
    if !fix_identity_ok(cursor.next_fix) {
        return output_unavailable(input, Refusal::InvalidFixIdentity);
    }
    if input.points.is_empty() {
        return output_unavailable(input, Refusal::RouteHasNoEnd);
    }
    let mut active_point = None;
    let mut active_index = 0usize;
    let mut matches = 0usize;
    for point in input.points.iter() {
        if same_fix(point.fix, cursor.next_fix) {
            if active_point.is_none() {
                active_point = Some(*point);
            }
            matches += 1;
        } else if active_point.is_none() {
            active_index += 1;
        }
    }
    let first = match active_point {
        Some(point) if matches == 1 => point,
        Some(_) => {
            return output_route_refusal(input, Refusal::AmbiguousActiveLeg, input.points.len(), 0)
        }
        None => {
            return output_route_refusal(input, Refusal::ActiveLegUnknown, input.points.len(), 0)
        }
    };
    if input.track.groundspeed_mmps == 0 || input.track.groundspeed_mmps == NO_SPEED_MMPS {
        return output_unavailable(input, Refusal::NoGroundspeed);
    }
    if clearance_evidence_valid
        && input.clearance.min_altitude_ft != NO_ALTITUDE_FT
        && input.clearance.max_altitude_ft != NO_ALTITUDE_FT
        && input.clearance.min_altitude_ft > input.clearance.max_altitude_ft
    {
        return output_unavailable(input, Refusal::InfeasibleAltitudeWindow);
    }
    let mut unsupported_route_constraint = false;
    let mut validated_route_points = 0usize;
    for p in input.points.iter().skip(active_index) {
        validated_route_points += 1;
        if !reserved_zero(&p.reserved) {
            return output_route_refusal(
                input,
                Refusal::InvalidReservedFields,
                validated_route_points,
                active_index,
            );
        }
        if !coord_ok(p.lat_e7, p.lon_e7) {
            return output_route_refusal(
                input,
                Refusal::InvalidCoordinate,
                validated_route_points,
                active_index,
            );
        }
        if !fix_identity_ok(p.fix) {
            return output_route_refusal(
                input,
                Refusal::InvalidFixIdentity,
                validated_route_points,
                active_index,
            );
        }
        if p.geometry == PathGeometry::Unsupported {
            return output_route_refusal(
                input,
                Refusal::UnsupportedGeometry,
                validated_route_points,
                active_index,
            );
        }
        if p.altitude_constraint_kind == AltitudeConstraintKind::Window
            && p.min_altitude_ft > p.max_altitude_ft
        {
            return output_route_refusal(
                input,
                Refusal::InfeasibleAltitudeWindow,
                validated_route_points,
                active_index,
            );
        }
        if matches!(
            p.altitude_constraint_kind,
            AltitudeConstraintKind::Unsupported
                | AltitudeConstraintKind::AtAltitude
                | AltitudeConstraintKind::Minimum
                | AltitudeConstraintKind::Maximum
                | AltitudeConstraintKind::Window
        ) {
            unsupported_route_constraint = true;
        }
        if (p.altitude_constraint_kind == AltitudeConstraintKind::Minimum
            && p.min_altitude_ft == NO_ALTITUDE_FT)
            || (p.altitude_constraint_kind == AltitudeConstraintKind::Maximum
                && p.max_altitude_ft == NO_ALTITUDE_FT)
            || (p.altitude_constraint_kind == AltitudeConstraintKind::Window
                && (p.min_altitude_ft == NO_ALTITUDE_FT || p.max_altitude_ft == NO_ALTITUDE_FT))
        {
            return output_route_refusal(
                input,
                Refusal::InfeasibleAltitudeWindow,
                validated_route_points,
                active_index,
            );
        }
        if p.altitude_constraint_kind == AltitudeConstraintKind::Window
            && clearance_evidence_valid
            && clearance_supported
        {
            let min = if input.clearance.min_altitude_ft == NO_ALTITUDE_FT {
                i32::MIN
            } else {
                input.clearance.min_altitude_ft
            };
            let max = if input.clearance.max_altitude_ft == NO_ALTITUDE_FT {
                i32::MAX
            } else {
                input.clearance.max_altitude_ft
            };
            if p.max_altitude_ft < min || p.min_altitude_ft > max {
                return output_route_refusal(
                    input,
                    Refusal::InfeasibleAltitudeWindow,
                    validated_route_points,
                    active_index,
                );
            }
        }
        if p.altitude_constraint_kind == AltitudeConstraintKind::Minimum
            && clearance_evidence_valid
            && clearance_supported
            && input.clearance.max_altitude_ft != NO_ALTITUDE_FT
            && input.clearance.max_altitude_ft < p.min_altitude_ft
        {
            return output_route_refusal(
                input,
                Refusal::InfeasibleAltitudeWindow,
                validated_route_points,
                active_index,
            );
        }
        if p.altitude_constraint_kind == AltitudeConstraintKind::Maximum
            && clearance_evidence_valid
            && clearance_supported
            && input.clearance.min_altitude_ft != NO_ALTITUDE_FT
            && input.clearance.min_altitude_ft > p.max_altitude_ft
        {
            return output_route_refusal(
                input,
                Refusal::InfeasibleAltitudeWindow,
                validated_route_points,
                active_index,
            );
        }
        if p.speed_constraint_kind == SpeedConstraintKind::Window
            && p.min_speed_mmps != NO_SPEED_MMPS
            && p.max_speed_mmps != NO_SPEED_MMPS
            && p.min_speed_mmps > p.max_speed_mmps
        {
            return output_route_refusal(
                input,
                Refusal::InfeasibleSpeedWindow,
                validated_route_points,
                active_index,
            );
        }
        let has_speed_constraint = matches!(
            p.speed_constraint_kind,
            SpeedConstraintKind::Minimum
                | SpeedConstraintKind::Maximum
                | SpeedConstraintKind::Window
        );
        let missing_speed_bound = (p.speed_constraint_kind == SpeedConstraintKind::Minimum
            && p.min_speed_mmps == NO_SPEED_MMPS)
            || (p.speed_constraint_kind == SpeedConstraintKind::Maximum
                && p.max_speed_mmps == NO_SPEED_MMPS)
            || (p.speed_constraint_kind == SpeedConstraintKind::Window
                && (p.min_speed_mmps == NO_SPEED_MMPS || p.max_speed_mmps == NO_SPEED_MMPS));
        if p.speed_constraint_kind == SpeedConstraintKind::Unsupported {
            unsupported_route_constraint = true;
        }
        if has_speed_constraint
            && (p.speed_reference != SpeedReference::GroundSpeed || missing_speed_bound)
        {
            unsupported_route_constraint = true;
        }
    }

    let speed_mps = input.track.groundspeed_mmps as f64 / 1000.0;
    let start = Place {
        lat_deg: input.track.lat_e7 as f64 / 10_000_000.0,
        lon_deg: input.track.lon_e7 as f64 / 10_000_000.0,
    };
    let remaining_m = great_circle_m(
        start,
        Place {
            lat_deg: first.lat_e7 as f64 / 10_000_000.0,
            lon_deg: first.lon_e7 as f64 / 10_000_000.0,
        },
    );
    let mut output = output_unavailable(input, Refusal::None);
    output.examined_route_points = (input.points.len() - active_index) as u16;
    output.over_capacity_unexamined = 0;
    output.prior_route_points_ignored = active_index as u16;
    output.model_flags = MODEL_CONSTANT_GROUNDSPEED
        | MODEL_CONSTANT_VERTICAL_RATE
        | MODEL_WIND_NOT_MODELLED
        | MODEL_PERFORMANCE_NOT_MODELLED
        | MODEL_FORWARD_UNCERTAINTY_NOT_MODELLED;
    if input.owner.owner_state == OwnerState::Unknown || !owner_valid {
        output.model_flags |= MODEL_OWNER_UNKNOWN;
    }
    if !clearance_evidence_valid || !clearance_supported {
        output.model_flags |= MODEL_CLEARANCE_UNAVAILABLE;
    }
    if unsupported_route_constraint {
        output.model_flags |= MODEL_ROUTE_CONSTRAINT_UNSUPPORTED;
    }
    if input.track.altitude_ft == NO_ALTITUDE_FT
        || input.track.vertical_rate_fpm == NO_VERTICAL_RATE_FPM
    {
        output.model_flags |= MODEL_ALTITUDE_UNAVAILABLE;
    }

    for (horizon_ms_ref, horizon_output) in HORIZON_MS.iter().zip(output.horizons.iter_mut()) {
        let horizon_ms = *horizon_ms_ref;
        let mut distance_m = speed_mps * (horizon_ms as f64 / 1000.0);
        let mut origin = start;
        let mut origin_e7 = (input.track.lat_e7, input.track.lon_e7);
        let mut first_leg = true;
        let mut position = None;
        let mut geometry_refused = false;
        let mut vectors_gap = false;
        let mut speed_constraint_checked = false;
        let mut degraded = !owner_valid
            || input.owner.owner_state == OwnerState::Unknown
            || !clearance_evidence_valid
            || !clearance_supported;
        let mut vertical_refusal = if !clearance_evidence_valid {
            Refusal::ClearanceUnavailableAtCutoff
        } else if !clearance_supported {
            Refusal::UnsupportedClearanceForm
        } else {
            Refusal::None
        };
        let mut route_iter = input.points.iter().skip(active_index).peekable();
        while let Some(point) = route_iter.next() {
            vectors_gap |= point.geometry == PathGeometry::VectorsGap;
            let leg_m = if first_leg {
                remaining_m
            } else {
                great_circle_m(
                    origin,
                    Place {
                        lat_deg: point.lat_e7 as f64 / 10_000_000.0,
                        lon_deg: point.lon_e7 as f64 / 10_000_000.0,
                    },
                )
            };
            let is_last = route_iter.peek().is_none();
            if distance_m <= leg_m || is_last {
                if is_last && distance_m > leg_m {
                    break;
                }
                let ratio = if leg_m <= 0.0 {
                    0.0
                } else {
                    distance_m / leg_m
                };
                match interpolate_great_circle(
                    origin,
                    Place {
                        lat_deg: point.lat_e7 as f64 / 10_000_000.0,
                        lon_deg: point.lon_e7 as f64 / 10_000_000.0,
                    },
                    origin_e7,
                    (point.lat_e7, point.lon_e7),
                    ratio,
                    leg_m,
                ) {
                    Some(value) => position = Some(value),
                    None => {
                        position = None;
                        geometry_refused = true;
                    }
                }
                degraded = degraded || vectors_gap;
                if point.altitude_constraint_kind == AltitudeConstraintKind::Unsupported
                    || point.speed_constraint_kind == SpeedConstraintKind::Unsupported
                {
                    degraded = true;
                    vertical_refusal = Refusal::UnsupportedConstraint;
                }
                let has_speed_constraint = matches!(
                    point.speed_constraint_kind,
                    SpeedConstraintKind::Minimum
                        | SpeedConstraintKind::Maximum
                        | SpeedConstraintKind::Window
                );
                let missing_speed_bound = (point.speed_constraint_kind
                    == SpeedConstraintKind::Minimum
                    && point.min_speed_mmps == NO_SPEED_MMPS)
                    || (point.speed_constraint_kind == SpeedConstraintKind::Maximum
                        && point.max_speed_mmps == NO_SPEED_MMPS)
                    || (point.speed_constraint_kind == SpeedConstraintKind::Window
                        && (point.min_speed_mmps == NO_SPEED_MMPS
                            || point.max_speed_mmps == NO_SPEED_MMPS));
                let speed_constraint_supported = has_speed_constraint
                    && point.speed_reference == SpeedReference::GroundSpeed
                    && !missing_speed_bound;
                speed_constraint_checked = has_speed_constraint && speed_constraint_supported;
                let speed_violation = speed_constraint_supported
                    && ((point.speed_constraint_kind == SpeedConstraintKind::Minimum
                        && input.track.groundspeed_mmps < point.min_speed_mmps)
                        || (point.speed_constraint_kind == SpeedConstraintKind::Maximum
                            && input.track.groundspeed_mmps > point.max_speed_mmps)
                        || (point.speed_constraint_kind == SpeedConstraintKind::Window
                            && (input.track.groundspeed_mmps < point.min_speed_mmps
                                || input.track.groundspeed_mmps > point.max_speed_mmps)));
                if has_speed_constraint && !speed_constraint_supported {
                    degraded = true;
                    vertical_refusal = Refusal::UnsupportedConstraint;
                }
                if point.altitude_constraint_kind != AltitudeConstraintKind::None {
                    degraded = true;
                    vertical_refusal = Refusal::UnsupportedConstraint;
                }
                if speed_violation {
                    degraded = true;
                    vertical_refusal = Refusal::InfeasibleSpeedWindow;
                }
                break;
            }
            if is_last {
                break;
            }
            distance_m -= leg_m;
            origin = Place {
                lat_deg: point.lat_e7 as f64 / 10_000_000.0,
                lon_deg: point.lon_e7 as f64 / 10_000_000.0,
            };
            origin_e7 = (point.lat_e7, point.lon_e7);
            first_leg = false;
        }
        let target_time = input.track.state_ms.checked_add(horizon_ms);
        if let (Some((lat, lon)), Some(predicted_time_ms)) = (position, target_time) {
            let mut altitude = if vertical_refusal == Refusal::None {
                if input.track.altitude_ft == NO_ALTITUDE_FT
                    || input.track.vertical_rate_fpm == NO_VERTICAL_RATE_FPM
                {
                    vertical_refusal = Refusal::NoAltitude;
                    NO_ALTITUDE_FT
                } else {
                    let delta = (input.track.vertical_rate_fpm as i64)
                        .checked_mul(horizon_ms)
                        .map(|v| v / 60_000);
                    let predicted =
                        delta.and_then(|v| (input.track.altitude_ft as i64).checked_add(v));
                    match predicted {
                        Some(v) if v > i32::MIN as i64 && v <= i32::MAX as i64 => v as i32,
                        _ => {
                            vertical_refusal = Refusal::AltitudeOverflow;
                            NO_ALTITUDE_FT
                        }
                    }
                }
            } else {
                NO_ALTITUDE_FT
            };
            if vertical_refusal != Refusal::None {
                degraded = true;
            }
            let mut flags = 0;
            let clearance = input.clearance;
            // Intent may capture observed motion approaching a cleared boundary. It must not
            // teleport a level aircraft to a different cleared altitude or reverse motion that
            // is already moving away from that boundary. Observed CA remains independent.
            let captured = if clearance_evidence_valid
                && clearance_supported
                && altitude != NO_ALTITUDE_FT
                && input.track.vertical_rate_fpm < 0
                && clearance.min_altitude_ft != NO_ALTITUDE_FT
                && input.track.altitude_ft >= clearance.min_altitude_ft
                && altitude <= clearance.min_altitude_ft
            {
                altitude = clearance.min_altitude_ft;
                true
            } else if clearance_evidence_valid
                && clearance_supported
                && altitude != NO_ALTITUDE_FT
                && input.track.vertical_rate_fpm > 0
                && clearance.max_altitude_ft != NO_ALTITUDE_FT
                && input.track.altitude_ft <= clearance.max_altitude_ft
                && altitude >= clearance.max_altitude_ft
            {
                altitude = clearance.max_altitude_ft;
                true
            } else {
                false
            };
            if captured {
                flags |= OUTPUT_CAPTURED_ALTITUDE;
            }
            if speed_constraint_checked && vertical_refusal != Refusal::InfeasibleSpeedWindow {
                flags |= OUTPUT_CONSTRAINT_CHECKED;
            }
            if input.owner.owner_state == OwnerState::Unknown || !owner_valid {
                flags |= OUTPUT_OWNER_UNKNOWN;
            }
            if !clearance_evidence_valid || !clearance_supported {
                flags |= OUTPUT_CLEARANCE_UNAVAILABLE;
            }
            if vertical_refusal == Refusal::UnsupportedConstraint {
                flags |= OUTPUT_CONSTRAINT_UNSUPPORTED;
            }
            if vertical_refusal == Refusal::InfeasibleSpeedWindow {
                flags |= OUTPUT_SPEED_CONSTRAINT_VIOLATED;
            }
            if vectors_gap {
                flags |= OUTPUT_VECTORS_GAP;
            }
            *horizon_output = HorizonOutput {
                horizon_ms,
                available: if degraded {
                    Availability::Degraded
                } else {
                    Availability::Available
                },
                refusal: vertical_refusal,
                lat_e7: lat,
                lon_e7: lon,
                altitude_ft: altitude,
                groundspeed_mmps: input.track.groundspeed_mmps,
                predicted_time_ms,
                flags,
                reserved: [0; 2],
            };
        } else {
            let refusal = if target_time.is_none() {
                Refusal::TimeOverflow
            } else if geometry_refused {
                Refusal::UnsupportedGeometry
            } else {
                Refusal::RouteHasNoEnd
            };
            *horizon_output = unavailable_horizon(horizon_ms, refusal);
        }
    }

    if input.eta_target.present {
        if !fix_identity_ok(input.eta_target.fix) {
            output.eta = EtaOutput {
                available: Availability::Unavailable,
                refusal: Refusal::InvalidFixIdentity,
                fix: input.eta_target.fix,
                eta_ms: 0,
                reserved: [0; 7],
            };
            return output;
        }
        let mut elapsed_m = 0.0;
        let mut from = start;
        let mut result = None;
        let mut prior_gap = false;
        let mut target_matches = 0u16; // suffix length was checked against 256 before this scan
        for point in input.points.iter().skip(active_index) {
            let to = Place {
                lat_deg: point.lat_e7 as f64 / 10_000_000.0,
                lon_deg: point.lon_e7 as f64 / 10_000_000.0,
            };
            let leg_m = great_circle_m(from, to);
            if same_fix(point.fix, input.eta_target.fix) {
                target_matches += 1;
                if prior_gap || point.geometry == PathGeometry::VectorsGap {
                    output.eta.refusal = Refusal::UnsupportedGeometry;
                    output.eta.available = Availability::Unavailable;
                } else {
                    result = Some(elapsed_m + leg_m);
                }
            }
            prior_gap = prior_gap || point.geometry == PathGeometry::VectorsGap;
            elapsed_m += leg_m;
            from = to;
        }
        if target_matches > 1 {
            output.eta.refusal = Refusal::AmbiguousEtaTarget;
            output.eta.available = Availability::Unavailable;
        } else if let Some(distance_m) = result {
            let duration_ms_f = distance_m / speed_mps * 1000.0;
            // Integer ETA uses truncation to whole milliseconds after a pre-cast signed range check.
            if (0.0..9_223_372_036_854_775_808.0).contains(&duration_ms_f) {
                let duration_ms = duration_ms_f as i64;
                if let Some(eta_ms) = input.track.state_ms.checked_add(duration_ms) {
                    output.eta = EtaOutput {
                        available: Availability::Available,
                        refusal: Refusal::None,
                        fix: input.eta_target.fix,
                        eta_ms,
                        reserved: [0; 7],
                    };
                } else {
                    output.eta.refusal = Refusal::TimeOverflow;
                }
            } else {
                output.eta.refusal = Refusal::TimeOverflow;
            }
        } else if output.eta.refusal == Refusal::None {
            output.eta.refusal = Refusal::EtaTargetNotFound;
        }
    } else {
        output.eta = EtaOutput {
            available: Availability::Unavailable,
            refusal: Refusal::None,
            fix: input.eta_target.fix,
            eta_ms: 0,
            reserved: [0; 7],
        };
    }
    output
}

/// Route revision candidate. The route payload hash binds the complete converted payload; no
/// point identity is replaced by a local name and no hash computation occurs in this core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RevisionEvidence {
    pub source_ms: i64,
    pub available_ms: i64,
    pub message_id: u64,
    pub payload_sha256: [u8; 32],
    pub key: FlightKey,
    /// A7: explicit zero bytes complete the 96-byte aligned record.
    pub reserved: [u8; 4],
}
// A3 exception: at most 256 immutable shell candidates for one offline issue, not a hot flight table.
// A7: 24 times + 32 hash + 36 key + 4 explicit alignment bytes = 96.
const _: () = assert!(core::mem::size_of::<RevisionEvidence>() == 96);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EvidenceRefusal {
    None = 0,
    TooManyCandidates = 1,
    SameSourceConflict = 2,
    NoAvailableEvidence = 3,
    InvalidIdentity = 4,
    InvalidEvidence = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct EvidenceCounts {
    pub examined: u64,
    pub foreign_gufi: u64,
    pub foreign_aircraft_address: u64,
    pub foreign_reporting_centre: u64,
    pub future_source: u64,
    pub future_availability: u64,
    pub invalid_time_order: u64,
    pub invalid_identity: u64,
    pub duplicates: u64,
    pub conflicts: u64,
    pub over_capacity_unexamined: u64,
}
const _: () = assert!(core::mem::size_of::<EvidenceCounts>() == 88);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct EvidenceSelection {
    pub selected: RevisionEvidence,
    pub counts: EvidenceCounts,
    pub available: Availability,
    pub refusal: EvidenceRefusal,
    /// A7: explicit zero bytes complete the 192-byte aligned record.
    pub reserved: [u8; 6],
}
// A7: 96 selection + 88 counts + 2 enums + 6 explicit alignment bytes = 192.
const _: () = assert!(core::mem::size_of::<EvidenceSelection>() == 192);

fn same_flight_identity(a: FlightKey, b: FlightKey) -> bool {
    a.gufi == b.gufi && a.aircraft_address == b.aircraft_address && a.centre == b.centre
}

/// Select the newest source revision available at the cutoff by GUFI, aircraft address and
/// reporting centre. Plan ID is part of revision state, not the identity filter: a newer plan B
/// must be returned even when the shell currently holds plan A, so the shell can refuse the view
/// mismatch instead of silently falling back to A. A same-source plan/payload conflict refuses.
pub fn select_revision_evidence(
    key: FlightKey,
    cutoff_ms: i64,
    candidates: &[RevisionEvidence],
) -> EvidenceSelection {
    let mut counts = EvidenceCounts {
        examined: 0,
        foreign_gufi: 0,
        foreign_aircraft_address: 0,
        foreign_reporting_centre: 0,
        future_source: 0,
        future_availability: 0,
        invalid_time_order: 0,
        invalid_identity: 0,
        duplicates: 0,
        conflicts: 0,
        over_capacity_unexamined: 0,
    };
    let zero = RevisionEvidence {
        key,
        source_ms: 0,
        available_ms: 0,
        message_id: 0,
        payload_sha256: [0; 32],
        reserved: [0; 4],
    };
    if candidates.len() > MAX_REVISION_EVIDENCE {
        counts.over_capacity_unexamined = candidates.len() as u64;
        return EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::TooManyCandidates,
            counts,
            reserved: [0; 6],
        };
    }
    if !flight_key_ok(key) {
        return EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::InvalidIdentity,
            counts,
            reserved: [0; 6],
        };
    }
    let comparisons = candidates
        .len()
        .saturating_mul(candidates.len().saturating_sub(1))
        / 2;
    let planned_work = candidates
        .len()
        .saturating_mul(
            4 + FLIGHT_KEY_IDENTITY_BYTE_WORK
                + FLIGHT_CENTRE_MATCH_BYTE_WORK
                + 2 * EVIDENCE_REVISION_BYTE_WORK,
        )
        .saturating_add(comparisons.saturating_mul(EVIDENCE_REVISION_BYTE_WORK))
        .saturating_add(FLIGHT_KEY_IDENTITY_BYTE_WORK);
    if comparisons > MAX_EVIDENCE_COMPARISONS || planned_work > MAX_EVIDENCE_SCAN_WORK {
        return EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::TooManyCandidates,
            counts,
            reserved: [0; 6],
        };
    }
    // Validate/count the whole cutoff-eligible set before selecting anything. A malformed
    // amendment for this flight blocks fallback to an older good route.
    let mut matching_invalid = false;
    for c in candidates.iter() {
        counts.examined = counts.examined.saturating_add(1);
        // Availability is the outer cutoff: future rows are counted without inspecting any
        // identity or payload fields.
        if c.available_ms > cutoff_ms {
            counts.future_availability = counts.future_availability.saturating_add(1);
            continue;
        }
        if c.key.gufi != key.gufi {
            counts.foreign_gufi = counts.foreign_gufi.saturating_add(1);
            continue;
        }
        if c.key.aircraft_address != key.aircraft_address {
            counts.foreign_aircraft_address = counts.foreign_aircraft_address.saturating_add(1);
            continue;
        }
        if c.key.centre != key.centre {
            counts.foreign_reporting_centre = counts.foreign_reporting_centre.saturating_add(1);
            continue;
        }
        let invalid_identity = !reserved_zero(&c.reserved) || !flight_key_ok(c.key);
        if invalid_identity {
            counts.invalid_identity = counts.invalid_identity.saturating_add(1);
            matching_invalid = true;
        }
        if c.source_ms > c.available_ms {
            counts.invalid_time_order = counts.invalid_time_order.saturating_add(1);
            matching_invalid = true;
        } else if c.source_ms > cutoff_ms {
            counts.future_source = counts.future_source.saturating_add(1);
        }
    }
    if matching_invalid {
        return EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::InvalidEvidence,
            counts,
            reserved: [0; 6],
        };
    }

    // With the eligible set validated, detect all same-source conflicts before choosing a row.
    for (i, c) in candidates.iter().enumerate() {
        if c.available_ms <= cutoff_ms && same_flight_identity(c.key, key) {
            for prev in candidates.iter().take(i) {
                if prev.available_ms <= cutoff_ms
                    && same_flight_identity(prev.key, key)
                    && prev.source_ms == c.source_ms
                    && (prev.key.plan_id != c.key.plan_id
                        || prev.payload_sha256 != c.payload_sha256)
                {
                    counts.conflicts = counts.conflicts.saturating_add(1);
                }
            }
        }
    }
    if counts.conflicts > 0 {
        return EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::SameSourceConflict,
            counts,
            reserved: [0; 6],
        };
    }

    let mut selected: Option<RevisionEvidence> = None;
    for c in candidates.iter() {
        if c.available_ms <= cutoff_ms && same_flight_identity(c.key, key) {
            let choose = match selected {
                None => true,
                Some(best) => {
                    c.source_ms > best.source_ms
                        || (c.source_ms == best.source_ms
                            && c.key.plan_id == best.key.plan_id
                            && c.payload_sha256 == best.payload_sha256
                            && (c.available_ms < best.available_ms
                                || (c.available_ms == best.available_ms
                                    && c.message_id < best.message_id)))
                }
            };
            if choose {
                selected = Some(*c);
            }
        }
    }
    match selected {
        Some(selected) => {
            // Count duplicate rows for this exact revision/payload, retaining earliest
            // availability then lowest message id through deterministic selection above.
            let mut n: usize = 0;
            for c in candidates.iter() {
                if same_flight_identity(c.key, key)
                    && c.source_ms == selected.source_ms
                    && c.key.plan_id == selected.key.plan_id
                    && c.payload_sha256 == selected.payload_sha256
                    && c.source_ms <= c.available_ms
                    && c.available_ms <= cutoff_ms
                {
                    n += 1;
                }
            }
            counts.duplicates = n.saturating_sub(1) as u64;
            EvidenceSelection {
                selected,
                available: Availability::Available,
                refusal: EvidenceRefusal::None,
                counts,
                reserved: [0; 6],
            }
        }
        None => EvidenceSelection {
            selected: zero,
            available: Availability::Unavailable,
            refusal: EvidenceRefusal::NoAvailableEvidence,
            counts,
            reserved: [0; 6],
        },
    }
}
