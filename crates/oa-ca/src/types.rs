//! Plain data of conflict alert. Core (pure).

use alloc::vec::Vec;
use oa_safety::geom::Polygon;

/// Most volumes a configuration may name (not counting the default volume). C2: a facility's
/// reduced-minima volumes (terminal areas, parallel approaches, helicopter routes) number in the
/// tens; 32 leaves room and keeps the per-pair volume lookup bounded.
pub const MAX_VOLUMES: usize = 32;

/// [`CaTrack::flags`]: the track is coasting (no report this cycle).
pub const F_COAST: u8 = 1;
/// [`CaTrack::flags`]: the track's position is uncertain.
pub const F_UNCERTAIN: u8 = 2;
/// [`CaTrack::flags`]: the track is held by radar only (no ADS-B).
pub const F_RADAR_ONLY: u8 = 4;
/// [`CaTrack::flags`]: the identity of the track is not confirmed.
pub const F_IDENTITY_UNCONFIRMED: u8 = 8;

/// [`CaTrack::ground`]: the tracker does not say.
pub const GROUND_UNKNOWN: u8 = 0;
/// [`CaTrack::ground`]: airborne.
pub const AIRBORNE: u8 = 1;
/// [`CaTrack::ground`]: on the ground.
pub const ON_GROUND: u8 = 2;

/// [`Alert::flags`]: an altitude was unknown on either track at some cycle of the alert (the
/// test was horizontal only, M5 rule 4).
pub const AF_ALT_UNKNOWN: u16 = 1;
/// [`Alert::flags`]: either track was coasting.
pub const AF_COAST: u16 = 2;
/// [`Alert::flags`]: either track was uncertain.
pub const AF_UNCERTAIN: u16 = 4;
/// [`Alert::flags`]: either track was radar only.
pub const AF_RADAR_ONLY: u16 = 8;
/// [`Alert::flags`]: either track's identity was unconfirmed.
pub const AF_IDENTITY_UNCONFIRMED: u16 = 16;
/// [`Alert::flags`]: either aircraft was on the ground.
pub const AF_ONE_GROUND: u16 = 32;
/// [`Alert::flags`]: a vertical rate was unknown on either track (the pair was tested horizontally only).
pub const AF_RATE_UNKNOWN: u16 = 64;
/// The current alert instance is intentionally hidden by an explicit suppression or inhibition.
/// Unlike diagnostic flags, this bit follows the current state and is cleared when suppression ends.
pub const AF_SUPPRESSED: u16 = 128;

/// The vertical limit is widened by this for each aircraft of a pair whose track is coasting or
/// uncertain, feet: its altitude is then older or less sure than a reported one (M5 rule 4,
/// "a poor track widens the alert, never hides it"). A named constant of this crate, not an
/// adaptation value of any facility; the horizontal limit is widened by each track's own 95 %
/// radius instead.
pub const COAST_V_ALLOWANCE_FT: f64 = 100.0;

/// Largest altitude magnitude a row may carry, feet; a row beyond it is refused as bad. A
/// data-sanity bound, so that the vertical solver's stated bounds hold.
pub const MAX_ABS_ALT_FT: f64 = 1.0e5;
/// Largest vertical rate magnitude a row may carry, feet per minute (a data-sanity bound).
pub const MAX_ABS_VRATE_FPM: f64 = 6.0e4;

/// One track's state at a cycle, as given by the shell from the tracker's row.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct CaTrack {
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
// Budget: one per live track per cycle, copied and sorted every cycle.
const _: () = assert!(core::mem::size_of::<CaTrack>() == 48);

/// Where a volume is.
// A7.2: cold struct (adaptation), size guard waived. The polygon is held inline so that the
// configuration stays `Copy` and allocates nothing (C1); there are at most `MAX_VOLUMES` of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Area {
    /// A box in latitude and longitude, 1e-7 degree, edges included. It must not cross the
    /// antimeridian (a volume that does is two boxes).
    Box {
        lat_min_e7: i32,
        lat_max_e7: i32,
        lon_min_e7: i32,
        lon_max_e7: i32,
    },
    /// A polygon of `oa-safety` (a point on its edge is outside it).
    Polygon(Polygon),
}

/// An alerting volume: where it is, the altitude band it covers (feet, both ends included) and
/// the minima inside it. Adaptation data: nothing here has a default.
// A7.2: cold struct (adaptation), size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volume {
    pub area: Area,
    pub alt_lo_ft: i32,
    pub alt_hi_ft: i32,
    /// Horizontal minimum, metres.
    pub h_m: f64,
    /// Vertical minimum, feet.
    pub v_ft: f64,
}

/// The settings of conflict alert. Every value is the caller's (no defaults, M5: no safety value
/// is chosen by the core). A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaConfig {
    /// Tracks per cycle (capacity of every per-track table).
    pub max_tracks: usize,
    /// Open alerts at once.
    pub max_open: usize,
    /// Pairs kept between the grid scan and the evaluation in one cycle.
    pub max_candidates: usize,
    /// Look-ahead, nanoseconds.
    pub look_ahead_ns: i64,
    /// An alert whose pair has been out of conflict this long ends, nanoseconds.
    pub hysteresis_ns: i64,
    /// Grid work per cycle: band steps, cell probes and tracks visited in a cell.
    pub work_budget: u64,
    /// Exact pair evaluations per cycle.
    pub eval_budget: usize,
    /// A row faster than this, metres per second, is refused as bad.
    pub max_speed_mps: f64,
    /// A track's 95 % radius is held to this, metres (a row without a usable radius takes it).
    pub r95_cap_m: f64,
    /// Height of a band of the candidate grid, metres.
    pub grid_band_m: f64,
    /// The volume of a pair outside every volume of the list.
    pub default_volume: Volume,
    /// The volumes, tried in order; `n_volumes` of them are used.
    pub volumes: [Volume; MAX_VOLUMES],
    pub n_volumes: usize,
}

/// One alert: one pair in conflict, from its first cycle to its last.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Alert {
    pub addr_a: u32,
    pub addr_b: u32,
    /// First cycle in conflict, Unix ns.
    pub start_ns: i64,
    /// Last cycle in conflict (or kept alive unevaluated), Unix ns.
    pub end_ns: i64,
    /// The cycle of the most urgent prediction (smallest time to loss, then smallest predicted
    /// horizontal miss; the first if equal), Unix ns.
    pub peak_ns: i64,
    /// Time to loss at the first cycle, and at the peak, seconds (zero: current).
    pub ttl_first_s: f32,
    pub ttl_peak_s: f32,
    /// Horizontal separation at the peak cycle, metres, and the predicted horizontal miss then.
    pub h_now_peak_m: f32,
    pub h_miss_peak_m: f32,
    /// Smallest predicted horizontal miss over every cycle of the alert, metres.
    pub min_h_miss_m: f32,
    /// Smallest predicted vertical miss over every cycle with a known altitude, feet; NaN if none.
    pub min_v_miss_ft: f32,
    /// The effective limits at the peak cycle: horizontal metres, vertical feet (NaN if the
    /// altitude was unknown).
    pub h_eff_m: f32,
    pub v_eff_ft: f32,
    /// Mean of the two altitudes at the peak cycle, feet; NaN if either was unknown.
    pub alt_ft: f32,
    /// Midpoint of the two positions at the peak cycle, 1e-7 degree.
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Cycles in conflict; of them, current ones; and cycles kept alive without being evaluated.
    pub ticks_conflict: u32,
    pub ticks_current: u32,
    pub ticks_unevaluated: u32,
    /// Diagnostic [`AF_`] bits seen during the alert plus the current [`AF_SUPPRESSED`] state.
    pub flags: u16,
    /// 1 if either aircraft was on the ground at the peak cycle, else 0.
    pub ground_either: u8,
}
// Budget: one per open alert, searched every evaluated pair.
const _: () = assert!(core::mem::size_of::<Alert>() == 96);

/// A track that had a known altitude and has lost it (M5 rule 4), raised once per loss.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct AltLost {
    pub t_ns: i64,
    pub addr: u32,
    /// The last known altitude, feet.
    pub last_alt_ft: f32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    pub flags: u8,
    pub ground: u8,
}
// Budget: one per altitude loss; a log entry.
const _: () = assert!(core::mem::size_of::<AltLost>() == 32);

/// The finished alerts of a run of cycles, bounded: a push past the capacity is counted in
/// `overflow` and the alert is lost (RULESET C2). The consumer clears it after reading.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertLog {
    pub capacity: usize,
    pub overflow: u64,
    pub items: Vec<Alert>,
}

/// The altitude-loss events of a run of cycles, bounded like [`AlertLog`].
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventLog {
    pub capacity: usize,
    pub overflow: u64,
    pub items: Vec<AltLost>,
}

/// What one cycle did, and (as the totals) what a run did. Every shortfall is a count here (M5
/// rule 5). A7.2: cold struct, size guard waived.
// A7.1: add six overlapping rejection-reason totals (48 bytes, 272 -> 320) so a bad row remains
// one rejected row while reports can identify every failed validation predicate without samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleStats {
    pub cycles: u64,
    /// A cycle refused because its time was not after the previous cycle's.
    pub out_of_order: u64,
    pub tracks: u64,
    /// Rows with a position that is not a coordinate, a velocity, altitude or rate that is not
    /// finite (altitude and rate: NaN is unknown) or beyond the data-sanity bounds.
    pub rows_bad: u64,
    /// Overlapping subsets of `rows_bad`: rows failing each predicate, not additional row losses.
    pub rows_bad_lat: u64,
    pub rows_bad_lon: u64,
    pub rows_bad_velocity_component: u64,
    pub rows_bad_speed: u64,
    pub rows_bad_altitude: u64,
    pub rows_bad_vrate: u64,
    /// Rows sharing an address with another row of the cycle (all of them are left out).
    pub rows_duplicate: u64,
    /// Rows past the track capacity.
    pub rows_over_capacity: u64,
    /// Rows above 85 degrees of latitude. They are evaluated, but the cell search is not shown to
    /// find every pair there, so they degrade the net (see the crate documentation).
    pub rows_high_lat: u64,
    /// Rows whose 95 % radius was not finite, negative or above the cap, and was held to the cap.
    pub r95_clamped: u64,
    pub grid_work: u64,
    /// Tracks the grid scan did not reach before its budget ran out.
    pub tracks_unscanned: u64,
    /// Pairs the grid tested by the chord, not the flat test, because the flat test's validity
    /// (for the pair's own radius and latitudes) did not hold. Not a shortfall: the chord test
    /// has no latitude limit.
    pub pairs_chord: u64,
    /// Pairs the grid found inside the radius and the altitude test: every one, before the
    /// table-full test. `evaluated + not_evaluated == candidates`.
    pub candidates: u64,
    /// Pairs the grid skipped because both altitudes and rates were known and they cannot come
    /// within the vertical limit inside the look-ahead.
    pub pairs_skipped_vertical: u64,
    /// Of `not_evaluated`: candidates that did not fit the candidate table.
    pub candidates_dropped: u64,
    pub evaluated: u64,
    /// Candidates not evaluated: those beyond the evaluation budget (the largest current
    /// separations), those that did not fit the candidate table (the largest, see
    /// `max_candidates`) and those whose projection failed.
    pub not_evaluated: u64,
    /// Of `not_evaluated`: pairs whose projection or velocity conversion failed.
    pub eval_failed: u64,
    /// Pairs left out because both were on the ground (a surface matter, not conflict alert).
    pub ground_excluded: u64,
    /// Evaluated pairs found farther apart than they can close in the look-ahead.
    pub beyond_reach: u64,
    /// Evaluated pairs tested horizontally only (an altitude unknown).
    pub alt_unknown_pairs: u64,
    /// Pair cycles in conflict.
    pub in_conflict: u64,
    /// Pair cycles hidden by an explicit pair/group control or area inhibition.
    pub suppressed: u64,
    /// Suppression audit entries that did not fit; suppression still applied and the net degrades.
    pub suppressions_dropped: u64,
    /// Explicit-control lookups refused for an insufficient work budget.
    pub control_lookup_refused: u64,
    pub opened: u64,
    pub closed: u64,
    /// Alerts not opened because the open table was full.
    pub open_dropped: u64,
    /// Altitude losses raised; and those the event log could not hold.
    pub alt_lost: u64,
    pub alt_lost_dropped: u64,
    /// Finished alerts the alert log could not hold.
    pub alerts_dropped: u64,
    /// Reports to the net state table that it refused.
    pub net_refused: u64,
    /// Evaluated pairs with both altitudes known and a rate unknown, tested horizontally only.
    pub rate_unknown_pairs: u64,
    /// Rows the caller says were lost before they reached this cycle (a table that overflowed, a
    /// row with a column missing): see [`crate::ca_note_lost_input`].
    pub rows_lost_upstream: u64,
    /// Of `alerts_dropped`: alerts ended by [`crate::ca_finish`], after the last report.
    pub finish_dropped: u64,
}

const _: () = assert!(core::mem::size_of::<CycleStats>() == 320);
