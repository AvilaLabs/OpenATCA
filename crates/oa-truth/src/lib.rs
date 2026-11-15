#![forbid(unsafe_code)]
//! oa-truth: reference trajectories from ADS-B, the truth side of the M2 benchmark
//! (docs/M2_DESIGN.md section 1). Core (pure, B1): [`build_truth`] and [`truth_at`] are pure
//! functions of their arguments; the `oa-truth-check` binary is the shell (it reads a file,
//! reads the clock and writes stdout).
//!
//! # What it does
//!
//! 1. **Selection.** Reports must carry NACp >= 8, NIC >= 7, SIL 3 and, where present, ADS-B
//!    version 2 ([`DEFAULT_CONFIG`]). The quality fields are printed by readsb on a minority of
//!    rows, so each is carried forward within an aircraft for at most 60 s; a row with no value
//!    in that age is not truth. Rows with readsb's stale flag (position older than 20 s, the
//!    only fault flag a trace row has), rows whose source is not ADS-B (MLAT, TIS-B, ADS-R and
//!    the rest carry no meaningful NACp), duplicate times (same-time reports that disagree beyond their NACp radii, or more than 16 distinct), reports of the reserved addresses
//!    (all zeros and all ones, under `invalid`) and kinematic jumps are removed. A jump is a
//!    displacement the speed rule (`oa_adsb::jump_verdict`) calls impossible: over the hard cap of
//!    the emitter category, or inconsistent with the aircraft's own ground speed and the gap. Every
//!    removed row is counted under one reason ([`REMOVE_NAMES`]) in [`TruthStats`], so
//!    `rows_in = rows_kept + removed` always balances.
//! 2. **Legs.** An aircraft (the 24-bit address plus `oa_adsb::NON_ICAO`, the same key as the
//!    ADS-B table in oa-replay, so non-ICAO addresses stay separate) splits into legs at
//!    readsb's new-leg flag, at a gap of more than 120 s between kept reports, and (counted
//!    separately) after repeated jump rejections or when the leg leaves the frame's range.
//! 3. **Smoother.** Per leg, in a local east-north frame about the leg's first report
//!    (`frame.rs`; oa-geo has no local frame), a fixed-interval Rauch-Tung-Striebel smoother
//!    with a nearly-constant-acceleration model (`smooth.rs`). The measurement noise per report
//!    is its NACp bound as a 95 % radius, converted to a per-axis sigma.
//! 4. **No extrapolation.** [`truth_at`] answers only inside a leg and only where a real report
//!    lies within 10 s on both sides of the query time (a report exactly at the time counts on
//!    both sides). Otherwise it says why ([`TruthQuery`]).
//! 5. **Accuracy, measured.** Leave-one-out: every fifth report of every leg is hidden, the leg
//!    is smoothed again without them and each hidden report is predicted from the rest under
//!    the same bracket rule. [`LooStats`] has the residual distribution (horizontal, cross-track,
//!    along-track, altitude) and how often the predicted 95 % uncertainty was honest. A truth
//!    sample is refused ([`TruthQuery::TooUncertain`]) when its 95 % horizontal radius exceeds
//!    50 m.
//! 6. **Altitude.** Barometric altitude is not smoothed: linear interpolation between the two
//!    bracketing reports, absent if either lacks it.
//!
//! The gates are named constants ([`MIN_NACP`] and the rest) copied into [`TruthConfig`]; the
//! leave-one-out result decides whether they stay.
//!
//! # Determinism
//!
//! Same inputs give the same [`Truth`] and [`TruthStats`], bit for bit. What guarantees it:
//! reports are put in the total order (address, time, input index) before anything else, so
//! input order does not matter; there are no hash maps, threads or randomness; every sum and
//! matrix product is written in a fixed order (`linalg.rs`) and the compiler does not fuse or
//! reorder floating-point operations; time differences are exact `i64` nanoseconds converted
//! once with a division; the leave-one-out hidden set is by position in the leg. The only
//! platform dependence is the C library's `sin`, `cos`, `atan2` and `asin` in the frame, which
//! can differ in the last bit between libm versions; one binary on one machine is bit exact,
//! and [`truth_hash_into`] is the digest that shows it.
//!
//! # Bounds and refusal
//!
//! Input is bounded by [`MAX_REPORTS`]; columns of different lengths and an invalid
//! [`TruthConfig`] are refused with [`TruthError`] (E5). A row that fails a rule is removed and
//! counted, never passed on and never dropped silently. Cold, offline code (A7.2, C6): the
//! truth model is a struct-of-arrays, one column per field, with no hot per-row struct.

mod build;
mod frame;
mod linalg;
mod loo;
mod quality;
mod query;
mod smooth;

pub use build::{build_truth, pair_evals_this_thread};
pub use quality::{
    nacp_epu_m, nacv_epu_mps, quality_check, quality_eval, Held, QualityState, QualityVerdict,
    HELD_NONE,
};
pub use query::{
    aircraft_addrs, leg_addr, leg_count, leg_end_ms, leg_end_ns, leg_reports, leg_start_ms,
    leg_start_ns, legs_of, truth_at, truth_hash_into,
};
pub use smooth::SIGMA95;

use std::fmt;

/// Limit: reports accepted by [`build_truth`], 200 Mi (about 14 GB as decoded reports; a day over
/// one centre's box is a few million).
pub const MAX_REPORTS: usize = 200 * 1024 * 1024;

/// Gate: minimum NACp (95 % horizontal position uncertainty under 0.05 NM, 92.6 m).
pub const MIN_NACP: u8 = 8;
/// Gate: minimum NIC.
pub const MIN_NIC: u8 = 7;
/// Gate: required SIL.
pub const REQUIRED_SIL: u8 = 3;
/// Gate: minimum ADS-B version, applied where the version is known.
pub const MIN_VERSION: u8 = 2;
/// Gate: a quality value is carried forward at most this long, ms.
pub const QUALITY_CARRY_MS: i64 = 60_000;
/// Gate: a gap between kept reports longer than this starts a new leg, ms.
pub const LEG_GAP_MS: i64 = 120_000;
/// Gate: truth needs a real report within this long on both sides of the query time, ms.
pub const BRACKET_MS: i64 = 10_000;
/// Gate: truth with a 95 % horizontal radius above this is refused, metres.
pub const MAX_SIGMA95_H_M: f64 = 50.0;
/// Gate: the hard speed cap of the ordinary emitter classes, knots (`oa_adsb::CAP_ORDINARY_KT`).
/// Whether a displacement between consecutive kept reports is a jump is decided by
/// `oa_adsb::jump_verdict`: this cap, the aircraft's own reported ground speed and the time gap.
pub const JUMP_CAP_KT: f64 = oa_adsb::CAP_ORDINARY_KT;
/// Allowance added to the jump distance so position noise at short intervals is not a jump, m.
pub const JUMP_SLACK_M: f64 = 200.0;
/// After this many jump rejections in a row the next report starts a new leg (the report the
/// comparisons were anchored to was probably the bad one).
pub const JUMP_RESYNC: u32 = 5;
/// Motion model: spectral density of the white jerk driving the acceleration, m^2/s^5.
pub const ACCEL_Q: f64 = 0.1;
/// Multiplier on the per-axis sigma derived from NACp (1.0 takes the NACp bound at face value).
pub const SIGMA_SCALE: f64 = 1.0;
/// A leg with fewer kept reports than this is not smoothed; its reports are removed and counted.
pub const MIN_LEG_POINTS: u32 = 3;
/// Leave-one-out: every this-many-th report of a leg is hidden.
pub const LOO_STRIDE: u32 = 5;
/// A leg whose reports reach this far from its origin (radians of arc, 3,000 km) is split.
pub const MAX_FRAME_ARC_RAD: f64 = 3_000_000.0 / 6_371_000.0;

/// Source types kept as ADS-B (`oa_adsb::SOURCE_NAMES` indexes: `adsb_icao`, `adsb_icao_nt`,
/// `adsb_other`). Files from before 2022 have no source column and are kept.
pub const ADSB_SOURCES: [u8; 3] = [0, 1, 8];

/// Tag and version of [`truth_hash_into`].
pub const TRUTH_HASH_TAG: &[u8; 5] = b"OATR1";

/// Why a report was removed, indexing [`TruthStats::removed`].
pub const REMOVE_NAMES: [&str; REMOVE_KINDS] = [
    "invalid",
    "stale_flag",
    "source_not_adsb",
    "no_quality_in_age",
    "nacp",
    "nic",
    "sil",
    "version",
    "duplicate_time",
    "jump",
    "short_leg",
    "oversized_same_time_group",
];
/// Number of removal reasons.
pub const REMOVE_KINDS: usize = 12;
pub const REM_INVALID: usize = 0;
pub const REM_STALE: usize = 1;
pub const REM_SOURCE: usize = 2;
pub const REM_NO_QUALITY: usize = 3;
pub const REM_NACP: usize = 4;
pub const REM_NIC: usize = 5;
pub const REM_SIL: usize = 6;
pub const REM_VERSION: usize = 7;
pub const REM_DUPLICATE: usize = 8;
pub const REM_JUMP: usize = 9;
pub const REM_SHORT_LEG: usize = 10;
pub const REM_OVERSIZED: usize = 11;
/// Same-time reports of one aircraft: the 2-degree-of-freedom chi-square value at 99.9 %, the
/// per-axis sigma of a NACp 95 % radius is `radius / SIGMA95`.
pub const SAME_TIME_CHI2: f64 = 13.82;
/// Same-time reports of one aircraft: most distinct eligible reports tested pairwise; a larger
/// group is refused.
pub const MAX_SAME_TIME_DISTINCT: usize = 16;

/// Why a leg was started, indexing [`TruthStats::splits`]. The first leg of an aircraft is not
/// a split.
pub const SPLIT_NAMES: [&str; SPLIT_KINDS] = ["new_leg_flag", "gap", "jump_resync", "frame_range"];
/// Number of split reasons.
pub const SPLIT_KINDS: usize = 4;
pub const SPLIT_NEW_LEG: usize = 0;
pub const SPLIT_GAP: usize = 1;
pub const SPLIT_JUMP: usize = 2;
pub const SPLIT_FRAME: usize = 3;

/// Settings of [`build_truth`]; the gates of the design. Plain record (A1).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct TruthConfig {
    pub min_nacp: u8,
    pub min_nic: u8,
    pub required_sil: u8,
    pub min_version: u8,
    pub quality_carry_ms: i64,
    pub leg_gap_ms: i64,
    pub bracket_ms: i64,
    pub max_sigma95_h_m: f64,
    pub max_jump_kt: f64,
    pub jump_slack_m: f64,
    pub jump_resync: u32,
    pub accel_q: f64,
    pub sigma_scale: f64,
    pub min_leg_points: u32,
    /// Run the leave-one-out accuracy measurement (doubles the smoothing work).
    pub loo: bool,
    pub loo_stride: u32,
}

/// The design's values.
pub const DEFAULT_CONFIG: TruthConfig = TruthConfig {
    min_nacp: MIN_NACP,
    min_nic: MIN_NIC,
    required_sil: REQUIRED_SIL,
    min_version: MIN_VERSION,
    quality_carry_ms: QUALITY_CARRY_MS,
    leg_gap_ms: LEG_GAP_MS,
    bracket_ms: BRACKET_MS,
    max_sigma95_h_m: MAX_SIGMA95_H_M,
    max_jump_kt: JUMP_CAP_KT,
    jump_slack_m: JUMP_SLACK_M,
    jump_resync: JUMP_RESYNC,
    accel_q: ACCEL_Q,
    sigma_scale: SIGMA_SCALE,
    min_leg_points: MIN_LEG_POINTS,
    loo: true,
    loo_stride: LOO_STRIDE,
};

impl Default for TruthConfig {
    fn default() -> Self {
        DEFAULT_CONFIG
    }
}

/// Why [`build_truth`] refused its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruthError {
    /// The report columns have different lengths.
    ColumnLength,
    /// More than [`MAX_REPORTS`] reports.
    TooManyReports(usize),
    /// A configuration value is out of range; the name says which.
    BadConfig(&'static str),
    /// More points or legs than a `u32` index holds (cannot happen under [`MAX_REPORTS`]).
    Capacity,
}

impl fmt::Display for TruthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TruthError::ColumnLength => write!(f, "report columns have different lengths"),
            TruthError::TooManyReports(n) => {
                write!(f, "{n} reports exceeds the limit of {MAX_REPORTS}")
            }
            TruthError::BadConfig(w) => write!(f, "bad configuration: {w}"),
            TruthError::Capacity => write!(f, "index capacity exceeded"),
        }
    }
}

impl std::error::Error for TruthError {}

/// Index of one leg in a [`Truth`]. Meaningful for that truth only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LegIx(pub u32);

/// The legs of one aircraft: `count` legs from `first`, consecutive and in time order. `count`
/// is 0 for an unknown aircraft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegRange {
    pub addr: u32,
    pub first: LegIx,
    pub count: u32,
}

/// The truth model, struct-of-arrays (A3): aircraft, legs and report columns, each with its own
/// index space joined by prefix offsets. Built by [`build_truth`], read by [`truth_at`] and the
/// accessors; the fields are public so the model can be hashed and inspected, but nothing
/// outside this crate should depend on the layout.
#[derive(Debug, Clone, Default, PartialEq)]
// A7.2: cold struct (collection of columns, built once, queried read-only)
pub struct Truth {
    /// The settings it was built with (queries use the gates and the motion model).
    pub cfg: TruthConfig,
    /// Aircraft address words, ascending.
    pub ac_addr: Vec<u32>,
    /// First leg of each aircraft, plus one final entry equal to the leg count.
    pub ac_first_leg: Vec<u32>,
    /// Address word of each leg.
    pub leg_addr: Vec<u32>,
    /// First report of each leg, plus one final entry equal to the report count.
    pub leg_first_pt: Vec<u32>,
    /// Frame origin latitude of each leg, degrees.
    pub leg_lat0: Vec<f64>,
    /// Frame origin longitude of each leg, degrees.
    pub leg_lon0: Vec<f64>,
    /// Report time, Unix ns.
    pub pt_t_ns: Vec<i64>,
    /// Barometric altitude, feet; `oa_adsb::NONE_I32` if absent.
    pub pt_alt_ft: Vec<i32>,
    /// Reported vertical rate, fpm; `oa_adsb::NONE_I32` if absent.
    pub pt_vrate_fpm: Vec<i32>,
    /// Forward state `[east, east vel, east acc, north, north vel, north acc]` in the leg frame.
    pub pt_xf: Vec<[f64; 6]>,
    /// Forward covariance, packed `[p00, p01, p02, p11, p12, p22]` (shared by both axes).
    pub pt_pf: Vec<[f64; 6]>,
    /// Smoothed state.
    pub pt_xs: Vec<[f64; 6]>,
    /// Smoothed covariance.
    pub pt_ps: Vec<[f64; 6]>,
}

/// One truth sample.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived (returned by value per query)
pub struct TruthSample {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Interpolated barometric altitude; `None` if either bracketing report lacks it.
    pub baro_alt_ft: Option<f64>,
    pub vel_east_mps: f64,
    pub vel_north_mps: f64,
    /// Vertical rate: the bracketing reports' vertical rates interpolated (barometric, or
    /// geometric where readsb printed that), else the altitude difference over the bracket if
    /// it is at least 2 s wide, else `None`.
    pub vert_rate_fpm: Option<f64>,
    /// 95 % horizontal radius of the smoothed position, metres.
    pub sigma95_h_m: f64,
    /// The leg the sample is in.
    pub leg: LegIx,
}

/// The answer of [`truth_at`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TruthQuery {
    Ok(TruthSample),
    /// No truth for this address at all.
    UnknownAircraft,
    /// The time is outside every leg of the aircraft (before its first report, after its last,
    /// or between legs): nothing to bracket, no extrapolation.
    OutsideLegs,
    /// The time is inside a leg but the nearest report on at least one side is more than the
    /// bracket (10 s) away.
    NoNearbyReport,
    /// Bracketed, but the 95 % horizontal radius exceeds the gate (50 m).
    TooUncertain,
}

/// A distribution of errors. `mean` is the signed mean (bias) of the quantity; `rms`, `median`,
/// `p95`, `p99` and `max` are of its absolute value, percentiles by nearest rank. All zero when
/// `n` is 0. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Dist {
    pub n: u64,
    pub mean: f64,
    pub rms: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// A count out of a total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Fraction {
    pub n: u64,
    pub hit: u64,
}

/// The leave-one-out result (design section 1.5). Residuals are hidden report minus the
/// prediction from the rest, so they include the hidden report's own noise: they bound the
/// truth's error from above.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct LooStats {
    /// Every this-many-th report was hidden.
    pub stride: u32,
    /// Reports hidden.
    pub hidden: u64,
    /// Hidden reports with no real report within the bracket on both sides of the rest: not
    /// predicted, as in a query.
    pub no_bracket: u64,
    /// Horizontal distance, metres.
    pub horizontal: Dist,
    /// Cross-track component (positive right of the predicted track), metres; hidden reports
    /// where the predicted speed is under 1 m/s have no track and are left out of this and
    /// `along`.
    pub cross: Dist,
    /// Along-track component (positive ahead), metres.
    pub along: Dist,
    /// Barometric altitude, feet: hidden report minus the interpolation between its
    /// neighbours in the rest (only where all three have an altitude).
    pub alt: Dist,
    /// Predicted 95 % horizontal radius (of the truth) at or under the gate.
    pub under_gate: Fraction,
    /// Horizontal residual inside the predicted 95 % radius of a report (truth plus the hidden
    /// report's own noise); near 95 % if the model's uncertainty is honest.
    pub coverage95: Fraction,
}

/// What a build did. Plain record (A1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct TruthStats {
    /// Reports offered.
    pub rows_in: u64,
    /// Distinct address words in them.
    pub aircraft_in: u64,
    /// Reports in legs of the truth.
    pub rows_kept: u64,
    /// Reports removed, by reason ([`REMOVE_NAMES`]); `rows_in = rows_kept + sum`.
    pub removed: [u64; REMOVE_KINDS],
    /// Aircraft with at least one leg.
    pub aircraft_out: u64,
    /// Legs.
    pub legs: u64,
    /// Legs started by a split, by reason ([`SPLIT_NAMES`]).
    pub splits: [u64; SPLIT_KINDS],
    /// Reports that passed on a carried-forward quality value (at least one of the checked
    /// fields).
    pub quality_carried: u64,
    /// Share of report times with predicted 95 % horizontal radius within the gate.
    pub unc_at_reports: Fraction,
    /// Share of interval midpoints (intervals of up to twice the bracket) within the gate.
    pub unc_at_midpoints: Fraction,
    /// Leave-one-out accuracy; all zero if [`TruthConfig::loo`] was off.
    pub loo: LooStats,
}
