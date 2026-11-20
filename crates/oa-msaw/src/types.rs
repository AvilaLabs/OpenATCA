//! Plain data of the minimum safe altitude warning. Core (pure).

use alloc::vec::Vec;
use oa_safety::geom::Polygon;

/// Most sample points of the predicted path of one track. C2: the shell does one terrain,
/// obstacle and chart lookup per point and track, so the bound is also a bound on that work; 32
/// points over a look-ahead of a few minutes is a point every few seconds.
pub const MAX_SAMPLES: usize = 32;
/// Most procedure areas a configuration may name. C2: a facility's approach and departure areas
/// are runway ends, tens of them in a terminal area.
pub const MAX_PROCS: usize = 32;
/// Largest `max_tracks` the configuration may ask for (every per-track table is allocated whole).
pub const MAX_TRACKS_LIMIT: usize = 1 << 20;
/// Largest `max_open` the configuration may ask for.
pub const MAX_OPEN_LIMIT: usize = 1 << 20;
/// Largest look-ahead, seconds (a data-sanity bound, so that the arithmetic bounds hold).
pub const MAX_LOOK_AHEAD_S: f64 = 600.0;
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
/// The latitude beyond which a row is refused as bad (east-west distances are no longer a flat
/// scale there), degrees.
pub const MAX_ABS_LAT_DEG: f64 = 85.0;

/// [`FloorSample`] value: the shell found no coverage (terrain, chart) or, for an obstacle, no
/// obstacle.
pub const FLOOR_NONE: i32 = i32::MIN;

/// [`MsawTrack::flags`]: the track is coasting (no report this cycle).
pub const F_COAST: u8 = 1;
/// [`MsawTrack::flags`]: the track's position is uncertain.
pub const F_UNCERTAIN: u8 = 2;
/// [`MsawTrack::flags`]: the track is held by radar only (no ADS-B).
pub const F_RADAR_ONLY: u8 = 4;
/// [`MsawTrack::flags`]: the identity of the track is not confirmed.
pub const F_IDENTITY_UNCONFIRMED: u8 = 8;

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
    /// Barometric altitude, feet; NaN when the track has none.
    pub alt_ft: f32,
    /// Vertical rate, feet per minute; NaN when the track has none.
    pub vrate_fpm: f32,
    /// [`F_COAST`], [`F_UNCERTAIN`], [`F_RADAR_ONLY`], [`F_IDENTITY_UNCONFIRMED`].
    pub flags: u8,
    /// [`GROUND_UNKNOWN`], [`AIRBORNE`] or [`ON_GROUND`].
    pub ground: u8,
}

/// A point of a track's predicted path where the shell looks up the floor.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplePoint {
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// The query radius, metres: the track's 95 % radius plus half the distance to the next
    /// point, so that the terrain and obstacles between two points are inside some radius.
    pub radius_m: f32,
}

/// What the shell found at a [`SamplePoint`], in whole feet, rounded up (never down: a floor is
/// conservative when it is high). [`FLOOR_NONE`] means no coverage for `terrain_ft` and
/// `mva_ft` (the net is unavailable there, rule 1) and no obstacle for `obstacle_ft`.
// A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloorSample {
    /// The highest terrain elevation over the point's radius, feet MSL.
    pub terrain_ft: i32,
    /// The highest obstacle top over the point's radius, feet MSL.
    pub obstacle_ft: i32,
    /// The minimum vectoring altitude of the sector containing the point, feet MSL.
    pub mva_ft: i32,
}

/// A procedure area: where a published approach or departure altitude lowers the floor.
// A7.2: cold struct (adaptation), size guard waived. The polygon is held inline so that the
// configuration stays `Copy` and allocates nothing (C1); there are at most `MAX_PROCS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcArea {
    pub area: Polygon,
    /// The published altitude, feet MSL: in the area it replaces the chart altitude when lower.
    pub floor_ft: i32,
}

/// The settings of the net. Every value is the caller's (no defaults: no safety value is chosen
/// by the core). A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsawConfig {
    /// Tracks per cycle (capacity of every per-track table).
    pub max_tracks: usize,
    /// Open alerts at once.
    pub max_open: usize,
    /// Tracks evaluated per cycle: the named budget of rule 5. Above it, the lowest altitudes
    /// are evaluated first and the rest are counted.
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
    /// Clearance above terrain and obstacles inside a procedure area, feet.
    pub proc_clearance_ft: i32,
    /// A row faster than this, metres per second, is refused as bad.
    pub max_speed_mps: f64,
    /// A track's 95 % radius is held to this, metres (a row without a usable radius takes it).
    pub r95_cap_m: f64,
    /// The procedure areas; `n_procs` of them are used.
    pub procs: [ProcArea; MAX_PROCS],
    pub n_procs: usize,
}

/// What set the floor at the worst point of an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FloorKind {
    Terrain = 1,
    Obstacle = 2,
    Mva = 3,
    /// A procedure area's published altitude.
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
    /// Smallest predicted margin over the floor (altitude minus floor, feet; negative is
    /// below), at the worst cycle and point; and what set that floor.
    pub worst_margin_ft: f32,
    pub worst_floor_ft: i32,
    pub worst_kind: FloorKind,
    /// Shortest time to violation over the alert, seconds (zero: below the floor now).
    pub ttv_min_s: f32,
    pub cycles: u32,
    /// [`AF_RATE_UNKNOWN`] and the other `AF_` bits, of any cycle of the alert.
    pub flags: u16,
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
    /// Rows with no vertical rate (predicted level).
    pub rate_unknown: u64,
    /// Rows with a bad value.
    pub rows_bad: u64,
    /// Rows past the track capacity.
    pub rows_over_capacity: u64,
    /// Rows whose 95 % radius was not usable or above the cap and was held to the cap.
    pub r95_held: u64,
    /// Rows lost before they reached the net (told by the caller).
    pub rows_lost_upstream: u64,
    /// Tracks not evaluated for want of budget.
    pub over_budget: u64,
    /// Tracks evaluated.
    pub evaluated: u64,
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
