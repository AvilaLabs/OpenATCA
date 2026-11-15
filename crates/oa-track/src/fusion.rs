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
//!    current registration correction, gated against every live track by a coarse window and then
//!    by the exact Mahalanobis distance (horizontal against the track's predicted position and
//!    covariance, vertical against its altitude), and the gated pairs are assigned by global
//!    nearest neighbour (`assoc.rs`). **A Mode A code never takes part**: it is copied to the
//!    measurement and read by nothing. A plot whose assigned track has a competitor within
//!    [`AMBIGUITY_MARGIN`] is **held**, not applied: kinematics alone never decides between two
//!    close tracks, and a track coasts through a crossing. A track whose last [`HOLD_MAX_RUN`]
//!    plots were all held takes the next one on the assignment's own evidence (counted in
//!    [`FusionStats::held_forced`]): two aircraft that stay together would otherwise never
//!    update their tracks, which would coast off while every plot started tracks beside them.
//!    A Mode S plot is applied to the track of its address only inside a wide gate
//!    ([`HARD_GATE_CHI2`]; a gross mismatch is counted, not applied), and **a track takes at
//!    most one plot per radar and bucket**: of several Mode S plots of one address the nearest,
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
//! The per-key bookkeeping (a `BTreeMap`, a fixed order) and the candidate list are small
//! against the tracker's tables and live in ordinary vectors; the tracker itself allocates
//! nothing after construction.

use std::collections::BTreeMap;

use oa_geo::aeqd::{frame_new, project};

use crate::assoc::{
    ambiguous_pairs, assigned_pairs, assoc_ambiguous, assoc_pair_of, assoc_plot_track,
    assoc_scratch_buffers, assoc_scratch_new, gnn, nearest, AssocScratch, GnnStats, Pair,
    NONE as NO_TRACK,
};
use crate::fusion_params::{
    AMBIGUITY_MARGIN, ASSIGN_LOG_MAX, BUCKET_NS, CAND_ALT_SLACK_FT, CAND_MAX_ACCEL_MPS2,
    CAND_MAX_SPEED_MPS, CAND_MAX_VRATE_FPS, CAND_SLACK_M, COARSE_M, GATE_H_CHI2,
    GATE_MANEUVER_MAX_DT_S, GATE_MANEUVER_MPS2, GATE_TOTAL_CHI2, GATE_VRATE_SIGMA_FPM, GATE_V_CHI2,
    HARD_GATE_CHI2, HOLD_MAX_RUN, KEY_RADAR_BASE, MAX_ASSOC_TRACKS, MAX_BUCKET_PAIRS,
    MAX_BUCKET_PLOTS, MAX_BUCKET_TESTS, MAX_CANDIDATES, MAX_PENDING, MAX_RADARS, MAX_SOLVE_TRACKS,
    MAX_TRACK_RECS, RADAR_INIT_M, RADAR_INIT_N, REG_HISTORY_MAX, REG_MAX_ADSB_AGE_NS,
    REG_MAX_TRACK_SIGMA_M, REG_MIN_ADSB_UPDATES, REG_MIN_NACP, REG_MIN_RANGE_M, REG_PUBLISH_NS,
    RESCUED_COST_FRACTION,
};
use crate::params::{ALT_BASE_SIGMA_FT, NS_PER_S, OOO_WINDOW_NS};
use crate::radar::{
    plot_check, plot_convert, polar_of, radar_geom, site_check, Converted, RadarError, RadarGeom,
    RadarPlot, RadarSite, FT_M, NONE_ALT, NO_BIAS,
};
use crate::registration::{
    reg_bias, reg_new, reg_offer, reg_predict, reg_residual, reg_sample, RegSample, RegState,
    Residual,
};
use crate::tracker::{
    tracker_advance, tracker_flush, tracker_live, tracker_new, tracker_predict, tracker_stats,
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
const D_APPLY: u8 = 1;
const D_HELD: u8 = 2;
const D_REFUSED: u8 = 3;
/// A Mode S plot not applied: outside the wide gate of its address's track, or a second plot of
/// that track in the bucket. Kept out of initiation like a refused plot.
const D_MISMATCH: u8 = 4;

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
        log_assign: false,
    }
}

/// Rows of the replay scratch table: a candidate's plots span at most `RADAR_INIT_N` scans of
/// at most 12 s, one tick row per second plus the update rows.
pub const SCRATCH_ROWS: usize = 4096;

/// Scans without any update after which a track is dropped, as a multiple of the longest scan
/// period.
pub const DROP_PERIODS: i64 = 6;

/// A tracker configuration for a fusion run: `base` with the expected and initiation intervals
/// set to the longest scan period of `sites`, the drop time [`DROP_PERIODS`] such periods and
/// the out-of-order window long enough to replay a candidate's plots (the initiation window of
/// `RADAR_INIT_N` scans plus two buckets).
pub fn fusion_tracker_config(base: &TrackerConfig, sites: &[RadarSite]) -> TrackerConfig {
    let period = sites.iter().map(|s| s.period_ns).max().unwrap_or(NS_PER_S);
    TrackerConfig {
        expected_interval_ns: period,
        init_interval_ns: period,
        drop_after_ns: DROP_PERIODS * period,
        ooo_window_ns: base
            .ooo_window_ns
            .max(i64::from(RADAR_INIT_N) * period + 2 * BUCKET_NS)
            .max(OOO_WINDOW_NS),
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
    /// Hard-constraint plots left out because the track took a better plot in the bucket (a
    /// track takes at most one plot per radar bucket).
    pub hard_extra: u64,
    /// Plots applied on the assignment's own evidence because the track's last
    /// [`HOLD_MAX_RUN`] plots were all held.
    pub held_forced: u64,
    /// Plots beyond [`MAX_BUCKET_PLOTS`] of one radar in one bucket: dropped.
    pub bucket_plot_overflow: u64,
    /// Associations cut short at [`MAX_BUCKET_TESTS`] exact tests or [`MAX_BUCKET_PAIRS`] pairs.
    pub bucket_work_capped: u64,
    /// Assignments the solver refused (an invalid gate or too large an input): every plot of
    /// that solve is then unassigned.
    pub assoc_refused: u64,
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
    pend_adsb: Vec<Measurement>,
    pend_plots: Vec<PendPlot>,
    /// Per key: consecutive plots held for ambiguity since the track last took one (a small
    /// map: entries exist only while a run is going).
    hold_run: BTreeMap<u32, u8>,
    /// The earliest bucket of any pending input; `i64::MAX` with none (so a call that finds
    /// nothing ready is constant time, not a scan of a long queue).
    pend_min_bucket: i64,
    /// Every buffer of the assignment solves, allocated once.
    assoc: AssocScratch,
    /// The candidate pairs of one association and their by-products, kept between buckets so a
    /// bucket allocates only to grow past the largest before it (never past the pair limit).
    pairs_buf: Vec<Pair>,
    infos_buf: Vec<PairInfo>,
    ms_buf: Vec<Measurement>,
    /// Per address: what registration needs of its ADS-B. Holds only keys the tracker holds (cut
    /// back to them whenever it reaches the tracker's capacity).
    keyinfo: BTreeMap<u32, KeyInfo>,
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
    if cfg.bucket_ns <= 0 || cfg.bucket_ns > 10 * NS_PER_S {
        return Err(FusionError::BadConfig("bucket_ns"));
    }
    if cfg.init_m == 0 || cfg.init_n < cfg.init_m || cfg.init_n > 4 {
        return Err(FusionError::BadConfig("init_m or init_n"));
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
        reg: sites.iter().map(|_| reg_new()).collect(),
        reg_hist: sites.iter().map(|_| Vec::new()).collect(),
        reg_pub_ns: sites.iter().map(|_| i64::MIN).collect(),
        pend_adsb: Vec::new(),
        pend_plots: Vec::new(),
        hold_run: BTreeMap::new(),
        pend_min_bucket: i64::MAX,
        assoc: assoc_scratch_new(MAX_SOLVE_TRACKS, MAX_BUCKET_PLOTS, MAX_BUCKET_PAIRS),
        pairs_buf: Vec::new(),
        infos_buf: Vec::new(),
        ms_buf: Vec::new(),
        keyinfo: BTreeMap::new(),
        cands: Vec::new(),
        next_serial: 0,
        stats: FusionStats::default(),
        log: AssignLog::default(),
        tracks: Vec::new(),
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
    assoc_scratch_buffers(&f.assoc)
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
/// long-running caller does this regularly; the list is held to [`MAX_TRACK_RECS`]).
pub fn fusion_take_radar_tracks(f: &mut Fusion) -> Vec<RadarTrackRec> {
    std::mem::take(&mut f.tracks)
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
        if plot_check(p).is_err() {
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
    // Ready: buckets strictly before `complete`.
    let b = f.cfg.bucket_ns;
    let (mut ready_a, mut keep_a): (Vec<Measurement>, Vec<Measurement>) = (Vec::new(), Vec::new());
    for m in f.pend_adsb.drain(..) {
        if m.toa_ns.div_euclid(b) < complete {
            ready_a.push(m);
        } else {
            keep_a.push(m);
        }
    }
    f.pend_adsb = keep_a;
    let (mut ready_p, mut keep_p): (Vec<PendPlot>, Vec<PendPlot>) = (Vec::new(), Vec::new());
    for p in f.pend_plots.drain(..) {
        if p.plot.time_ns.div_euclid(b) < complete {
            ready_p.push(p);
        } else {
            keep_p.push(p);
        }
    }
    f.pend_plots = keep_p;
    f.pend_min_bucket = f
        .pend_adsb
        .iter()
        .map(|m| bucket_of(f, m.toa_ns))
        .chain(f.pend_plots.iter().map(|p| bucket_of(f, p.plot.time_ns)))
        .min()
        .unwrap_or(i64::MAX);
    ready_a.sort_by_key(|m| m.toa_ns);
    ready_p.sort_by_key(|p| (p.plot.time_ns, p.radar));
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
    if plot.alt_ft == NONE_ALT || row.flags & O_HAS_ALT == 0 {
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
    let dz = f64::from(plot.alt_ft) - f64::from(row.alt_ft);
    dz * dz / var
}

/// One live track at the start of the bucket, with its slant range and azimuth from the radar.
#[derive(Debug, Clone, Copy)]
struct TrackAt {
    key: u32,
    rho: f64,
    az: f64,
    ground: f64,
    sigma: f64,
}

fn tracks_at(f: &Fusion, radar: usize, at_ns: i64) -> Vec<TrackAt> {
    let Some(g) = f.geoms.get(radar) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for (key, _) in tracker_live(&f.tracker).into_iter().take(MAX_ASSOC_TRACKS) {
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
        v.push(TrackAt {
            key,
            rho,
            az,
            ground,
            sigma: c[0].max(c[2]).max(0.0).sqrt(),
        });
    }
    v
}

fn az_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// Whether a reference track may register a radar (see the module documentation).
fn reg_eligible(f: &Fusion, key: u32, row: &OutRow, plot: &RadarPlot) -> bool {
    let Some(k) = f.keyinfo.get(&key) else {
        return false;
    };
    let c = cov_f64(row.pos_cov_m2);
    k.high_integrity
        && k.adsb_n >= REG_MIN_ADSB_UPDATES
        && plot.time_ns - k.last_adsb_ns <= REG_MAX_ADSB_AGE_NS
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
    let dt_s = ((plot.time_ns - last_ns).max(0) as f64) / NS_PER_S as f64;
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
    let dv = d2_vertical(row, plot, quant_ft, plot.time_ns - last_ns);
    PairCost { dh, dh_gate, dv }
}

/// The decisions for one radar's plots of a bucket (see the module documentation, step 1).
fn associate_radar(f: &mut Fusion, radar: usize, plots: &[RadarPlot]) -> Vec<Decision> {
    let (Some(&g), Some(&s)) = (f.geoms.get(radar), f.sites.get(radar)) else {
        return Vec::new();
    };
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
    let mut dec = vec![none; plots.len()];
    let t0 = plots.first().map_or(0, |p| p.time_ns);
    if let Some(r) = f.reg.get_mut(radar) {
        reg_predict(r, t0);
    }
    let bias = if f.cfg.registration {
        f.reg.get(radar).map_or(NO_BIAS, reg_bias)
    } else {
        NO_BIAS
    };
    let tracks = tracks_at(f, radar, t0);
    // Plots per anonymous index, to map back from the solver's compact plot numbers.
    let mut anon: Vec<usize> = Vec::new();
    // Hard-constraint plots: the track with the plot's key, or none. A plot must lie inside the
    // wide gate of that track, and a track takes one plot per bucket: the nearest.
    let mut hard: Vec<(u32, f64, usize, Decision)> = Vec::new();
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
    let mut taken: Vec<u32> = Vec::new();
    for (key, _, j, d) in &hard {
        if taken.last() == Some(key) {
            f.stats.hard_extra += 1;
            if let Some(slot) = dec.get_mut(*j) {
                slot.kind = D_MISMATCH;
            }
            continue;
        }
        taken.push(*key);
        f.hold_run.remove(key);
        if let Some(slot) = dec.get_mut(*j) {
            *slot = *d;
        }
        f.stats.applied_hard += 1;
    }
    // A track that took a hard plot takes no anonymous one in the same bucket.
    let skip: Vec<bool> = tracks
        .iter()
        .map(|t| taken.binary_search(&t.key).is_ok())
        .collect();
    // Anonymous plots: converted once if they carry Mode C.
    let conv_own: Vec<Option<Converted>> = anon
        .iter()
        .map(|&j| {
            plots
                .get(j)
                .filter(|p| p.alt_ft != NONE_ALT)
                .and_then(|p| plot_convert(&g, &s, p, &bias, None, NO_ADDR).ok())
        })
        .collect();
    let mut pairs = std::mem::take(&mut f.pairs_buf);
    let mut infos = std::mem::take(&mut f.infos_buf);
    pairs.clear();
    infos.clear();
    let mut tests = 0usize;
    let mut capped = false;
    'plots: for (a, &j) in anon.iter().enumerate() {
        let Some(p) = plots.get(j) else { continue };
        let prho = p.range_m - bias.range_m;
        let paz = (p.az_deg - bias.az_deg).rem_euclid(360.0);
        for (ti, t) in tracks.iter().enumerate() {
            if skip.get(ti).copied().unwrap_or(false) {
                continue;
            }
            let coarse = COARSE_M + 8.0 * t.sigma;
            if (prho - t.rho).abs() > coarse
                || az_diff(paz, t.az).to_radians().abs() * t.ground > coarse
            {
                continue;
            }
            // A flooded bucket costs a bounded amount: past the limits the rest are not tested.
            if tests >= MAX_BUCKET_TESTS || pairs.len() >= MAX_BUCKET_PAIRS {
                capped = true;
                break 'plots;
            }
            let Some((row, _, last_ns)) = tracker_predict(&f.tracker, t.key, p.time_ns) else {
                continue;
            };
            let conv = match conv_own.get(a).copied().flatten() {
                Some(c) => c,
                None => match plot_convert(&g, &s, p, &bias, track_height(&row), t.key) {
                    Ok(c) => c,
                    Err(_) => {
                        f.stats.no_height_pairs += 1;
                        continue;
                    }
                },
            };
            tests += 1;
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
                pairs.push(Pair {
                    track: ti as u32,
                    plot: a as u32,
                    cost,
                });
                infos.push(PairInfo { m, row });
            }
        }
    }
    if capped {
        f.stats.bucket_work_capped += 1;
    }
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
            f.stats.unassigned += 1;
            continue;
        }
        let k = assoc_pair_of(&f.assoc).get(a).copied().unwrap_or(NO_TRACK);
        let Some(info) = infos.get(k as usize) else {
            continue;
        };
        let key = info.m.addr;
        if hold && assoc_ambiguous(&f.assoc).get(a).copied().unwrap_or(false) {
            let run = f.hold_run.get(&key).copied().unwrap_or(0);
            if run < HOLD_MAX_RUN {
                if f.hold_run.len() >= MAX_ASSOC_TRACKS {
                    f.hold_run.pop_first();
                }
                f.hold_run.insert(key, run + 1);
                f.stats.held += 1;
                if let Some(d) = dec.get_mut(j) {
                    d.kind = D_HELD;
                }
                continue;
            }
            // The track has been starved of plots by ambiguity: it takes this one.
            f.stats.held_forced += 1;
        }
        f.hold_run.remove(&key);
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
    f.pairs_buf = pairs;
    f.infos_buf = infos;
    dec
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
    let e = f.keyinfo.entry(m.addr).or_insert(KeyInfo {
        last_adsb_ns: i64::MIN,
        adsb_n: 0,
        high_integrity: false,
    });
    e.last_adsb_ns = m.toa_ns;
    e.adsb_n = e.adsb_n.saturating_add(1);
    e.high_integrity = sigma_ok;
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
    // Group plots by radar, keeping time order.
    let mut groups: Vec<(usize, Vec<RadarPlot>)> = Vec::new();
    for p in plots {
        let r = usize::from(p.radar);
        match groups.iter_mut().find(|(g, _)| *g == r) {
            Some((_, v)) => v.push(p.plot),
            None => groups.push((r, vec![p.plot])),
        }
    }
    groups.sort_by_key(|(r, _)| *r);
    for (_, v) in &mut groups {
        if v.len() > MAX_BUCKET_PLOTS {
            f.stats.bucket_plot_overflow += (v.len() - MAX_BUCKET_PLOTS) as u64;
            v.truncate(MAX_BUCKET_PLOTS);
        }
    }
    let decisions: Vec<(usize, Vec<Decision>)> = groups
        .iter()
        .map(|(r, v)| (*r, associate_radar(f, *r, v)))
        .collect();
    // Events in time order.
    let mut evs: Vec<Ev> = Vec::new();
    for (i, m) in adsb.iter().enumerate() {
        evs.push(Ev {
            t: m.toa_ns,
            src: 0,
            radar: 0,
            idx: i,
        });
    }
    for (gi, (r, d)) in decisions.iter().enumerate() {
        for (j, dj) in d.iter().enumerate() {
            if dj.kind == D_APPLY {
                evs.push(Ev {
                    t: dj.m.toa_ns,
                    src: 1,
                    radar: *r,
                    idx: gi * 1_000_000 + j,
                });
            }
        }
    }
    evs.sort_by_key(|e| (e.t, e.src, e.radar, e.idx));
    for e in &evs {
        if e.src == 0 {
            if let Some(m) = adsb.get(e.idx) {
                let before = tracker_stats(&f.tracker).applied;
                tracker_update(&mut f.tracker, m, out);
                // Registration trusts only what the tracker took.
                if tracker_stats(&f.tracker).applied > before {
                    note_adsb(f, m);
                }
            }
        } else {
            let (gi, j) = (e.idx / 1_000_000, e.idx % 1_000_000);
            let (Some((_, d)), Some(g)) = (decisions.get(gi), groups.get(gi)) else {
                continue;
            };
            let (Some(dj), Some(p)) = (d.get(j), g.1.get(j)) else {
                continue;
            };
            tracker_update(&mut f.tracker, &dj.m, out);
            log_assign(f, p.time_ns, dj.key, p.label, e.radar as u8, dj.how);
        }
    }
    // Registration, then initiation, per radar.
    for (gi, (r, d)) in decisions.iter().enumerate() {
        let Some((_, g)) = groups.get(gi) else {
            continue;
        };
        if f.cfg.registration {
            for (j, dj) in d.iter().enumerate() {
                if let (Some(z), Some(p)) = (dj.resid, g.get(j)) {
                    if let Some(st) = f.reg.get_mut(*r) {
                        reg_offer(st, p.time_ns, dj.key, &z);
                    }
                }
            }
            publish_registration(f, *r, g.last().map_or(0, |p| p.time_ns));
        }
        initiate(f, *r, g, d, out);
    }
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
    if *last == i64::MIN || t_ns - *last >= REG_PUBLISH_NS {
        if hist.len() < REG_HISTORY_MAX {
            hist.push(reg_sample(st, t_ns));
        }
        *last = t_ns;
    }
}

// ---- radar-only initiation ------------------------------------------------------------

fn scan_of(f: &Fusion, radar: usize, t_ns: i64) -> i64 {
    let (p, e) = f
        .sites
        .get(radar)
        .map_or((NS_PER_S, 0), |s| (s.period_ns, s.epoch_ns));
    (t_ns - e).div_euclid(p)
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
    let dt = (m.toa_ns - last.toa_ns) as f64 / 1.0e9;
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
        let dtp = (last.toa_ns - prev.toa_ns) as f64 / 1.0e9;
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
    let d = (x - pred.0).hypot(y - pred.1);
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
    let free: Vec<usize> = (0..plots.len())
        .filter(|&j| dec.get(j).is_some_and(|d| d.kind == D_UNASSIGNED))
        .collect();
    let mut joined: Vec<(usize, Measurement)> = Vec::new();
    // Address plots: join or start the candidate of that address.
    let mut anon: Vec<usize> = Vec::new();
    for &j in &free {
        let Some(p) = plots.get(j) else { continue };
        if f.cfg.use_address && p.addr != NO_ADDR {
            let Ok(c) = plot_convert(&g, &s, p, &bias, None, p.addr) else {
                f.stats.cand_no_height += 1;
                continue;
            };
            let scan = scan_of(f, radar, p.time_ns);
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
    let cand_ix: Vec<usize> = (0..f.cands.len())
        .filter(|&i| {
            f.cands
                .get(i)
                .is_some_and(|c| usize::from(c.radar) == radar && c.addr == NO_ADDR)
        })
        .collect();
    let mut pairs = std::mem::take(&mut f.pairs_buf);
    // The converted measurement of each pair, parallel to `pairs`.
    let mut ms = std::mem::take(&mut f.ms_buf);
    pairs.clear();
    ms.clear();
    let mut tests = 0usize;
    let mut capped = false;
    'plots: for (a, &j) in anon.iter().enumerate() {
        let Some(p) = plots.get(j) else { continue };
        let scan = scan_of(f, radar, p.time_ns);
        for (ci, &i) in cand_ix.iter().enumerate() {
            let Some(c) = f.cands.get(i) else {
                continue;
            };
            if tests >= MAX_BUCKET_TESTS || pairs.len() >= MAX_BUCKET_PAIRS {
                capped = true;
                break 'plots;
            }
            tests += 1;
            // The plot's height: its Mode C altitude, else the candidate's last altitude.
            let last_alt = c
                .meas
                .get(usize::from(c.n).saturating_sub(1))
                .map(|m| m.alt_ft)
                .filter(|&a| a != NONE_I32);
            let h = last_alt.map(|a| (f64::from(a) * FT_M, 15.0));
            let Ok(conv) = plot_convert(&g, &s, p, &bias, h, NO_ADDR) else {
                continue;
            };
            if let Some(cost) = cand_cost(c, &conv.m, scan) {
                pairs.push(Pair {
                    track: ci as u32,
                    plot: a as u32,
                    cost,
                });
                ms.push(conv.m);
            }
        }
    }
    if capped {
        f.stats.bucket_work_capped += 1;
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
        let scan = scan_of(f, radar, p.time_ns);
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
    joined.clear();
    // Confirm and expire.
    let now_scan = plots.first().map_or(0, |p| scan_of(f, radar, p.time_ns));
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
            confirm(f, radar, &c, out);
        } else if now_scan - c.first_scan >= i64::from(f.cfg.init_n) {
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
}

/// A candidate reached M hits: its plots become a track keyed by its address, or a new radar key.
fn confirm(f: &mut Fusion, radar: usize, c: &Cand, out: &mut TrackOutput) {
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
    }
    copy_rows_from(&f.scratch, decision_ns, out);
    if tracker_stats(&f.tracker).capacity_refused > before {
        f.stats.track_refused += 1;
    }
    let first = c.meas.first().map_or(0, |m| m.toa_ns);
    let last = c.meas.get(n.saturating_sub(1)).map_or(0, |m| m.toa_ns);
    if f.tracks.len() >= f.track_rec_max {
        f.stats.track_rec_overflow += 1;
        return;
    }
    f.tracks.push(RadarTrackRec {
        key,
        radar: radar as u8,
        first_ns: first,
        confirm_ns: last,
    });
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
    use crate::types::{output_len, DEFAULT_CONFIG};
    use oa_sensorsim::prng::Rng;

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
        let g = radar_geom(&SITE);
        let frame = oa_geo::aeqd::frame_new(SITE.lat_deg, SITE.lon_deg);
        let mut rng = Rng::from_seed(seed);
        let mut v = Vec::new();
        for k in 0..scans {
            for (li, l) in lines.iter().enumerate() {
                let t_s = (k as f64) * 4.8 + (li as f64) * 0.1;
                let (x, y) = (l.x0 + l.vx * t_s, l.y0 + l.vy * t_s);
                let (lat, lon) = oa_geo::aeqd::unproject(&frame, x, y);
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
                    oa_geo::aeqd::unproject(&frame, x + 4.1 * rng.normal(), y + 4.1 * rng.normal());
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
}
