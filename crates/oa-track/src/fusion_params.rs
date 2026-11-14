//! Every tunable of the radar fusion layer as a named constant, each saying what sets it. Kept
//! apart from `params.rs` (the filters' constants). The gates are chi-square quantiles, not fits;
//! the geometry limits are airliner physics; the registration priors are assumptions, since no
//! sourced radar bias exists (docs/BENCHMARKS.md 3.1, "Gaps"): the sweep in the report is the
//! evidence, not these.

use crate::params::NS_PER_S;

// ---- Batching -------------------------------------------------------------------------

/// Plots are associated in time buckets of this length, ns (1 s). A rotating radar delivers a
/// scan over its whole period (4.8 s terminal, 12 s en route), and a plot waiting for the end of
/// the scan would be older than the tracker's out-of-order window. Plots in one bucket are
/// assigned jointly; a track still takes at most one plot per bucket. Plots a second apart are
/// different azimuth sectors, so they compete for the same track only at the edges of a bucket.
/// Set from the scan periods and the tracker's out-of-order window.
pub const BUCKET_NS: i64 = NS_PER_S;
/// Limit: events held waiting for their bucket (ADS-B measurements and plots each), 1 Mi.
pub const MAX_PENDING: usize = 1 << 20;

// ---- Gating ---------------------------------------------------------------------------

/// Horizontal gate on the squared Mahalanobis distance: chi-square, 2 degrees of freedom, tail
/// 1e-4.
pub const GATE_H_CHI2: f64 = 18.42;
/// Vertical gate: chi-square, 1 degree of freedom, tail 1e-4.
pub const GATE_V_CHI2: f64 = 15.14;
/// Gate on the sum (horizontal plus vertical), chi-square, 3 degrees of freedom, tail 1e-4; the
/// assignment's unassigned penalty is half of it per side.
pub const GATE_TOTAL_CHI2: f64 = 21.11;
/// Coarse window before the exact gate, metres: a plot more than this plus eight track standard
/// deviations from a track's bucket-start position (slant range or cross-range) is not tested.
/// Covers the motion within a bucket at 360 m/s with room; it only saves work, the exact gate
/// decides.
pub const COARSE_M: f64 = 3_000.0;
/// A plot held rather than applied when another track is within this cost (chi-square units) of
/// its assigned track. About one standard deviation of the cost difference of two tracks 2
/// sigma apart; set from the swap counts of the crossing runs.
pub const AMBIGUITY_MARGIN: f64 = 4.0;
/// Limit: live tracks considered for one radar's association, 8,192.
pub const MAX_ASSOC_TRACKS: usize = 8_192;

// ---- Radar-only initiation ------------------------------------------------------------

/// Hits needed to confirm a radar-only track (the M of M-of-N, docs/M3_DESIGN.md section 3).
pub const RADAR_INIT_M: u8 = 3;
/// Scans the M must fall in, counting the first (the N).
pub const RADAR_INIT_N: u8 = 4;
/// Largest ground speed a candidate pair of plots may imply, m/s (700 kt, the tracker's
/// `MAX_PLAUSIBLE_KT`, in SI).
pub const CAND_MAX_SPEED_MPS: f64 = 360.0;
/// Slack added to every candidate gate, metres (plot noise at long range).
pub const CAND_SLACK_M: f64 = 400.0;
/// Largest acceleration a candidate's third plot is allowed to disagree with a straight line by,
/// m/s^2 (a 6 degree/s turn at 250 m/s is 26). Set from the turn rates of the clip.
pub const CAND_MAX_ACCEL_MPS2: f64 = 30.0;
/// Largest climb or descent rate a candidate's altitudes may imply, ft/s (6,000 ft/min).
pub const CAND_MAX_VRATE_FPS: f64 = 100.0;
/// Altitude slack of the candidate gate, feet (three Mode C quanta).
pub const CAND_ALT_SLACK_FT: f64 = 300.0;
/// Limit: candidates alive at once, 16,384.
pub const MAX_CANDIDATES: usize = 16_384;

// ---- Registration ---------------------------------------------------------------------

/// Prior standard deviation of a radar's range bias, metres. An assumption: no sourced bias
/// exists; wide enough to cover the swept range (up to 150 m) at two sigma.
pub const REG_PRIOR_SIGMA_RANGE_M: f64 = 250.0;
/// Prior standard deviation of a radar's azimuth bias, degrees (assumption, covers 0.15 degree
/// at two sigma with margin).
pub const REG_PRIOR_SIGMA_AZ_DEG: f64 = 0.25;
/// Random-walk density of the range bias, m^2 per second (0.03 m per root second). A bias that
/// is mechanical alignment drifts slowly; set from the stability of a real radar once data
/// exists; the simulator's bias is constant.
pub const REG_Q_RANGE_M2_PER_S: f64 = 1.0e-3;
/// Random-walk density of the azimuth bias, degrees^2 per second.
pub const REG_Q_AZ_DEG2_PER_S: f64 = 1.0e-9;
/// A residual more than this many standard deviations from the estimate is not used.
pub const REG_GATE_SIGMAS: f64 = 4.0;
/// Reference tracks must have at least this NACp on their latest ADS-B report ("high
/// integrity", docs/M3_DESIGN.md section 3: NACp 9 is a 95 % bound of 30 m).
pub const REG_MIN_NACP: u8 = 9;
/// Reference tracks must have a predicted position standard deviation (each axis) of at most
/// this, metres.
pub const REG_MAX_TRACK_SIGMA_M: f64 = 15.0;
/// The reference track's latest ADS-B update must be no older than this, ns.
pub const REG_MAX_ADSB_AGE_NS: i64 = 5 * NS_PER_S;
/// ADS-B updates a reference track must have had.
pub const REG_MIN_ADSB_UPDATES: u32 = 3;
/// Plots closer than this are not used for registration (the cone near the antenna), metres.
pub const REG_MIN_RANGE_M: f64 = 5_000.0;
/// The estimates are published at most this often, ns.
pub const REG_PUBLISH_NS: i64 = 10 * NS_PER_S;
/// Limit: published estimates per radar, 1 Mi (a day at one per 10 s is 8,640).
pub const REG_HISTORY_MAX: usize = 1 << 20;
/// Largest bias the estimator accepts, range metres and azimuth degrees; an estimate beyond is
/// counted and held at the limit (a radar that far off is faulty, not mis-registered).
pub const REG_LIMIT_RANGE_M: f64 = 2_000.0;
pub const REG_LIMIT_AZ_DEG: f64 = 2.0;

// ---- Capacity -------------------------------------------------------------------------

/// Limit: radars.
pub const MAX_RADARS: usize = 8;
/// Radar-only tracks are keyed above every 24-bit address (and the non-ICAO bit 24).
pub const KEY_RADAR_BASE: u32 = 0x4000_0000;
/// Limit: assignment log rows (scoring runs).
pub const ASSIGN_LOG_MAX: usize = 4 << 20;
