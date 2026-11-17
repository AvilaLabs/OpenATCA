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
/// The standard coarse search radius, metres: the grid is searched this far from a plot, which
/// covers every track whose own window ([`COARSE_M`] plus eight standard deviations) is no wider,
/// that is, a standard deviation up to 187 m. A track with a wider window is on the wide list.
pub const COARSE_STD_M: f64 = 4_500.0;
/// Limit: tracks on the wide list of one association, 4,096 (the tracks with a standard deviation
/// above 187 m: starved tracks and the coarse ones; the limit is far above what the stress
/// traffic makes). Past it a track is left out of that association and counted.
pub const WIDE_TRACKS_MAX: usize = 4_096;
/// A plot held rather than applied when another track is within this cost (chi-square units) of
/// its assigned track. About one standard deviation of the cost difference of two tracks 2
/// sigma apart; set from the swap counts of the crossing runs.
pub const AMBIGUITY_MARGIN: f64 = 4.0;
/// Limit: live tracks considered for one radar's association, 65,536 (the whole-NAS capacity
/// of 38,000 with room). [`crate::fusion::fusion_new`] refuses a tracker holding more, so no live
/// track is ever left out of an association.
pub const MAX_ASSOC_TRACKS: usize = 65_536;
/// Limit: slots of each per-key table of the fusion layer (hold runs per track and radar, and
/// what registration remembers of each address), 131,072. Each is bounded by an entry count of
/// at most [`MAX_ASSOC_TRACKS`] (the hold table evicts at that count, the address table is cut
/// back to the tracker's capacity, itself at most that); open addressing wants a load of one
/// half or less for short probes, so twice the bound, a power of two. A new key into a full table
/// is refused and counted; the table never grows.
pub const KEY_TABLE_SLOTS: usize = 2 * MAX_ASSOC_TRACKS;
const _: () = assert!(KEY_TABLE_SLOTS.is_power_of_two() && KEY_TABLE_SLOTS >= 2 * MAX_ASSOC_TRACKS);
/// Limit: records of radar-only tracks the fusion layer keeps for its caller, 65,536. Past it
/// new records are counted and dropped; a long-running caller drains them
/// ([`crate::fusion::fusion_take_radar_tracks`]).
pub const MAX_TRACK_RECS: usize = 65_536;

// ---- Radar-only initiation ------------------------------------------------------------

/// Hits needed to confirm a radar-only track (the M of M-of-N, docs/M3_DESIGN.md section 3).
pub const RADAR_INIT_M: u8 = 3;
/// Scans the M must fall in, counting the first (the N).
pub const RADAR_INIT_N: u8 = 4;
/// Largest ground speed a candidate pair of plots may imply, m/s (700 kt; radar plots carry no
/// category or ground speed for the speed rule of `oa_adsb::jump_verdict`).
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

/// Reference aircraft a radar's estimator holds by default (a slot each; a configuration value,
/// `FusionConfig::reg_ref_slots`). A scan of a busy radar has a few hundred eligible aircraft,
/// and a reference needs [`REG_REF_MIN_SAMPLES`] residuals in its own slot before it counts, so
/// the slots must outnumber the aircraft that are eligible together.
pub const REG_REF_SLOTS: usize = 256;
/// Limit: the slots a radar's estimator may be configured with (each residual is compared with
/// every slot).
pub const REG_REF_SLOTS_MAX: usize = 4096;
/// A reference's slot is given up when its aircraft has made no residual for this many scans of
/// the radar: long enough for an aircraft to miss a few scans (a dropped plot, a moment out of
/// the reference rules) and keep what it has averaged; short enough that a departed aircraft
/// frees its slot within half a minute at a terminal radar and a minute and a quarter en route.
pub const REG_REF_STALE_SCANS: u32 = 6;
/// The scan period assumed for an estimator made without one, ns (a terminal radar's 4.8 s,
/// rounded up).
pub const REG_DEFAULT_SCAN_NS: i64 = 5 * NS_PER_S;
/// A reference's mean residual averages at most this many residuals (older ones fade).
pub const REG_REF_MEMORY: u32 = 10;
/// Residuals a reference must have made before it counts or is counted on: five plots of 70 m
/// scatter average to 31 m, so a 150 m disagreement is five standard deviations of the mean.
pub const REG_REF_MIN_SAMPLES: u32 = 5;
/// Distinct current references needed before any residual is used: the median of the others
/// then has two or more members besides a lone liar, so one spoofed or faulty reference is
/// outvoted. Set from how many ADS-B aircraft a radar sees; raise it where there are many.
pub const REG_MIN_REFS: usize = 4;
/// A reference is current if it made a residual within this long, ns (two minutes: several scans
/// of an en route radar).
pub const REG_REF_WINDOW_NS: i64 = 120 * NS_PER_S;
/// A reference's mean must be within this many standard deviations of the difference to the
/// median of the others' means: chi-square-ish 3 sigma, so a lone honest reference is refused
/// about 0.3 % of the time and a 150 m liar among 70 m plots after five residuals is refused.
pub const REG_AGREE_SIGMAS: f64 = 3.0;
/// CUSUM allowance, in standard deviations: shifts under twice this are not chased.
pub const REG_CUSUM_K: f64 = 0.5;
/// CUSUM decision threshold, in standard deviations: with the allowance above, a false restart
/// of an honest radar is about once in a million residuals; a 2 sigma shift (150 m at 70 m of
/// plot scatter) is found in about eight.
pub const REG_CUSUM_H: f64 = 12.0;
/// Each standardised residual enters the CUSUM clipped to this many standard deviations, so a
/// few wild plots cannot count as a shift.
pub const REG_CUSUM_CLIP: f64 = 3.0;

// ---- Capacity -------------------------------------------------------------------------

/// Limit: radars.
pub const MAX_RADARS: usize = 8;
/// Radar-only tracks are keyed above every 24-bit address (and the non-ICAO bit 24).
pub const KEY_RADAR_BASE: u32 = crate::types::RADAR_KEY_BASE;
/// Limit: plots of one radar associated in one bucket; later ones are counted and dropped (a
/// flooded bucket must cost a bounded amount). Far above any real scan: a radar delivers a few
/// hundred plots a scan, a bucket is one second of it.
pub const MAX_BUCKET_PLOTS: usize = 16_384;
/// Limit: gated candidate pairs of one association; pairs beyond are counted and not formed.
pub const MAX_BUCKET_PAIRS: usize = 1 << 18;
/// Limit: work in the associations of one bucket, all radars together, 1 Mi: a (plot, track) pair
/// given the exact gate costs one unit (about two microseconds) and so does a candidate the coarse
/// window is applied to (much less), so this is about the longest a bucket can take.
pub const MAX_BUCKET_TESTS: usize = 1 << 20;
/// Initiation work units shared by every radar in one bucket. One million units matches the
/// existing association work ceiling, so their separate caps give two million charged units
/// combined. One unit is a candidate visit/exact pair; grid build/sort charges input visits,
/// loop iterations, comparisons and swaps; query charges each cell, probe, and key visit.
/// The sole refusal-accounting pass is separate and is bounded by
/// `MAX_RADARS * MAX_BUCKET_PLOTS` (131,072) plot-slot visits
/// per bucket; `FusionStats::init_refusal_pass_visits` reports that work.
pub const MAX_INIT_WORK: usize = 1 << 20;
/// Limit: tracks (and candidates) in one association solve.
pub const MAX_SOLVE_TRACKS: usize = if MAX_ASSOC_TRACKS > MAX_CANDIDATES {
    MAX_ASSOC_TRACKS
} else {
    MAX_CANDIDATES
};

// ---- Scratch capacities (RULESET C2, C3) ----------------------------------------------
//
// Every buffer the processing path fills is made once, at `fusion_new`, with one of the
// capacities below or above, and cleared and reused after that. The analysis is the one the
// limits above carry: the whole-NAS capacity is 38,000 tracks, a radar delivers a few hundred
// plots a scan (a bucket is one second of it), and each limit is far above that. A buffer that
// reaches its capacity refuses the item and counts it (`FusionStats::scratch_full`, or the
// counter of the limit it is the size of); it is never grown.

/// Capacity of the plots of one radar in one bucket, which is also each radar's decision,
/// anonymous-plot, converted-plot, hard-plot and candidate-plot lists: a bucket takes at most
/// [`MAX_BUCKET_PLOTS`] of a radar's plots, so none of them can need more.
pub const RADAR_PLOTS_MAX: usize = MAX_BUCKET_PLOTS;
/// Capacity of the inputs ready to process at one call, ADS-B and plots each: at most what can be
/// pending, [`MAX_PENDING`].
pub const READY_MAX: usize = MAX_PENDING;
/// Capacity of the time-ordered events of one bucket: every ADS-B measurement of it (at most
/// [`MAX_PENDING`]) and every applied plot (at most [`MAX_BUCKET_PLOTS`] of each of
/// [`MAX_RADARS`] radars).
pub const BUCKET_EVENTS_MAX: usize = MAX_PENDING + MAX_RADARS * MAX_BUCKET_PLOTS;
/// Capacity of the candidate indices of one radar's association: the candidates, at most
/// [`MAX_CANDIDATES`].
pub const CAND_IX_MAX: usize = MAX_CANDIDATES;
/// Bitset words for ordering the candidate pool indexes without comparison sorting.
pub const CAND_ORDER_WORDS: usize = MAX_CANDIDATES.div_ceil(64);

/// The standard reach of a candidate in initiation, metres: a candidate whose plots can lie no
/// further than this from its predicted place (the gate radius at the longest interval of the
/// bucket, the drift of the prediction over the bucket and the noise, with margin) is in the grid
/// of candidates; one that can lie further is on the wide list.
pub const INIT_STD_M: f64 = 6_000.0;
/// Side of a cell of the grid of candidates, metres: the reach of the grid's plots (the standard
/// reach and the noise of the plot) is within one cell either side in nearly every case.
pub const INIT_CELL_M: f64 = 12_000.0;
/// Limit: candidates on the wide list of one radar's initiation, 1,024 (a candidate lives at most
/// [`RADAR_INIT_N`] scans and the wide ones are the starved: one hit, a long interval). Past it a
/// candidate is not tested in that bucket and is counted.
pub const INIT_WIDE_MAX: usize = 1_024;

/// A plot with a Mode S address is applied to the track of that address only if its distance
/// from the track (horizontal plus vertical, squared Mahalanobis) is below this: chi-square, 3
/// degrees of freedom, tail about 8e-11. Far wider than the ordinary gate (an address is strong
/// evidence, and the filter's own jump gate still counts and tests it), so only a gross
/// mismatch (another aircraft on the address, a corrupt plot) is refused.
pub const HARD_GATE_CHI2: f64 = 50.0;
/// A track whose last this many plots were all held for ambiguity takes the next one on the
/// assignment's own evidence. Isolated holds (a plot without Mode C between two aircraft at one
/// place and two altitudes) never reach it, so identity is still not decided by kinematics where
/// the ambiguity is brief; two aircraft that stay together (a formation, a close parallel pair)
/// would otherwise never be updated, their tracks would coast off, and every plot would start
/// new tracks beside them. Set from the pair worlds: 2 keeps their swap counts at the held
/// design's (docs/BENCHMARKS.md, M3 step 2 report).
pub const HOLD_MAX_RUN: u8 = 2;
/// A Mode S plot outside the wide gate of its address's track marks the track as contradicted by
/// radar for this many scans of that radar after the plot, so the mark stays through a scan or two
/// that happen to miss the aircraft and clears after this many clean ones. Three scans: a
/// terminal radar's ten seconds, an en route radar's thirty-six.
pub const RADAR_DISAGREE_SCANS: i64 = 3;
/// Limit: assignment log rows (scoring runs).
pub const ASSIGN_LOG_MAX: usize = 4 << 20;

/// Vertical-gate allowance for a change of climb rate since the track's last update, one
/// standard deviation, feet per minute: the gate variance grows by (this x the time since the
/// last update, at most [`GATE_MANEUVER_MAX_DT_S`])^2. The cost does not (a pair inside the gate
/// only by this allowance costs [`RESCUED_COST_FRACTION`] of the total gate), so the allowance
/// widens what a track may take and never what it prefers. Set from the real-clip runs: the
/// climb rate of an aircraft changes by 500 to 2,100 ft/min between two plots in 20 of the 34
/// terminal and 15 of the 24 en-route second tracks started while the first was alive (see the
/// re-pin note of `PINNED_CLIP_DIGESTS`); a sweep of 400 to 1,000 found the wrong plots of the
/// five runs no higher than with the allowance off up to 700 and higher in the two-radar run
/// (65 to 82) and the terminal run (2 to 9) from 800. Applied alone, without the separate cost,
/// the allowance had raised the en-route position error from 260 m to 2,000 m (wrong
/// aircraft taken).
pub const GATE_VRATE_SIGMA_FPM: f64 = 700.0;

/// Horizontal-gate allowance for a manoeuvre since the track's last update, metres per second
/// squared: the gate covariance grows by (0.5 x this x the time since the last update, at most
/// [`GATE_MANEUVER_MAX_DT_S`])^2 in each axis (the cost does not, see [`RESCUED_COST_FRACTION`]).
/// OFF (0): the study measured it. At 8 m/s^2 the synthetic terminal world's duplicate tracks
/// (a track lost at a turn onset and started again) fall from 8 to 6 of 44 aircraft, but on the
/// real-clip shapes identity swaps rise from 66 to 101 in 41,543 terminal plots.
pub const GATE_MANEUVER_MPS2: f64 = 0.0;

/// The time since the last update beyond which the allowances stop growing, seconds. Without
/// the limit a track that has missed a turn is held with a velocity wrong by the whole speed
/// (the 12 s en-route scans with the allowance on).
pub const GATE_MANEUVER_MAX_DT_S: f64 = 6.0;

/// The cost, as a fraction of the total gate, of a pair that is inside the gate only by the
/// manoeuvre allowance (and of any pair costing more): just under the gate, so that it is
/// assigned only if nothing better wants either side.
pub const RESCUED_COST_FRACTION: f64 = 0.999;
