//! Plain data of the encounter record. Core (pure).

use alloc::vec::Vec;

/// Most classes a configuration may name.
pub const MAX_CLASSES: usize = 8;
/// [`Encounter::class`] of an encounter inside the record volume that meets no named class.
pub const CLASS_VOLUME_ONLY: u8 = 254;
/// [`Encounter::class`] of an encounter whose altitude was unknown on either track at every tick
/// it was inside the record volume.
pub const CLASS_ALT_UNKNOWN: u8 = 255;

/// [`Obs::ground`]: the tracker does not say.
pub const GROUND_UNKNOWN: u8 = 0;
/// [`Obs::ground`]: airborne.
pub const AIRBORNE: u8 = 1;
/// [`Obs::ground`]: on the ground.
pub const ON_GROUND: u8 = 2;

/// Metres in a foot.
pub const M_PER_FT: f64 = 0.3048;

/// One track's state at a tick, as given by the shell from the tracker's row.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Obs {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub addr: u32,
    /// Barometric altitude, feet; NaN when the row has none.
    pub alt_ft: f32,
    /// Vertical rate, feet per minute; NaN when the row has none.
    pub vrate_fpm: f32,
    /// The tracker's flag byte, kept as given for the record.
    pub flags: u8,
    /// [`GROUND_UNKNOWN`], [`AIRBORNE`] or [`ON_GROUND`].
    pub ground: u8,
}
// Budget: one per live track per tick, copied and sorted every cycle.
const _: () = assert!(core::mem::size_of::<Obs>() == 32);

/// One class: a pair is in it when its horizontal separation is under `h_m` and its vertical
/// separation under `v_ft`, both strictly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassLimit {
    pub h_m: f64,
    pub v_ft: f64,
}

/// The settings of the record. Every value is the caller's (no defaults). A7.2: cold struct,
/// size guard waived.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncounterConfig {
    /// Tracks per cycle (capacity of every per-track table). Peak traffic in one facility's
    /// airspace is a few thousand; the tracker's own limit is 4096.
    pub max_tracks: usize,
    /// Open encounters at once. Bounded by pairs inside the volume, which is a small multiple of
    /// the tracks.
    pub max_open: usize,
    /// Pairs kept between the grid scan and the evaluation in one cycle.
    pub max_candidates: usize,
    /// Record volume: horizontal radius, metres, and vertical half-height, feet (strict).
    pub volume_h_m: f64,
    pub volume_v_ft: f64,
    /// Classes, tightest first; the first that fits names a tick. `n_classes` of them are used.
    pub classes: [ClassLimit; MAX_CLASSES],
    pub n_classes: usize,
    /// A pair out of the volume this long ends its encounter, ns.
    pub hysteresis_ns: i64,
    /// Grid work per cycle: cell probes plus tracks visited in a cell. Reaching it stops the scan.
    pub work_budget: u64,
    /// Exact pair evaluations per cycle.
    pub eval_budget: usize,
}

/// One finished or open encounter between aircraft `addr_a` < `addr_b`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Encounter {
    pub addr_a: u32,
    pub addr_b: u32,
    /// First tick inside the volume, Unix ns.
    pub start_ns: i64,
    /// Last tick inside the volume (or kept alive unevaluated), Unix ns.
    pub end_ns: i64,
    /// Tick of the closest horizontal approach (the first, if several are equal), Unix ns.
    pub closest_ns: i64,
    /// Time of the true minimum horizontal separation, from the relative position taken as
    /// moving in a straight line between two consecutive ticks, Unix ns.
    pub interp_ns: i64,
    /// Smallest horizontal separation of any tick, metres (the sampled minimum).
    pub min_h_m: f32,
    /// The interpolated minimum horizontal separation, metres; never above `min_h_m`.
    pub min_h_interp_m: f32,
    /// Smallest vertical separation of any tick with a known altitude, feet; NaN if none.
    pub min_v_ft: f32,
    /// Vertical rates of aircraft `a` and `b` at the closest approach, feet per minute (NaN if
    /// unknown), and the largest magnitude of either over every tick inside (NaN if never known).
    pub vrate_a_fpm: f32,
    pub vrate_b_fpm: f32,
    pub max_abs_vrate_fpm: f32,
    /// Vertical separation at the closest approach, feet; NaN if unknown there.
    pub v_at_closest_ft: f32,
    /// Smallest separation with the vertical scaled so the record volume is a cylinder of equal
    /// reach (`hypot(h, v / volume_v * volume_h)`), metres; NaN if altitude was never known.
    pub min_scaled_m: f32,
    /// Mean of the two altitudes at the closest approach, feet; NaN if either was unknown.
    pub alt_ft: f32,
    /// Midpoint of the two positions at the closest approach, 1e-7 degree.
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// Ticks inside the volume; of them, with an altitude unknown; and ticks kept alive without
    /// being evaluated (over budget).
    pub ticks_inside: u32,
    pub ticks_alt_unknown: u32,
    pub ticks_unevaluated: u32,
    /// Tracker flags of aircraft `a` and `b` at the closest approach.
    pub flags_a: u8,
    pub flags_b: u8,
    /// 1 if either was on the ground at the closest approach, else 0.
    pub ground_either: u8,
    /// The tightest class any tick with a known altitude met (an index into the configured
    /// classes), [`CLASS_VOLUME_ONLY`], or [`CLASS_ALT_UNKNOWN`].
    pub class: u8,
}
// Budget: one per open encounter, scanned every cycle.
const _: () = assert!(core::mem::size_of::<Encounter>() == 104);

/// The finished encounters of a run of cycles, bounded: a push past the capacity is counted in
/// `overflow` and the encounter is lost (RULESET C2). The consumer clears it after reading.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EncounterLog {
    pub capacity: usize,
    pub overflow: u64,
    pub items: Vec<Encounter>,
}

/// What one cycle did, and (as the totals) what a run did. Every shortfall is a count here
/// (M5 rule 5): a consumer shows "degraded" when any of `not_evaluated`, `tracks_unscanned`,
/// `candidates_dropped`, `open_dropped` is non-zero. A7.2: cold struct, size guard waived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleStats {
    pub cycles: u64,
    /// A cycle refused because its time was not after the previous cycle's.
    pub out_of_order: u64,
    pub tracks: u64,
    /// Rows with a position that is not a coordinate.
    pub rows_bad: u64,
    /// Rows sharing an address with another row of the cycle (all of them are left out).
    pub rows_duplicate: u64,
    /// Rows past the track capacity.
    pub rows_over_capacity: u64,
    pub grid_work: u64,
    /// Tracks the grid scan did not reach before its budget ran out.
    pub tracks_unscanned: u64,
    pub candidates: u64,
    /// Candidates that did not fit the candidate table.
    pub candidates_dropped: u64,
    pub evaluated: u64,
    /// Candidates beyond the evaluation budget, the largest separations.
    pub not_evaluated: u64,
    /// Pairs inside the volume that were left out because both were on the ground.
    pub ground_excluded: u64,
    /// Pair ticks inside the record volume.
    pub in_volume: u64,
    pub opened: u64,
    pub closed: u64,
    /// Encounters not opened because the open table was full.
    pub open_dropped: u64,
}
