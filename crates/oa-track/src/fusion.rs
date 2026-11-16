//! Radar and ADS-B fusion: plots are associated to tracks, radar-only tracks are initiated, each
//! radar is registered against ADS-B, and everything goes into one [`Tracker`] at the
//! measurement level (docs/M3_DESIGN.md section 3). Core (pure): time is the measurements' own;
//! the same inputs give the same output.
//!
//! # Flow
//!
//! Inputs are buffered ([`fusion_adsb`], [`fusion_plots`]) and processed by one-second buckets
//! of time ([`fusion_advance`], [`fusion_finish`]); `fusion_params.rs` says why. For each bucket:
//!
//! 1. **Association, per radar, from the state at the start of the bucket.** A plot with a Mode S
//!    address goes to the track with that key (a hard constraint; the filter's own jump gate
//!    still counts a contradiction). Any other plot is converted (`radar.rs`) with the radar's
//!    current registration correction, gated against the live tracks by a coarse window (applied
//!    to the tracks in the cells of a latitude and longitude grid the plot can reach, `coarse.rs`,
//!    which finds exactly what testing every track finds; the radars' plots are gated in turn,
//!    the radar that has spent least work first, against one work limit per bucket) and then by
//!    the exact Mahalanobis distance (horizontal against the track's predicted position and
//!    covariance, vertical against its altitude), and the gated pairs are assigned by global
//!    nearest neighbour (`assoc.rs`). **A Mode A code never takes part**: it is copied to the
//!    measurement and read by nothing. A plot whose assigned track has a competitor within
//!    [`AMBIGUITY_MARGIN`] is **held**, not applied: kinematics alone never decides between two
//!    close tracks, and a track coasts through a crossing. A track whose last [`HOLD_MAX_RUN`]
//!    plots of one radar were all held takes the next one on the assignment's own evidence
//!    (counted in [`FusionStats::held_forced`]): two aircraft that stay together would otherwise
//!    never update their tracks, which would coast off while every plot started tracks beside
//!    them. A Mode S plot is applied to the track of its address only inside a wide gate
//!    ([`HARD_GATE_CHI2`]; a gross mismatch is counted, not applied, and flags the track's rows
//!    with [`crate::types::O_RADAR_DISAGREES`] for [`RADAR_DISAGREE_SCANS`] scans of that radar), and **a
//!    track takes at most one plot per radar and bucket**: of several Mode S plots of one address the nearest,
//!    and no anonymous plot at all once a Mode S plot has taken the track. Plots keep their
//!    scoring label through the run and nothing reads it.
//! 2. **Application, in time order.** The bucket's ADS-B measurements and assigned plots are
//!    applied to the tracker by time of applicability.
//! 3. **Registration.** A plot assigned to a reference track (latest ADS-B NACp at least 9,
//!    predicted position standard deviation small, ADS-B update recent) gives a range and azimuth
//!    residual (`registration.rs`) to that radar's estimator.
//! 4. **Initiation.** Plots no track took (and not held) join radar-only candidates by a
//!    second assignment; a candidate with [`RADAR_INIT_M`] hits in [`RADAR_INIT_N`] scans becomes
//!    a track. A plot with an address and no track starts or joins a candidate for that address
//!    only. The three plots of a new track are replayed into the tracker (its out-of-order window
//!    is sized for it by [`fusion_tracker_config`]); rows for the earlier two are discarded, so no
//!    output precedes the confirmation decision.
//!
//! Radar-only tracks are keyed above every address ([`KEY_RADAR_BASE`]). ADS-B tracks are keyed
//! by address and radar plots join them, which is the fusion.
//!
//! The queues, the scratch of a bucket, the candidates, the track records and the histories are
//! vectors made at start with the capacities of `fusion_params.rs` and reused (RULESET C2, C3).
//! The per-key bookkeeping is two fixed-capacity hash tables (`fixed_map.rs`, [`KEY_TABLE_SLOTS`]
//! slots each, made at start); the tracker allocates nothing after construction.

use oa_geo::aeqd::{frame_new, project, unproject, Frame};

use crate::assoc::{
    ambiguous_pairs, assigned_pairs, assoc_ambiguous, assoc_pair_of, assoc_plot_track,
    assoc_scratch_buffers, assoc_scratch_new, gnn, nearest, AssocScratch, GnnStats, Pair,
    NONE as NO_TRACK,
};
use crate::coarse::{
    coarse_buffer, coarse_build, coarse_distance, coarse_new, coarse_passes, coarse_query,
    point_grid_buffer, point_grid_build, point_grid_new, point_grid_query, CoarseIndex, PointGrid,
};
use crate::fixed_map::FixedMap;
use crate::fusion_params::{
    AMBIGUITY_MARGIN, ASSIGN_LOG_MAX, BUCKET_EVENTS_MAX, BUCKET_NS, CAND_ALT_SLACK_FT, CAND_IX_MAX,
    CAND_MAX_ACCEL_MPS2, CAND_MAX_SPEED_MPS, CAND_MAX_VRATE_FPS, CAND_SLACK_M, COARSE_M,
    COARSE_STD_M, GATE_H_CHI2, GATE_MANEUVER_MAX_DT_S, GATE_MANEUVER_MPS2, GATE_TOTAL_CHI2,
    GATE_VRATE_SIGMA_FPM, GATE_V_CHI2, HARD_GATE_CHI2, HOLD_MAX_RUN, INIT_CELL_M, INIT_STD_M,
    INIT_WIDE_MAX, KEY_RADAR_BASE, KEY_TABLE_SLOTS, MAX_ASSOC_TRACKS, MAX_BUCKET_PAIRS,
    MAX_BUCKET_PLOTS, MAX_BUCKET_TESTS, MAX_CANDIDATES, MAX_PENDING, MAX_RADARS, MAX_SOLVE_TRACKS,
    MAX_TRACK_RECS, RADAR_DISAGREE_SCANS, RADAR_INIT_M, RADAR_INIT_N, RADAR_PLOTS_MAX, READY_MAX,
    REG_HISTORY_MAX, REG_MAX_ADSB_AGE_NS, REG_MAX_TRACK_SIGMA_M, REG_MIN_ADSB_UPDATES,
    REG_MIN_NACP, REG_MIN_RANGE_M, REG_MIN_REFS, REG_PUBLISH_NS, REG_REF_SLOTS, REG_REF_SLOTS_MAX,
    RESCUED_COST_FRACTION, WIDE_TRACKS_MAX,
};
use crate::params::{ALT_BASE_SIGMA_FT, NS_PER_S, OOO_WINDOW_NS};
use crate::radar::{
    plot_check, plot_convert, polar_of, radar_geom, site_check, Bias, Converted, RadarError,
    RadarGeom, RadarPlot, RadarSite, FT_M, NONE_ALT, NO_BIAS,
};
use crate::registration::{
    reg_bias, reg_new_with, reg_offer, reg_predict, reg_residual, reg_sample, RegSample, RegState,
    Residual,
};
use crate::tracker::{
    tracker_advance, tracker_buffers, tracker_flush, tracker_live_iter, tracker_new,
    tracker_note_radar_disagreement, tracker_predict, tracker_set_expected_interval, tracker_stats,
    tracker_update, Tracker,
};
use crate::types::{
    output_clear, output_new, output_push, Measurement, OutRow, TrackError, TrackOutput,
    TrackerConfig, TrackerStats, NONE_I32, NO_ADDR, O_HAS_ALT,
};

/// Association rule of the run: exact global nearest neighbour, or the plain nearest neighbour
/// of the baseline.
pub const ASSOC_GNN: u8 = 0;
pub const ASSOC_NEAREST: u8 = 1;

/// How a plot reached a track, in the assignment log.
pub const HOW_HARD: u8 = 0;
pub const HOW_ASSOC: u8 = 1;
pub const HOW_INIT: u8 = 2;

/// Decision kinds of one plot.
const D_UNASSIGNED: u8 = 0;
/// Periods back a radar's plot counts toward a track's expected update interval.
const EXPECT_PERIODS: i64 = 3;
const D_APPLY: u8 = 1;
const D_HELD: u8 = 2;
const D_REFUSED: u8 = 3;
/// A Mode S plot not applied: outside the wide gate of its address's track, or a second plot of
/// that track in the bucket. Kept out of initiation like a refused plot.
const D_MISMATCH: u8 = 4;
/// An anonymous plot the bucket's work limit left ungated: it is neither a hit nor evidence that
/// no track holds it, so initiation leaves it alone.
const D_DEFERRED: u8 = 5;

/// Why the fusion layer refused its configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionError {
    /// A configuration value is out of range; the name says which.
    BadConfig(&'static str),
    /// A radar site is invalid.
    BadSite(usize, RadarError),
    /// The tracker refused its configuration.
    Tracker(TrackError),
}

impl std::fmt::Display for FusionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FusionError::BadConfig(w) => write!(f, "bad fusion configuration: {w}"),
            FusionError::BadSite(i, e) => write!(f, "radar {i}: {e}"),
            FusionError::Tracker(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FusionError {}

/// Settings of the fusion layer. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionConfig {
    pub tracker: TrackerConfig,
    /// Bucket length, ns.
    pub bucket_ns: i64,
    /// Estimate and apply radar registration.
    pub registration: bool,
    /// [`ASSOC_GNN`] or [`ASSOC_NEAREST`].
    pub assoc: u8,
    /// Hold a plot whose assigned track has a close competitor.
    pub hold_ambiguous: bool,
    /// A Mode S address on a plot is a hard constraint.
    pub use_address: bool,
    pub init_m: u8,
    pub init_n: u8,
    /// Reference aircraft slots of each radar's registration estimator (allocated at start).
    pub reg_ref_slots: usize,
    /// Record which plot went to which key (scoring runs).
    pub log_assign: bool,
}

/// The design's values on a tracker configuration.
pub fn fusion_config(tracker: TrackerConfig) -> FusionConfig {
    FusionConfig {
        tracker,
        bucket_ns: BUCKET_NS,
        registration: true,
        assoc: ASSOC_GNN,
        hold_ambiguous: true,
        use_address: true,
        init_m: RADAR_INIT_M,
        init_n: RADAR_INIT_N,
        reg_ref_slots: REG_REF_SLOTS,
        log_assign: false,
    }
}

/// Rows of the replay scratch table: a candidate's plots span at most `RADAR_INIT_N` scans of
/// at most 12 s, one tick row per second plus the update rows.
pub const SCRATCH_ROWS: usize = 4096;

/// Scans without any update after which a track is dropped, as a multiple of the longest scan
/// period.
pub const DROP_PERIODS: i64 = 6;

/// How far the drop time stays above the clock-step limit: validation needs the limit strictly
/// below it.
pub const DROP_MARGIN_NS: i64 = 1;

/// A tracker configuration for a fusion run: `base` with the expected and initiation intervals
/// set to the longest scan period of `sites`, the drop time [`DROP_PERIODS`] such periods and
/// the out-of-order window long enough to replay a candidate's plots (the initiation window of
/// `RADAR_INIT_N` scans plus two buckets). The drop time is at least [`DROP_MARGIN_NS`] above the
/// step limit.
pub fn fusion_tracker_config(base: &TrackerConfig, sites: &[RadarSite]) -> TrackerConfig {
    let period = sites.iter().map(|s| s.period_ns).max().unwrap_or(NS_PER_S);
    let ooo = base
        .ooo_window_ns
        .max(
            i64::from(RADAR_INIT_N)
                .saturating_mul(period)
                .saturating_add(2 * BUCKET_NS),
        )
        .max(OOO_WINDOW_NS);
    // The step limit is never below the window it must not be smaller than.
    let step = base.clock_step_limit_ns.max(ooo);
    TrackerConfig {
        expected_interval_ns: period,
        init_interval_ns: period,
        drop_after_ns: DROP_PERIODS
            .saturating_mul(period)
            .max(step.saturating_add(DROP_MARGIN_NS)),
        ooo_window_ns: ooo,
        clock_step_limit_ns: step,
        ..*base
    }
}

/// What the layer did. Plain record (A1); A7.2: cold struct.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FusionStats {
    pub adsb_in: u64,
    pub plots_in: u64,
    /// Plots refused by validation, by [`RadarError`]: range/azimuth/altitude, no height.
    pub plots_refused: u64,
    /// Plots dropped because their bucket was already processed.
    pub late: u64,
    /// Inputs dropped because the pending queue was full.
    pub pending_overflow: u64,
    pub applied_hard: u64,
    pub applied_assoc: u64,
    /// Plots held for ambiguity.
    pub held: u64,
    /// Plots no track took.
    pub unassigned: u64,
    /// Hard-constraint plots with no track of that address (went to a candidate).
    pub hard_no_track: u64,
    /// Hard-constraint plots outside [`HARD_GATE_CHI2`] of their address's track: counted, not
    /// applied.
    pub hard_mismatch: u64,
    /// Hard-constraint plots outside the wide gate that raised (or kept) the track's
    /// [`crate::types::O_RADAR_DISAGREES`] flag; a subset of `hard_mismatch` (a track that is not live has no
    /// flag to raise).
    pub radar_disagree: u64,
    /// Hard-constraint plots left out because the track took a better plot in the bucket (a
    /// track takes at most one plot per radar bucket).
    pub hard_extra: u64,
    /// Hold runs dropped to make room in a full table: the entry that was touched longest ago.
    pub hold_evicted: u64,
    /// Plots applied on the assignment's own evidence because the track's last
    /// [`HOLD_MAX_RUN`] plots were all held.
    pub held_forced: u64,
    /// Plots beyond [`MAX_BUCKET_PLOTS`] of one radar in one bucket: dropped.
    pub bucket_plot_overflow: u64,
    /// Associations cut short at [`MAX_BUCKET_TESTS`] units of work (coarse candidates and exact
    /// tests, shared by the radars of a bucket) or [`MAX_BUCKET_PAIRS`] pairs.
    pub bucket_work_capped: u64,
    /// Assignments the solver refused (an invalid gate or too large an input): every plot of
    /// that solve is then unassigned.
    pub assoc_refused: u64,
    /// Candidates the coarse window was applied to, taken from the grid cells a plot reaches
    /// (charged to the bucket's work limit like the exact tests).
    pub coarse_visits: u64,
    /// Pairs tested by the exact gate, and those inside it.
    pub pairs_tested: u64,
    pub pairs_gated: u64,
    /// Pairs skipped for want of a height (no Mode C and no track altitude).
    pub no_height_pairs: u64,
    pub cand_started: u64,
    pub cand_joined: u64,
    pub cand_confirmed: u64,
    pub cand_expired: u64,
    /// Unassigned plots that could not start a candidate (no altitude).
    pub cand_no_height: u64,
    pub cand_capacity: u64,
    /// A confirmed candidate whose track the tracker could not hold (capacity).
    pub track_refused: u64,
    pub assign_log_overflow: u64,
    /// Sums of the assignment solves (radar association and candidate association).
    pub gnn: GnnStats,
    pub reg_clamped: u64,
    /// Inputs processed (ADS-B measurements and plots, after the buckets completed).
    pub processed: u64,
    /// Records of radar-only tracks not kept because [`MAX_TRACK_RECS`] were held undrained.
    pub track_rec_overflow: u64,
    /// ADS-B keys not remembered for registration because the table was full of live tracks.
    pub keyinfo_overflow: u64,
    /// Items a scratch buffer refused at its capacity (none within the limits: they are sized
    /// from them, RULESET C2).
    pub scratch_full: u64,
    /// Registration estimates not published because the history was at [`REG_HISTORY_MAX`].
    pub reg_history_full: u64,
    /// Live tracks predicted for association, summed over the radar-buckets (the size of the
    /// per-bucket track pass).
    pub tracks_at_sum: u64,
    /// Anonymous plots of an association that were never gated because the bucket's work limit
    /// or the pair limit was reached first.
    pub anon_plots_ungated: u64,
    /// Initiations cut short at the work or pair limit (a subset of `bucket_work_capped`).
    pub init_capped: u64,
    /// Plot-candidate pairs tested by initiation.
    pub init_tests: u64,
    /// Most candidates alive at once.
    pub cands_live_max: u64,
    /// Radar-buckets that had plots to associate, and the sum and the largest of the coarse
    /// window width (metres) of those associations.
    pub assoc_runs: u64,
    pub coarse_width_sum_m: u64,
    pub coarse_width_max_m: u64,
    /// Buckets processed.
    pub buckets: u64,
    /// Anonymous candidates that reached their hits but whose latest plot gated with a live track
    /// (one another radar had confirmed, in this bucket or before): no second track was made.
    pub cand_merged_on_confirm: u64,
    /// Anonymous candidates of other radars cancelled because a track confirmed in their place.
    pub cand_cancelled_by_track: u64,
    /// Live tracks tested against a candidate's latest plot by those two checks.
    pub confirm_gate_tests: u64,
    /// Candidates that gated with a track but were kept because that track had a plot of the same
    /// radar in the same scan (two aircraft).
    pub cand_same_scan_kept: u64,
    /// Scan notes not stored because the table was full.
    pub plot_scan_full: u64,
    /// Tracks put on the wide list (a coarse window above [`COARSE_STD_M`]), summed over the
    /// radar-buckets, and those refused because the list was full (left out of that association).
    pub wide_tracks_sum: u64,
    pub wide_full: u64,
    /// Candidates the grid of initiation returned for the plots (the exact tests are `init_tests`).
    pub init_visits: u64,
    /// Candidates left untested for a bucket because the initiation's wide list was full.
    pub init_wide_full: u64,
}

/// Plot-to-key log for scoring: one row per plot applied or used to start a track. A7.2: cold
/// struct (collection of columns).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssignLog {
    pub t_ns: Vec<i64>,
    pub key: Vec<u32>,
    /// The plot's scoring label.
    pub label: Vec<u32>,
    pub radar: Vec<u8>,
    /// [`HOW_HARD`], [`HOW_ASSOC`] or [`HOW_INIT`].
    pub how: Vec<u8>,
}

/// An assignment log with room for [`ASSIGN_LOG_MAX`] rows (scoring runs).
fn assign_log_new() -> AssignLog {
    AssignLog {
        t_ns: Vec::with_capacity(ASSIGN_LOG_MAX),
        key: Vec::with_capacity(ASSIGN_LOG_MAX),
        label: Vec::with_capacity(ASSIGN_LOG_MAX),
        radar: Vec::with_capacity(ASSIGN_LOG_MAX),
        how: Vec::with_capacity(ASSIGN_LOG_MAX),
    }
}

/// A radar-only (or address-keyed) track made by initiation. Plain record (A1); A7.2: cold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarTrackRec {
    pub key: u32,
    pub radar: u8,
    /// Time of the first plot, Unix ns.
    pub first_ns: i64,
    /// Time of the plot that confirmed it (the decision), Unix ns.
    pub confirm_ns: i64,
}

/// What is known of a key from ADS-B. A7.2: cold, per-key.
#[derive(Debug, Clone, Copy)]
struct KeyInfo {
    last_adsb_ns: i64,
    adsb_n: u32,
    high_integrity: bool,
}

/// A track in formation. A7.2: cold struct (a handful alive).
#[derive(Debug, Clone, Copy)]
struct Cand {
    radar: u8,
    /// Mode S address, or [`NO_ADDR`] for an anonymous candidate.
    addr: u32,
    n: u8,
    first_scan: i64,
    last_scan: i64,
    meas: [Measurement; 4],
    label: [u32; 4],
}

/// A run of held plots of one radar on one track. A7.2: cold, per-key.
#[derive(Debug, Clone, Copy)]
struct HoldRun {
    run: u8,
    /// The bucket that last held a plot of it.
    bucket: i64,
}

/// A pending plot with its radar.
#[derive(Debug, Clone, Copy)]
struct PendPlot {
    radar: u8,
    plot: RadarPlot,
}

/// The decision about one plot of a bucket. A7.2: transient.
#[derive(Debug, Clone, Copy)]
struct Decision {
    kind: u8,
    key: u32,
    how: u8,
    m: Measurement,
    resid: Option<Residual>,
}

/// One candidate pair's by-products, kept so the winner needs no recomputation. A7.2: transient.
#[derive(Debug, Clone, Copy)]
struct PairInfo {
    m: Measurement,
    row: OutRow,
}

/// One radar's lists in the association of a bucket (see the capacities in `fusion_params.rs`):
/// made at start, taken out for the association and put back. A7.2: transient.
#[derive(Default)]
struct RadarScratch {
    /// The radar's plots of the bucket, in time order ([`RADAR_PLOTS_MAX`]).
    plots: Vec<RadarPlot>,
    dec: Vec<Decision>,
    anon: Vec<usize>,
    conv_own: Vec<Option<Converted>>,
    hard: Vec<(u32, f64, usize, Decision)>,
    taken: Vec<u32>,
    /// The live tracks at the start of the bucket, and the ones that took a hard plot: the
    /// tracker's capacity.
    tracks: Vec<TrackAt>,
    skip: Vec<bool>,
    /// Indices of the tracks of `tracks` whose coarse window is wider than the standard one
    /// ([`WIDE_TRACKS_MAX`]).
    wide: Vec<u32>,
}

fn radar_scratch_new(max_tracks: usize) -> RadarScratch {
    RadarScratch {
        plots: Vec::with_capacity(RADAR_PLOTS_MAX),
        dec: Vec::with_capacity(RADAR_PLOTS_MAX),
        anon: Vec::with_capacity(RADAR_PLOTS_MAX),
        conv_own: Vec::with_capacity(RADAR_PLOTS_MAX),
        hard: Vec::with_capacity(RADAR_PLOTS_MAX),
        taken: Vec::with_capacity(RADAR_PLOTS_MAX),
        tracks: Vec::with_capacity(max_tracks),
        skip: Vec::with_capacity(max_tracks),
        wide: Vec::with_capacity(WIDE_TRACKS_MAX.min(max_tracks)),
    }
}

/// Pushes within the capacity the vector was made with; `false`, with nothing pushed, at it.
fn push_within<T>(v: &mut Vec<T>, x: T) -> bool {
    if v.len() < v.capacity() {
        v.push(x);
        true
    } else {
        false
    }
}

/// The fusion layer. A subsystem (D5): fields private, interface the free functions below.
// A7.2: cold struct
pub struct Fusion {
    cfg: FusionConfig,
    tracker: Tracker,
    sites: Vec<RadarSite>,
    geoms: Vec<RadarGeom>,
    reg: Vec<RegState>,
    reg_hist: Vec<Vec<RegSample>>,
    reg_pub_ns: Vec<i64>,
    /// Inputs waiting for their bucket, made with [`MAX_PENDING`] each (a full queue counts
    /// `pending_overflow`).
    pend_adsb: Vec<Measurement>,
    pend_plots: Vec<PendPlot>,
    /// The inputs ready at one call, in arrival order with their place in the queue (so an
    /// unstable sort gives the order of a stable one), and then in time order: [`READY_MAX`].
    sort_a: Vec<(u32, Measurement)>,
    sort_p: Vec<(u32, PendPlot)>,
    ready_a: Vec<Measurement>,
    ready_p: Vec<PendPlot>,
    /// Per radar, the scratch of its association of a bucket.
    rscratch: Vec<RadarScratch>,
    /// The time-ordered events of a bucket: [`BUCKET_EVENTS_MAX`].
    evs: Vec<Ev>,
    /// Initiation's lists of plot indices ([`RADAR_PLOTS_MAX`]) and of candidate indices
    /// ([`CAND_IX_MAX`]).
    init_free: Vec<usize>,
    init_anon: Vec<usize>,
    cand_ix: Vec<usize>,
    /// Initiation's index of the candidates of one radar: where each is expected and how far a
    /// plot of it can lie from there, the grid of the ones of standard reach, the others, and
    /// the candidates of one plot ([`CAND_IX_MAX`] each, the wide list [`INIT_WIDE_MAX`]).
    init_reach: Vec<CandReach>,
    init_grid: PointGrid,
    init_wide: Vec<u32>,
    init_near: Vec<u32>,
    /// Per (key, radar): consecutive plots of that radar held for ambiguity since the track last
    /// took one of its plots (a small map: entries exist only while a run is going).
    hold_run: FixedMap<(u32, u8), HoldRun>,
    /// Entries of `hold_run` kept at most (a field so a test can lower it).
    hold_run_max: usize,
    /// Per (key, radar): the time of that radar's newest plot the track took. A radar gives one
    /// plot per aircraft per scan, so a candidate of that radar whose latest plot is in a scan the
    /// track already took a plot in is another aircraft, not the track's; and the radars whose
    /// periods set the track's expected interval are those with a plot this recent.
    plot_scan: FixedMap<(u32, u8), i64>,
    /// The earliest bucket of any pending input; `i64::MAX` with none (so a call that finds
    /// nothing ready is constant time, not a scan of a long queue).
    pend_min_bucket: i64,
    /// Every buffer of the assignment solves, allocated once.
    assoc: AssocScratch,
    /// The candidate pairs of one association and their by-products, kept between buckets so a
    /// bucket allocates only to grow past the largest before it (never past the pair limit).
    pairs_buf: Vec<Pair>,
    ms_buf: Vec<Measurement>,
    /// Per radar, the gated pairs of its association and their by-products (kept between buckets
    /// as above).
    pairs_bufs: Vec<Vec<Pair>>,
    infos_bufs: Vec<Vec<PairInfo>>,
    /// Per radar, the grid of the live tracks, and the candidates of one plot: all allocated at
    /// start for the tracker's capacity.
    grids: Vec<CoarseIndex>,
    cand_buf: Vec<u32>,
    /// Work (coarse candidates and exact gates) one bucket's associations may spend in all (a
    /// field so a test can lower it).
    bucket_tests_max: usize,
    /// Tracks on one association's wide list at most (a field so a test can lower it).
    wide_max: usize,
    /// Test only: test every track against the coarse window, as before the grid.
    #[cfg(test)]
    brute_coarse: bool,
    /// Per address: what registration needs of its ADS-B. Holds only keys the tracker holds (cut
    /// back to them whenever it reaches the tracker's capacity).
    keyinfo: FixedMap<u32, KeyInfo>,
    cands: Vec<Cand>,
    next_serial: u32,
    stats: FusionStats,
    log: AssignLog,
    tracks: Vec<RadarTrackRec>,
    /// Records kept at most (a field so a test can lower it).
    track_rec_max: usize,
    scratch: TrackOutput,
    /// Newest bucket processed; `i64::MIN` before the first.
    done_bucket: i64,
}

/// A fusion layer over `sites`. Refuses an invalid site or configuration (E5).
pub fn fusion_new(cfg: &FusionConfig, sites: &[RadarSite]) -> Result<Fusion, FusionError> {
    if sites.is_empty() || sites.len() > MAX_RADARS {
        return Err(FusionError::BadConfig("radar count"));
    }
    for (i, s) in sites.iter().enumerate() {
        site_check(s).map_err(|e| FusionError::BadSite(i, e))?;
    }
    // The tracker identifies a report by time and sensor id: two radars with one id would lose
    // the second report of a bucket as a duplicate.
    for (i, s) in sites.iter().enumerate() {
        if sites.iter().take(i).any(|o| o.sensor == s.sensor) {
            return Err(FusionError::BadConfig("radar sensor ids must be unique"));
        }
    }
    if cfg.bucket_ns <= 0 || cfg.bucket_ns > 10 * NS_PER_S {
        return Err(FusionError::BadConfig("bucket_ns"));
    }
    if cfg.init_m == 0 || cfg.init_n < cfg.init_m || cfg.init_n > 4 {
        return Err(FusionError::BadConfig("init_m or init_n"));
    }
    if cfg.reg_ref_slots < REG_MIN_REFS || cfg.reg_ref_slots > REG_REF_SLOTS_MAX {
        return Err(FusionError::BadConfig("reg_ref_slots"));
    }
    if cfg.assoc > ASSOC_NEAREST {
        return Err(FusionError::BadConfig("assoc"));
    }
    if cfg.tracker.max_tracks > MAX_ASSOC_TRACKS {
        return Err(FusionError::BadConfig("tracker max_tracks"));
    }
    let tracker = tracker_new(&cfg.tracker).map_err(FusionError::Tracker)?;
    Ok(Fusion {
        cfg: *cfg,
        tracker,
        sites: sites.to_vec(),
        geoms: sites.iter().map(radar_geom).collect(),
        reg: sites
            .iter()
            .map(|s| reg_new_with(cfg.reg_ref_slots, s.period_ns))
            .collect(),
        reg_hist: sites
            .iter()
            .map(|_| Vec::with_capacity(REG_HISTORY_MAX))
            .collect(),
        reg_pub_ns: sites.iter().map(|_| i64::MIN).collect(),
        pend_adsb: Vec::with_capacity(MAX_PENDING),
        pend_plots: Vec::with_capacity(MAX_PENDING),
        sort_a: Vec::with_capacity(READY_MAX),
        sort_p: Vec::with_capacity(READY_MAX),
        ready_a: Vec::with_capacity(READY_MAX),
        ready_p: Vec::with_capacity(READY_MAX),
        rscratch: sites
            .iter()
            .map(|_| radar_scratch_new(cfg.tracker.max_tracks))
            .collect(),
        evs: Vec::with_capacity(BUCKET_EVENTS_MAX),
        init_free: Vec::with_capacity(RADAR_PLOTS_MAX),
        init_anon: Vec::with_capacity(RADAR_PLOTS_MAX),
        cand_ix: Vec::with_capacity(CAND_IX_MAX),
        init_reach: Vec::with_capacity(CAND_IX_MAX),
        init_grid: point_grid_new(CAND_IX_MAX, INIT_CELL_M),
        init_wide: Vec::with_capacity(INIT_WIDE_MAX),
        init_near: Vec::with_capacity(CAND_IX_MAX),
        hold_run: FixedMap::new(KEY_TABLE_SLOTS),
        hold_run_max: MAX_ASSOC_TRACKS,
        plot_scan: FixedMap::new(KEY_TABLE_SLOTS),
        pend_min_bucket: i64::MAX,
        assoc: assoc_scratch_new(MAX_SOLVE_TRACKS, MAX_BUCKET_PLOTS, MAX_BUCKET_PAIRS),
        pairs_buf: Vec::with_capacity(MAX_BUCKET_PAIRS),
        ms_buf: Vec::with_capacity(MAX_BUCKET_PAIRS),
        pairs_bufs: sites
            .iter()
            .map(|_| Vec::with_capacity(MAX_BUCKET_PAIRS))
            .collect(),
        infos_bufs: sites
            .iter()
            .map(|_| Vec::with_capacity(MAX_BUCKET_PAIRS))
            .collect(),
        grids: sites
            .iter()
            .map(|_| coarse_new(cfg.tracker.max_tracks))
            .collect(),
        cand_buf: Vec::with_capacity(cfg.tracker.max_tracks),
        bucket_tests_max: MAX_BUCKET_TESTS,
        wide_max: WIDE_TRACKS_MAX,
        #[cfg(test)]
        brute_coarse: false,
        keyinfo: FixedMap::new(KEY_TABLE_SLOTS),
        cands: Vec::with_capacity(MAX_CANDIDATES),
        next_serial: 0,
        stats: FusionStats::default(),
        log: if cfg.log_assign {
            assign_log_new()
        } else {
            AssignLog::default()
        },
        tracks: Vec::with_capacity(MAX_TRACK_RECS),
        track_rec_max: MAX_TRACK_RECS,
        scratch: output_new(SCRATCH_ROWS),
        done_bucket: i64::MIN,
    })
}

/// The counters.
pub fn fusion_stats(f: &Fusion) -> &FusionStats {
    &f.stats
}

/// The tracker's counters.
pub fn fusion_tracker_stats(f: &Fusion) -> &TrackerStats {
    tracker_stats(&f.tracker)
}

/// The tracker (for its log and live tracks).
pub fn fusion_tracker(f: &Fusion) -> &Tracker {
    &f.tracker
}

/// Radar `radar`'s current estimate and its published history; `None` for a bad index.
pub fn fusion_registration(f: &Fusion, radar: usize) -> Option<(&RegState, &[RegSample])> {
    Some((f.reg.get(radar)?, f.reg_hist.get(radar)?.as_slice()))
}

/// The address and capacity of every buffer of the association (the solver's scratch), in a
/// fixed order. A test compares it before and after a run: equal means
/// nothing was reallocated there (RULESET C5, checked on the buffers because a counting
/// allocator needs `unsafe`, which A0 forbids here).
pub fn fusion_assoc_buffers(f: &Fusion) -> Vec<(usize, usize)> {
    let mut v = assoc_scratch_buffers(&f.assoc);
    // The grid of each radar and the candidates of a plot.
    v.extend(f.grids.iter().map(coarse_buffer));
    v.push((f.cand_buf.as_ptr() as usize, f.cand_buf.capacity()));
    v
}

/// The address and capacity of every buffer the processing path owns (the association's, the
/// tracker's, the queues, the scratch and the histories), in a fixed order. A test compares it
/// before and after a run: equal means no buffer was reallocated. It does not see a temporary
/// allocation that is freed within a call (RULESET C5 needs a counting allocator, which A0
/// forbids here).
#[doc(hidden)]
pub fn fusion_buffers(f: &Fusion) -> Vec<(usize, usize)> {
    fn of<T>(v: &Vec<T>) -> (usize, usize) {
        (v.as_ptr() as usize, v.capacity())
    }
    let mut v = fusion_assoc_buffers(f);
    v.extend(tracker_buffers(&f.tracker));
    v.extend([
        of(&f.pend_adsb),
        of(&f.pend_plots),
        of(&f.sort_a),
        of(&f.sort_p),
        of(&f.ready_a),
        of(&f.ready_p),
        of(&f.evs),
        of(&f.init_free),
        of(&f.init_anon),
        of(&f.cand_ix),
        of(&f.init_reach),
        of(&f.init_wide),
        of(&f.init_near),
        of(&f.pairs_buf),
        of(&f.ms_buf),
        of(&f.cands),
        of(&f.tracks),
        f.hold_run.buffer(),
        f.plot_scan.buffer(),
        point_grid_buffer(&f.init_grid),
        f.keyinfo.buffer(),
    ]);
    v.extend(f.pairs_bufs.iter().map(of));
    v.extend(f.infos_bufs.iter().map(of));
    v.extend(f.reg_hist.iter().map(of));
    for r in &f.rscratch {
        v.extend([
            of(&r.plots),
            of(&r.dec),
            of(&r.anon),
            of(&r.conv_own),
            of(&r.hard),
            of(&r.taken),
            of(&r.tracks),
            of(&r.skip),
            of(&r.wide),
        ]);
    }
    v.extend([
        of(&f.log.t_ns),
        of(&f.log.key),
        of(&f.log.label),
        of(&f.log.radar),
        of(&f.log.how),
    ]);
    v
}

/// The plot-to-key log (empty unless `log_assign`).
pub fn fusion_assign_log(f: &Fusion) -> &AssignLog {
    &f.log
}

/// Every track made by initiation, in creation order.
pub fn fusion_radar_tracks(f: &Fusion) -> &[RadarTrackRec] {
    &f.tracks
}

/// Hands over the records of radar-only tracks made so far, emptying the layer's list (a
/// long-running caller does this regularly; the list is held to [`MAX_TRACK_RECS`]). The copy
/// handed over is the caller's; the layer keeps its own buffer.
pub fn fusion_take_radar_tracks(f: &mut Fusion) -> Vec<RadarTrackRec> {
    let v = f.tracks.clone();
    f.tracks.clear();
    v
}

/// `(keys remembered for registration, radar-only track records held)`, for a test or a report.
pub fn fusion_bookkeeping_len(f: &Fusion) -> (usize, usize) {
    (f.keyinfo.len(), f.tracks.len())
}

/// Offers an ADS-B (or any addressed) measurement. It is applied when its bucket completes.
pub fn fusion_adsb(f: &mut Fusion, m: &Measurement) {
    f.stats.adsb_in += 1;
    if f.pend_adsb.len() >= MAX_PENDING {
        f.stats.pending_overflow += 1;
        return;
    }
    f.pend_adsb.push(*m);
    f.pend_min_bucket = f.pend_min_bucket.min(bucket_of(f, m.toa_ns));
}

/// Offers one radar's plots. Plots refused by validation are counted and dropped.
pub fn fusion_plots(f: &mut Fusion, radar: usize, plots: &[RadarPlot]) {
    let Ok(r) = u8::try_from(radar) else { return };
    if radar >= f.sites.len() {
        return;
    }
    for p in plots {
        f.stats.plots_in += 1;
        if plot_check(p).is_err() || scan_of(f, radar, p.time_ns).is_none() {
            f.stats.plots_refused += 1;
            continue;
        }
        if f.pend_plots.len() >= MAX_PENDING {
            f.stats.pending_overflow += 1;
            continue;
        }
        f.pend_plots.push(PendPlot { radar: r, plot: *p });
        f.pend_min_bucket = f.pend_min_bucket.min(bucket_of(f, p.time_ns));
    }
}

/// Processes every bucket that ended before `until_ns`, then runs the tracker to the end of
/// the last of them (its fixed-rate rows, coasting and expiry), whether or not anything was
/// pending: with every sensor quiet, time still passes for the tracks.
pub fn fusion_advance(f: &mut Fusion, until_ns: i64, out: &mut TrackOutput) {
    let complete = until_ns.div_euclid(f.cfg.bucket_ns);
    process_pending(f, complete, out);
    // Every bucket before `complete` is closed, whether or not it held an event: a plot timed
    // inside one is late (OA-TRK-088).
    f.done_bucket = f.done_bucket.max(complete.saturating_sub(1));
    // Up to the last instant before the boundary, as a measurement at the boundary would: the
    // tracker emits a tick at a measurement's own time only after applying it.
    let boundary = complete.saturating_mul(f.cfg.bucket_ns);
    tracker_advance(&mut f.tracker, boundary.saturating_sub(1), out);
}

/// Processes everything pending, then runs the tracker to `now_ns` and flushes it.
pub fn fusion_finish(f: &mut Fusion, now_ns: i64, out: &mut TrackOutput) {
    process_pending(f, i64::MAX, out);
    tracker_advance(&mut f.tracker, now_ns, out);
    tracker_flush(&mut f.tracker, now_ns, out);
}

fn bucket_of(f: &Fusion, t_ns: i64) -> i64 {
    t_ns.div_euclid(f.cfg.bucket_ns)
}

fn process_pending(f: &mut Fusion, complete: i64, out: &mut TrackOutput) {
    if f.pend_min_bucket >= complete {
        // Nothing pending is ready.
        return;
    }
    // Ready: buckets strictly before `complete`. The queues keep their order; the ready ones go,
    // with their place in the queue, to buffers made at start.
    let b = f.cfg.bucket_ns;
    let mut sort_a = std::mem::take(&mut f.sort_a);
    let mut sort_p = std::mem::take(&mut f.sort_p);
    let mut ready_a = std::mem::take(&mut f.ready_a);
    let mut ready_p = std::mem::take(&mut f.ready_p);
    sort_a.clear();
    sort_p.clear();
    ready_a.clear();
    ready_p.clear();
    let (mut seq, mut refused) = (0u32, 0u64);
    f.pend_adsb.retain(|m| {
        seq += 1;
        if m.toa_ns.div_euclid(b) < complete && push_within(&mut sort_a, (seq, *m)) {
            return false;
        }
        refused += u64::from(m.toa_ns.div_euclid(b) < complete);
        true
    });
    seq = 0;
    f.pend_plots.retain(|p| {
        seq += 1;
        if p.plot.time_ns.div_euclid(b) < complete && push_within(&mut sort_p, (seq, *p)) {
            return false;
        }
        refused += u64::from(p.plot.time_ns.div_euclid(b) < complete);
        true
    });
    f.stats.scratch_full += refused;
    f.pend_min_bucket = f
        .pend_adsb
        .iter()
        .map(|m| bucket_of(f, m.toa_ns))
        .chain(f.pend_plots.iter().map(|p| bucket_of(f, p.plot.time_ns)))
        .min()
        .unwrap_or(i64::MAX);
    // Time order, ties in arrival order (the place in the queue is the last key).
    sort_a.sort_unstable_by_key(|(q, m)| (m.toa_ns, *q));
    sort_p.sort_unstable_by_key(|(q, p)| (p.plot.time_ns, p.radar, *q));
    ready_a.extend(sort_a.iter().map(|(_, m)| *m));
    ready_p.extend(sort_p.iter().map(|(_, p)| *p));
    let (mut ai, mut pi) = (0usize, 0usize);
    while ai < ready_a.len() || pi < ready_p.len() {
        let ba = ready_a.get(ai).map(|m| bucket_of(f, m.toa_ns));
        let bp = ready_p.get(pi).map(|p| bucket_of(f, p.plot.time_ns));
        let bucket = match (ba, bp) {
            (Some(x), Some(y)) => x.min(y),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => break,
        };
        let a_end = ai
            + ready_a
                .get(ai..)
                .unwrap_or(&[])
                .iter()
                .take_while(|m| bucket_of(f, m.toa_ns) == bucket)
                .count();
        let p_end = pi
            + ready_p
                .get(pi..)
                .unwrap_or(&[])
                .iter()
                .take_while(|p| bucket_of(f, p.plot.time_ns) == bucket)
                .count();
        let adsb = ready_a.get(ai..a_end).unwrap_or(&[]);
        let plots = ready_p.get(pi..p_end).unwrap_or(&[]);
        ai = a_end;
        pi = p_end;
        if bucket <= f.done_bucket {
            f.stats.late += (adsb.len() + plots.len()) as u64;
            continue;
        }
        f.done_bucket = bucket;
        process_bucket(f, adsb, plots, out);
    }
    f.sort_a = sort_a;
    f.sort_p = sort_p;
    f.ready_a = ready_a;
    f.ready_p = ready_p;
}

// ---- one bucket -----------------------------------------------------------------------

fn track_height(row: &OutRow) -> Option<(f64, f64)> {
    (row.flags & O_HAS_ALT != 0).then(|| {
        (
            f64::from(row.alt_ft) * FT_M,
            f64::from(row.alt_var_ft2).max(0.0).sqrt() * FT_M,
        )
    })
}

fn cov_f64(c: [f32; 3]) -> [f64; 3] {
    [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
}

/// Squared Mahalanobis distance of a converted position from a track row, horizontally.
fn d2_horizontal(row: &OutRow, z_lat: f64, z_lon: f64, z_cov: [f64; 3]) -> f64 {
    let frame = frame_new(f64::from(row.lat_e7) / 1.0e7, f64::from(row.lon_e7) / 1.0e7);
    let (x, y) = project(&frame, z_lat, z_lon);
    let p = cov_f64(row.pos_cov_m2);
    let s = [p[0] + z_cov[0], p[1] + z_cov[1], p[2] + z_cov[2]];
    let det = s[0] * s[2] - s[1] * s[1];
    if !(det.is_finite() && det > 0.0) {
        return f64::INFINITY;
    }
    (s[2] * x * x - 2.0 * s[1] * x * y + s[0] * y * y) / det
}

/// Squared normalised altitude difference, where the plot and the row both have an altitude.
fn d2_vertical(row: &OutRow, plot: &RadarPlot, quant_ft: u16, since_update_ns: i64) -> f64 {
    let alt = (plot.alt_ft != NONE_ALT).then(|| f64::from(plot.alt_ft));
    d2_vertical_alt(row, alt, quant_ft, since_update_ns)
}

/// The same for an altitude in feet, if there is one.
fn d2_vertical_alt(row: &OutRow, alt_ft: Option<f64>, quant_ft: u16, since_update_ns: i64) -> f64 {
    let Some(alt_ft) = alt_ft else { return 0.0 };
    if row.flags & O_HAS_ALT == 0 {
        return 0.0;
    }
    let q = f64::from(quant_ft);
    let dt_min = since_update_ns.clamp(0, (GATE_MANEUVER_MAX_DT_S * NS_PER_S as f64) as i64) as f64
        / (60.0 * NS_PER_S as f64);
    let onset = GATE_VRATE_SIGMA_FPM * dt_min;
    let var = f64::from(row.alt_var_ft2).max(0.0)
        + q * q / 12.0
        + ALT_BASE_SIGMA_FT * ALT_BASE_SIGMA_FT
        + onset * onset;
    let dz = alt_ft - f64::from(row.alt_ft);
    dz * dz / var
}

/// Whether the latest plot `m` of a candidate gates with a live track's row (predicted at or
/// after `m`, its newest update at `last_ns`): the association gate (horizontal, vertical and
/// total), with the plot's own covariance and the manoeuvre allowance of the time since the
/// update, widened by the distance the track moves in the time between the row and the plot.
fn cand_gates_row(row: &OutRow, last_ns: i64, m: &Measurement, quant_ft: u16) -> bool {
    let (lat, lon) = (f64::from(m.lat_e7) / 1.0e7, f64::from(m.lon_e7) / 1.0e7);
    let secs = |ns: i64| (ns as f64) / NS_PER_S as f64;
    let man_dt = secs(m.toa_ns.saturating_sub(last_ns).max(0)).min(GATE_MANEUVER_MAX_DT_S);
    let skew_s =
        secs(m.toa_ns.saturating_sub(row.t_ns).saturating_abs()).min(GATE_MANEUVER_MAX_DT_S);
    let speed = oa_math::hypot(f64::from(row.vel_east_mps), f64::from(row.vel_north_mps));
    let man = 0.5 * GATE_MANEUVER_MPS2 * man_dt * man_dt + speed * skew_s;
    let zc = cov_f64(m.pos_cov_m2);
    let dh = d2_horizontal(row, lat, lon, [zc[0] + man * man, zc[1], zc[2] + man * man]);
    let alt = (m.alt_ft != NONE_I32).then(|| f64::from(m.alt_ft));
    let since = m.toa_ns.saturating_sub(last_ns);
    let dv = d2_vertical_alt(row, alt, quant_ft, since);
    dh <= GATE_H_CHI2 && dv <= GATE_V_CHI2 && dh + dv <= GATE_TOTAL_CHI2
}

/// One live track at the start of the bucket, with its slant range and azimuth from the radar.
#[derive(Debug, Clone, Copy)]
struct TrackAt {
    key: u32,
    lat: f64,
    lon: f64,
    /// Height above the radar, metres.
    dh: f64,
    rho: f64,
    az: f64,
    ground: f64,
    sigma: f64,
}

/// The live tracks at the start of a bucket, in `v` (cleared first); the number it had no room
/// for (none while the tracker's capacity is the buffer's).
fn tracks_at(f: &Fusion, radar: usize, at_ns: i64, v: &mut Vec<TrackAt>) -> u64 {
    v.clear();
    let Some(g) = f.geoms.get(radar) else {
        return 0;
    };
    let mut refused = 0;
    for (key, _) in tracker_live_iter(&f.tracker).take(MAX_ASSOC_TRACKS) {
        let Some((row, true, _)) = tracker_predict(&f.tracker, key, at_ns) else {
            continue;
        };
        let (lat, lon) = (f64::from(row.lat_e7) / 1.0e7, f64::from(row.lon_e7) / 1.0e7);
        let h = if row.flags & O_HAS_ALT != 0 {
            f64::from(row.alt_ft) * FT_M
        } else {
            g.height_m
        };
        let (rho, az) = polar_of(g, lat, lon, h);
        let dh = h - g.height_m;
        let ground = (rho * rho - dh * dh).max(1.0).sqrt();
        let c = cov_f64(row.pos_cov_m2);
        let at = TrackAt {
            key,
            lat,
            lon,
            dh,
            rho,
            az,
            ground,
            sigma: c[0].max(c[2]).max(0.0).sqrt(),
        };
        if !push_within(v, at) {
            refused += 1;
        }
    }
    refused
}

/// Whether a reference track may register a radar (see the module documentation).
fn reg_eligible(f: &Fusion, key: u32, row: &OutRow, plot: &RadarPlot) -> bool {
    let Some(k) = f.keyinfo.get(&key) else {
        return false;
    };
    let c = cov_f64(row.pos_cov_m2);
    k.high_integrity
        && k.adsb_n >= REG_MIN_ADSB_UPDATES
        && plot.time_ns.saturating_sub(k.last_adsb_ns) <= REG_MAX_ADSB_AGE_NS
        && row.flags & O_HAS_ALT != 0
        && c[0].max(c[2]).max(0.0).sqrt() <= REG_MAX_TRACK_SIGMA_M
        && plot.range_m >= REG_MIN_RANGE_M
}

fn residual_for(f: &Fusion, radar: usize, row: &OutRow, plot: &RadarPlot) -> Option<Residual> {
    let (g, s) = (f.geoms.get(radar)?, f.sites.get(radar)?);
    let h = f64::from(row.alt_ft) * FT_M;
    let var_h = f64::from(row.alt_var_ft2).max(0.0) * FT_M * FT_M;
    Some(reg_residual(
        g,
        s,
        plot,
        f64::from(row.lat_e7) / 1.0e7,
        f64::from(row.lon_e7) / 1.0e7,
        h,
        cov_f64(row.pos_cov_m2),
        var_h,
    ))
}

/// The distances of one converted plot from one track row. A7.2: transient.
#[derive(Debug, Clone, Copy)]
struct PairCost {
    /// Horizontal squared Mahalanobis distance.
    dh: f64,
    /// The same with the manoeuvre allowance, which the gate (not the cost) uses.
    dh_gate: f64,
    /// Vertical squared normalised distance.
    dv: f64,
}

fn pair_cost(
    row: &OutRow,
    conv: &Converted,
    plot: &RadarPlot,
    quant_ft: u16,
    last_ns: i64,
) -> PairCost {
    let dt_s = (plot.time_ns.saturating_sub(last_ns).max(0) as f64) / NS_PER_S as f64;
    let man_dt = dt_s.min(GATE_MANEUVER_MAX_DT_S);
    let man = 0.5 * GATE_MANEUVER_MPS2 * man_dt * man_dt;
    let (plat, plon) = (
        f64::from(conv.m.lat_e7) / 1.0e7,
        f64::from(conv.m.lon_e7) / 1.0e7,
    );
    let zc = cov_f64(conv.m.pos_cov_m2);
    let dh = d2_horizontal(row, plat, plon, zc);
    // The gate allows for a manoeuvre the filter has not caught up with; the cost does not, so a
    // pair that passes only by that allowance costs just under the gate and is assigned only
    // where no better pair competes for either side.
    let dh_gate = d2_horizontal(
        row,
        plat,
        plon,
        [zc[0] + man * man, zc[1], zc[2] + man * man],
    );
    let dv = d2_vertical(row, plot, quant_ft, plot.time_ns.saturating_sub(last_ns));
    PairCost { dh, dh_gate, dv }
}

/// One radar's association in progress: what the first part (hard-constraint plots, the tracks,
/// the conversions) made and the second (the coarse and exact gates) and third (the assignment)
/// need. A7.2: transient.
struct RadarAssoc {
    radar: usize,
    r8: u8,
    g: RadarGeom,
    s: RadarSite,
    bias: Bias,
    plots: Vec<RadarPlot>,
    dec: Vec<Decision>,
    tracks: Vec<TrackAt>,
    /// The tracks that took a hard plot, which take no anonymous one.
    skip: Vec<bool>,
    /// Indices of the anonymous plots, to map back from the solver's compact plot numbers.
    anon: Vec<usize>,
    conv_own: Vec<Option<Converted>>,
    grid: CoarseIndex,
    /// The tracks with a coarse window above the standard one, ascending, and the tallest of
    /// the others above or below the radar.
    wide: Vec<u32>,
    dh_max: f64,
    pairs: Vec<Pair>,
    infos: Vec<PairInfo>,
    /// Where the loop over the anonymous plots starts (see [`loop_start`]), the number gated so
    /// far, counted from there, and how many pairs the plots before the wrap had made.
    start: usize,
    next: usize,
    wrap_pairs: Option<usize>,
    /// Work spent (coarse candidates visited and exact gates tested) by this radar's plots.
    work: usize,
    capped: bool,
}

/// The plot the association loop of `radar` starts at in `bucket`, of `n` anonymous plots: a hash
/// of the bucket and the radar, so the plots a cut bucket leaves ungated are not always the same
/// stretch of the scan. Deterministic (the same inputs give the same start).
fn loop_start(bucket: i64, radar: usize, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut z = (bucket as u64) ^ ((radar as u64) << 56) ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z % n as u64) as usize
}

/// The first part of one radar's association of a bucket (see the module documentation, step 1):
/// the hard-constraint plots are decided and the tracks, the conversions and the grid made.
fn assoc_prepare(f: &mut Fusion, radar: usize) -> Option<RadarAssoc> {
    let (Some(&g), Some(&s)) = (f.geoms.get(radar), f.sites.get(radar)) else {
        return None;
    };
    // Everything taken below exists for a radar of the layer; checked before any is taken.
    if f.grids.get(radar).is_none()
        || f.pairs_bufs.get(radar).is_none()
        || f.infos_bufs.get(radar).is_none()
        || f.rscratch.get(radar).is_none()
    {
        return None;
    }
    let RadarScratch {
        plots,
        mut dec,
        mut anon,
        mut conv_own,
        mut hard,
        mut taken,
        mut tracks,
        mut skip,
        mut wide,
    } = std::mem::take(f.rscratch.get_mut(radar)?);
    // Radars are numbered below MAX_RADARS.
    let r8 = u8::try_from(radar).unwrap_or(u8::MAX);
    let blank = Measurement {
        toa_ns: 0,
        pos_cov_m2: [1.0, 0.0, 1.0],
        vel_cov_m2s2: [0.0; 3],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: 0,
        lon_e7: 0,
        alt_ft: NONE_I32,
        vrate_fpm: NONE_I32,
        addr: NO_ADDR,
        alt_quant_ft: 0,
        sensor: s.sensor,
        ident: crate::types::NO_IDENT,
        flags: 0,
        category: 0,
    };
    let none = Decision {
        kind: D_UNASSIGNED,
        key: NO_ADDR,
        how: HOW_ASSOC,
        m: blank,
        resid: None,
    };
    // At most `plots.len()` (below `RADAR_PLOTS_MAX`) of each list below.
    dec.clear();
    dec.resize(plots.len(), none);
    let t0 = plots.first().map_or(0, |p| p.time_ns);
    if let Some(r) = f.reg.get_mut(radar) {
        reg_predict(r, t0);
    }
    let bias = if f.cfg.registration {
        f.reg.get(radar).map_or(NO_BIAS, reg_bias)
    } else {
        NO_BIAS
    };
    f.stats.scratch_full += tracks_at(f, radar, t0, &mut tracks);
    f.stats.tracks_at_sum += tracks.len() as u64;
    anon.clear();
    hard.clear();
    taken.clear();
    // Hard-constraint plots: the track with the plot's key, or none. A plot must lie inside the
    // wide gate of that track, and a track takes one plot per bucket: the nearest.
    for (j, p) in plots.iter().enumerate() {
        if !(f.cfg.use_address && p.addr != NO_ADDR) {
            anon.push(j);
            continue;
        }
        let Some((row, true, last_ns)) = tracker_predict(&f.tracker, p.addr, p.time_ns) else {
            f.stats.hard_no_track += 1;
            continue;
        };
        let Ok(c) = plot_convert(&g, &s, p, &bias, track_height(&row), p.addr) else {
            f.stats.plots_refused += 1;
            if let Some(d) = dec.get_mut(j) {
                d.kind = D_REFUSED;
            }
            continue;
        };
        let pc = pair_cost(&row, &c, p, s.alt_quantum_ft, last_ns);
        let cost = pc.dh_gate + pc.dv;
        if cost.is_nan() || cost > HARD_GATE_CHI2 {
            f.stats.hard_mismatch += 1;
            // A radar contradicts the track: flag its rows for a few scans (no position moves).
            let until = p
                .time_ns
                .saturating_add(RADAR_DISAGREE_SCANS.saturating_mul(s.period_ns));
            if tracker_note_radar_disagreement(&mut f.tracker, p.addr, until) {
                f.stats.radar_disagree += 1;
            }
            if let Some(d) = dec.get_mut(j) {
                d.kind = D_MISMATCH;
            }
            continue;
        }
        let resid = (f.cfg.registration && reg_eligible(f, p.addr, &row, p))
            .then(|| residual_for(f, radar, &row, p))
            .flatten();
        hard.push((
            p.addr,
            cost,
            j,
            Decision {
                kind: D_APPLY,
                key: p.addr,
                how: HOW_HARD,
                m: c.m,
                resid,
            },
        ));
    }
    hard.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    // Keys of the tracks that took a hard plot, ascending (the list is sorted by key).
    for (key, _, j, d) in &hard {
        if taken.last() == Some(key) {
            f.stats.hard_extra += 1;
            if let Some(slot) = dec.get_mut(*j) {
                slot.kind = D_MISMATCH;
            }
            continue;
        }
        taken.push(*key);
        f.hold_run.remove(&(*key, r8));
        if let Some(slot) = dec.get_mut(*j) {
            *slot = *d;
        }
        f.stats.applied_hard += 1;
    }
    // A track that took a hard plot takes no anonymous one in the same bucket.
    skip.clear();
    for t in &tracks {
        // `tracks` and `skip` are made with the same capacity.
        push_within(&mut skip, taken.binary_search(&t.key).is_ok());
    }
    // Anonymous plots: converted once if they carry Mode C.
    conv_own.clear();
    for &j in &anon {
        conv_own.push(
            plots
                .get(j)
                .filter(|p| p.alt_ft != NONE_ALT)
                .and_then(|p| plot_convert(&g, &s, p, &bias, None, NO_ADDR).ok()),
        );
    }
    // Each track has its own coarse window, [`COARSE_M`] plus eight standard deviations. The tracks
    // whose window is above the standard search radius ([`COARSE_STD_M`]) are on the wide list,
    // which every plot tests; the rest are in the grid, searched at the standard radius.
    //
    // Superset: the old search was the grid at the widest window of any live track, `W`, which
    // returns every track that passes its own window at a plot (own window <= W, and the grid's
    // proof covers any window up to the width it is searched at). Now a track with its own window
    // <= COARSE_STD_M is in the grid, and the search width COARSE_STD_M is at least that window,
    // so the same proof returns it whenever it passes its window; a track with a wider window (or
    // a window that is not a number, which passes whatever its place) is on the wide list, tested
    // for every plot. The union of the two lists, in track order, holds every track that passes
    // its own window, which is every track the old search could test. A track refused for want of
    // room on the wide list is left out of the association (`wide_full`).
    wide.clear();
    for (i, t) in tracks.iter().enumerate() {
        if skip.get(i).copied().unwrap_or(true) {
            continue;
        }
        if COARSE_M + 8.0 * t.sigma <= COARSE_STD_M {
            continue;
        }
        if wide.len() < f.wide_max && push_within(&mut wide, i as u32) {
            f.stats.wide_tracks_sum += 1;
        } else {
            f.stats.wide_full += 1;
            if let Some(k) = skip.get_mut(i) {
                *k = true;
            }
        }
    }
    let on_wide = |i: usize| wide.binary_search(&(i as u32)).is_ok();
    let mut grid = std::mem::take(f.grids.get_mut(radar)?);
    coarse_build(
        &mut grid,
        (s.lat_deg, s.lon_deg),
        tracks.len(),
        |i| {
            tracks
                .get(i)
                .filter(|t| t.rho.is_finite() && t.az.is_finite() && t.ground.is_finite())
                .map(|t| (t.lat, t.lon))
        },
        |i| skip.get(i).copied().unwrap_or(false) || on_wide(i),
    );
    let live = |i: usize| !skip.get(i).copied().unwrap_or(false);
    // The tallest of the tracks in the grid above or below the radar.
    let dh_max = tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| live(*i) && !on_wide(*i))
        .map(|(_, t)| t.dh.abs())
        .fold(0.0, f64::max);
    // The widest window of any live track (recorded, not searched at).
    let width = tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| live(*i))
        .map(|(_, t)| COARSE_M + 8.0 * t.sigma)
        .fold(0.0, f64::max);
    f.stats.assoc_runs += 1;
    f.stats.coarse_width_sum_m += width as u64;
    f.stats.coarse_width_max_m = f.stats.coarse_width_max_m.max(width as u64);
    let mut pairs = std::mem::take(f.pairs_bufs.get_mut(radar)?);
    let mut infos = std::mem::take(f.infos_bufs.get_mut(radar)?);
    pairs.clear();
    infos.clear();
    let anon_len = anon.len();
    if let Some(slot) = f.rscratch.get_mut(radar) {
        slot.hard = hard;
        slot.taken = taken;
    }
    Some(RadarAssoc {
        radar,
        r8,
        g,
        s,
        bias,
        plots,
        dec,
        tracks,
        skip,
        anon,
        conv_own,
        grid,
        wide,
        dh_max,
        pairs,
        infos,
        start: loop_start(f.done_bucket, radar, anon_len),
        next: 0,
        wrap_pairs: None,
        work: 0,
        capped: false,
    })
}

/// Gates the anonymous plot `a` of `ra` against the tracks near it: the coarse window (on the
/// tracks of the grid cells it can reach, the nearest first if the budget is short, each charged to `budget`) and
/// then the exact gate. Returns the work spent; a plot that finds the budget or the pair limit
/// spent stops and marks the association capped.
fn assoc_plot(f: &mut Fusion, ra: &mut RadarAssoc, a: usize, budget: usize) -> usize {
    let Some(&j) = ra.anon.get(a) else { return 0 };
    let Some(p) = ra.plots.get(j) else { return 0 };
    let (g, s, bias) = (ra.g, ra.s, ra.bias);
    let prho = p.range_m - bias.range_m;
    let paz = (p.az_deg - bias.az_deg).rem_euclid(360.0);
    let mut cand = std::mem::take(&mut f.cand_buf);
    #[cfg(test)]
    let brute = f.brute_coarse;
    #[cfg(not(test))]
    let brute = false;
    if brute {
        cand.clear();
        cand.extend(0..ra.tracks.len() as u32);
    } else {
        coarse_query(&ra.grid, &mut cand, prho, paz, ra.dh_max, COARSE_STD_M);
        // The wide tracks are in no cell: add them, and keep the candidates in track order.
        if !ra.wide.is_empty() {
            for &w in &ra.wide {
                if !push_within(&mut cand, w) {
                    f.stats.scratch_full += 1;
                }
            }
            cand.sort_unstable();
        }
    }
    // Visiting a candidate is work: charged, counted, and the plot stops where the budget does.
    // A plot with more candidates than budget visits three quarters of the budget and leaves the
    // rest for the exact gates of the nearest of them.
    let visits = if brute {
        0
    } else if cand.len() <= budget {
        cand.len()
    } else {
        budget - budget / 4
    };
    f.stats.coarse_visits += visits as u64;
    let mut spent = visits;
    if !brute && visits < cand.len() {
        // Keep the nearest, not the lowest numbered (sorted in place: nothing is allocated).
        let key = |ti: u32| match (ra.tracks.get(ti as usize), ra.skip.get(ti as usize)) {
            (Some(t), Some(false)) => {
                coarse_distance((prho, paz), (t.rho, t.az, t.ground, t.sigma))
            }
            _ => f64::INFINITY,
        };
        cand.sort_unstable_by(|&x, &y| key(x).total_cmp(&key(y)).then(x.cmp(&y)));
        cand.truncate(visits);
        ra.capped = true;
    }
    for &ti in &cand {
        let ti = ti as usize;
        let (Some(t), false) = (ra.tracks.get(ti), ra.skip.get(ti).copied().unwrap_or(false))
        else {
            continue;
        };
        if !coarse_passes(COARSE_M, (prho, paz), (t.rho, t.az, t.ground, t.sigma)) {
            continue;
        }
        // A flooded bucket costs a bounded amount: past the limits the rest are not tested.
        if spent >= budget || ra.pairs.len() >= MAX_BUCKET_PAIRS {
            ra.capped = true;
            break;
        }
        let Some((row, _, last_ns)) = tracker_predict(&f.tracker, t.key, p.time_ns) else {
            continue;
        };
        let conv = match ra.conv_own.get(a).copied().flatten() {
            Some(c) => c,
            None => match plot_convert(&g, &s, p, &bias, track_height(&row), t.key) {
                Ok(c) => c,
                Err(_) => {
                    f.stats.no_height_pairs += 1;
                    continue;
                }
            },
        };
        spent += 1;
        f.stats.pairs_tested += 1;
        let pc = pair_cost(&row, &conv, p, s.alt_quantum_ft, last_ns);
        if pc.dh_gate <= GATE_H_CHI2
            && pc.dv <= GATE_V_CHI2
            && pc.dh_gate + pc.dv <= GATE_TOTAL_CHI2
        {
            f.stats.pairs_gated += 1;
            let mut m = conv.m;
            m.addr = t.key;
            let cost = (pc.dh + pc.dv).min(RESCUED_COST_FRACTION * GATE_TOTAL_CHI2);
            ra.pairs.push(Pair {
                track: ti as u32,
                plot: a as u32,
                cost,
            });
            ra.infos.push(PairInfo { m, row });
        }
    }
    f.cand_buf = cand;
    ra.work += spent;
    spent
}

/// The third part of one radar's association: the assignment of the gated pairs, and the
/// decision for each plot, left in the radar's scratch (`dec`, parallel to `plots`).
fn assoc_finish(f: &mut Fusion, ra: RadarAssoc) {
    let RadarAssoc {
        radar,
        r8,
        plots,
        mut dec,
        tracks,
        skip,
        anon,
        conv_own,
        grid,
        wide,
        mut pairs,
        mut infos,
        capped,
        next,
        start,
        wrap_pairs,
        ..
    } = ra;
    if let Some(slot) = f.grids.get_mut(radar) {
        *slot = grid;
    }
    if capped {
        f.stats.bucket_work_capped += 1;
        f.stats.anon_plots_ungated += anon.len().saturating_sub(next) as u64;
    }
    // The plots were gated from `start` round the end and on from the first: put the pairs back in
    // plot order, which is the order a loop from the first plot makes them in.
    if let Some(w) = wrap_pairs {
        let (np, ni) = (pairs.len(), infos.len());
        pairs.rotate_left(w.min(np));
        infos.rotate_left(w.min(ni));
    }
    // The plots the loop never reached: after `next` of them from `start`, round the ring.
    let n_anon = anon.len();
    let ungated = |a: usize| capped && (a + n_anon - start) % n_anon.max(1) >= next;
    let solved = if f.cfg.assoc == ASSOC_GNN {
        gnn(
            &mut f.assoc,
            tracks.len(),
            anon.len(),
            &pairs,
            GATE_TOTAL_CHI2,
        )
        .map(|st| add_gnn(&mut f.stats.gnn, &st))
    } else {
        nearest(&mut f.assoc, anon.len(), &pairs)
    };
    if solved.is_err() {
        f.stats.assoc_refused += 1;
    }
    let hold = f.cfg.hold_ambiguous;
    if hold {
        ambiguous_pairs(&mut f.assoc, &pairs, AMBIGUITY_MARGIN);
    }
    assigned_pairs(&mut f.assoc, &pairs);
    for (a, &j) in anon.iter().enumerate() {
        let Some(p) = plots.get(j) else { continue };
        let t = assoc_plot_track(&f.assoc)
            .get(a)
            .copied()
            .unwrap_or(NO_TRACK);
        if t == NO_TRACK {
            if ungated(a) {
                if let Some(d) = dec.get_mut(j) {
                    d.kind = D_DEFERRED;
                }
            } else {
                f.stats.unassigned += 1;
            }
            continue;
        }
        let k = assoc_pair_of(&f.assoc).get(a).copied().unwrap_or(NO_TRACK);
        let Some(info) = infos.get(k as usize) else {
            continue;
        };
        let key = info.m.addr;
        if hold && assoc_ambiguous(&f.assoc).get(a).copied().unwrap_or(false) {
            let run = f.hold_run.get(&(key, r8)).map_or(0, |h| h.run);
            if run < HOLD_MAX_RUN {
                hold_note(f, key, r8, run + 1);
                f.stats.held += 1;
                if let Some(d) = dec.get_mut(j) {
                    d.kind = D_HELD;
                }
                continue;
            }
            // The track has been starved of plots by ambiguity: it takes this one.
            f.stats.held_forced += 1;
        }
        f.hold_run.remove(&(key, r8));
        let resid = (f.cfg.registration && reg_eligible(f, key, &info.row, p))
            .then(|| residual_for(f, radar, &info.row, p))
            .flatten();
        if let Some(d) = dec.get_mut(j) {
            *d = Decision {
                kind: D_APPLY,
                key,
                how: HOW_ASSOC,
                m: info.m,
                resid,
            };
        }
        f.stats.applied_assoc += 1;
    }
    if let Some(slot) = f.pairs_bufs.get_mut(radar) {
        *slot = pairs;
    }
    if let Some(slot) = f.infos_bufs.get_mut(radar) {
        *slot = infos;
    }
    if let Some(slot) = f.rscratch.get_mut(radar) {
        slot.plots = plots;
        slot.dec = dec;
        slot.anon = anon;
        slot.conv_own = conv_own;
        slot.tracks = tracks;
        slot.skip = skip;
        slot.wide = wide;
    }
}

/// The decisions for the plots of every radar of a bucket (each radar's plots are in its
/// scratch, in time order; its decisions are left there). The hard-constraint plots are decided
/// first; the anonymous plots are then gated one at a time, always for the radar that has spent
/// the least work so far (the lowest radar on a tie), so when the bucket's work limit is reached
/// every radar has had its share and one radar's flood does not leave the others' plots ungated;
/// the assignments follow.
fn associate_bucket(f: &mut Fusion) {
    // At most `MAX_RADARS` radars (checked by `fusion_new`).
    let mut ras: [Option<RadarAssoc>; MAX_RADARS] = std::array::from_fn(|_| None);
    for (r, slot) in ras.iter_mut().enumerate() {
        if f.rscratch.get(r).is_some_and(|rs| !rs.plots.is_empty()) {
            *slot = assoc_prepare(f, r);
        }
    }
    let budget = f.bucket_tests_max;
    let mut total = 0usize;
    loop {
        let pick = ras
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                r.as_ref()
                    .filter(|ra| !ra.capped && ra.next < ra.anon.len())
                    .map(|ra| (ra.work, i))
            })
            .min();
        let Some((_, i)) = pick else { break };
        if total >= budget {
            // Out of work for the bucket: the plots still waiting are not gated.
            for ra in ras.iter_mut().flatten() {
                ra.capped |= ra.next < ra.anon.len();
            }
            break;
        }
        let Some(ra) = ras.get_mut(i).and_then(Option::as_mut) else {
            break;
        };
        // The plots in a rotation from the start; where it wraps past the last plot, note the
        // pairs made so far so that they can be put back in plot order.
        let k = ra.next;
        ra.next += 1;
        let n = ra.anon.len();
        if ra.start + k == n && ra.start > 0 {
            ra.wrap_pairs = Some(ra.pairs.len());
        }
        let a = (ra.start + k) % n.max(1);
        total += assoc_plot(f, ra, a, budget - total);
    }
    for ra in &mut ras {
        if let Some(ra) = ra.take() {
            assoc_finish(f, ra);
        }
    }
}

/// Records a run of `run` held plots of `radar` on `key`, in the bucket being processed. In a full
/// table the entry touched longest ago goes (the lowest key among those), counted.
fn hold_note(f: &mut Fusion, key: u32, radar: u8, run: u8) {
    let at = (key, radar);
    if !f.hold_run.contains_key(&at) && f.hold_run.len() >= f.hold_run_max {
        // The oldest bucket, the lowest key among equals: the order the table once iterated in.
        let oldest = f
            .hold_run
            .iter()
            .fold(None::<((u32, u8), i64)>, |best, (k, h)| match best {
                Some((bk, b)) if (b, bk) <= (h.bucket, *k) => best,
                _ => Some((*k, h.bucket)),
            });
        if let Some((k, _)) = oldest {
            f.hold_run.remove(&k);
            f.stats.hold_evicted += 1;
        }
    }
    // Room is made above (the count is held under the table's slots), so this is not refused.
    f.hold_run.insert(
        at,
        HoldRun {
            run,
            bucket: f.done_bucket,
        },
    );
}

fn add_gnn(a: &mut GnnStats, b: &GnnStats) {
    a.components += b.components;
    a.single += b.single;
    a.solved += b.solved;
    a.greedy_components += b.greedy_components;
    a.largest = a.largest.max(b.largest);
}

fn log_assign(f: &mut Fusion, t_ns: i64, key: u32, label: u32, radar: u8, how: u8) {
    if !f.cfg.log_assign {
        return;
    }
    if f.log.t_ns.len() >= ASSIGN_LOG_MAX {
        f.stats.assign_log_overflow += 1;
        return;
    }
    f.log.t_ns.push(t_ns);
    f.log.key.push(key);
    f.log.label.push(label);
    f.log.radar.push(radar);
    f.log.how.push(how);
}

/// An ADS-B measurement's bookkeeping, at the time it is applied.
fn note_adsb(f: &mut Fusion, m: &Measurement) {
    let sigma_ok = {
        let thr = oa_truth::nacp_epu_m(REG_MIN_NACP) / oa_truth::SIGMA95;
        let c = cov_f64(m.pos_cov_m2);
        c[0].max(c[2]).max(0.0).sqrt() <= thr * (1.0 + 1e-6)
    };
    if !f.keyinfo.contains_key(&m.addr) && f.keyinfo.len() >= f.cfg.tracker.max_tracks {
        // Full: forget the keys whose tracks have ended. The tracker took this report, so at
        // most max_tracks - 1 of the others are live and there is room afterwards.
        let tracker = &f.tracker;
        f.keyinfo
            .retain(|k, _| tracker_predict(tracker, *k, 0).is_some());
        if f.keyinfo.len() >= f.cfg.tracker.max_tracks {
            f.stats.keyinfo_overflow += 1;
            return;
        }
    }
    let mut e = f.keyinfo.get(&m.addr).copied().unwrap_or(KeyInfo {
        last_adsb_ns: i64::MIN,
        adsb_n: 0,
        high_integrity: false,
    });
    e.last_adsb_ns = m.toa_ns;
    e.adsb_n = e.adsb_n.saturating_add(1);
    e.high_integrity = sigma_ok;
    if !f.keyinfo.insert(m.addr, e) {
        f.stats.keyinfo_overflow += 1;
    }
}

#[derive(Clone, Copy)]
struct Ev {
    t: i64,
    /// 0 ADS-B, 1 plot.
    src: u8,
    radar: usize,
    idx: usize,
}

fn process_bucket(f: &mut Fusion, adsb: &[Measurement], plots: &[PendPlot], out: &mut TrackOutput) {
    f.stats.reg_clamped = f.reg.iter().map(|r| r.clamped).sum();
    f.stats.processed += (adsb.len() + plots.len()) as u64;
    // Group plots by radar, keeping time order, in each radar's scratch.
    for rs in &mut f.rscratch {
        rs.plots.clear();
        rs.dec.clear();
    }
    for p in plots {
        let Some(rs) = f.rscratch.get_mut(usize::from(p.radar)) else {
            continue;
        };
        if rs.plots.len() < RADAR_PLOTS_MAX {
            rs.plots.push(p.plot);
        } else {
            f.stats.bucket_plot_overflow += 1;
        }
    }
    f.stats.buckets += 1;
    prune_plot_scan(f, plots.first().map_or(0, |p| p.plot.time_ns));
    associate_bucket(f);
    // Events in time order.
    let mut evs = std::mem::take(&mut f.evs);
    evs.clear();
    let mut full = 0u64;
    for (i, m) in adsb.iter().enumerate() {
        let ev = Ev {
            t: m.toa_ns,
            src: 0,
            radar: 0,
            idx: i,
        };
        full += u64::from(!push_within(&mut evs, ev));
    }
    for (r, rs) in f.rscratch.iter().enumerate() {
        for (j, dj) in rs.dec.iter().enumerate() {
            if dj.kind == D_APPLY {
                let ev = Ev {
                    t: dj.m.toa_ns,
                    src: 1,
                    radar: r,
                    idx: r * 1_000_000 + j,
                };
                full += u64::from(!push_within(&mut evs, ev));
            }
        }
    }
    f.stats.scratch_full += full;
    // Every key is distinct, so the unstable sort gives the one order.
    evs.sort_unstable_by_key(|e| (e.t, e.src, e.radar, e.idx));
    for e in &evs {
        if e.src == 0 {
            if let Some(m) = adsb.get(e.idx) {
                let before = tracker_stats(&f.tracker).applied;
                tracker_update(&mut f.tracker, m, out);
                // Registration trusts only what the tracker took.
                if tracker_stats(&f.tracker).applied > before {
                    note_adsb(f, m);
                    refresh_expected(f, m.addr, m.toa_ns);
                }
            }
        } else {
            let (r, j) = (e.idx / 1_000_000, e.idx % 1_000_000);
            let Some(rs) = f.rscratch.get(r) else {
                continue;
            };
            let (Some(&dj), Some(&p)) = (rs.dec.get(j), rs.plots.get(j)) else {
                continue;
            };
            tracker_update(&mut f.tracker, &dj.m, out);
            log_assign(f, p.time_ns, dj.key, p.label, e.radar as u8, dj.how);
            note_plot_scan(f, dj.key, e.radar, p.time_ns);
            refresh_expected(f, dj.key, p.time_ns);
        }
    }
    f.evs = evs;
    // Registration, then initiation, per radar.
    for r in 0..f.rscratch.len() {
        let Some(rs) = f.rscratch.get_mut(r).map(std::mem::take) else {
            continue;
        };
        if !rs.plots.is_empty() {
            if f.cfg.registration {
                for (j, dj) in rs.dec.iter().enumerate() {
                    if let (Some(z), Some(p)) = (dj.resid, rs.plots.get(j)) {
                        if let Some(st) = f.reg.get_mut(r) {
                            reg_offer(st, p.time_ns, dj.key, &z);
                        }
                    }
                }
                publish_registration(f, r, rs.plots.last().map_or(0, |p| p.time_ns));
            }
            initiate(f, r, &rs.plots, &rs.dec, out);
        }
        if let Some(slot) = f.rscratch.get_mut(r) {
            *slot = rs;
        }
    }
}

/// Notes that radar `radar`'s plot at `t_ns` was taken by the track `key`.
fn note_plot_scan(f: &mut Fusion, key: u32, radar: usize, t_ns: i64) {
    if scan_of(f, radar, t_ns).is_none() {
        return;
    }
    let r8 = u8::try_from(radar).unwrap_or(u8::MAX);
    if let Some(s) = f.plot_scan.get_mut(&(key, r8)) {
        *s = (*s).max(t_ns);
    } else if !f.plot_scan.insert((key, r8), t_ns) {
        f.stats.plot_scan_full += 1;
    }
}

/// Sets the expected update interval of the track `key`, updated at `t_ns`: the shortest scan
/// period among the radars that updated it within their last three periods; none such leaves it
/// to the configured interval.
fn refresh_expected(f: &mut Fusion, key: u32, t_ns: i64) {
    let mut best = i64::MAX;
    for (r, s) in f.sites.iter().enumerate() {
        let r8 = u8::try_from(r).unwrap_or(u8::MAX);
        let near = f.plot_scan.get(&(key, r8)).is_some_and(|&lt| {
            t_ns.saturating_sub(lt) <= s.period_ns.saturating_mul(EXPECT_PERIODS)
        });
        if near {
            best = best.min(s.period_ns);
        }
    }
    let ns = if best == i64::MAX { 0 } else { best };
    tracker_set_expected_interval(&mut f.tracker, key, ns);
}

/// Whether the track `key` has taken a plot of `radar` in scan `scan` or a later one.
fn took_plot_in_scan(f: &Fusion, key: u32, radar: usize, scan: i64) -> bool {
    let r8 = u8::try_from(radar).unwrap_or(u8::MAX);
    f.plot_scan
        .get(&(key, r8))
        .is_some_and(|&t| scan_at(&f.sites, radar, t).is_some_and(|s| s >= scan))
}

/// Drops the plot notes older than [`RADAR_INIT_N`] + 2 scans before `t_ns` once the table is half
/// full (a note that old can matter to neither candidates, which are at most that many scans old,
/// nor the expected interval, which looks back three periods).
fn prune_plot_scan(f: &mut Fusion, t_ns: i64) {
    if f.plot_scan.len() < f.plot_scan.capacity() / 2 {
        return;
    }
    let mut min_t = [i64::MIN; MAX_RADARS];
    for (m, s) in min_t.iter_mut().zip(&f.sites) {
        *m = t_ns.saturating_sub(s.period_ns.saturating_mul(i64::from(RADAR_INIT_N) + 2));
    }
    f.plot_scan
        .retain(|k, s| *s >= min_t.get(usize::from(k.1)).copied().unwrap_or(i64::MIN));
}

fn publish_registration(f: &mut Fusion, radar: usize, t_ns: i64) {
    let (Some(st), Some(last), Some(hist)) = (
        f.reg.get(radar),
        f.reg_pub_ns.get_mut(radar),
        f.reg_hist.get_mut(radar),
    ) else {
        return;
    };
    if st.used == 0 {
        return;
    }
    if *last == i64::MIN || t_ns.saturating_sub(*last) >= REG_PUBLISH_NS {
        if hist.len() < REG_HISTORY_MAX {
            hist.push(reg_sample(st, t_ns));
        } else {
            f.stats.reg_history_full += 1;
        }
        *last = t_ns;
    }
}

// ---- radar-only initiation ------------------------------------------------------------

/// The scan number of a time: scans since the radar's epoch. `None` for a time before the epoch
/// or one whose distance from it does not fit, never a clamped number.
fn scan_of(f: &Fusion, radar: usize, t_ns: i64) -> Option<i64> {
    scan_at(&f.sites, radar, t_ns)
}

/// The same from the sites alone.
fn scan_at(sites: &[RadarSite], radar: usize, t_ns: i64) -> Option<i64> {
    let (p, e) = sites
        .get(radar)
        .map_or((NS_PER_S, 0), |s| (s.period_ns, s.epoch_ns));
    let since = t_ns.checked_sub(e).filter(|d| *d >= 0)?;
    Some(since.div_euclid(p))
}

/// Normalised cost of a plot joining a candidate: the distance from the candidate's straight-line
/// (or, with one hit, stationary) prediction over the gate radius, squared, scaled so the gate is
/// [`GATE_TOTAL_CHI2`]; `None` outside it.
fn cand_cost(c: &Cand, m: &Measurement, plot_scan: i64) -> Option<f64> {
    if plot_scan <= c.last_scan {
        return None;
    }
    let n = usize::from(c.n);
    let last = c.meas.get(n.checked_sub(1)?)?;
    let dt = m.toa_ns.saturating_sub(last.toa_ns) as f64 / 1.0e9;
    if dt <= 0.0 {
        return None;
    }
    let frame = frame_new(
        f64::from(last.lat_e7) / 1.0e7,
        f64::from(last.lon_e7) / 1.0e7,
    );
    let (x, y) = project(
        &frame,
        f64::from(m.lat_e7) / 1.0e7,
        f64::from(m.lon_e7) / 1.0e7,
    );
    let noise = |c: [f32; 3]| 4.0 * f64::from(c[0]).max(f64::from(c[2])).max(0.0).sqrt();
    let (pred, radius) = if n >= 2 {
        let prev = c.meas.get(n - 2)?;
        let (px, py) = project(
            &frame,
            f64::from(prev.lat_e7) / 1.0e7,
            f64::from(prev.lon_e7) / 1.0e7,
        );
        let dtp = last.toa_ns.saturating_sub(prev.toa_ns) as f64 / 1.0e9;
        if dtp <= 0.0 {
            return None;
        }
        // The previous point is at (px, py) relative to the last; velocity is last - prev.
        let (vx, vy) = (-px / dtp, -py / dtp);
        (
            (vx * dt, vy * dt),
            CAND_SLACK_M + 0.5 * CAND_MAX_ACCEL_MPS2 * dt * dt,
        )
    } else {
        ((0.0, 0.0), CAND_SLACK_M + CAND_MAX_SPEED_MPS * dt)
    };
    let radius = radius + noise(m.pos_cov_m2) + noise(last.pos_cov_m2);
    let d = oa_math::hypot(x - pred.0, y - pred.1);
    if d >= radius {
        return None;
    }
    if last.alt_ft != NONE_I32 && m.alt_ft != NONE_I32 {
        let dz = f64::from(m.alt_ft - last.alt_ft).abs();
        if dz > CAND_MAX_VRATE_FPS * dt + CAND_ALT_SLACK_FT {
            return None;
        }
    }
    Some(GATE_TOTAL_CHI2 * (d / radius) * (d / radius))
}

fn add_hit(c: &mut Cand, m: Measurement, label: u32, scan: i64) {
    let n = usize::from(c.n);
    if let (Some(slot), Some(lab)) = (c.meas.get_mut(n), c.label.get_mut(n)) {
        *slot = m;
        *lab = label;
        c.n += 1;
        c.last_scan = scan;
    }
}

/// Where a candidate's next plot can be, in the radar's plane (metres): none (it can gate no plot
/// of the bucket), within `reach` of a place, or anywhere.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CandReach {
    Never,
    At(f64, f64, f64),
    Anywhere,
}

/// Where the plots of a bucket (times `t_lo` to `t_hi`) can be, relative to the candidate `c`, in
/// the plane of `frame` (the radar's).
///
/// Superset: [`cand_cost`] passes a plot at time `tp` only if its distance in the plane of the
/// candidate's last plot from the prediction (the last plot, moved by the velocity of the last two
/// plots for the time since the last, if there are two) is below the gate radius, which at
/// most is the radius at the longest interval, `t_hi` less the last time, plus the noise of the
/// last plot and of the plot. The prediction at `tp` is within the speed times `t_hi - t_lo` of
/// the prediction at `t_lo`, which is the place returned. So a plot the candidate can gate is
/// within `radius at t_hi + noise of the last plot + drift` of that place, plus the plot's own
/// noise, in the plane of the candidate's last plot; the radar's plane and that one differ in
/// distance by a fraction of a per cent over the radar's range, so a margin of 3 % and 300 m is
/// added. A candidate with no interval to a plot of the bucket (or, with two plots, none between
/// them) gates none, which `cand_cost` also refuses.
fn cand_reach(c: &Cand, frame: &Frame, t_lo: i64, t_hi: i64) -> CandReach {
    let n = usize::from(c.n);
    let Some(last) = n.checked_sub(1).and_then(|k| c.meas.get(k)) else {
        return CandReach::Never;
    };
    let secs = |ns: i64| ns as f64 / 1.0e9;
    let dt_max = secs(t_hi.saturating_sub(last.toa_ns));
    if t_hi < t_lo || dt_max <= 0.0 {
        return CandReach::Never;
    }
    let at_last = frame_new(
        f64::from(last.lat_e7) / 1.0e7,
        f64::from(last.lon_e7) / 1.0e7,
    );
    let (offset, radius, speed) = if n >= 2 {
        let Some(prev) = c.meas.get(n - 2) else {
            return CandReach::Never;
        };
        let dtp = secs(last.toa_ns.saturating_sub(prev.toa_ns));
        if dtp <= 0.0 {
            return CandReach::Never;
        }
        let (px, py) = project(
            &at_last,
            f64::from(prev.lat_e7) / 1.0e7,
            f64::from(prev.lon_e7) / 1.0e7,
        );
        let (vx, vy) = (-px / dtp, -py / dtp);
        let dt_lo = secs(t_lo.saturating_sub(last.toa_ns));
        (
            (vx * dt_lo, vy * dt_lo),
            CAND_SLACK_M + 0.5 * CAND_MAX_ACCEL_MPS2 * dt_max * dt_max,
            oa_math::hypot(vx, vy),
        )
    } else {
        ((0.0, 0.0), CAND_SLACK_M + CAND_MAX_SPEED_MPS * dt_max, 0.0)
    };
    let noise = 4.0
        * f64::from(last.pos_cov_m2[0])
            .max(f64::from(last.pos_cov_m2[2]))
            .max(0.0)
            .sqrt();
    let Some((lat, lon)) = unproject(&at_last, offset.0, offset.1) else {
        return CandReach::Anywhere;
    };
    let (x, y) = project(frame, lat, lon);
    let reach = 1.03 * (radius + noise + speed * secs(t_hi - t_lo)) + 300.0;
    if x.is_finite() && y.is_finite() && reach.is_finite() {
        CandReach::At(x, y, reach)
    } else {
        CandReach::Anywhere
    }
}

fn initiate(
    f: &mut Fusion,
    radar: usize,
    plots: &[RadarPlot],
    dec: &[Decision],
    out: &mut TrackOutput,
) {
    let (Some(&g), Some(&s)) = (f.geoms.get(radar), f.sites.get(radar)) else {
        return;
    };
    let bias = if f.cfg.registration {
        f.reg.get(radar).map_or(NO_BIAS, reg_bias)
    } else {
        NO_BIAS
    };
    // Candidate plots: unassigned (not held, not refused).
    // The lists are the layer's, made at start: at most `plots.len()` (below `RADAR_PLOTS_MAX`)
    // plots and `MAX_CANDIDATES` candidates go in them.
    let mut free = std::mem::take(&mut f.init_free);
    let mut anon = std::mem::take(&mut f.init_anon);
    let mut cand_ix = std::mem::take(&mut f.cand_ix);
    free.clear();
    anon.clear();
    cand_ix.clear();
    for j in 0..plots.len() {
        if dec.get(j).is_some_and(|d| d.kind == D_UNASSIGNED) {
            free.push(j);
        }
    }
    // Address plots: join or start the candidate of that address.
    for &j in &free {
        let Some(p) = plots.get(j) else { continue };
        if f.cfg.use_address && p.addr != NO_ADDR {
            let Ok(c) = plot_convert(&g, &s, p, &bias, None, p.addr) else {
                f.stats.cand_no_height += 1;
                continue;
            };
            // Plot times were checked against the epoch when offered.
            let Some(scan) = scan_of(f, radar, p.time_ns) else {
                f.stats.plots_refused += 1;
                continue;
            };
            let at = f
                .cands
                .iter()
                .position(|c| usize::from(c.radar) == radar && c.addr == p.addr);
            match at {
                Some(i) => {
                    let ok = f.cands.get(i).is_some_and(|cd| scan > cd.last_scan);
                    if ok {
                        if let Some(cd) = f.cands.get_mut(i) {
                            add_hit(cd, c.m, p.label, scan);
                        }
                        f.stats.cand_joined += 1;
                    }
                }
                None => start_candidate(f, radar, p.addr, c.m, p.label, scan),
            }
        } else {
            anon.push(j);
        }
    }
    // Anonymous plots against anonymous candidates of this radar: exact assignment.
    for i in 0..f.cands.len() {
        if f.cands
            .get(i)
            .is_some_and(|c| usize::from(c.radar) == radar && c.addr == NO_ADDR)
        {
            cand_ix.push(i);
        }
    }
    let mut pairs = std::mem::take(&mut f.pairs_buf);
    // The converted measurement of each pair, parallel to `pairs`.
    let mut ms = std::mem::take(&mut f.ms_buf);
    pairs.clear();
    ms.clear();
    let mut tests = 0usize;
    let mut capped = false;
    // The candidates are indexed by where each is expected and how far from there a plot of this
    // bucket can lie (see [`cand_reach`]).
    let frame = frame_new(s.lat_deg, s.lon_deg);
    let (mut t_lo, mut t_hi) = (i64::MAX, i64::MIN);
    for &j in &anon {
        if let Some(p) = plots.get(j) {
            t_lo = t_lo.min(p.time_ns);
            t_hi = t_hi.max(p.time_ns);
        }
    }
    #[cfg(test)]
    let all = f.brute_coarse;
    #[cfg(not(test))]
    let all = false;
    let mut reach = std::mem::take(&mut f.init_reach);
    let mut grid = std::mem::take(&mut f.init_grid);
    let mut wide = std::mem::take(&mut f.init_wide);
    let mut near = std::mem::take(&mut f.init_near);
    reach.clear();
    wide.clear();
    for (ci, &i) in cand_ix.iter().enumerate() {
        let r = f
            .cands
            .get(i)
            .map_or(CandReach::Never, |c| cand_reach(c, &frame, t_lo, t_hi));
        // The grid takes the candidates of standard reach, the wide list the rest, as long as it
        // has room: the one that finds it full is not tested in this bucket.
        let r = match r {
            CandReach::At(_, _, w) if w > INIT_STD_M => {
                if wide.len() < INIT_WIDE_MAX && push_within(&mut wide, ci as u32) {
                    r
                } else {
                    f.stats.init_wide_full += 1;
                    CandReach::Never
                }
            }
            CandReach::Anywhere => {
                if wide.len() < INIT_WIDE_MAX && push_within(&mut wide, ci as u32) {
                    r
                } else {
                    f.stats.init_wide_full += 1;
                    CandReach::Never
                }
            }
            r => r,
        };
        push_within(&mut reach, r);
    }
    point_grid_build(&mut grid, reach.len(), |ci| match reach.get(ci) {
        Some(CandReach::At(x, y, w)) if *w <= INIT_STD_M => Some((*x, *y)),
        _ => None,
    });
    'plots: for (a, &j) in anon.iter().enumerate() {
        let Some(p) = plots.get(j) else { continue };
        let Some(scan) = scan_of(f, radar, p.time_ns) else {
            continue;
        };
        // A plot with Mode C converts the same for every candidate: once, and the candidates it can
        // reach are those near it (if its noise is too wide for the grid, all of them).
        let own = (p.alt_ft != NONE_ALT).then(|| plot_convert(&g, &s, p, &bias, None, NO_ADDR));
        let (conv_own, window) = match &own {
            Some(Ok(c)) => {
                let q = project(&frame, c.geo.lat_deg, c.geo.lon_deg);
                let noise = 4.0
                    * f64::from(c.m.pos_cov_m2[0])
                        .max(f64::from(c.m.pos_cov_m2[2]))
                        .max(0.0)
                        .sqrt();
                (Some(c.m), Some((q, 1.03 * noise)))
            }
            // The conversion fails for every candidate alike.
            Some(Err(_)) => continue,
            None => (None, None),
        };
        near.clear();
        let nearby = !all
            && window.is_some_and(|(q, noise)| {
                point_grid_query(&grid, &mut near, q.0, q.1, INIT_STD_M + noise)
            });
        if nearby {
            for &w in &wide {
                push_within(&mut near, w);
            }
            near.sort_unstable();
        } else {
            near.clear();
            near.extend(0..cand_ix.len() as u32);
        }
        f.stats.init_visits += near.len() as u64;
        for &ci in &near {
            let ci = ci as usize;
            let Some(&i) = cand_ix.get(ci) else { continue };
            let Some(c) = f.cands.get(i) else {
                continue;
            };
            // A candidate the plot cannot reach (it is further than the candidate's reach and the
            // plot's noise): the exact test would refuse it.
            match (reach.get(ci), window) {
                _ if all => {}
                (Some(CandReach::Never), _) => continue,
                (Some(CandReach::At(x, y, w)), Some((q, noise)))
                    if oa_math::hypot(q.0 - x, q.1 - y) > w + noise =>
                {
                    continue;
                }
                _ => {}
            }
            if tests >= MAX_BUCKET_TESTS || pairs.len() >= MAX_BUCKET_PAIRS {
                capped = true;
                break 'plots;
            }
            tests += 1;
            f.stats.init_tests += 1;
            let m = match conv_own {
                Some(m) => m,
                None => {
                    // The plot's height: the candidate's last altitude.
                    let last_alt = c
                        .meas
                        .get(usize::from(c.n).saturating_sub(1))
                        .map(|m| m.alt_ft)
                        .filter(|&a| a != NONE_I32);
                    let h = last_alt.map(|a| (f64::from(a) * FT_M, 15.0));
                    let Ok(conv) = plot_convert(&g, &s, p, &bias, h, NO_ADDR) else {
                        continue;
                    };
                    conv.m
                }
            };
            if let Some(cost) = cand_cost(c, &m, scan) {
                pairs.push(Pair {
                    track: ci as u32,
                    plot: a as u32,
                    cost,
                });
                ms.push(m);
            }
        }
    }
    f.init_reach = reach;
    f.init_grid = grid;
    f.init_wide = wide;
    f.init_near = near;
    if capped {
        f.stats.bucket_work_capped += 1;
        f.stats.init_capped += 1;
    }
    match gnn(
        &mut f.assoc,
        cand_ix.len(),
        anon.len(),
        &pairs,
        GATE_TOTAL_CHI2,
    ) {
        Ok(st) => add_gnn(&mut f.stats.gnn, &st),
        Err(_) => f.stats.assoc_refused += 1,
    }
    assigned_pairs(&mut f.assoc, &pairs);
    f.pairs_buf = pairs;
    for (a, &j) in anon.iter().enumerate() {
        let Some(p) = plots.get(j) else { continue };
        let Some(scan) = scan_of(f, radar, p.time_ns) else {
            continue;
        };
        let ci = assoc_plot_track(&f.assoc)
            .get(a)
            .copied()
            .unwrap_or(NO_TRACK);
        if ci != NO_TRACK {
            let k = assoc_pair_of(&f.assoc).get(a).copied().unwrap_or(NO_TRACK);
            let m = ms.get(k as usize).copied();
            if let (Some(m), Some(&i)) = (m, cand_ix.get(ci as usize)) {
                if let Some(cd) = f.cands.get_mut(i) {
                    add_hit(cd, m, p.label, scan);
                    f.stats.cand_joined += 1;
                }
                continue;
            }
        }
        // Starts a candidate if it has an altitude to convert at.
        match plot_convert(&g, &s, p, &bias, None, NO_ADDR) {
            Ok(c) => start_candidate(f, radar, NO_ADDR, c.m, p.label, scan),
            Err(_) => f.stats.cand_no_height += 1,
        }
    }
    f.ms_buf = ms;
    f.init_free = free;
    f.init_anon = anon;
    f.cand_ix = cand_ix;
    // Confirm and expire.
    let now_scan = plots
        .first()
        .map_or(Some(0), |p| scan_of(f, radar, p.time_ns));
    let mut i = 0usize;
    while i < f.cands.len() {
        let Some(c) = f.cands.get(i).copied() else {
            break;
        };
        if usize::from(c.radar) != radar {
            i += 1;
            continue;
        }
        if c.n >= f.cfg.init_m {
            f.cands.remove(i);
            if let Some((key, decision_ns)) = confirm(f, radar, &c, out) {
                // Candidates before `i` that go shift the walk.
                i -= cancel_gated_cands(f, radar, key, decision_ns, i);
            }
        } else if now_scan
            .and_then(|n| n.checked_sub(c.first_scan))
            .is_some_and(|d| d >= i64::from(f.cfg.init_n))
        {
            f.cands.remove(i);
            f.stats.cand_expired += 1;
        } else {
            i += 1;
        }
    }
}

/// Appends the rows of `src` at or after `from_ns` to `dst`.
fn copy_rows_from(src: &TrackOutput, from_ns: i64, dst: &mut TrackOutput) {
    for i in 0..src.t_ns.len() {
        let (Some(&t), Some(&addr)) = (src.t_ns.get(i), src.addr.get(i)) else {
            continue;
        };
        if t < from_ns {
            continue;
        }
        let g = |v: &[f32]| v.get(i).copied().unwrap_or(0.0);
        output_push(
            dst,
            &OutRow {
                t_ns: t,
                addr,
                lat_e7: src.lat_e7.get(i).copied().unwrap_or(0),
                lon_e7: src.lon_e7.get(i).copied().unwrap_or(0),
                vel_east_mps: g(&src.vel_east_mps),
                vel_north_mps: g(&src.vel_north_mps),
                pos_cov_m2: src.pos_cov_m2.get(i).copied().unwrap_or([0.0; 3]),
                vel_cov_m2s2: src.vel_cov_m2s2.get(i).copied().unwrap_or([0.0; 3]),
                turn_rate_rps: g(&src.turn_rate_rps),
                alt_ft: g(&src.alt_ft),
                alt_var_ft2: g(&src.alt_var_ft2),
                vrate_fpm: g(&src.vrate_fpm),
                mu_h: src.mu_h.get(i).copied().unwrap_or([0.0; 2]),
                mu_v: src.mu_v.get(i).copied().unwrap_or([0.0; 2]),
                flags: src.flags.get(i).copied().unwrap_or(0),
            },
        );
    }
}

fn start_candidate(f: &mut Fusion, radar: usize, addr: u32, m: Measurement, label: u32, scan: i64) {
    if f.cands.len() >= MAX_CANDIDATES {
        f.stats.cand_capacity += 1;
        return;
    }
    let mut c = Cand {
        radar: radar as u8,
        addr,
        n: 0,
        first_scan: scan,
        last_scan: i64::MIN,
        meas: [m; 4],
        label: [label; 4],
    };
    add_hit(&mut c, m, label, scan);
    f.cands.push(c);
    f.stats.cand_started += 1;
    f.stats.cands_live_max = f.stats.cands_live_max.max(f.cands.len() as u64);
}

/// A candidate reached M hits: its plots become a track keyed by its address, or a new radar key.
/// An anonymous candidate whose latest plot gates with a live track makes none (that track is
/// the aircraft's: another radar confirmed it, in this bucket or earlier). Returns the key and
/// the time of the decision when a track was made.
fn confirm(f: &mut Fusion, radar: usize, c: &Cand, out: &mut TrackOutput) -> Option<(u32, i64)> {
    if c.addr == NO_ADDR && cand_matches_live_track(f, radar, c) {
        f.stats.cand_merged_on_confirm += 1;
        return None;
    }
    let key = if c.addr != NO_ADDR {
        c.addr
    } else {
        f.next_serial += 1;
        KEY_RADAR_BASE + f.next_serial
    };
    let n = usize::from(c.n);
    f.stats.cand_confirmed += 1;
    let before = tracker_stats(&f.tracker).capacity_refused;
    // The plots are replayed into a scratch table and only rows at or after the confirming
    // plot go to the real output: nothing, not even a retroactive tick, precedes the decision.
    let decision_ns = c.meas.get(n.saturating_sub(1)).map_or(0, |m| m.toa_ns);
    output_clear(&mut f.scratch);
    for i in 0..n {
        let Some(mut m) = c.meas.get(i).copied() else {
            continue;
        };
        m.addr = key;
        tracker_update(&mut f.tracker, &m, &mut f.scratch);
        let label = c.label.get(i).copied().unwrap_or(NO_ADDR);
        log_assign(f, m.toa_ns, key, label, radar as u8, HOW_INIT);
        note_plot_scan(f, key, radar, m.toa_ns);
    }
    refresh_expected(
        f,
        key,
        c.meas.get(n.saturating_sub(1)).map_or(0, |m| m.toa_ns),
    );
    copy_rows_from(&f.scratch, decision_ns, out);
    if tracker_stats(&f.tracker).capacity_refused > before {
        f.stats.track_refused += 1;
    }
    let first = c.meas.first().map_or(0, |m| m.toa_ns);
    let last = c.meas.get(n.saturating_sub(1)).map_or(0, |m| m.toa_ns);
    if f.tracks.len() >= f.track_rec_max {
        f.stats.track_rec_overflow += 1;
    } else {
        f.tracks.push(RadarTrackRec {
            key,
            radar: radar as u8,
            first_ns: first,
            confirm_ns: last,
        });
    }
    Some((key, decision_ns))
}

/// Whether the latest plot of the anonymous candidate `c` gates with any live track.
fn cand_matches_live_track(f: &mut Fusion, radar: usize, c: &Cand) -> bool {
    let Some(m) = c.meas.get(usize::from(c.n).saturating_sub(1)).copied() else {
        return false;
    };
    let quant = f.sites.get(radar).map_or(100, |s| s.alt_quantum_ft);
    let mut tests = 0u64;
    let mut hit = false;
    for (key, _) in tracker_live_iter(&f.tracker).take(MAX_ASSOC_TRACKS) {
        let Some((row, _, last_ns)) = tracker_predict(&f.tracker, key, m.toa_ns) else {
            continue;
        };
        tests += 1;
        if cand_gates_row(&row, last_ns, &m, quant) {
            if took_plot_in_scan(f, key, radar, c.last_scan) {
                f.stats.cand_same_scan_kept += 1;
                continue;
            }
            hit = true;
            break;
        }
    }
    f.stats.confirm_gate_tests += tests;
    hit
}

/// Cancels the anonymous candidates of the other radars that gate with the track `key` just made
/// (its row at `at_ns`); returns how many of them were before index `before` in the list.
fn cancel_gated_cands(f: &mut Fusion, radar: usize, key: u32, at_ns: i64, before: usize) -> usize {
    let Some((row, _, last_ns)) = tracker_predict(&f.tracker, key, at_ns) else {
        return 0;
    };
    let (mut tests, mut gone, mut gone_before, mut at) = (0u64, 0u64, 0usize, 0usize);
    let (sites, plot_scan) = (&f.sites, &f.plot_scan);
    f.cands.retain(|c| {
        let i = at;
        at += 1;
        if usize::from(c.radar) == radar || c.addr != NO_ADDR {
            return true;
        }
        let Some(m) = c.meas.get(usize::from(c.n).saturating_sub(1)) else {
            return true;
        };
        tests += 1;
        let quant = sites
            .get(usize::from(c.radar))
            .map_or(100, |s| s.alt_quantum_ft);
        let same_scan = plot_scan.get(&(key, c.radar)).is_some_and(|&t| {
            scan_at(sites, usize::from(c.radar), t).is_some_and(|s| s >= c.last_scan)
        });
        if cand_gates_row(&row, last_ns, m, quant) && !same_scan {
            gone += 1;
            if i < before {
                gone_before += 1;
            }
            false
        } else {
            true
        }
    });
    f.stats.confirm_gate_tests += tests;
    f.stats.cand_cancelled_by_track += gone;
    gone_before
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::radar::NONE_CODE;
    use crate::tracker::tracker_live;
    use crate::types::{output_len, DEFAULT_CONFIG, O_RADAR_DISAGREES};
    use oa_sensorsim::prng::Rng;
    use std::collections::BTreeMap;

    pub(crate) const SITE: RadarSite = RadarSite {
        lat_deg: 41.976_940_27,
        lon_deg: -87.908_149_72,
        height_m: 207.0,
        sigma_range_m: 70.0,
        sigma_az_deg: 0.08,
        range_quantum_m: 0.0,
        alt_quantum_ft: 100,
        period_ns: 4_800_000_000,
        epoch_ns: 0,
        sensor: 1,
    };

    /// A straight track in the radar's local plane at constant altitude, as plots.
    struct Line {
        x0: f64,
        y0: f64,
        vx: f64,
        vy: f64,
        alt_ft: i32,
        label: u32,
    }

    fn plots_of(lines: &[Line], scans: i64, seed: u64, mode_s: bool) -> Vec<RadarPlot> {
        plots_of_site(&SITE, lines, scans, seed, mode_s)
    }

    /// The same, seen from `site` (the lines are in the plane of `SITE`).
    fn plots_of_site(
        site: &RadarSite,
        lines: &[Line],
        scans: i64,
        seed: u64,
        mode_s: bool,
    ) -> Vec<RadarPlot> {
        let g = radar_geom(site);
        let frame = oa_geo::aeqd::frame_new(SITE.lat_deg, SITE.lon_deg);
        let mut rng = Rng::from_seed(seed);
        let mut v = Vec::new();
        for k in 0..scans {
            for (li, l) in lines.iter().enumerate() {
                let t_s = (k as f64) * (site.period_ns as f64 / 1.0e9) + (li as f64) * 0.1;
                let (x, y) = (l.x0 + l.vx * t_s, l.y0 + l.vy * t_s);
                let (lat, lon) = oa_geo::aeqd::unproject(&frame, x, y).unwrap();
                let h = f64::from(l.alt_ft) * FT_M;
                let (rho, az) = polar_of(&g, lat, lon, h);
                v.push(RadarPlot {
                    time_ns: (t_s * 1.0e9) as i64,
                    range_m: rho + 70.0 * rng.normal(),
                    az_deg: (az + 0.08 * rng.normal()).rem_euclid(360.0),
                    alt_ft: ((f64::from(l.alt_ft) / 100.0).round() * 100.0) as i32,
                    addr: if mode_s { l.label } else { NO_ADDR },
                    label: l.label,
                    mode_a: NONE_CODE,
                });
            }
        }
        v.sort_by_key(|p| p.time_ns);
        v
    }

    fn run(f: &mut Fusion, plots: &[RadarPlot], adsb: &[Measurement]) -> TrackOutput {
        let mut out = output_new(1 << 18);
        let end = plots
            .iter()
            .map(|p| p.time_ns)
            .chain(adsb.iter().map(|m| m.toa_ns))
            .max()
            .unwrap_or(0);
        let (mut pi, mut ai) = (0, 0);
        while pi < plots.len() || ai < adsb.len() {
            let tp = plots.get(pi).map_or(i64::MAX, |p| p.time_ns);
            let ta = adsb.get(ai).map_or(i64::MAX, |m| m.toa_ns);
            if tp <= ta {
                fusion_plots(f, 0, &plots[pi..=pi]);
                pi += 1;
                fusion_advance(f, tp, &mut out);
            } else {
                fusion_adsb(f, &adsb[ai]);
                ai += 1;
                fusion_advance(f, ta, &mut out);
            }
        }
        fusion_finish(f, end, &mut out);
        out
    }

    pub(crate) fn make_fusion(mutate: impl Fn(&mut FusionConfig)) -> Fusion {
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 18,
                ..DEFAULT_CONFIG
            },
            &[SITE],
        );
        let mut c = fusion_config(tc);
        c.registration = false;
        c.log_assign = true;
        mutate(&mut c);
        fusion_new(&c, &[SITE]).unwrap()
    }

    fn easting(lines: &[Line]) -> Vec<Line> {
        // Helper for readability: nothing to change.
        lines
            .iter()
            .map(|l| Line {
                x0: l.x0,
                y0: l.y0,
                vx: l.vx,
                vy: l.vy,
                alt_ft: l.alt_ft,
                label: l.label,
            })
            .collect()
    }

    // REQ: OA-TRK-048
    #[test]
    fn a_radar_only_track_is_confirmed_by_three_plots_in_four_scans() {
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let plots = plots_of(&lines, 12, 3, false);
        let mut f = make_fusion(|_| {});
        let out = run(&mut f, &plots, &[]);
        let recs = fusion_radar_tracks(&f);
        assert_eq!(recs.len(), 1, "{:?}", fusion_stats(&f));
        // Confirmed on the third plot: two scans after the first.
        let dt = (recs[0].confirm_ns - recs[0].first_ns) as f64 / 1.0e9;
        assert!((9.0..10.2).contains(&dt), "initiation delay {dt} s");
        assert!(recs[0].key >= KEY_RADAR_BASE);
        // No output row before the decision.
        assert!(out
            .t_ns
            .iter()
            .all(|&t| t >= recs[0].confirm_ns - 1_000_000));
        assert!(output_len(&out) > 8);
        // Every plot after the first scans was applied to the same key.
        let log = fusion_assign_log(&f);
        assert!(log.key.iter().all(|&k| k == recs[0].key));
        assert_eq!(fusion_stats(&f).cand_confirmed, 1);
    }

    // REQ: OA-TRK-048
    #[test]
    fn two_isolated_false_plots_never_make_a_track() {
        // Many scans of random isolated false plots, one every scan: 3 of 4 cannot be met.
        let mut rng = Rng::from_seed(5);
        let mut plots = Vec::new();
        for k in 0..60i64 {
            plots.push(RadarPlot {
                time_ns: k * 4_800_000_000 + (rng.unit() * 4.0e9) as i64,
                range_m: 20_000.0 + 80_000.0 * rng.unit(),
                az_deg: 360.0 * rng.unit(),
                alt_ft: 10_000,
                addr: NO_ADDR,
                label: NO_ADDR,
                mode_a: NONE_CODE,
            });
        }
        plots.sort_by_key(|p| p.time_ns);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        assert_eq!(fusion_stats(&f).cand_confirmed, 0);
        assert!(fusion_stats(&f).cand_expired > 40);
    }

    // REQ: OA-TRK-049
    #[test]
    fn a_mode_a_code_alone_never_assigns_a_plot() {
        // Two aircraft far apart that squawk the same code stay two tracks; a plot whose code
        // matches a track's code but whose position is outside every gate is unassigned.
        let lines = easting(&[
            Line {
                x0: 30_000.0,
                y0: 20_000.0,
                vx: -150.0,
                vy: 40.0,
                alt_ft: 20_000,
                label: 0xA00001,
            },
            Line {
                x0: -50_000.0,
                y0: -30_000.0,
                vx: 140.0,
                vy: 20.0,
                alt_ft: 30_000,
                label: 0xA00002,
            },
        ]);
        let mut plots = plots_of(&lines, 14, 4, false);
        for p in &mut plots {
            p.mode_a = 0o1200;
        }
        // A stray plot far from both, with the same code, in the middle of the run.
        let stray = RadarPlot {
            time_ns: 30_000_000_000,
            range_m: 90_000.0,
            az_deg: 250.0,
            alt_ft: 25_000,
            addr: NO_ADDR,
            label: 0xBAD,
            mode_a: 0o1200,
        };
        plots.push(stray);
        plots.sort_by_key(|p| p.time_ns);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        assert_eq!(fusion_radar_tracks(&f).len(), 2);
        let log = fusion_assign_log(&f);
        // The stray was never applied to either track.
        assert!(!log.label.contains(&0xBAD));
    }

    // REQ: OA-TRK-050
    #[test]
    fn a_mode_s_address_is_a_hard_constraint_and_keys_the_track() {
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let plots = plots_of(&lines, 12, 3, true);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        let recs = fusion_radar_tracks(&f);
        assert_eq!(recs.len(), 1);
        // The track is keyed by the address, not by a radar key.
        assert_eq!(recs[0].key, 0xA00001);
        let log = fusion_assign_log(&f);
        assert!(fusion_stats(&f).applied_hard >= 8, "{:?}", fusion_stats(&f));
        assert!(log.how.contains(&HOW_HARD));
    }

    // REQ: OA-TRK-051
    #[test]
    fn labels_are_never_read_by_a_decision() {
        let lines = easting(&[
            Line {
                x0: 30_000.0,
                y0: 20_000.0,
                vx: -150.0,
                vy: 40.0,
                alt_ft: 20_000,
                label: 0xA00001,
            },
            Line {
                x0: 20_000.0,
                y0: 20_000.0,
                vx: -100.0,
                vy: 90.0,
                alt_ft: 20_000,
                label: 0xA00002,
            },
        ]);
        let plots = plots_of(&lines, 14, 9, false);
        let scrambled: Vec<RadarPlot> = plots
            .iter()
            .enumerate()
            .map(|(i, p)| RadarPlot {
                label: 0x123 + (i as u32 % 7),
                ..*p
            })
            .collect();
        let mut f1 = make_fusion(|_| {});
        let mut f2 = make_fusion(|_| {});
        let o1 = run(&mut f1, &plots, &[]);
        let o2 = run(&mut f2, &scrambled, &[]);
        let (mut h1, mut h2) = (oa_sha256::Sha256::new(), oa_sha256::Sha256::new());
        crate::types::output_hash_into(&mut h1, &o1);
        crate::types::output_hash_into(&mut h2, &o2);
        assert_eq!(h1.finalize(), h2.finalize());
        assert_eq!(fusion_assign_log(&f1).key, fusion_assign_log(&f2).key);
        assert_eq!(fusion_stats(&f1), fusion_stats(&f2));
    }

    // REQ: OA-TRK-052
    #[test]
    fn crossing_tracks_at_one_altitude_are_held_not_swapped() {
        // Two aircraft crossing at 60 degrees at the same altitude, passing within 150 m: the
        // plots near the crossing are ambiguous and held; the identities hold through it.
        let lines = easting(&[
            Line {
                x0: 30_000.0,
                y0: 40_000.0,
                vx: -200.0,
                vy: 0.0,
                alt_ft: 25_000,
                label: 0xA00001,
            },
            // Crosses line 1 at (x = 10,000, t = 100 s): start there minus 100 s of its motion,
            // with a 150 m offset in y so the paths do not coincide exactly.
            Line {
                x0: 10_000.0 + 100.0 * 100.0,
                y0: 40_150.0 - 100.0 * 173.2,
                vx: -100.0,
                vy: 173.2,
                alt_ft: 25_000,
                label: 0xA00002,
            },
        ]);
        let plots = plots_of(&lines, 40, 21, false);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        let log = fusion_assign_log(&f);
        // Per radar key, the sequence of labels of applied true plots never changes.
        let mut by_key: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (k, l) in log.key.iter().zip(log.label.iter()) {
            by_key.entry(*k).or_default().push(*l);
        }
        assert_eq!(by_key.len(), 2, "{:?}", fusion_stats(&f));
        for (k, labels) in &by_key {
            assert!(
                labels.windows(2).all(|w| w[0] == w[1]),
                "key {k:x} swapped: {labels:x?}"
            );
        }
        assert!(fusion_stats(&f).held > 0, "nothing was held");
    }

    /// Nine aircraft (the first of them radar-only, the others ADS-B equipped) under a radar of
    /// bias `(b_r, b_a)`; every ADS-B report of the aircraft in `spoofed` is displaced
    /// `spoof_m` metres east. Returns the plots and the ADS-B.
    fn registration_world(
        b_r: f64,
        b_a: f64,
        spoofed: &[u32],
        spoof_m: f64,
    ) -> (Vec<RadarPlot>, Vec<Measurement>) {
        let frame = oa_geo::aeqd::frame_new(SITE.lat_deg, SITE.lon_deg);
        let mut rng = Rng::from_seed(17);
        let at = |i: usize, x0: f64, y0: f64, vx: f64, vy: f64, alt_ft: i32| Line {
            x0,
            y0,
            vx,
            vy,
            alt_ft,
            label: 0xA00001 + i as u32,
        };
        let lines = easting(&[
            at(0, -30_000.0, 50_000.0, 120.0, -60.0, 20_000),
            at(1, 40_000.0, 20_000.0, -150.0, 10.0, 30_000),
            at(2, 60_000.0, -40_000.0, -90.0, 130.0, 25_000),
            at(3, -50_000.0, -30_000.0, 140.0, 20.0, 28_000),
            at(4, 10_000.0, 70_000.0, 60.0, -150.0, 33_000),
            at(5, -70_000.0, 10_000.0, 150.0, 40.0, 24_000),
            at(6, 25_000.0, -60_000.0, -80.0, 120.0, 31_000),
            at(7, -20_000.0, -25_000.0, 100.0, 90.0, 22_000),
            at(8, 80_000.0, 30_000.0, -140.0, -30.0, 27_000),
        ]);
        let mut plots = plots_of(&lines, 80, 31, false);
        for p in &mut plots {
            p.range_m += b_r;
            p.az_deg = (p.az_deg + b_a).rem_euclid(360.0);
        }
        // ADS-B for every aircraft but the first, every second, NACp 10 noise (4.1 m per axis).
        let mut adsb = Vec::new();
        let end = plots.last().map_or(0, |p| p.time_ns);
        let mut t = 0i64;
        while t <= end {
            for l in lines.iter().skip(1) {
                let stamp = t + 100_000_000 * (i64::from(l.label) & 1);
                let ts = stamp as f64 / 1.0e9;
                let lie = if spoofed.contains(&l.label) {
                    spoof_m
                } else {
                    0.0
                };
                let (x, y) = (l.x0 + l.vx * ts + lie, l.y0 + l.vy * ts);
                let (lat, lon) =
                    oa_geo::aeqd::unproject(&frame, x + 4.1 * rng.normal(), y + 4.1 * rng.normal())
                        .unwrap();
                adsb.push(Measurement {
                    toa_ns: stamp,
                    pos_cov_m2: [16.7, 0.0, 16.7],
                    vel_cov_m2s2: [1.4, 0.0, 1.4],
                    vel_east_mps: l.vx as f32,
                    vel_north_mps: l.vy as f32,
                    lat_e7: (lat * 1e7).round() as i32,
                    lon_e7: (lon * 1e7).round() as i32,
                    alt_ft: l.alt_ft,
                    vrate_fpm: NONE_I32,
                    addr: l.label,
                    alt_quant_ft: 25,
                    sensor: 0,
                    ident: crate::types::NO_IDENT,
                    flags: crate::types::M_HAS_VEL,
                    category: 0,
                });
            }
            t += 1_000_000_000;
        }
        adsb.sort_by_key(|m| m.toa_ns);
        (plots, adsb)
    }

    // REQ: OA-TRK-053
    #[test]
    fn registration_learns_the_bias_from_adsb_tracks_and_corrects_radar_only_ones() {
        // One radar-only aircraft and eight ADS-B ones; the radar has a 150 m / 0.15 deg bias.
        let (b_r, b_a) = (150.0, 0.15);
        let (plots, adsb) = registration_world(b_r, b_a, &[], 0.0);
        let mut with = make_fusion(|c| c.registration = true);
        let mut without = make_fusion(|c| c.registration = false);
        let _ = run(&mut with, &plots, &adsb);
        let _ = run(&mut without, &plots, &adsb);
        let (st, hist) = fusion_registration(&with, 0).unwrap();
        assert!(st.used > 100, "used {}", st.used);
        assert!(
            (st.range_m - b_r).abs() < 15.0,
            "range bias est {}",
            st.range_m
        );
        assert!((st.az_deg - b_a).abs() < 0.02, "az bias est {}", st.az_deg);
        assert!(hist.len() > 10);
        // The estimate's own uncertainty covers the error (within 4 sigma).
        assert!((st.range_m - b_r).abs() < 4.0 * st.var_range_m2.sqrt() + 1.0);
        assert_eq!(fusion_registration(&without, 0).unwrap().0.used, 0);
    }

    // REQ: OA-TRK-074
    #[test]
    fn spoofed_adsb_aircraft_cannot_shift_the_registration_of_a_radar() {
        // Two of the eight reference aircraft report positions 200 m east of the truth.
        let (b_r, b_a) = (80.0, 0.05);
        let (plots, adsb) = registration_world(b_r, b_a, &[0xA00002, 0xA00003], 200.0);
        let mut f = make_fusion(|c| c.registration = true);
        let _ = run(&mut f, &plots, &adsb);
        let (st, _) = fusion_registration(&f, 0).unwrap();
        assert!(st.used > 100, "used {}", st.used);
        assert!(
            (st.range_m - b_r).abs() < 20.0,
            "range bias est {}",
            st.range_m
        );
        assert!((st.az_deg - b_a).abs() < 0.03, "az bias est {}", st.az_deg);
        assert!(st.inconsistent > 20, "inconsistent {}", st.inconsistent);
    }

    // REQ: OA-TRK-072
    #[test]
    fn a_mode_s_plot_far_from_its_track_is_not_applied() {
        // A Mode S aircraft is followed, and one plot carrying its address turns up 70 km from
        // it: another aircraft on the address, or a corrupt plot. The address is strong
        // evidence but not proof, so a gross mismatch is not applied blindly.
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let mut plots = plots_of(&lines, 16, 3, true);
        plots.push(RadarPlot {
            time_ns: 43_500_000_000,
            range_m: 100_000.0,
            az_deg: 250.0,
            alt_ft: 20_000,
            addr: 0xA00001,
            label: 0xBAD,
            mode_a: NONE_CODE,
        });
        plots.sort_by_key(|p| p.time_ns);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        let log = fusion_assign_log(&f);
        assert!(
            !log.label.contains(&0xBAD),
            "the mismatching plot was applied"
        );
        assert!(
            fusion_stats(&f).applied_hard >= 12,
            "{:?}",
            fusion_stats(&f)
        );
    }

    /// A stationary ADS-B fix of `addr` at the position of [`adsb_fix`] moved `dlat_e7` north.
    fn adsb_moved(addr: u32, t_ns: i64, dlat_e7: i32) -> Measurement {
        let mut m = adsb_fix(addr, t_ns);
        m.lat_e7 += dlat_e7;
        m
    }

    /// A Mode S plot of `addr` for an aircraft at `adsb_fix` moved `dlat_e7` north.
    fn mode_s_plot_at(addr: u32, t_ns: i64, dlat_e7: i32) -> RadarPlot {
        let lat = f64::from(421_500_000 + dlat_e7) / 1.0e7;
        let (rho, az) = polar_of(&radar_geom(&SITE), lat, -87.9, 20_000.0 * FT_M);
        RadarPlot {
            time_ns: t_ns,
            range_m: rho,
            az_deg: az,
            alt_ft: 20_000,
            addr,
            label: addr,
            mode_a: NONE_CODE,
        }
    }

    // REQ: OA-TRK-176
    #[test]
    fn radar_contradicting_an_adsb_track_flags_it_for_three_scans_and_moves_nothing() {
        // 0xA00001's ADS-B says 5 km north of where two radars see its Mode S plots for five
        // scans (10 s to 30 s), after which the plots agree with the track. 0xA00002 is
        // honest throughout. Scans are 4.8 s.
        const DISPLACED: i32 = 450_000;
        let mut f = make_fusion_radars(2);
        let mut out = output_new(1 << 18);
        for s in 0..70i64 {
            let t = s * NS_PER_S;
            fusion_adsb(&mut f, &adsb_moved(0xA00001, t, DISPLACED));
            fusion_adsb(&mut f, &adsb_moved(0xA00002, t, 0));
            for r in 0..2usize {
                for k in 0..10i64 {
                    let tp = 10 * NS_PER_S + k * 4_800_000_000 + (r as i64) * 500_000_000;
                    if tp / NS_PER_S != s {
                        continue;
                    }
                    let off = if k < 5 { 0 } else { DISPLACED };
                    fusion_plots(&mut f, r, &[mode_s_plot_at(0xA00001, tp, off)]);
                    fusion_plots(&mut f, r, &[mode_s_plot_at(0xA00002, tp, 0)]);
                }
            }
            fusion_advance(&mut f, (s + 1) * NS_PER_S, &mut out);
        }
        fusion_finish(&mut f, 70 * NS_PER_S, &mut out);
        let flagged = |addr: u32, from_s: f64, to_s: f64| -> usize {
            (0..out.t_ns.len())
                .filter(|&i| {
                    let ts = out.t_ns[i] as f64 / 1.0e9;
                    out.addr[i] == addr
                        && ts >= from_s
                        && ts <= to_s
                        && out.flags[i] & O_RADAR_DISAGREES != 0
                })
                .count()
        };
        let rows = |addr: u32, from_s: f64, to_s: f64| -> usize {
            (0..out.t_ns.len())
                .filter(|&i| {
                    let ts = out.t_ns[i] as f64 / 1.0e9;
                    out.addr[i] == addr && ts >= from_s && ts <= to_s
                })
                .count()
        };
        // The last contradicting plot is at 29.7 s: flagged from the first scan to three scans
        // (14.4 s) after the last, and clear after.
        assert_eq!(flagged(0xA00001, 0.0, 9.9), 0);
        let during = rows(0xA00001, 11.0, 44.0);
        assert!(during > 30, "{during} rows");
        assert_eq!(flagged(0xA00001, 11.0, 44.0), during);
        assert_eq!(flagged(0xA00001, 45.0, 70.0), 0);
        assert!(rows(0xA00001, 45.0, 70.0) > 20);
        // The honest aircraft never carries it.
        assert!(rows(0xA00002, 0.0, 70.0) > 60);
        assert_eq!(flagged(0xA00002, 0.0, 70.0), 0);
        let st = fusion_stats(&f);
        assert_eq!(st.radar_disagree, 10, "{st:?}");
        assert_eq!(st.hard_mismatch, 10, "{st:?}");
        // Nothing moved: the track stayed where ADS-B put it (5 km north of the plots).
        let i = (0..out.t_ns.len())
            .rev()
            .find(|&i| out.addr[i] == 0xA00001)
            .unwrap();
        assert!(
            (out.lat_e7[i] - 421_500_000 - DISPLACED).abs() < 200,
            "{}",
            out.lat_e7[i]
        );
    }

    // REQ: OA-TRK-072
    #[test]
    fn a_track_takes_at_most_one_plot_per_radar_bucket() {
        // One Mode S aircraft. In scan 6 its plot is joined, in the same second, by an
        // anonymous plot 100 m away (a split plot) and by a second plot with its address 50 m
        // away: the track takes one of the three.
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let mut plots = plots_of(&lines, 16, 3, true);
        let at = plots
            .iter()
            .find(|p| p.time_ns > 28_000_000_000)
            .copied()
            .unwrap();
        plots.push(RadarPlot {
            time_ns: at.time_ns + 100_000_000,
            range_m: at.range_m + 100.0,
            addr: NO_ADDR,
            label: 0x5117,
            ..at
        });
        plots.push(RadarPlot {
            time_ns: at.time_ns + 200_000_000,
            range_m: at.range_m + 50.0,
            label: 0x2222,
            ..at
        });
        plots.sort_by_key(|p| p.time_ns);
        let mut f = make_fusion(|_| {});
        run(&mut f, &plots, &[]);
        let log = fusion_assign_log(&f);
        let mut seen = std::collections::BTreeSet::new();
        for (t, k) in log.t_ns.iter().zip(log.key.iter()) {
            let second = t.div_euclid(1_000_000_000);
            assert!(
                seen.insert((*k, second)),
                "track {k:x} took two plots in second {second}"
            );
        }
    }

    // REQ: OA-TRK-076
    #[test]
    fn two_close_radar_only_aircraft_are_not_starved_by_the_ambiguity_hold() {
        // Two aircraft flying together 90 m apart (plot scatter is 70 m): every plot is within
        // the ambiguity margin of both tracks. Held for ever, neither track would be updated.
        let lines = easting(&[
            Line {
                x0: 30_000.0,
                y0: 20_000.0,
                vx: -150.0,
                vy: 40.0,
                alt_ft: 20_000,
                label: 0xA00001,
            },
            Line {
                x0: 30_000.0,
                y0: 20_090.0,
                vx: -150.0,
                vy: 40.0,
                alt_ft: 20_000,
                label: 0xA00002,
            },
        ]);
        for seed in [1u64, 2, 3] {
            let plots = plots_of(&lines, 40, seed, false);
            let mut f = make_fusion(|_| {});
            run(&mut f, &plots, &[]);
            let st = fusion_stats(&f);
            assert!(st.held > 0, "seed {seed}: nothing held");
            // 80 plots, 6 of them making the two tracks: a track holds two plots and takes the
            // third, so about a third of the rest are applied (the held design applied none).
            assert!(
                st.applied_assoc >= 20,
                "seed {seed}: applied {}",
                st.applied_assoc
            );
            assert_eq!(st.cand_confirmed, 2, "seed {seed}: {st:?}");
            // The plots were held, and then applied on the assignment's own evidence.
            assert!(st.held_forced > 0, "seed {seed}: {st:?}");
        }
    }

    /// A fusion over `n` radars, all at [`SITE`] (differing in sensor number only).
    fn make_fusion_radars(n: usize) -> Fusion {
        let sites: Vec<RadarSite> = (0..n)
            .map(|i| RadarSite {
                sensor: 1 + i as u16,
                ..SITE
            })
            .collect();
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 18,
                ..DEFAULT_CONFIG
            },
            &sites,
        );
        let mut c = fusion_config(tc);
        c.registration = false;
        c.log_assign = true;
        fusion_new(&c, &sites).unwrap()
    }

    /// Two ADS-B aircraft 60 m apart (tracks after ten reports) and, from each of `radars`
    /// radars in the next second, one plot of each, between them in range and exact in azimuth:
    /// every plot is within the ambiguity margin of both tracks.
    fn ambiguous_pair_seen_by(radars: usize) -> Fusion {
        let mut f = make_fusion_radars(radars);
        let mut out = output_new(1 << 18);
        for k in 0..10i64 {
            for (addr, dlat) in [(0xA00001, 0), (0xA00002, 5_400)] {
                let mut m = adsb_fix(addr, k * NS_PER_S);
                m.lat_e7 += dlat;
                fusion_adsb(&mut f, &m);
            }
            fusion_advance(&mut f, k * NS_PER_S, &mut out);
        }
        let g = radar_geom(&SITE);
        for r in 0..radars {
            let plots: Vec<RadarPlot> = [(0xA00001u32, 0), (0xA00002, 5_400)]
                .iter()
                .enumerate()
                .map(|(i, &(label, dlat))| {
                    let lat = f64::from(421_500_000 + dlat) / 1.0e7;
                    let (rho, az) = polar_of(&g, lat, -87.9, 20_000.0 * FT_M);
                    RadarPlot {
                        time_ns: 10 * NS_PER_S + (r * 2 + i) as i64 * 10_000_000,
                        range_m: rho + 20.0,
                        az_deg: az,
                        alt_ft: 20_000,
                        addr: NO_ADDR,
                        label,
                        mode_a: NONE_CODE,
                    }
                })
                .collect();
            fusion_plots(&mut f, r, &plots);
        }
        fusion_advance(&mut f, 11 * NS_PER_S, &mut out);
        f
    }

    // REQ: OA-TRK-175
    #[test]
    fn each_radar_has_its_own_hold_run_on_a_track() {
        // Three radars see the same ambiguous pair in one bucket. A run counted per track alone
        // would be used up by the first two radars and the third's plots applied at once.
        let f = ambiguous_pair_seen_by(3);
        let st = fusion_stats(&f);
        assert_eq!(st.held, 6, "{st:?}");
        assert_eq!(st.held_forced, 0, "{st:?}");
        assert_eq!(st.applied_assoc, 0, "{st:?}");
    }

    // REQ: OA-TRK-175
    #[test]
    fn a_full_hold_table_drops_the_entry_touched_longest_ago_and_counts_it() {
        let mut f = make_fusion(|_| {});
        f.hold_run_max = 3;
        for (bucket, key, radar) in [(5, 0x30, 0), (4, 0x20, 1), (4, 0x10, 0)] {
            f.done_bucket = bucket;
            hold_note(&mut f, key, radar, 1);
        }
        assert_eq!(f.stats.hold_evicted, 0);
        // Touching an entry already there is not an insertion: nothing goes.
        f.done_bucket = 6;
        hold_note(&mut f, 0x30, 0, 2);
        assert_eq!(f.stats.hold_evicted, 0);
        // Full: of the two touched in bucket 4, the lower key goes (not the lowest key overall,
        // which is what the first of the table would be).
        f.done_bucket = 7;
        hold_note(&mut f, 0x05, 0, 1);
        assert_eq!(f.stats.hold_evicted, 1);
        let mut keys: Vec<(u32, u8)> = f.hold_run.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec![(0x05, 0), (0x20, 1), (0x30, 0)]);
        // The next to go is the one in bucket 4 that is left.
        f.done_bucket = 8;
        hold_note(&mut f, 0x06, 0, 1);
        let mut keys: Vec<(u32, u8)> = f.hold_run.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec![(0x05, 0), (0x06, 0), (0x30, 0)]);
        assert_eq!(f.stats.hold_evicted, 2);
    }

    /// A fusion over `sites` with room for `max_tracks`, without registration.
    fn make_fusion_sites(sites: &[RadarSite], max_tracks: usize) -> Fusion {
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 18,
                max_tracks,
                ..DEFAULT_CONFIG
            },
            sites,
        );
        let mut c = fusion_config(tc);
        c.registration = false;
        c.log_assign = true;
        fusion_new(&c, sites).unwrap()
    }

    /// Offers `n` stationary ADS-B tracks at 20,000 ft, scattered over a square of `side_m` round
    /// `site` (addresses from `addr0`), four seconds of reports; returns their places (latitude,
    /// longitude). The caller advances time.
    fn feed_tracks(
        f: &mut Fusion,
        site: &RadarSite,
        n: usize,
        side_m: f64,
        seed: u64,
        addr0: u32,
    ) -> Vec<(f64, f64)> {
        feed_tracks_at(f, site, n, side_m, seed, addr0, 0)
    }

    /// The same, the reports starting at `t0_ns`.
    fn feed_tracks_at(
        f: &mut Fusion,
        site: &RadarSite,
        n: usize,
        side_m: f64,
        seed: u64,
        addr0: u32,
        t0_ns: i64,
    ) -> Vec<(f64, f64)> {
        let frame = oa_geo::aeqd::frame_new(site.lat_deg, site.lon_deg);
        let mut rng = Rng::from_seed(seed);
        let at: Vec<(f64, f64)> = (0..n)
            .map(|_| {
                oa_geo::aeqd::unproject(
                    &frame,
                    side_m * (rng.unit() - 0.5),
                    side_m * (rng.unit() - 0.5),
                )
                .unwrap()
            })
            .collect();
        for k in 0..4i64 {
            for (i, &(lat, lon)) in at.iter().enumerate() {
                let mut m = adsb_fix(addr0 + i as u32, t0_ns + k * NS_PER_S);
                m.lat_e7 = (lat * 1.0e7).round() as i32;
                m.lon_e7 = (lon * 1.0e7).round() as i32;
                fusion_adsb(f, &m);
            }
        }
        at
    }

    /// A plot of `site` for each of the first `n` places, `noise_m` off in range and a little
    /// in azimuth, in the bucket after the reports, in time order.
    fn plots_on(
        site: &RadarSite,
        at: &[(f64, f64)],
        n: usize,
        noise_m: f64,
        seed: u64,
    ) -> Vec<RadarPlot> {
        let g = radar_geom(site);
        let mut rng = Rng::from_seed(seed);
        at.iter()
            .take(n)
            .enumerate()
            .map(|(i, &(lat, lon))| {
                let (rho, az) = polar_of(&g, lat, lon, 20_000.0 * FT_M);
                RadarPlot {
                    time_ns: 5 * NS_PER_S + (i as i64) * 10_000,
                    range_m: rho + noise_m * rng.normal(),
                    az_deg: (az + noise_m / 40_000.0 * rng.normal()).rem_euclid(360.0),
                    alt_ft: 20_000,
                    addr: NO_ADDR,
                    label: i as u32,
                    mode_a: NONE_CODE,
                }
            })
            .collect()
    }

    // REQ: OA-TRK-177
    #[test]
    fn ten_thousand_plots_over_twenty_thousand_tracks_associate_in_seconds() {
        // A busy bucket of one radar over a whole airspace of tracks: every plot used to be
        // tested against every track (2e8 window tests).
        let mut f = make_fusion_sites(&[SITE], 20_000);
        // This density of tracks sits within a tenth of the work limit; the standard search radius
        // is a little wider than the one window this test used to have, so the limit is doubled:
        // the test is of the cost of the search, not of the limit.
        f.bucket_tests_max = 2 * MAX_BUCKET_TESTS;
        let at = feed_tracks(&mut f, &SITE, 20_000, 800_000.0, 12, 0x100000);
        let mut out = output_new(1 << 18);
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        let plots = plots_on(&SITE, &at, 10_000, 0.0, 3);
        let started = std::time::Instant::now();
        fusion_plots(&mut f, 0, &plots);
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let took = started.elapsed().as_secs_f64();
        let st = fusion_stats(&f);
        assert_eq!(st.bucket_work_capped, 0, "{st:?}");
        assert!(st.applied_assoc + st.held >= 9_900, "{st:?}");
        assert_eq!(st.plots_refused, 0, "{st:?}");
        eprintln!("associated in {took} s");
        // A debug build on a loaded machine; the scan of every pair took several times this.
        assert!(took < 12.0, "{took} s: {st:?}");
    }

    /// Everything a run decided, to compare two runs of the same inputs.
    fn outcome(f: &Fusion, out: &TrackOutput) -> (FusionStats, AssignLog, String) {
        let mut h = oa_sha256::Sha256::new();
        crate::types::output_hash_into(&mut h, out);
        let mut st = *fusion_stats(f);
        st.coarse_visits = 0;
        st.init_tests = 0;
        st.init_visits = 0;
        (
            st,
            fusion_assign_log(f).clone(),
            oa_sha256::to_hex(&h.finalize()),
        )
    }

    // REQ: OA-TRK-177
    #[test]
    fn the_grid_gives_the_associations_of_testing_every_track() {
        // Radars on the +-180 degree seam, at the pole's limit and ordinary; 600 tracks round
        // each, plots a few hundred metres off them (some with their addresses) and random ones.
        let site = |lat_deg: f64, lon_deg: f64, sensor: u16| RadarSite {
            lat_deg,
            lon_deg,
            sensor,
            ..SITE
        };
        let sites = [
            site(0.0, 179.95, 1),
            site(85.0, 20.0, 2),
            site(41.98, -87.91, 3),
        ];
        let run_once = |brute: bool| {
            let mut f = make_fusion_sites(&sites, 2_000);
            f.brute_coarse = brute;
            let mut out = output_new(1 << 18);
            let mut places = Vec::new();
            for (r, s) in sites.iter().enumerate() {
                places.push(feed_tracks(
                    &mut f,
                    s,
                    600,
                    300_000.0,
                    40 + r as u64,
                    0x100000 * (r as u32 + 1),
                ));
            }
            fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
            let mut rng = Rng::from_seed(9);
            for (r, s) in sites.iter().enumerate() {
                let mut plots = plots_on(s, &places[r], 600, 300.0, 50 + r as u64);
                for (i, p) in plots.iter_mut().enumerate() {
                    if i % 7 == 0 {
                        p.addr = 0x100000 * (r as u32 + 1) + i as u32;
                    }
                }
                for k in 0..200i64 {
                    plots.push(RadarPlot {
                        time_ns: 5 * NS_PER_S + 7_000_000 + k * 10_000,
                        range_m: 1_000.0 + 400_000.0 * rng.unit(),
                        az_deg: 360.0 * rng.unit(),
                        alt_ft: 20_000,
                        addr: NO_ADDR,
                        label: 9_000 + k as u32,
                        mode_a: NONE_CODE,
                    });
                }
                fusion_plots(&mut f, r, &plots);
            }
            fusion_advance(&mut f, 12 * NS_PER_S, &mut out);
            fusion_finish(&mut f, 12 * NS_PER_S, &mut out);
            let visits = fusion_stats(&f).coarse_visits;
            (outcome(&f, &out), visits)
        };
        let (grid, visits) = run_once(false);
        let (brute, none) = run_once(true);
        assert_eq!(none, 0);
        assert!(grid.0.pairs_gated > 600, "{:?}", grid.0);
        assert!(grid.0.applied_hard > 50, "{:?}", grid.0);
        assert!(grid.0.held > 0 || grid.0.unassigned > 100, "{:?}", grid.0);
        assert!(visits > 0);
        assert_eq!(grid.0, brute.0);
        assert_eq!(grid.1, brute.1);
        assert_eq!(grid.2, brute.2);
    }

    // REQ: OA-TRK-222
    #[test]
    fn a_short_budget_keeps_the_nearest_candidates_not_the_lowest_numbered() {
        // 5,000 tracks inside one grid cell (42.0 to 42.25 N, 88.0 to 87.75 W, 20 to 40 km from
        // the radar), the plot's own track numbered last; the plot may spend 100 (75 visits, then
        // the exact gates).
        let mut f = make_fusion_sites(&[SITE], 5_000);
        f.bucket_tests_max = 100;
        let mut out = output_new(1 << 18);
        let mut rng = Rng::from_seed(31);
        let at: Vec<(f64, f64)> = (0..5_000)
            .map(|_| (42.005 + 0.24 * rng.unit(), -87.995 + 0.24 * rng.unit()))
            .collect();
        for k in 0..4i64 {
            for (i, &(lat, lon)) in at.iter().enumerate() {
                let mut m = adsb_fix(0x100000 + i as u32, k * NS_PER_S);
                m.lat_e7 = (lat * 1.0e7).round() as i32;
                m.lon_e7 = (lon * 1.0e7).round() as i32;
                fusion_adsb(&mut f, &m);
            }
        }
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        let plots = plots_on(&SITE, &at[4_999..], 1, 0.0, 3);
        fusion_plots(&mut f, 0, &plots);
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.bucket_work_capped, 1, "{st:?}");
        let log = fusion_assign_log(&f);
        let i = log.label.iter().position(|&l| l == 0);
        assert!(i.is_some(), "the plot was not associated: {st:?}");
        assert_eq!(
            log.key.get(i.unwrap_or(0)).copied(),
            Some(0x100000 + 4_999),
            "{:?}",
            log.how
        );
    }

    // REQ: OA-TRK-297
    #[test]
    fn the_stage_counters_count_what_a_cut_association_leaves_ungated() {
        // 200 tracks within 20 km, 100 plots over them, the bucket's work limit cut to 500: the
        // plots after the limit are not gated and are counted as such; the width of the coarse
        // window and the buckets are recorded.
        let mut f = make_fusion_sites(&[SITE], 400);
        f.bucket_tests_max = 500;
        let mut out = output_new(1 << 18);
        let at = feed_tracks(&mut f, &SITE, 200, 20_000.0, 7, 0x100000);
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        let plots = plots_on(&SITE, &at, 100, 0.0, 3);
        fusion_plots(&mut f, 0, &plots);
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = *fusion_stats(&f);
        assert_eq!(st.assoc_runs, 1, "{st:?}");
        assert_eq!(st.bucket_work_capped, 1, "{st:?}");
        assert!(
            st.anon_plots_ungated > 0 && st.anon_plots_ungated < 100,
            "{st:?}"
        );
        assert_eq!(st.tracks_at_sum, 200, "{st:?}");
        assert!(st.coarse_width_max_m >= COARSE_M as u64, "{st:?}");
        assert_eq!(st.coarse_width_sum_m, st.coarse_width_max_m, "{st:?}");
        // Four buckets of ADS-B and the one with the plots.
        assert_eq!(st.buckets, 5, "{st:?}");
        // The counters are a record only: a second run gives the same ones.
        let mut g = make_fusion_sites(&[SITE], 400);
        g.bucket_tests_max = 500;
        feed_tracks(&mut g, &SITE, 200, 20_000.0, 7, 0x100000);
        fusion_advance(&mut g, 4 * NS_PER_S, &mut out);
        fusion_plots(&mut g, 0, &plots);
        fusion_advance(&mut g, 7 * NS_PER_S, &mut out);
        assert_eq!(*fusion_stats(&g), st);
    }

    // REQ: OA-TRK-177
    #[test]
    fn a_flood_from_one_radar_does_not_stop_another_radars_plots() {
        // 100 tracks in a 3 km cluster and a flood of 2,000 plots from radar 0 over it, every
        // one a candidate for every track; twenty isolated tracks 50 km away seen by radar 1.
        // The bucket's work limit is cut to 40,000 so the flood runs into it: the flood is cut
        // (and counted), radar 1's plots, though they come second, are all gated.
        let sites = [
            RadarSite { sensor: 1, ..SITE },
            RadarSite { sensor: 2, ..SITE },
        ];
        let mut f = make_fusion_sites(&sites, 200);
        f.bucket_tests_max = 40_000;
        let mut out = output_new(1 << 18);
        let cluster = feed_tracks(&mut f, &SITE, 100, 3_000.0, 1, 0x100000);
        // The isolated ones, 50 km east of the cluster.
        let frame = oa_geo::aeqd::frame_new(SITE.lat_deg, SITE.lon_deg);
        let mut isolated = Vec::new();
        for i in 0..20u32 {
            let (lat, lon) =
                oa_geo::aeqd::unproject(&frame, 50_000.0 + 1_000.0 * f64::from(i), 0.0).unwrap();
            for k in 0..4i64 {
                let mut m = adsb_fix(0x300000 + i, k * NS_PER_S);
                m.lat_e7 = (lat * 1.0e7).round() as i32;
                m.lon_e7 = (lon * 1.0e7).round() as i32;
                fusion_adsb(&mut f, &m);
            }
            isolated.push((lat, lon));
        }
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        let mut rng = Rng::from_seed(4);
        let flood: Vec<RadarPlot> = (0..2_000)
            .map(|i| {
                let (lat, lon) = cluster[(rng.unit() * 100.0) as usize % 100];
                RadarPlot {
                    label: 5_000 + i,
                    time_ns: 5 * NS_PER_S + i64::from(i) * 10_000,
                    ..plots_on(&SITE, &[(lat, lon)], 1, 100.0, u64::from(i))[0]
                }
            })
            .collect();
        let victim = plots_on(&sites[1], &isolated, 20, 0.0, 8);
        fusion_plots(&mut f, 0, &flood);
        fusion_plots(&mut f, 1, &victim);
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.bucket_work_capped, 1, "{st:?}");
        let log = fusion_assign_log(&f);
        let second = log.radar.iter().filter(|&&r| r == 1).count();
        assert_eq!(second, 20, "radar 1 plots applied: {second}; {st:?}");
        // The flood was cut, not all of it gated: well under the 2,000 x 100 tests it asks for.
        assert!(st.pairs_tested + st.coarse_visits <= 40_000 + 200, "{st:?}");
    }

    fn try_fusion(sites: &[RadarSite]) -> Result<Fusion, FusionError> {
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 16,
                ..DEFAULT_CONFIG
            },
            sites,
        );
        fusion_new(&fusion_config(tc), sites)
    }

    /// Three plots of one aircraft, one rotation apart, shifted by `epoch_ns`.
    fn three_scans_after(epoch_ns: i64) -> Vec<RadarPlot> {
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let mut plots = plots_of(&lines, 3, 3, false);
        for p in &mut plots {
            p.time_ns += epoch_ns;
        }
        plots
    }

    // REQ: OA-TRK-287
    #[test]
    fn a_site_epoch_outside_the_supported_years_is_refused_at_config_time() {
        for epoch_ns in [i64::MIN, -1, crate::radar::EPOCH_MAX_NS + 1, i64::MAX] {
            let site = RadarSite { epoch_ns, ..SITE };
            assert!(
                matches!(try_fusion(&[site]), Err(FusionError::BadSite(0, _))),
                "epoch {epoch_ns} accepted"
            );
        }
        for epoch_ns in [0, 1_700_000_000_000_000_000, crate::radar::EPOCH_MAX_NS] {
            let site = RadarSite { epoch_ns, ..SITE };
            assert!(try_fusion(&[site]).is_ok(), "epoch {epoch_ns} refused");
        }
    }

    // REQ: OA-TRK-287
    #[test]
    fn three_scans_under_a_valid_epoch_confirm_a_radar_only_track() {
        let epoch_ns = 1_700_000_000_000_000_000;
        let site = RadarSite { epoch_ns, ..SITE };
        let mut f = try_fusion(&[site]).unwrap();
        let mut out = output_new(1 << 16);
        for p in three_scans_after(epoch_ns) {
            fusion_plots(&mut f, 0, &[p]);
            fusion_advance(&mut f, p.time_ns, &mut out);
        }
        fusion_finish(&mut f, epoch_ns + 20 * NS_PER_S, &mut out);
        assert_eq!(fusion_radar_tracks(&f).len(), 1, "{:?}", fusion_stats(&f));
    }

    // REQ: OA-TRK-287
    #[test]
    fn a_plot_before_the_site_epoch_is_refused_and_counted_not_clamped() {
        let epoch_ns = 100 * NS_PER_S;
        let site = RadarSite { epoch_ns, ..SITE };
        let mut f = try_fusion(&[site]).unwrap();
        let mut out = output_new(1 << 16);
        let early = three_scans_after(0);
        fusion_plots(&mut f, 0, &early);
        fusion_advance(&mut f, 60 * NS_PER_S, &mut out);
        fusion_finish(&mut f, 60 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.plots_in, 3);
        assert_eq!(st.plots_refused, 3, "{st:?}");
        assert_eq!(st.processed, 0);
        assert_eq!(st.cand_started, 0);
    }

    // REQ: OA-TRK-289
    #[test]
    fn the_fusion_tracker_config_passes_validation_for_every_accepted_rotation_period() {
        use crate::radar::site_check;
        use crate::types::check_config;
        let base = TrackerConfig {
            max_tracks: 64,
            out_capacity: 1024,
            log_capacity: 64,
            ..DEFAULT_CONFIG
        };
        let (lo, hi) = (100_000_000i64, 600_000_000_000i64);
        let mut periods = vec![lo, lo + 1, hi - 1, hi];
        // Log-spaced between the endpoints, and the whole seconds a site is likely to have.
        for k in 0..=200 {
            let x = (k as f64) / 200.0;
            periods.push(((lo as f64) * ((hi as f64) / (lo as f64)).powf(x)).round() as i64);
        }
        periods.extend((1..=600).map(|s| s * NS_PER_S));
        for period_ns in periods {
            let site = RadarSite { period_ns, ..SITE };
            assert!(site_check(&site).is_ok(), "{period_ns}");
            let tc = fusion_tracker_config(&base, &[site]);
            assert!(check_config(&tc).is_ok(), "period {period_ns} ns: {tc:?}");
        }
        for period_ns in [lo, hi] {
            let site = RadarSite { period_ns, ..SITE };
            let c = fusion_config(fusion_tracker_config(&base, &[site]));
            assert!(fusion_new(&c, &[site]).is_ok(), "period {period_ns} ns");
        }
    }

    // REQ: OA-TRK-291
    #[test]
    fn the_processing_path_keeps_every_preallocated_buffer_under_a_representative_load() {
        // Three radars (two periods), registration on, ADS-B tracks plus one radar-only aircraft,
        // a Mode S half of the plots of two radars, 384 s of it. A buffer reallocated by the run
        // would change its address or capacity (a temporary freed within a call would not show).
        let sites = [
            SITE,
            RadarSite {
                lat_deg: SITE.lat_deg + 0.4,
                lon_deg: SITE.lon_deg - 0.3,
                sensor: 2,
                ..SITE
            },
            RadarSite {
                lat_deg: SITE.lat_deg - 0.3,
                lon_deg: SITE.lon_deg + 0.5,
                sensor: 3,
                period_ns: 12_000_000_000,
                ..SITE
            },
        ];
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 18,
                max_tracks: 256,
                ..DEFAULT_CONFIG
            },
            &sites,
        );
        let mut c = fusion_config(tc);
        c.log_assign = true;
        let mut f = fusion_new(&c, &sites).unwrap();
        let fresh = fusion_buffers(&f);
        let (plots0, adsb) = registration_world(30.0, 0.05, &[], 0.0);
        let mut rng = Rng::from_seed(99);
        let mut events: Vec<(i64, usize, RadarPlot)> =
            plots0.iter().map(|p| (p.time_ns, 0, *p)).collect();
        for (r, site) in sites.iter().enumerate().skip(1) {
            let g = radar_geom(site);
            for (i, m) in adsb.iter().enumerate() {
                let (lat, lon) = (f64::from(m.lat_e7) / 1.0e7, f64::from(m.lon_e7) / 1.0e7);
                let (rho, az) = polar_of(&g, lat, lon, f64::from(m.alt_ft) * FT_M);
                let t = m.toa_ns + 200_000_000 * r as i64;
                events.push((
                    t,
                    r,
                    RadarPlot {
                        time_ns: t,
                        range_m: rho + 70.0 * rng.normal(),
                        az_deg: (az + 0.08 * rng.normal()).rem_euclid(360.0),
                        alt_ft: m.alt_ft,
                        addr: if i % 2 == 0 { m.addr } else { NO_ADDR },
                        label: m.addr,
                        mode_a: NONE_CODE,
                    },
                ));
            }
        }
        events.sort_by_key(|e| (e.0, e.1));
        let mut out = output_new(1 << 18);
        let mut ai = 0;
        let mut warm = None;
        for (t, r, p) in &events {
            while adsb.get(ai).is_some_and(|m| m.toa_ns <= *t) {
                fusion_adsb(&mut f, &adsb[ai]);
                ai += 1;
            }
            fusion_plots(&mut f, *r, &[*p]);
            fusion_advance(&mut f, *t, &mut out);
            crate::types::output_clear(&mut out);
            if warm.is_none() && *t > 100 * NS_PER_S {
                warm = Some(fusion_buffers(&f));
            }
        }
        let end_ns = events.last().map_or(0, |e| e.0);
        fusion_finish(&mut f, end_ns, &mut out);
        let st = fusion_stats(&f);
        assert!(st.applied_hard > 100 && st.applied_assoc > 100, "{st:?}");
        assert!(st.cand_confirmed > 0, "{st:?}");
        assert!(fusion_registration(&f, 0).unwrap().1.len() > 3);
        assert_eq!((st.scratch_full, st.reg_history_full), (0, 0));
        assert_eq!(st.assoc_refused, 0);
        let warm = warm.unwrap();
        assert_eq!(warm, fresh);
        assert_eq!(fusion_buffers(&f), warm);
        // Handing over the records keeps the layer's buffer.
        let n = fusion_radar_tracks(&f).len();
        assert_eq!(fusion_take_radar_tracks(&mut f).len(), n);
        assert_eq!(fusion_buffers(&f), warm);
    }

    // REQ: OA-TRK-291
    #[test]
    fn a_scratch_buffer_at_its_capacity_refuses_and_is_not_grown() {
        let mut v: Vec<u32> = Vec::with_capacity(4);
        let (ptr, cap) = (v.as_ptr(), v.capacity());
        let pushed = (0..10).filter(|&i| push_within(&mut v, i)).count();
        assert_eq!(pushed, cap);
        assert_eq!((v.as_ptr(), v.capacity()), (ptr, cap));
    }

    /// A plot that, if processed, would start a candidate (no address, no track near).
    fn stray_plot(t_ns: i64) -> RadarPlot {
        RadarPlot {
            time_ns: t_ns,
            range_m: 50_000.0,
            az_deg: 100.0,
            alt_ft: 10_000,
            addr: NO_ADDR,
            label: NO_ADDR,
            mode_a: NONE_CODE,
        }
    }

    // REQ: OA-TRK-088
    #[test]
    fn an_empty_advance_closes_its_buckets_so_a_plot_in_them_is_late() {
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 16);
        fusion_advance(&mut f, 10 * NS_PER_S, &mut out);
        fusion_plots(&mut f, 0, &[stray_plot(9_500_000_000)]);
        fusion_advance(&mut f, 11 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.late, 1, "{st:?}");
        assert_eq!(st.processed, 0);
        assert_eq!(st.cand_started, 0);
    }

    // REQ: OA-TRK-088
    #[test]
    fn an_empty_advance_closes_its_buckets_on_a_tracker_with_active_tracks() {
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 18);
        for k in 0..20i64 {
            fusion_adsb(&mut f, &adsb_fix(0xA00001, k * NS_PER_S));
            fusion_advance(&mut f, k * NS_PER_S, &mut out);
        }
        fusion_advance(&mut f, 21 * NS_PER_S, &mut out);
        assert_eq!(tracker_live(fusion_tracker(&f)).len(), 1);
        let before = *fusion_stats(&f);
        fusion_advance(&mut f, 30 * NS_PER_S, &mut out);
        fusion_plots(&mut f, 0, &[stray_plot(29_500_000_000)]);
        fusion_advance(&mut f, 31 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.late, before.late + 1, "{st:?}");
        assert_eq!(st.processed, before.processed);
        assert_eq!(st.cand_started, before.cand_started);
    }

    // REQ: OA-TRK-071
    #[test]
    fn association_runs_in_buffers_allocated_at_start() {
        // A crossing (ambiguity, holds and forced resolution), two close aircraft, a Mode S
        // aircraft with an extra plot: every association path. Nothing is reallocated.
        let lines = easting(&[
            Line {
                x0: 30_000.0,
                y0: 40_000.0,
                vx: -200.0,
                vy: 0.0,
                alt_ft: 25_000,
                label: 0xA00001,
            },
            Line {
                x0: 10_000.0 + 100.0 * 100.0,
                y0: 40_150.0 - 100.0 * 173.2,
                vx: -100.0,
                vy: 173.2,
                alt_ft: 25_000,
                label: 0xA00002,
            },
            Line {
                x0: 20_000.0,
                y0: -30_000.0,
                vx: 100.0,
                vy: 20.0,
                alt_ft: 18_000,
                label: 0xA00003,
            },
            Line {
                x0: 20_000.0,
                y0: -29_920.0,
                vx: 100.0,
                vy: 20.0,
                alt_ft: 18_000,
                label: 0xA00004,
            },
        ]);
        let plots = plots_of(&lines, 40, 21, false);
        let mut f = make_fusion(|_| {});
        let before = fusion_assoc_buffers(&f);
        run(&mut f, &plots, &[]);
        let st = fusion_stats(&f);
        assert!(st.held > 0 && st.gnn.solved > 0, "{st:?}");
        assert_eq!(fusion_assoc_buffers(&f), before);
        assert_eq!(st.assoc_refused, 0);
    }

    // REQ: OA-TRK-073
    #[test]
    fn a_bucket_with_more_plots_than_the_limit_is_cut_and_counted() {
        // 16,584 anonymous plots of one radar inside one second: the limit is 16,384 a radar a
        // bucket; the rest are counted and dropped, and the run finishes.
        let mut rng = Rng::from_seed(8);
        let n = MAX_BUCKET_PLOTS + 200;
        let plots: Vec<RadarPlot> = (0..n)
            .map(|i| RadarPlot {
                time_ns: 10_000_000_000 + (i as i64) * 1_000,
                range_m: 20_000.0 + 80_000.0 * rng.unit(),
                az_deg: 360.0 * rng.unit(),
                alt_ft: 10_000,
                addr: NO_ADDR,
                label: NO_ADDR,
                mode_a: NONE_CODE,
            })
            .collect();
        let mut f = make_fusion(|_| {});
        let t0 = std::time::Instant::now();
        run(&mut f, &plots, &[]);
        let st = fusion_stats(&f);
        assert_eq!(st.bucket_plot_overflow, 200, "{st:?}");
        assert_eq!(st.plots_in, n as u64);
        assert!(t0.elapsed().as_secs_f64() < 30.0, "{:?}", t0.elapsed());
    }

    /// A stationary ADS-B fix 20 km north of the radar, NACp 9, with a barometric altitude.
    fn adsb_fix(addr: u32, t_ns: i64) -> Measurement {
        Measurement {
            toa_ns: t_ns,
            pos_cov_m2: [144.0, 0.0, 144.0],
            vel_cov_m2s2: [1.0, 0.0, 1.0],
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            lat_e7: 421_500_000,
            lon_e7: -879_000_000,
            alt_ft: 20_000,
            vrate_fpm: 0,
            addr,
            alt_quant_ft: 25,
            sensor: 7,
            ident: crate::types::NO_IDENT,
            flags: crate::types::M_HAS_VEL,
            category: 0,
        }
    }

    // REQ: OA-TRK-172
    #[test]
    fn a_plot_time_outside_the_trackers_limit_is_refused_and_counted_without_a_panic() {
        use crate::types::TIME_LIMIT_NS;
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 18);
        for k in 0..10i64 {
            fusion_adsb(&mut f, &adsb_fix(0xA00001, k * NS_PER_S));
            fusion_advance(&mut f, k * NS_PER_S, &mut out);
        }
        let (row, _, _) = tracker_predict(fusion_tracker(&f), 0xA00001, 9 * NS_PER_S).unwrap();
        let times = [i64::MIN, i64::MAX, TIME_LIMIT_NS + 1, -TIME_LIMIT_NS - 1];
        let mut plots = Vec::new();
        for &t in &times {
            // One with the track's address (the hard path) and one anonymous, both where the
            // track is.
            for addr in [0xA00001, NO_ADDR] {
                plots.push(RadarPlot {
                    time_ns: t,
                    range_m: 20_000.0,
                    az_deg: 10.0,
                    alt_ft: 20_000,
                    addr,
                    label: 1,
                    mode_a: NONE_CODE,
                });
            }
        }
        fusion_plots(&mut f, 0, &plots);
        fusion_advance(&mut f, 12 * NS_PER_S, &mut out);
        fusion_finish(&mut f, 12 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.plots_in, 8);
        assert_eq!(st.plots_refused, 8, "{st:?}");
        // The limit itself is a time the tracker takes, so a plot there is not refused.
        let at = RadarPlot {
            time_ns: TIME_LIMIT_NS,
            ..plots[0]
        };
        assert!(plot_check(&at).is_ok());
        assert!(plot_check(&RadarPlot {
            time_ns: -TIME_LIMIT_NS,
            ..at
        })
        .is_ok());
        // The cost takes a time difference that would overflow if it were not saturating.
        let (g, bias) = (radar_geom(&SITE), NO_BIAS);
        let ok = RadarPlot {
            time_ns: 9 * NS_PER_S,
            ..plots[0]
        };
        let c = plot_convert(&g, &SITE, &ok, &bias, None, 0xA00001).unwrap();
        for (t, last) in [(i64::MIN, i64::MAX), (i64::MAX, i64::MIN), (i64::MIN, 5)] {
            let p = RadarPlot {
                time_ns: t,
                ..plots[0]
            };
            let pc = pair_cost(&row, &c, &p, 100, last);
            assert!(!pc.dh.is_nan() && !pc.dv.is_nan());
        }
    }

    // REQ: OA-TRK-082
    #[test]
    fn a_quiet_fusion_still_coasts_and_drops_its_tracks() {
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 18);
        for k in 0..20i64 {
            fusion_adsb(&mut f, &adsb_fix(0xA00001, k * NS_PER_S));
            fusion_advance(&mut f, k * NS_PER_S, &mut out);
        }
        fusion_advance(&mut f, 21 * NS_PER_S, &mut out);
        assert_eq!(tracker_live(fusion_tracker(&f)).len(), 1);
        crate::types::output_clear(&mut out);
        // Nothing offered from here: one call a second for well past the drop time.
        let drop_s = crate::tracker::tracker_config(fusion_tracker(&f)).drop_after_ns / NS_PER_S;
        for s in 22..(22 + drop_s + 10) {
            fusion_advance(&mut f, s * NS_PER_S, &mut out);
        }
        let coasting = out
            .flags
            .iter()
            .filter(|x| **x & crate::types::O_COAST != 0)
            .count();
        assert!(coasting > 0, "no coasting rows in {} rows", out.t_ns.len());
        assert!(out.t_ns.len() > 10, "{} rows", out.t_ns.len());
        assert!(tracker_live(fusion_tracker(&f)).is_empty(), "still live");
        assert_eq!(
            fusion_tracker_stats(&f).ended[usize::from(crate::types::END_DROPPED)],
            1
        );
    }

    // REQ: OA-TRK-088
    #[test]
    fn a_completed_bucket_is_not_reopened_by_a_later_call() {
        let lines = easting(&[Line {
            x0: 30_000.0,
            y0: 20_000.0,
            vx: -150.0,
            vy: 40.0,
            alt_ft: 20_000,
            label: 0xA00001,
        }]);
        let plots = plots_of(&lines, 4, 3, true);
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 16);
        let p = plots[1];
        fusion_plots(&mut f, 0, &[p]);
        fusion_advance(&mut f, p.time_ns + 2 * NS_PER_S, &mut out);
        assert_eq!(fusion_stats(&f).processed, 1);
        // Another plot in the same second, offered after that second was completed.
        let q = RadarPlot {
            time_ns: p.time_ns + 1,
            range_m: p.range_m + 40.0,
            ..p
        };
        fusion_plots(&mut f, 0, &[q]);
        fusion_advance(&mut f, p.time_ns + 4 * NS_PER_S, &mut out);
        assert_eq!(fusion_stats(&f).processed, 1, "{:?}", fusion_stats(&f));
        assert_eq!(fusion_stats(&f).late, 1);
    }

    // REQ: OA-TRK-087
    #[test]
    fn a_refused_or_repeated_adsb_report_does_not_count_toward_registration() {
        let mut f = make_fusion(|_| {});
        let mut out = output_new(1 << 16);
        let a = 0xA00005;
        let bad = Measurement {
            pos_cov_m2: [-5.0, 0.0, -5.0],
            ..adsb_fix(a, 3 * NS_PER_S)
        };
        for m in [
            adsb_fix(a, NS_PER_S),
            adsb_fix(a, 2 * NS_PER_S),
            adsb_fix(a, 2 * NS_PER_S),
            bad,
        ] {
            fusion_adsb(&mut f, &m);
        }
        fusion_advance(&mut f, 10 * NS_PER_S, &mut out);
        assert_eq!(f.keyinfo.get(&a).map(|k| k.adsb_n), Some(2));
        assert_eq!(
            f.keyinfo.get(&a).map(|k| k.last_adsb_ns),
            Some(2 * NS_PER_S)
        );
        let s = fusion_tracker_stats(&f);
        assert_eq!((s.applied, s.duplicates), (2, 1));
    }

    // REQ: OA-TRK-084
    #[test]
    fn bookkeeping_stays_within_the_trackers_capacity_over_many_turnovers() {
        let mut f = make_fusion_capacity(2);
        let mut out = output_new(1 << 18);
        let drop_s = crate::tracker::tracker_config(fusion_tracker(&f)).drop_after_ns / NS_PER_S;
        let mut t = 0i64;
        let mut worst = 0usize;
        for i in 0..150u32 {
            for k in 0..3i64 {
                fusion_adsb(&mut f, &adsb_fix(0xB00000 + i, (t + k) * NS_PER_S));
            }
            t += drop_s + 20;
            fusion_advance(&mut f, t * NS_PER_S, &mut out);
            crate::types::output_clear(&mut out);
            worst = worst.max(fusion_bookkeeping_len(&f).0);
        }
        assert!(worst <= 2, "keys held: {worst}");
        assert_eq!(fusion_tracker_stats(&f).started, 150);
        // Addresses past the tracker's capacity are refused by it and not remembered.
        let mut f = make_fusion_capacity(2);
        for i in 0..50u32 {
            fusion_adsb(&mut f, &adsb_fix(0xC00000 + i, i64::from(i % 3) * NS_PER_S));
        }
        fusion_advance(&mut f, 10 * NS_PER_S, &mut out);
        assert!(
            fusion_bookkeeping_len(&f).0 <= 2,
            "{:?}",
            fusion_bookkeeping_len(&f)
        );
    }

    // REQ: OA-TRK-084
    #[test]
    fn the_radar_track_records_are_held_to_a_limit_and_can_be_drained() {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(Line {
                x0: 20_000.0 + 15_000.0 * f64::from(i),
                y0: 20_000.0,
                vx: -100.0,
                vy: 30.0,
                alt_ft: 10_000 + 3_000 * i,
                label: 0xE00000 + i as u32,
            });
        }
        let plots = plots_of(&lines, 12, 4, false);
        let mut f = make_fusion(|_| {});
        f.track_rec_max = 3;
        run(&mut f, &plots, &[]);
        assert_eq!(fusion_radar_tracks(&f).len(), 3);
        assert_eq!(
            fusion_stats(&f).track_rec_overflow,
            3,
            "{:?}",
            fusion_stats(&f)
        );
        assert_eq!(fusion_take_radar_tracks(&mut f).len(), 3);
        assert!(fusion_radar_tracks(&f).is_empty());
    }

    /// A fusion whose tracker holds `n` tracks.
    fn make_fusion_capacity(n: usize) -> Fusion {
        let tc = fusion_tracker_config(
            &TrackerConfig {
                out_capacity: 1 << 18,
                max_tracks: n,
                ..DEFAULT_CONFIG
            },
            &[SITE],
        );
        let mut c = fusion_config(tc);
        c.registration = false;
        c.log_assign = true;
        fusion_new(&c, &[SITE]).unwrap()
    }

    // REQ: OA-TRK-085
    #[test]
    fn every_live_track_up_to_the_configured_capacity_takes_part_in_association() {
        let n = 8_193usize;
        let mut f = make_fusion_capacity(n);
        let mut out = output_new(1 << 20);
        // A grid of stationary aircraft about a kilometre apart, 2,000 ft apart in altitude from
        // each neighbour, so that a plot belongs to one of them only.
        let alt = |i: usize| 2_000 + 2_000 * ((i + i / 90) % 15) as i32;
        let at = |i: usize| {
            (
                421_000_000 + (i / 90) as i32 * 10_000,
                -880_000_000 + (i % 90) as i32 * 14_000,
            )
        };
        for i in 0..n {
            let (la, lo) = at(i);
            for k in 0..3i64 {
                fusion_adsb(
                    &mut f,
                    &Measurement {
                        lat_e7: la,
                        lon_e7: lo,
                        alt_ft: alt(i),
                        ..adsb_fix(0xD00000 + i as u32, k * NS_PER_S)
                    },
                );
            }
            if i % 512 == 0 {
                fusion_advance(&mut f, 0, &mut out);
                crate::types::output_clear(&mut out);
            }
        }
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        crate::types::output_clear(&mut out);
        assert_eq!(tracker_live(fusion_tracker(&f)).len(), n);
        // Two plots at the last track's place: one with its address, one anonymous.
        let g = radar_geom(&SITE);
        let (la, lo) = at(n - 1);
        let (rho, az) = polar_of(
            &g,
            f64::from(la) / 1.0e7,
            f64::from(lo) / 1.0e7,
            f64::from(alt(n - 1)) * FT_M,
        );
        let plot = RadarPlot {
            time_ns: 6 * NS_PER_S,
            range_m: rho,
            az_deg: az,
            alt_ft: alt(n - 1),
            addr: NO_ADDR,
            label: 1,
            mode_a: NONE_CODE,
        };
        let hard = RadarPlot {
            time_ns: 10 * NS_PER_S,
            addr: 0xD00000 + (n as u32 - 1),
            ..plot
        };
        fusion_plots(&mut f, 0, &[plot, hard]);
        fusion_advance(&mut f, 12 * NS_PER_S, &mut out);
        let st = fusion_stats(&f);
        assert_eq!(st.hard_no_track, 0, "{st:?}");
        assert_eq!(st.applied_hard, 1, "{st:?}");
        assert_eq!(st.applied_assoc, 1, "{st:?}");
        // The anonymous plot went to the last track, not to a neighbour.
        let log = fusion_assign_log(&f);
        let i = log.how.iter().position(|h| *h == HOW_ASSOC).unwrap();
        assert_eq!(log.key[i], 0xD00000 + (n as u32 - 1), "{:x}", log.key[i]);
        // A tracker bigger than the association can hold is refused at the start.
        let tc = fusion_tracker_config(
            &TrackerConfig {
                max_tracks: MAX_ASSOC_TRACKS + 1,
                ..DEFAULT_CONFIG
            },
            &[SITE],
        );
        assert!(matches!(
            fusion_new(&fusion_config(tc), &[SITE]),
            Err(FusionError::BadConfig(_))
        ));
    }

    // REQ: OA-TRK-054
    #[test]
    fn invalid_configuration_and_inputs_are_refused() {
        let tc = fusion_tracker_config(&DEFAULT_CONFIG, &[SITE]);
        assert!(matches!(
            fusion_new(&fusion_config(tc), &[]),
            Err(FusionError::BadConfig(_))
        ));
        let bad_site = RadarSite {
            sigma_range_m: -1.0,
            ..SITE
        };
        assert!(matches!(
            fusion_new(&fusion_config(tc), &[bad_site]),
            Err(FusionError::BadSite(0, _))
        ));
        let mut c = fusion_config(tc);
        c.init_m = 5;
        assert!(fusion_new(&c, &[SITE]).is_err());
        let mut c = fusion_config(tc);
        c.bucket_ns = 0;
        assert!(fusion_new(&c, &[SITE]).is_err());
        let mut f = make_fusion(|_| {});
        let bad = RadarPlot {
            time_ns: 0,
            range_m: f64::NAN,
            az_deg: 0.0,
            alt_ft: 100,
            addr: NO_ADDR,
            label: NO_ADDR,
            mode_a: NONE_CODE,
        };
        fusion_plots(&mut f, 0, &[bad]);
        fusion_plots(&mut f, 3, &[bad]);
        assert_eq!(fusion_stats(&f).plots_refused, 1);
        assert_eq!(fusion_stats(&f).plots_in, 1);
    }

    /// A second terminal radar 40 km east of `SITE`, on the same scan clock.
    const SITE_EAST: RadarSite = RadarSite {
        lon_deg: -87.908_149_72 + 0.48,
        sensor: 2,
        ..SITE
    };

    /// Feeds the plots of radar 0 and radar 1 in time order, advancing to each plot.
    fn run_two(f: &mut Fusion, a: &[RadarPlot], b: &[RadarPlot]) -> TrackOutput {
        run_two_for(f, a, b, 0)
    }

    /// The same, then time passes `tail_s` seconds more with no plot.
    fn run_two_for(f: &mut Fusion, a: &[RadarPlot], b: &[RadarPlot], tail_s: i64) -> TrackOutput {
        let mut out = output_new(1 << 18);
        let mut all: Vec<(i64, usize, RadarPlot)> = a
            .iter()
            .map(|p| (p.time_ns, 0, *p))
            .chain(b.iter().map(|p| (p.time_ns, 1, *p)))
            .collect();
        all.sort_by_key(|x| (x.0, x.1));
        for (t, r, p) in &all {
            fusion_plots(f, *r, &[*p]);
            fusion_advance(f, *t, &mut out);
        }
        let end = all.last().map_or(0, |x| x.0);
        if tail_s > 0 {
            fusion_advance(f, end + tail_s * NS_PER_S, &mut out);
        }
        fusion_finish(f, end + NS_PER_S, &mut out);
        out
    }

    // REQ: OA-TRK-298
    #[test]
    fn two_radars_starting_cold_on_one_radar_only_aircraft_make_one_track() {
        // Both radars build a candidate from their first plots and both reach their third hit in
        // the same bucket: one track results, the other candidate is merged into it.
        let lines = easting(&[Line {
            x0: 20_000.0,
            y0: 25_000.0,
            vx: -140.0,
            vy: 30.0,
            alt_ft: 22_000,
            label: 0xA00001,
        }]);
        let pa = plots_of_site(&SITE, &lines, 14, 3, false);
        let pb = plots_of_site(&SITE_EAST, &lines, 14, 4, false);
        let mut f = make_fusion_sites(&[SITE, SITE_EAST], 64);
        let out = run_two(&mut f, &pa, &pb);
        let st = *fusion_stats(&f);
        assert!(st.cand_started >= 2, "{st:?}");
        assert_eq!(
            fusion_radar_tracks(&f).len(),
            1,
            "{st:?} {:?}",
            fusion_radar_tracks(&f)
        );
        assert_eq!(st.cand_confirmed, 1, "{st:?}");
        assert_eq!(
            st.cand_merged_on_confirm + st.cand_cancelled_by_track,
            1,
            "{st:?}"
        );
        let keys: std::collections::BTreeSet<u32> = out.addr.iter().copied().collect();
        assert_eq!(keys.len(), 1, "{keys:?}");
    }

    /// Plots over 200 stationary tracks, the bucket's work limit cut to 500 so the tail of the
    /// bucket is never gated.
    fn capped_fusion() -> (Fusion, Vec<(f64, f64)>) {
        let mut f = make_fusion_sites(&[SITE], 400);
        f.bucket_tests_max = 500;
        let mut out = output_new(1 << 18);
        let at = feed_tracks(&mut f, &SITE, 200, 20_000.0, 7, 0x100000);
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        (f, at)
    }

    /// The plots of the radar's last bucket that were left ungated.
    fn deferred(f: &Fusion) -> Vec<usize> {
        f.rscratch[0]
            .dec
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == D_DEFERRED)
            .map(|(j, _)| j)
            .collect()
    }

    // REQ: OA-TRK-299
    #[test]
    fn plots_the_work_limit_left_ungated_start_no_candidate() {
        let (mut f, at) = capped_fusion();
        let mut out = output_new(1 << 18);
        fusion_plots(&mut f, 0, &plots_on(&SITE, &at, 100, 0.0, 3));
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = *fusion_stats(&f);
        assert_eq!(st.bucket_work_capped, 1, "{st:?}");
        assert!(st.anon_plots_ungated > 0, "{st:?}");
        // Every one of those plots lies on a track: none is evidence of a new aircraft.
        assert_eq!(st.cand_started, 0, "{st:?}");
        assert_eq!(deferred(&f).len() as u64, st.anon_plots_ungated, "{st:?}");
        assert_eq!(st.unassigned, 0, "{st:?}");
    }

    // REQ: OA-TRK-299
    #[test]
    fn the_ungated_tail_is_not_the_same_plots_in_the_next_bucket() {
        let (mut f, at) = capped_fusion();
        let mut out = output_new(1 << 18);
        let plots = plots_on(&SITE, &at, 100, 0.0, 3);
        fusion_plots(&mut f, 0, &plots);
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let first = deferred(&f);
        let later: Vec<RadarPlot> = plots
            .iter()
            .map(|p| RadarPlot {
                time_ns: p.time_ns + 2 * NS_PER_S,
                ..*p
            })
            .collect();
        fusion_plots(&mut f, 0, &later);
        fusion_advance(&mut f, 9 * NS_PER_S, &mut out);
        let second = deferred(&f);
        assert!(!first.is_empty() && !second.is_empty());
        assert_ne!(first, second);
        // The plots that were gated are other plots too.
        let gated = |d: &Vec<usize>| {
            (0..plots.len())
                .filter(|j| !d.contains(j))
                .collect::<Vec<_>>()
        };
        assert_ne!(gated(&first), gated(&second));
    }

    // REQ: OA-TRK-299
    #[test]
    fn where_the_loop_starts_changes_nothing_when_nothing_is_cut() {
        // The same plots in buckets 5, 6, 7 and 8 (so with other starting points): the same
        // keys take the same plots.
        let run_at = |shift_s: i64| {
            let mut f = make_fusion_sites(&[SITE], 400);
            let mut out = output_new(1 << 18);
            let at = feed_tracks_at(
                &mut f,
                &SITE,
                200,
                20_000.0,
                7,
                0x100000,
                shift_s * NS_PER_S,
            );
            fusion_advance(&mut f, (4 + shift_s) * NS_PER_S, &mut out);
            let mut plots = plots_on(&SITE, &at, 150, 150.0, 3);
            for p in &mut plots {
                p.time_ns += shift_s * NS_PER_S;
            }
            fusion_plots(&mut f, 0, &plots);
            fusion_advance(&mut f, (8 + shift_s) * NS_PER_S, &mut out);
            let st = *fusion_stats(&f);
            assert_eq!(st.bucket_work_capped, 0, "{st:?}");
            let log = fusion_assign_log(&f);
            let v: Vec<(u32, u32)> = log
                .key
                .iter()
                .copied()
                .zip(log.label.iter().copied())
                .collect();
            (v, st.pairs_gated, st.held, st.unassigned)
        };
        let base = run_at(0);
        assert!(base.1 > 100, "{base:?}");
        for shift in 1..4 {
            let r = run_at(shift);
            assert_eq!(r.0.len(), base.0.len(), "shift {shift}");
            let d: Vec<_> =
                r.0.iter()
                    .zip(&base.0)
                    .filter(|(x, y)| x != y)
                    .take(5)
                    .collect();
            assert_eq!(
                (&r.1, &r.2, &r.3),
                (&base.1, &base.2, &base.3),
                "shift {shift} {d:?}"
            );
            assert!(d.is_empty(), "shift {shift} {d:?}");
        }
    }

    /// `n` stationary tracks over `side_m` round `site` and one more for each of `fat_sigmas_m`,
    /// reported with that standard deviation (from `addr0`); the places of all of them.
    fn feed_with_fat_tracks(
        f: &mut Fusion,
        site: &RadarSite,
        side_m: f64,
        n: usize,
        fat_sigmas_m: &[f64],
        seed: u64,
        addr0: u32,
    ) -> Vec<(f64, f64)> {
        let mut at = feed_tracks(f, site, n, side_m, seed, addr0);
        let frame = oa_geo::aeqd::frame_new(site.lat_deg, site.lon_deg);
        let mut rng = Rng::from_seed(seed + 1_000);
        for (i, &sigma) in fat_sigmas_m.iter().enumerate() {
            let (lat, lon) = oa_geo::aeqd::unproject(
                &frame,
                side_m * (rng.unit() - 0.5),
                side_m * (rng.unit() - 0.5),
            )
            .unwrap();
            for k in 0..4i64 {
                let mut m = adsb_fix(addr0 + 0x8000 + i as u32, k * NS_PER_S);
                m.lat_e7 = (lat * 1.0e7).round() as i32;
                m.lon_e7 = (lon * 1.0e7).round() as i32;
                let var = (sigma * sigma) as f32;
                m.pos_cov_m2 = [var, 0.0, var];
                fusion_adsb(f, &m);
            }
            at.push((lat, lon));
        }
        at
    }

    // REQ: OA-TRK-300
    #[test]
    fn one_uncertain_track_does_not_widen_the_search_for_every_plot() {
        let mut f = make_fusion_sites(&[SITE], 1_000);
        let mut out = output_new(1 << 18);
        let at = feed_with_fat_tracks(&mut f, &SITE, 300_000.0, 600, &[20_000.0], 11, 0x100000);
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        fusion_plots(&mut f, 0, &plots_on(&SITE, &at, 600, 100.0, 3));
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = *fusion_stats(&f);
        assert!(st.coarse_width_max_m > 50_000, "{st:?}");
        // Every plot used to take the whole table (about 600 candidates) from the one wide
        // window; now it visits the few tracks near it and the one wide track.
        assert!(st.coarse_visits < 600 * 100, "{st:?}");
        assert!(st.pairs_gated > 550, "{st:?}");
        assert_eq!(st.wide_full, 0, "{st:?}");
    }

    // REQ: OA-TRK-300
    #[test]
    fn the_wide_list_and_the_standard_grid_give_the_associations_of_testing_every_track() {
        // Three radars (the seam, the pole's limit, ordinary); each with 300 tracks and 24 of
        // standard deviations from 100 m to 30 km, plots a few hundred metres off all of them (some
        // with addresses) and random ones; the same outcome as testing every track, whatever the
        // seed.
        let site = |lat_deg: f64, lon_deg: f64, sensor: u16| RadarSite {
            lat_deg,
            lon_deg,
            sensor,
            ..SITE
        };
        let sites = [
            site(0.0, 179.95, 1),
            site(85.0, 20.0, 2),
            site(41.98, -87.91, 3),
        ];
        for seed in 0..3u64 {
            let run_once = |brute: bool| {
                let mut f = make_fusion_sites(&sites, 2_000);
                f.brute_coarse = brute;
                let mut out = output_new(1 << 18);
                let mut rng = Rng::from_seed(100 + seed);
                let mut places = Vec::new();
                for (r, s) in sites.iter().enumerate() {
                    let sigmas: Vec<f64> = (0..24)
                        .map(|_| 100.0 * (300.0f64).powf(rng.unit()))
                        .collect();
                    places.push(feed_with_fat_tracks(
                        &mut f,
                        s,
                        300_000.0,
                        300,
                        &sigmas,
                        40 + 10 * seed + r as u64,
                        0x100000 * (r as u32 + 1),
                    ));
                }
                fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
                for (r, s) in sites.iter().enumerate() {
                    let n = places[r].len();
                    let mut plots = plots_on(s, &places[r], n, 300.0, 50 + r as u64);
                    for (i, p) in plots.iter_mut().enumerate() {
                        if i % 7 == 0 {
                            p.addr = 0x100000 * (r as u32 + 1) + i as u32;
                        }
                    }
                    for k in 0..150i64 {
                        plots.push(RadarPlot {
                            time_ns: 5 * NS_PER_S + 7_000_000 + k * 10_000,
                            range_m: 1_000.0 + 400_000.0 * rng.unit(),
                            az_deg: 360.0 * rng.unit(),
                            alt_ft: 20_000,
                            addr: NO_ADDR,
                            label: 9_000 + k as u32,
                            mode_a: NONE_CODE,
                        });
                    }
                    fusion_plots(&mut f, r, &plots);
                }
                fusion_advance(&mut f, 12 * NS_PER_S, &mut out);
                fusion_finish(&mut f, 12 * NS_PER_S, &mut out);
                (outcome(&f, &out), fusion_stats(&f).coarse_visits)
            };
            let (grid, visits) = run_once(false);
            let (brute, none) = run_once(true);
            assert_eq!(none, 0);
            assert!(visits > 0);
            assert!(grid.0.wide_tracks_sum > 20, "seed {seed}: {:?}", grid.0);
            assert!(grid.0.pairs_gated > 300, "seed {seed}: {:?}", grid.0);
            assert_eq!(grid.0, brute.0, "seed {seed}");
            assert_eq!(grid.1, brute.1, "seed {seed}");
            assert_eq!(grid.2, brute.2, "seed {seed}");
        }
    }

    // REQ: OA-TRK-300
    #[test]
    fn a_full_wide_list_leaves_the_track_out_and_counts_it() {
        let mut f = make_fusion_sites(&[SITE], 1_000);
        f.wide_max = 3;
        let mut out = output_new(1 << 18);
        let sigmas = [10_000.0; 8];
        let at = feed_with_fat_tracks(&mut f, &SITE, 100_000.0, 50, &sigmas, 11, 0x100000);
        fusion_advance(&mut f, 4 * NS_PER_S, &mut out);
        fusion_plots(&mut f, 0, &plots_on(&SITE, &at, at.len(), 100.0, 3));
        fusion_advance(&mut f, 7 * NS_PER_S, &mut out);
        let st = *fusion_stats(&f);
        // Eight uncertain tracks, room for three.
        assert_eq!(st.wide_tracks_sum, 3, "{st:?}");
        assert_eq!(st.wide_full, 5, "{st:?}");
        assert_eq!(st.scratch_full, 0, "{st:?}");
    }

    /// `n` radar-only aircraft over a square of `side_m`, straight at 100 to 250 m/s.
    fn many_lines(n: usize, side_m: f64, seed: u64) -> Vec<Line> {
        let mut rng = Rng::from_seed(seed);
        (0..n)
            .map(|i| {
                let (dir, speed) = (
                    std::f64::consts::TAU * rng.unit(),
                    100.0 + 150.0 * rng.unit(),
                );
                Line {
                    x0: side_m * (rng.unit() - 0.5),
                    y0: side_m * (rng.unit() - 0.5),
                    vx: speed * dir.cos(),
                    vy: speed * dir.sin(),
                    alt_ft: 15_000 + 100 * (rng.unit() * 200.0) as i32,
                    label: 0xB00000 + i as u32,
                }
            })
            .collect()
    }

    // REQ: OA-TRK-301
    #[test]
    fn initiation_tests_the_candidates_near_a_plot_not_all_of_them() {
        let lines = many_lines(150, 200_000.0, 21);
        let plots = plots_of(&lines, 10, 5, false);
        let mut f = make_fusion_sites(&[SITE], 1_000);
        run(&mut f, &plots, &[]);
        let st = *fusion_stats(&f);
        assert!(st.cand_confirmed > 100, "{st:?}");
        // Every plot against every candidate would be 150 x 150 a scan (about 160,000 here).
        assert!(st.init_tests < 2_000, "{st:?}");
    }

    // REQ: OA-TRK-301
    #[test]
    fn the_candidate_grid_gives_the_initiations_of_testing_every_candidate() {
        // Two radars, 120 aircraft crossing the same sky (a few per second a scan on each), some
        // plots without Mode C, some clutter, some aircraft that vanish: the tracks made and the
        // plots they took are the same as when every plot is tested against every candidate.
        for seed in 0..4u64 {
            let run_once = |brute: bool| {
                let lines = many_lines(120, 160_000.0, 30 + seed);
                let mut pa = plots_of_site(&SITE, &lines, 9, 40 + seed, false);
                let mut pb = plots_of_site(&SITE_EAST, &lines, 9, 50 + seed, false);
                let mut rng = Rng::from_seed(60 + seed);
                for (k, p) in pa.iter_mut().chain(pb.iter_mut()).enumerate() {
                    if k % 11 == 0 {
                        p.alt_ft = NONE_ALT;
                    }
                }
                for k in 0..60i64 {
                    for (r, v) in [&mut pa, &mut pb].into_iter().enumerate() {
                        v.push(RadarPlot {
                            time_ns: k * 700_000_000 + r as i64 * 3_000_000,
                            range_m: 3_000.0 + 150_000.0 * rng.unit(),
                            az_deg: 360.0 * rng.unit(),
                            alt_ft: 10_000 + 100 * (rng.unit() * 250.0) as i32,
                            addr: NO_ADDR,
                            label: 9_000 + k as u32,
                            mode_a: NONE_CODE,
                        });
                    }
                }
                // Aircraft that vanish: the plots of the last scans of some are dropped.
                pa.retain(|p| !(p.label % 5 == 0 && p.time_ns > 20_000_000_000));
                pa.sort_by_key(|p| p.time_ns);
                pb.sort_by_key(|p| p.time_ns);
                let mut f = make_fusion_sites(&[SITE, SITE_EAST], 1_000);
                f.brute_coarse = brute;
                let out = run_two(&mut f, &pa, &pb);
                (outcome(&f, &out), fusion_stats(&f).init_tests)
            };
            let (grid, grid_tests) = run_once(false);
            let (brute, brute_tests) = run_once(true);
            assert!(grid.0.cand_confirmed > 60, "seed {seed}: {:?}", grid.0);
            assert!(grid.0.cand_joined > 100, "seed {seed}: {:?}", grid.0);
            assert_eq!(grid.0, brute.0, "seed {seed}");
            assert_eq!(grid.1, brute.1, "seed {seed}");
            assert_eq!(grid.2, brute.2, "seed {seed}");
            assert!(
                grid_tests * 3 < brute_tests,
                "seed {seed}: {grid_tests} {brute_tests}"
            );
        }
    }

    /// Seconds from `last` (the newest update, ns) to the first coasting row of `key`.
    fn first_coast_after(out: &TrackOutput, key: u32, last: i64) -> Option<f64> {
        (0..out.t_ns.len())
            .filter(|&i| out.addr[i] == key && out.flags[i] & crate::types::O_COAST != 0)
            .map(|i| out.t_ns[i])
            .filter(|&t| t > last)
            .min()
            .map(|t| (t - last) as f64 / 1.0e9)
    }

    // REQ: OA-TRK-302
    #[test]
    fn a_track_coasts_after_three_of_the_intervals_of_the_radars_that_update_it() {
        // Two radars at one place: a terminal one (4.8 s) and an en-route one (12 s). One aircraft
        // is seen only by the terminal radar, one only by the en-route radar; the plots stop and
        // time passes. The terminal track coasts after 3 x 4.8 s, the en-route one after 3 x 12 s.
        let slow = RadarSite {
            period_ns: 12_000_000_000,
            sensor: 2,
            ..SITE
        };
        let la = easting(&[Line {
            x0: 20_000.0,
            y0: 25_000.0,
            vx: -120.0,
            vy: 20.0,
            alt_ft: 22_000,
            label: 0xA00001,
        }]);
        let lb = easting(&[Line {
            x0: -30_000.0,
            y0: -20_000.0,
            vx: 100.0,
            vy: 40.0,
            alt_ft: 31_000,
            label: 0xA00002,
        }]);
        let pa = plots_of_site(&SITE, &la, 8, 3, false);
        let pb = plots_of_site(&slow, &lb, 5, 4, false);
        let mut f = make_fusion_sites(&[SITE, slow], 64);
        let out = run_two_for(&mut f, &pa, &pb, 80);
        let recs: Vec<_> = fusion_radar_tracks(&f).to_vec();
        assert_eq!(recs.len(), 2, "{:?}", fusion_stats(&f));
        let last = |r: u8| {
            let k = recs.iter().find(|x| x.radar == r).unwrap().key;
            let t = (0..out.t_ns.len())
                .filter(|&i| out.addr[i] == k && out.flags[i] & crate::types::O_TICK == 0)
                .map(|i| out.t_ns[i])
                .max()
                .unwrap();
            (k, t)
        };
        let (ka, ta) = last(0);
        let (kb, tb) = last(1);
        let ca = first_coast_after(&out, ka, ta).unwrap();
        let cb = first_coast_after(&out, kb, tb).unwrap();
        assert!(
            (14.4..15.5).contains(&ca),
            "terminal track coasted after {ca} s"
        );
        assert!(
            (36.0..37.5).contains(&cb),
            "en-route track coasted after {cb} s"
        );
    }
}
