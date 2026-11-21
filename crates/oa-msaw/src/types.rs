//! Plain data of the minimum safe altitude warning. Core (pure).

use alloc::boxed::Box;
use alloc::vec::Vec;

/// Most sample points of the predicted path of one track. C2: the shell does one terrain,
/// obstacle and chart lookup per point and track, so the bound is also a bound on that work; 32
/// points over a look-ahead of a few minutes is a point every few seconds.
pub const MAX_SAMPLES: usize = 32;
/// Most corridors a configuration may name. C2: per runway end that has an instrument approach or
/// a departure procedure, an approach corridor, a strip along the runway and a departure
/// corridor; a facility's airports are some tens of runway ends, and the grid index of the
/// corridors holds a few dozen cells for each.
pub const MAX_CORRIDORS: usize = 16384;
/// Largest `max_tracks` the configuration may ask for (every per-track table is allocated whole).
pub const MAX_TRACKS_LIMIT: usize = 1 << 20;
/// Largest `max_open` the configuration may ask for.
pub const MAX_OPEN_LIMIT: usize = 1 << 20;
/// Largest look-ahead, seconds (a data-sanity bound, so that the arithmetic bounds hold).
pub const MAX_LOOK_AHEAD_S: f64 = 600.0;
/// Largest corridor length, metres (a data-sanity bound).
pub const MAX_CORRIDOR_LENGTH_M: f64 = 1.0e5;
/// Largest speed the configuration may allow, metres per second.
pub const MAX_SPEED_LIMIT_MPS: f64 = 1.0e3;
/// Largest altitude magnitude a row may carry, feet; a row beyond it is refused as bad.
pub const MAX_ABS_ALT_FT: f64 = 1.0e5;
/// Largest vertical rate magnitude a row may carry, feet per minute.
pub const MAX_ABS_VRATE_FPM: f64 = 6.0e4;
/// Largest query radius of a sample point, metres.
pub const MAX_RADIUS_M: f64 = 2.0e5;
/// Largest clearance or floor a configuration may state, feet.
pub const MAX_CLEARANCE_FT: i32 = 100_000;
/// Largest floor magnitude the net can compute, feet: the shell holds a terrain or obstacle value
/// to a million feet, and a clearance is at most [`MAX_CLEARANCE_FT`]; a corridor surface and a
/// chart altitude are far inside it. The time-to-violation proof holds over this whole range.
pub const MAX_FLOOR_FT: i32 = 1_100_000;
/// The latitude beyond which a row is refused as bad (east-west distances are no longer a flat
/// scale there), degrees.
pub const MAX_ABS_LAT_DEG: f64 = 85.0;

/// [`FloorSample`] value: the shell found no coverage (terrain, chart) or, for an obstacle, no
/// obstacle (see [`FloorSample::obstacle_covered`] for the absence of the obstacle data).
pub const FLOOR_NONE: i32 = i32::MIN;

/// [`MsawTrack::flags`]: the track is coasting (no report this cycle).
pub const F_COAST: u16 = 1;
/// [`MsawTrack::flags`]: the track's position is uncertain.
pub const F_UNCERTAIN: u16 = 2;
/// [`MsawTrack::flags`]: the track is held by radar only (no ADS-B).
pub const F_RADAR_ONLY: u16 = 4;
/// [`MsawTrack::flags`]: the identity of the track is not confirmed.
pub const F_IDENTITY_UNCONFIRMED: u16 = 8;
/// [`MsawTrack::flags`]: the shell had no usable altimeter setting and gave the pressure altitude
/// lowered by its allowance instead.
pub const F_QNH_MISSING: u16 = 16;
/// [`MsawTrack::flags`]: the shell lowered the altitude by the cold-temperature correction.
pub const F_COLD_CORRECTED: u16 = 32;
/// [`MsawTrack::flags`]: the temperature at the altimeter station was not known: no cold-temperature
/// correction was made.
pub const F_COLD_UNKNOWN: u16 = 64;
/// [`MsawTrack::flags`]: the altitude is more than 5,000 ft above the altimeter station, beyond the
/// terminal and approach use of the cold-temperature correction: not corrected.
pub const F_COLD_HIGH: u16 = 128;

/// [`MsawTrack::ground`]: the tracker does not say (the track is evaluated).
pub const GROUND_UNKNOWN: u8 = 0;
/// [`MsawTrack::ground`]: airborne.
pub const AIRBORNE: u8 = 1;
/// [`MsawTrack::ground`]: on the ground (excluded and counted).
pub const ON_GROUND: u8 = 2;

/// [`Alert::flags`]: the vertical rate was unknown at some cycle (the prediction was level).
pub const AF_RATE_UNKNOWN: u16 = 1;
/// [`Alert::flags`]: the track was coasting at some cycle of the alert.
pub const AF_COAST: u16 = 2;
/// [`Alert::flags`]: the track was uncertain at some cycle of the alert.
pub const AF_UNCERTAIN: u16 = 4;
/// [`Alert::flags`]: the track was radar only.
pub const AF_RADAR_ONLY: u16 = 8;
/// [`Alert::flags`]: the track's identity was unconfirmed.
pub const AF_IDENTITY_UNCONFIRMED: u16 = 16;
/// [`Alert::flags`]: part of the predicted path had no terrain or chart coverage (the rest was
/// evaluated).
pub const AF_PARTIAL_COVERAGE: u16 = 32;
/// [`Alert::flags`]: the ground state was unknown at some cycle.
pub const AF_GROUND_UNKNOWN: u16 = 64;
/// [`Alert::flags`]: the altitude had no altimeter setting at some cycle of the alert.
pub const AF_QNH_MISSING: u16 = 128;

/// [`Alert::flags`]: the altitude was stale at some cycle of the alert and judged at an
/// extrapolated altitude (a descent continued, never a climb).
pub const AF_STALE_EXTRAPOLATED: u16 = 256;
/// [`Alert::flags`]: the altitude was lowered by the cold-temperature correction at some cycle.
pub const AF_COLD_CORRECTED: u16 = 512;
/// [`Alert::flags`]: no cold-temperature correction was made for want of a station temperature, at
/// some cycle.
pub const AF_COLD_UNKNOWN: u16 = 1024;
/// [`Alert::flags`]: the track's 95 % radius was above [`WIDE_R95_M`] (or unusable) at some cycle.
pub const AF_WIDE_UNCERTAINTY: u16 = 2048;
/// [`Alert::flags`]: not cold-corrected for being over 5,000 ft above the station, at some cycle.
pub const AF_COLD_HIGH: u16 = 4096;

/// The largest 95 % radius a lookup is made with, metres. A radius is never shrunk below the
/// track's own: one that is unusable (NaN, negative) takes this, one above it is held to it.
pub const MAX_R95_M: f64 = 10_000.0;
/// A 95 % radius above this, metres, is flagged and counted as wide uncertainty.
pub const WIDE_R95_M: f64 = 2_000.0;

/// [`Alert::end_reason`]: the alert ended by the hysteresis (the track was clear).
pub const END_CLEAR: u8 = 0;
/// [`Alert::end_reason`]: the alert ended because the track's altitude went stale past the hold.
pub const END_LOST_ALTITUDE_STALE: u8 = 1;
/// [`Alert::end_reason`]: the alert ended (or stands at the end of the run) because the track lost its altitude.
pub const END_LOST_ALTITUDE_MISSING: u8 = 2;

/// One track's state at a cycle, as given by the shell from the tracker's row.
// A7.2: cold struct (a copy of the tracker's row), size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsawTrack {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub addr: u32,
    /// True east and north velocity, metres per second.
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    /// 95 % horizontal position radius from the tracker's covariance, metres.
    pub r95_m: f32,
    /// Altitude on the datum of the floors (mean sea level, corrected by the shell), feet; NaN
    /// when the track has none.
    pub alt_ft: f32,
    /// Vertical rate, feet per minute; NaN when the track has none.
    pub vrate_fpm: f32,
    /// [`F_COAST`], [`F_UNCERTAIN`], [`F_RADAR_ONLY`], [`F_IDENTITY_UNCONFIRMED`],
    /// [`F_QNH_MISSING`], [`F_COLD_CORRECTED`], [`F_COLD_UNKNOWN`].
    pub flags: u16,
    /// [`GROUND_UNKNOWN`], [`AIRBORNE`] or [`ON_GROUND`].
    pub ground: u8,
    /// Seconds since the track's latest barometric altitude was measured; NaN if not known (the
    /// altitude is then taken as fresh, and counted as of unknown age).
    pub alt_age_s: f32,
    /// The altitude now less the altitude 10 s before, feet; NaN if not known. Level within 50 ft
    /// is evidence that a track is not climbing where its rate says otherwise.
    pub alt_change_10s_ft: f32,
    /// The highest terrain at the track's own cell, feet; NaN if not given (the track is then
    /// ranked first for the budget). Given only when the tracks exceed the budget.
    pub ground_ft: f32,
}

/// A point of a track's predicted path where the shell looks up the floor.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplePoint {
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// The query radius, metres: the track's 95 % radius grown with the prediction time by the
    /// velocity uncertainty, plus half the distance to the next point, so that the terrain and
    /// obstacles between two points are inside some radius.
    pub radius_m: f32,
    /// The track's ground track, whole degrees true clockwise from north (0 to 359), or -1 where
    /// the track is too slow to have one. The approach and runway corridors hold a point only for a
    /// track aligned with them.
    pub track_deg: i16,
}

/// What the shell found at a [`SamplePoint`], in whole feet, rounded up (never down: a floor is
/// conservative when it is high). [`FLOOR_NONE`] means no coverage for `terrain_ft` and
/// `mva_ft` (the net is unavailable there, rule 1) and no obstacle for `obstacle_ft`; with
/// `obstacle_covered` false there is no obstacle data at all, which is no coverage too.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloorSample {
    /// The highest terrain elevation over the point's radius, feet MSL.
    pub terrain_ft: i32,
    /// The highest obstacle top over the point's radius, feet MSL.
    pub obstacle_ft: i32,
    /// The shell had obstacle data for the point (false: the floor is unavailable, rule 1).
    pub obstacle_covered: bool,
    /// The minimum vectoring altitude of the sector containing the point, feet MSL.
    pub mva_ft: i32,
    /// The clearance above terrain outside the corridors at the point, feet, where an area of the
    /// adaptation sets one; [`FLOOR_NONE`] for the configuration's `terrain_clearance_ft`.
    pub gtm_clearance_ft: i32,
    /// The clearance above terrain in an airport terminal area holding the point, feet (the lowest
    /// of the areas); [`FLOOR_NONE`] if none. It applies below `apt_ceiling_ft` and only where no
    /// `gtm_clearance_ft` is set.
    pub apt_clearance_ft: i32,
    /// The altitude below which `apt_clearance_ft` applies, feet MSL (the lowest of the areas).
    pub apt_ceiling_ft: i32,
}

/// [`Corridor::kind`]: a final-approach corridor, from the landing threshold outward to the final
/// approach fix, under a surface rising to the fix altitude.
pub const CK_APPROACH: u8 = 0;
/// [`Corridor::kind`]: the strip along a runway, held under a flat surface at the threshold
/// elevation: the approach corridor's continuation.
pub const CK_RUNWAY: u8 = 1;
/// [`Corridor::kind`]: a departure corridor, from the departure end outward along the runway
/// heading, with no surface and its own clearance above terrain and obstacles.
pub const CK_DEPARTURE: u8 = 2;

/// A corridor: the strip under the extended centreline of a runway end that has an instrument
/// approach, from the landing threshold outward to the final approach fix ([`CK_APPROACH`]), along
/// the runway ([`CK_RUNWAY`]) or out from its departure end ([`CK_DEPARTURE`]). Inside it (whole
/// query circle) the floor is the higher of the corridor's surface, if it has one, and terrain and
/// obstacles plus a clearance, and the chart altitude does not apply. A trapezoid: its half-width
/// runs linearly from the threshold end to the far end. An approach surface is the procedure's own
/// path (`tch_ft` over the threshold elevation at `path_angle_deg`); a runway surface is the
/// threshold elevation.
// A7.2: cold struct (adaptation), size guard waived. The corridors of a configuration are one boxed
// slice, allocated once at load (C1); there are at most `MAX_CORRIDORS`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corridor {
    /// `CK_*`.
    pub kind: u8,
    /// The near end of the strip (the landing threshold, or the departure end), 1e-7 degree.
    pub thr_lat_e7: i32,
    pub thr_lon_e7: i32,
    /// The direction of the centreline from the near end outward, degrees true clockwise from
    /// north.
    pub out_bearing_deg: f64,
    /// Near end to the far end, metres.
    pub length_m: f64,
    /// Half-width at the near end and at the far end, metres.
    pub near_half_width_m: f64,
    pub far_half_width_m: f64,
    /// The threshold elevation and the published altitude at the fix, feet MSL (the altitude is
    /// not used by a runway or departure corridor).
    pub thr_elev_ft: i32,
    pub faf_alt_ft: i32,
    /// For a runway strip: how far along it from the near end the runway itself begins, metres (the
    /// strip may start before the threshold). A path that reaches the runway elevation beyond it is
    /// landing. Zero for the other kinds.
    pub land_from_m: f64,
    /// For a runway strip: the half-width of the pavement, metres (the strip is wider by its
    /// margin), and the elevation at the far end of the runway, feet; the runway elevation at a
    /// point is interpolated along it from `thr_elev_ft`.
    pub pave_half_m: f64,
    pub far_elev_ft: i32,
    /// For an approach corridor: the angle of the procedure's own vertical path, degrees, and the
    /// height at which it crosses the threshold, feet above the threshold elevation. The surface
    /// is that path less the allowance.
    pub path_angle_deg: f64,
    pub tch_ft: i32,
}

/// What the corridors say of a point: the surface of the highest corridor that holds it, if any
/// has one, and the highest clearance of those that hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorridorHit {
    pub surface_ft: Option<i32>,
    pub clearance_ft: i32,
    /// The kinds of the corridors that hold the circle, as `KB_*` bits.
    pub kinds: u8,
}

/// [`CorridorHit::kinds`]: an approach corridor holds the circle.
pub const KB_APPROACH: u8 = 1;
/// [`CorridorHit::kinds`]: a runway strip holds it.
pub const KB_RUNWAY: u8 = 2;
/// [`CorridorHit::kinds`]: a departure corridor holds it.
pub const KB_DEPARTURE: u8 = 4;

/// The settings of the net. Every value is the caller's (no defaults: no safety value is chosen
/// by the core). A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, PartialEq)]
pub struct MsawConfig {
    /// Tracks per cycle (capacity of every per-track table).
    pub max_tracks: usize,
    /// Open alerts at once.
    pub max_open: usize,
    /// Tracks evaluated per cycle: the named budget of rule 5. Above it, the lowest predicted
    /// altitudes are evaluated first and the rest are counted.
    pub track_budget: usize,
    /// Look-ahead, seconds.
    pub look_ahead_s: f64,
    /// A violation within this many seconds is a warning, else a caution; not above the look-ahead.
    pub warning_s: f64,
    /// Sample points per track, at least 2.
    pub samples: usize,
    /// An alert whose track has been clear this long ends, nanoseconds.
    pub hysteresis_ns: i64,
    /// Clearance above terrain, feet.
    pub terrain_clearance_ft: i32,
    /// Clearance above an obstacle top, feet.
    pub obstacle_clearance_ft: i32,
    /// A row faster than this, metres per second, is refused as bad.
    pub max_speed_mps: f64,
    /// The net reads degraded when more than this share of a cycle's airborne tracks have a 95 %
    /// radius above [`WIDE_R95_M`].
    pub wide_degrade_share: f64,
    /// Inside a corridor the surface is held this far below the published altitude at the fix, feet.
    pub approach_allowance_ft: i32,
    /// Inside an approach or runway corridor the clearance above terrain and obstacle tops, feet.
    pub approach_min_clearance_ft: i32,
    /// Inside a departure corridor the clearance above terrain and obstacle tops, feet.
    pub departure_min_clearance_ft: i32,
    /// The growth of a sample point's radius with the prediction time, metres per second.
    pub velocity_uncertainty_mps: f64,
    /// A track whose latest barometric altitude is older than this, seconds, is judged at an
    /// extrapolated altitude until `stale_hold_s`.
    pub max_altitude_age_s: f64,
    /// Past this age of the altitude, seconds, a track is not monitored (not above 600 s).
    pub stale_hold_s: f64,
    /// The net degrades when more than this share (0 to 1) of the airborne tracks of a cycle are
    /// not monitored for altitude.
    pub altitude_loss_degrade_share: f64,
    /// The prediction time inside an approach corridor (a capture box) and no departure corridor,
    /// and in an airport terminal area below its ceiling, seconds; not above `look_ahead_s`.
    pub terminal_lookahead_s: f64,
    /// An approach corridor holds a track whose ground track is within this many degrees of its
    /// inbound course, a runway strip one within this of either direction of the runway.
    pub capture_heading_deg: f64,
    /// The corridors, at most `MAX_CORRIDORS`, held on the heap and allocated once at load.
    pub corridors: Box<[Corridor]>,
}

/// What set the floor at the worst point of an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FloorKind {
    Terrain = 1,
    Obstacle = 2,
    Mva = 3,
    /// The surface of a final-approach corridor.
    Procedure = 4,
}

/// The grade of an alert, from the time to the predicted violation (rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Grade {
    Caution = 1,
    Warning = 2,
}

/// One alert: one track below its floor within the look-ahead, from its first cycle to its last.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alert {
    pub addr: u32,
    pub start_ns: i64,
    /// Last cycle in violation, Unix ns.
    pub end_ns: i64,
    /// First cycle at caution or worse, and at warning (`None`: never).
    pub caution_ns: Option<i64>,
    pub warning_ns: Option<i64>,
    pub peak: Grade,
    /// Position and altitude at the first cycle, 1e-7 degree and feet.
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub alt_ft: f32,
    /// Vertical rate (feet per minute, NaN when unknown) and ground speed (metres per second) at
    /// the first cycle: what the track was doing.
    pub vrate_fpm: f32,
    pub speed_mps: f32,
    /// Smallest predicted margin over the floor (altitude minus floor, feet; negative is
    /// below), at the worst cycle and point; and what set that floor.
    pub worst_margin_ft: f32,
    pub worst_floor_ft: i32,
    pub worst_kind: FloorKind,
    /// The path point (1e-7 degree) and query radius (metres) of that worst margin, and the
    /// track's velocity (east, north, metres per second) at the first cycle: for diagnostics.
    pub worst_lat_e7: i32,
    pub worst_lon_e7: i32,
    pub worst_radius_m: f32,
    /// The sample index of that point and the track's 95 % radius as given, metres.
    pub worst_k: u8,
    /// The cycle time (Unix ns) and the `AF_` bits of the cycle that gave that worst margin.
    pub worst_ns: i64,
    pub worst_flags: u16,
    pub r95_m: f32,
    pub vel_east_mps: f32,
    pub vel_north_mps: f32,
    /// Shortest time to violation over the alert, seconds (zero: below the floor now).
    pub ttv_min_s: f32,
    /// The track's ground track at the first cycle (whole degrees, -1: none) and the kinds (`KB_*`)
    /// of the corridors that held the worst point for it: what rules applied there.
    pub track_deg: i16,
    pub worst_kinds: u8,
    pub cycles: u32,
    /// [`AF_RATE_UNKNOWN`] and the other `AF_` bits, of any cycle of the alert.
    pub flags: u16,
    /// [`END_CLEAR`], [`END_LOST_ALTITUDE_STALE`] or [`END_LOST_ALTITUDE_MISSING`]. A lost reason on
    /// an open alert means protection lost: the alert is held open.
    pub end_reason: u8,
    /// The last cycle at which the track was seen with its altitude lost (zero: not lost).
    pub protect_ns: i64,
}

/// Why a track was left out of a cycle's evaluation, wholly or in part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SkipWhy {
    /// No altitude (rule 4).
    AltitudeMissing = 1,
    /// No terrain or chart coverage at some point of the path (rule 1); the covered points were
    /// evaluated.
    NoCoverage = 2,
    /// Not evaluated: the cycle's track budget ran out (rule 5).
    OverBudget = 3,
    /// The row was not usable (a coordinate, velocity, altitude or rate that is not a number or
    /// beyond the data-sanity bounds).
    BadRow = 4,
    /// The row was past the track capacity.
    OverCapacity = 5,
    /// No altimeter setting was usable for the track's altitude (it was lowered by the allowance
    /// and evaluated).
    QnhMissing = 6,
    /// Predicted below the minimum vectoring altitude but not below the alert floor: listed and
    /// counted, never an alert and never a shortfall of the net.
    BelowMva = 7,
    /// The latest barometric altitude was older than `stale_hold_s`: the track is not monitored, and
    /// says so on that track. A gap of the aircraft: it does not degrade the net unless the share
    /// of tracks lost this way exceeds `altitude_loss_degrade_share`.
    AltitudeStale = 8,
    /// The latest barometric altitude was older than `max_altitude_age_s` and within `stale_hold_s`:
    /// the track is judged at an extrapolated altitude and says so.
    AltitudeExtrapolated = 9,
    /// The track is on the ground: excluded, and listed per track.
    GroundExcluded = 10,
    /// The track lost its altitude (missing, or past the hold) while it had an open alert: the
    /// alert stays open, and the protection of the track is lost until the altitude returns.
    ProtectionLost = 11,
}

/// A track a cycle did not (fully) evaluate, listed and never silently passed.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Skipped {
    pub t_ns: i64,
    pub addr: u32,
    pub why: SkipWhy,
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// The finished alerts of a run of cycles, bounded: a push past the capacity is counted in
/// `overflow` and the alert is lost, and the net reads degraded (RULESET C2). The consumer
/// clears it after reading.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertLog {
    pub capacity: usize,
    pub overflow: u64,
    pub items: Vec<Alert>,
}

/// The skipped tracks of a run of cycles, bounded like [`AlertLog`].
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkipLog {
    pub capacity: usize,
    pub overflow: u64,
    pub items: Vec<Skipped>,
}

/// What one cycle did, and (as the totals) what a run did. Every shortfall is a count here
/// (rules 1, 4 and 5). A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleStats {
    pub cycles: u64,
    /// A cycle refused because its time was not after the previous cycle's.
    pub out_of_order: u64,
    pub rows: u64,
    /// Rows on the ground, excluded.
    pub ground_excluded: u64,
    /// Rows whose ground state was unknown (evaluated).
    pub ground_unknown: u64,
    /// Rows with no altitude (rule 4).
    pub alt_missing: u64,
    /// Rows whose altitude was older than the hold (not monitored).
    pub alt_stale: u64,
    /// Rows whose altitude was stale within the hold (judged at an extrapolated altitude).
    pub alt_extrapolated: u64,
    /// Rows whose altitude had no known age (taken as fresh).
    pub alt_age_unknown: u64,
    /// Tracks not monitored for altitude in a cycle where they were more than the share that
    /// degrades the net.
    pub alt_loss: u64,
    /// Track cycles with a point that qualified as a landing over the runway (not judged).
    pub landings: u64,
    /// Path points not judged for a landed track (the roll-out), and those of them outside every
    /// runway strip (past the far end or off the runway).
    pub rollout_points: u64,
    pub rollout_off_runway: u64,
    /// Tracks whose altitude was lost (missing or past the hold): protection lost, whatever the
    /// share, and of them those that had an open alert (held open).
    pub protection_lost: u64,
    pub protection_lost_open: u64,
    /// Rows with no vertical rate (predicted level).
    pub rate_unknown: u64,
    /// Rows with a bad value.
    pub rows_bad: u64,
    /// Rows past the track capacity.
    pub rows_over_capacity: u64,
    /// Rows whose 95 % radius was not usable or above the cap and was held to the cap.
    pub r95_held: u64,
    /// Track cycles with a 95 % radius above [`WIDE_R95_M`] or unusable, and the cycles' count of
    /// them when more than `wide_degrade_share` of the airborne tracks (the net degrades).
    pub wide: u64,
    pub wide_degrade: u64,
    /// Rows lost before they reached the net (told by the caller).
    pub rows_lost_upstream: u64,
    /// Tracks not evaluated for want of budget.
    pub over_budget: u64,
    /// Tracks evaluated.
    pub evaluated: u64,
    /// Rows whose altitude had no altimeter setting (lowered by the allowance, evaluated).
    pub qnh_missing: u64,
    /// Tracks predicted below the chart altitude and not below their alert floor.
    pub below_mva: u64,
    /// Tracks with at least one point without coverage (rule 1).
    pub no_coverage: u64,
    /// Track cycles below the floor within the look-ahead.
    pub violations: u64,
    /// Of them, hidden by an inhibition.
    pub suppressed: u64,
    pub opened: u64,
    pub closed: u64,
    /// Alerts that did not fit the open table and were logged at once.
    pub open_full: u64,
    /// Finished alerts the alert log could not hold.
    pub alerts_dropped: u64,
    /// Skipped tracks the skip log could not hold.
    pub skips_dropped: u64,
    /// Cycles whose floors did not match the planned points.
    pub floor_mismatch: u64,
    /// Reports to the net state table that it refused.
    pub net_refused: u64,
}
