//! The capacity run: `n` simultaneous aircraft at their real update rates, fed to the fusion
//! layer one second at a time, each second timed. Shell code (it reads the clock and /proc and
//! allocates freely, RULESET C6); the layer it drives is core.
//!
//! Per second of traffic `k` the harness (untimed) makes the ADS-B reports and plots timed in
//! that second; then (timed, one `Instant` pair) offers them all and calls `fusion_advance` to
//! the end of the second; then (untimed) reads the rows emitted and empties the table. So a
//! plot or report reaches the output at the advance that ends its second: between 0 and 1 s of
//! traffic time after its own time, which the run records as the input-to-effect delay.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_assign_log, fusion_bookkeeping_len, fusion_buffers,
    fusion_clear_forensics, fusion_config, fusion_forensics, fusion_new, fusion_plots,
    fusion_stats, fusion_take_radar_tracks, fusion_tracker, fusion_tracker_config,
    fusion_tracker_stats, AssignLog, FusionStats, RadarAssociationStats, RadarInitStats,
    FORENSIC_APPLIED, FORENSIC_INIT, FORENSIC_NONE, HOW_ASSOC, HOW_HARD, HOW_INIT,
};
use oa_track::fusion_params::{ASSIGN_LOG_MAX, ASSOC_FORENSICS_MAX, KEY_RADAR_BASE, MAX_RADARS};
use oa_track::fusion_score::{identify_tracks, Kind, EPOCH_NS};
use oa_track::{
    output_clear, output_new, tracker_bytes, tracker_counts, tracker_live_iter, Measurement,
    TrackerConfig, TrackerStats, DEFAULT_CONFIG, O_COAST, O_TICK, REFUSE_NAMES,
};

use crate::gen::{half_km, Layout, Traffic, TrafficCfg};
use crate::stats::{dist_of, fmt_f, Dist, Hist, Kv};
use crate::sys::{loadavg3, status_kb};

const NS_PER_S: i64 = 1_000_000_000;

/// One capacity run.
#[derive(Debug, Clone)]
pub struct CapCfg {
    pub traffic: TrafficCfg,
    /// Seconds of traffic fed before the statistics start (tracks initiate, registration settles).
    pub warmup_s: i64,
    /// Seconds of traffic whose statistics are kept.
    pub measure_s: i64,
    /// Tracker capacity, tracks.
    pub max_tracks: usize,
    /// Stop feeding when this much wall time has passed (the run says so).
    pub max_wall_s: f64,
    /// Stop after 15 s of traffic if the run is taking more than twice real time, or after 2 s
    /// if more than thirty times (it is far past the knee and the rest would only take long to
    /// say so).
    pub early_stop: bool,
    /// Optional measured-window ADS-B outage, in seconds, beginning at the warm-up boundary.
    pub adsb_dropout_s: Option<i64>,
    /// Remove Mode C altitude from every measured-window radar plot.
    pub radar_no_altitude: bool,
    /// Opt-in bounded final-snapshot identity summary from plot-assignment evidence.
    pub truth_score: bool,
    /// Show per-radar initiation budget diagnostics in the report.
    pub init_diagnostics: bool,
    /// Show per-radar anonymous association outcomes and work.
    pub association_diagnostics: bool,
    /// Opt-in bounded wrong-assignment CSV evidence; requires `truth_score`.
    pub assignment_forensics: bool,
}

/// Truth-derived identity summary for final confirmed tracks. A7.2: cold report data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TruthSnapshotScore {
    /// Confirmed track keys live at the final run time.
    pub snapshot_tracks: usize,
    /// Distinct known truth aircraft offered during the measured interval.
    pub offered_aircraft: usize,
    /// Distinct offered truth aircraft represented by a final snapshot track.
    pub matched_aircraft: usize,
    /// Offered truth aircraft with no final snapshot track matched to them.
    pub missing_aircraft: usize,
    /// Duplicate final tracks for a truth identity, per fusion_score::identify.
    pub duplicate_tracks: u64,
    pub false_tracks: u64,
    /// True aircraft plots applied to a track identified as another aircraft.
    pub wrong_plots: u64,
    /// Synthetic false plots applied to a real-aircraft track.
    pub false_applied: u64,
    pub swaps: u64,
    /// Wrong-aircraft plot applications whose plot time is in the measured window.
    pub measured_wrong_plots: u64,
    /// Identity changes whose plot time is in the measured window.
    pub measured_swaps: u64,
    /// Assignment evidence rows whose plot time is in the measured window.
    pub measured_assignment_rows: usize,
    /// Assignment evidence spans warm-up and measured traffic, but is restricted to snapshot keys.
    pub assignment_rows: usize,
    /// First once-per-second confirmed appearance of each matched final key, from run start.
    pub first_confirmed_s: Option<Dist>,
}

/// One measured-window wrong-applied row, post-hoc identified against the final live-key roster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForensicWrongRow {
    pub t_ns: i64,
    pub plot_ordinal: u32,
    pub radar: u8,
    pub plot_addr: u32,
    pub truth_label: u32,
    pub action: u8,
    pub plot_range_m: f64,
    pub plot_az_deg: f64,
    pub plot_alt_ft: i32,
    pub plot_mode_a: u16,
    pub selected_key: u32,
    pub selected_truth: u32,
    pub selected_cost: f64,
    pub alternate_key: u32,
    pub alternate_truth: u32,
    pub alternate_cost: f64,
    pub ambiguous: bool,
    pub hold_run: u8,
    pub forced: bool,
    pub component_size: u32,
    pub solver_path: u8,
    pub exclusion_regret: f64,
    pub regret_status: u8,
    pub incomplete: bool,
    pub capped: bool,
}

/// What a capacity run measured.
#[derive(Debug, Clone)]
pub struct CapResult {
    pub n: usize,
    pub layout: Layout,
    pub init_diagnostics: bool,
    pub association_diagnostics: bool,
    pub assignment_forensics: bool,
    pub adsb_aircraft: usize,
    pub half_km: f64,
    pub warmup_s: i64,
    pub measure_s_asked: i64,
    /// Measured seconds actually fed (fewer if the wall limit stopped the run).
    pub measure_s_done: i64,
    pub truncated: bool,
    /// Stopped by the early-stop rule (see [`CapCfg::early_stop`]).
    pub stopped_early: bool,
    pub max_tracks: usize,
    /// Wall time of the measured seconds' timed calls, seconds.
    pub wall_measured_s: f64,
    /// Wall time of every second's timed call, warm-up included, seconds.
    pub wall_total_s: f64,
    /// Measured wall time per second of traffic (the same number as the ratio to real time).
    pub realtime_ratio: f64,
    /// Per-advance wall time over the measured seconds, milliseconds.
    pub advance_ms: Dist,
    /// Per-advance wall time over the warm-up seconds, milliseconds.
    pub warmup_advance_ms: Dist,
    pub seconds_over_1s: usize,
    pub adsb_per_s: f64,
    pub plots_per_s: f64,
    pub rows_per_s: f64,
    /// Rows whose age could not be found (track gone before the read).
    pub age_unknown: u64,
    pub age_tick_ms: Dist,
    /// Tick rows of address-keyed tracks (ADS-B) and of radar-only tracks.
    pub age_tick_adsb_ms: Dist,
    pub age_tick_radar_ms: Dist,
    pub age_update_ms: Dist,
    pub age_coast_ms: Dist,
    pub delay_adsb_ms: Dist,
    pub delay_plot_ms: Dist,
    pub fusion: FusionStats,
    /// Per-radar measured-window deltas, excluding warm-up.
    pub init_radar: [RadarInitStats; MAX_RADARS],
    /// Per-radar measured-window deltas, excluding warm-up.
    pub association_radar: [RadarAssociationStats; MAX_RADARS],
    pub tracker: TrackerStats,
    pub out_overflow: u64,
    /// The same three at the end of the warm-up: the loss counters of the measured window are the
    /// differences. If the run stopped inside warm-up, this is the final pre-measurement snapshot,
    /// so measured deltas remain zero.
    pub fusion_warm: FusionStats,
    pub tracker_warm: TrackerStats,
    pub out_overflow_warm: u64,
    pub tracks_tentative: usize,
    /// Confirmed tracking hypotheses, including unresolved anonymous duplicates.
    pub tracks_confirmed: usize,
    /// Distinct generated truth aircraft represented by at least one measured ADS-B report or
    /// radar plot offered to fusion. This is input coverage, not a tracker hypothesis count.
    pub offered_aircraft_unique: usize,
    /// Configured generated truth aircraft expected in the measured window.
    pub offered_aircraft_expected: usize,
    /// Offered ADS-B addresses or plot labels absent from the generated truth address map.
    pub unknown_input_labels: u64,
    /// ADS-B reports withheld by the opt-in outage, counted from generated reports.
    pub adsb_dropout_reports: u64,
    pub adsb_dropout_s: Option<i64>,
    pub radar_no_altitude: bool,
    /// Generated plot fields removed by the opt-in measured-window degradation.
    pub radar_altitudes_removed: u64,
    /// Warm address-bearing cohort size and fresh-row coverage in each measured second.
    pub continuity_cohort: usize,
    pub continuity_noncoast: Vec<(i64, Option<usize>)>,
    /// Exact confirmed ADS-B addresses in the warm-up cutover cohort. Used to verify paired runs.
    pub continuity_addresses: Vec<u32>,
    /// Available only when `CapCfg::truth_score` is enabled and complete evidence exists.
    pub truth_score: Option<TruthSnapshotScore>,
    /// The truth score was requested but could not be computed honestly.
    pub truth_score_unavailable: Option<&'static str>,
    /// Rows are present only when requested and the measured-window evidence is complete.
    pub assignment_forensics_rows: Option<Vec<ForensicWrongRow>>,
    pub assignment_forensics_unavailable: Option<&'static str>,
    pub radar_track_recs_taken: u64,
    pub bookkeeping: (usize, usize),
    pub buffers_stable: bool,
    pub tracker_table_mb: f64,
    /// Resident memory, kB: before the layer is made, after, and the run's peak (`VmHWM`).
    pub rss_before_kb: u64,
    pub rss_after_new_kb: u64,
    pub rss_end_kb: u64,
    /// Resident memory at the end of the warm-up (0 if the run stopped inside it).
    pub rss_warm_kb: u64,
    pub hwm_kb: u64,
    pub load_start: String,
    pub load_end: String,
    /// Per measured second: `(second, wall ms, adsb offered, plots offered, rows emitted)`.
    pub seconds: Vec<(i64, f64, usize, usize, usize)>,
}

impl CapResult {
    /// The loss-of-work counters that moved during the measured window, by name.
    pub fn loss(&self) -> Vec<(String, u64)> {
        let now = loss_counters(&self.fusion, &self.tracker, self.out_overflow);
        let warm = loss_counters(
            &self.fusion_warm,
            &self.tracker_warm,
            self.out_overflow_warm,
        );
        now.into_iter()
            .zip(warm)
            .map(|((name, a), (_, b))| (name, a.saturating_sub(b)))
            .filter(|(_, v)| *v > 0)
            .collect()
    }

    /// The run discarded work in the measured window: some loss counter moved.
    pub fn lost_work(&self) -> bool {
        !self.loss().is_empty()
    }

    /// Fast enough: the traffic was all fed and every measured second's advance finished within
    /// the real-time budget (under 1 s of wall time per second of traffic), with no exception. The
    /// mean and the 99th percentile are reported, not tested.
    pub fn in_time(&self) -> bool {
        !self.truncated
            && !self.stopped_early
            && self.realtime_ratio < 1.0
            && self.advance_ms.max < ADVANCE_BUDGET_MS
    }

    /// A capacity claim: fast enough and no work lost. The knee is the largest `n` where this
    /// holds.
    pub fn keeps_up(&self) -> bool {
        self.in_time()
            && !self.lost_work()
            && self.offered_aircraft_unique == self.offered_aircraft_expected
            && self.unknown_input_labels == 0
    }

    /// The same test as [`CapResult::keeps_up`] (the slowest advance is already under the budget);
    /// kept as its own key so result files and the strict knee stay.
    pub fn keeps_up_strict(&self) -> bool {
        self.keeps_up()
    }
}

/// The time one advance may take: one second of wall time per second of traffic, ms.
pub const ADVANCE_BUDGET_MS: f64 = 1000.0;

/// The names and values of the fusion counters.
pub fn fusion_counters(s: &FusionStats) -> Vec<(&'static str, u64)> {
    vec![
        ("adsb_in", s.adsb_in),
        ("adsb_address_refused", s.adsb_address_refused),
        ("plots_in", s.plots_in),
        ("plots_refused", s.plots_refused),
        ("late", s.late),
        ("pending_overflow", s.pending_overflow),
        ("applied_hard", s.applied_hard),
        ("applied_assoc", s.applied_assoc),
        ("held", s.held),
        ("unassigned", s.unassigned),
        ("hard_no_track", s.hard_no_track),
        ("forensics_overflow", s.forensics_overflow),
        ("forensics_pair_visits", s.forensics_pair_visits),
        ("forensics_plot_visits", s.forensics_plot_visits),
        ("forensics_metadata_writes", s.forensics_metadata_writes),
        ("forensics_regret_work", s.forensics_regret_work),
        (
            "forensics_regret_unavailable",
            s.forensics_regret_unavailable,
        ),
        ("hard_mismatch", s.hard_mismatch),
        ("radar_disagree", s.radar_disagree),
        ("hard_extra", s.hard_extra),
        ("hold_evicted", s.hold_evicted),
        ("held_forced", s.held_forced),
        ("bucket_plot_overflow", s.bucket_plot_overflow),
        ("bucket_work_capped", s.bucket_work_capped),
        ("assoc_refused", s.assoc_refused),
        ("coarse_visits", s.coarse_visits),
        ("polar_index_work", s.polar_index_work),
        ("polar_index_build_work", s.polar_index_build_work),
        ("polar_index_query_work", s.polar_index_query_work),
        ("polar_index_queries", s.polar_index_queries),
        ("polar_index_track_visits", s.polar_index_track_visits),
        ("pairs_tested", s.pairs_tested),
        ("pairs_rejected_by_altitude", s.pairs_rejected_by_altitude),
        ("pairs_gated", s.pairs_gated),
        ("no_height_pairs", s.no_height_pairs),
        ("cand_started", s.cand_started),
        ("cand_joined", s.cand_joined),
        ("cand_confirmed", s.cand_confirmed),
        ("cand_expired", s.cand_expired),
        ("cand_no_height", s.cand_no_height),
        ("cand_capacity", s.cand_capacity),
        ("track_refused", s.track_refused),
        ("assign_log_overflow", s.assign_log_overflow),
        ("reg_clamped", s.reg_clamped),
        ("processed", s.processed),
        ("track_rec_overflow", s.track_rec_overflow),
        ("keyinfo_overflow", s.keyinfo_overflow),
        ("scratch_full", s.scratch_full),
        ("reg_history_full", s.reg_history_full),
        ("anon_plots_ungated", s.anon_plots_ungated),
        ("assoc_incomplete_batches", s.assoc_incomplete_batches),
        ("assoc_incomplete_deferred", s.assoc_incomplete_deferred),
        ("assoc_refused_deferred", s.assoc_refused_deferred),
        ("init_capped", s.init_capped),
        ("init_work", s.init_work),
        ("init_plots_refused", s.init_plots_refused),
        ("init_refusal_pass_visits", s.init_refusal_pass_visits),
        ("plot_scan_full", s.plot_scan_full),
        ("prediction_refused", s.prediction_refused),
        ("wide_full", s.wide_full),
        ("init_wide_full", s.init_wide_full),
    ]
}

/// Fusion counters that are not loss of work: traffic and outcomes, the work done, the
/// hard-constraint plots counted and not applied by design, and the high-water marks.
pub const FUSION_NOT_LOSS: &[&str] = &[
    "adsb_in",
    "plots_in",
    "applied_hard",
    "applied_assoc",
    "held",
    "unassigned",
    "hard_no_track",
    "forensics_overflow",
    "forensics_pair_visits",
    "forensics_plot_visits",
    "forensics_metadata_writes",
    "forensics_regret_work",
    "forensics_regret_unavailable",
    "hard_mismatch",
    "radar_disagree",
    "hard_extra",
    "held_forced",
    "coarse_visits",
    "polar_index_work",
    "polar_index_build_work",
    "polar_index_query_work",
    "polar_index_queries",
    "polar_index_track_visits",
    "pairs_tested",
    "pairs_rejected_by_altitude",
    "pairs_gated",
    "cand_started",
    "cand_joined",
    "cand_confirmed",
    "cand_expired",
    "gnn",
    "reg_clamped",
    "processed",
    "tracks_at_sum",
    "init_tests",
    "init_work",
    "init_refusal_pass_visits",
    "init_radar",        // Diagnostic breakdown of existing counters.
    "association_radar", // Derived breakdown of association outcomes, not a separate loss verdict.
    "cands_live_max",
    "assoc_runs",
    "coarse_width_sum_m",
    "coarse_width_max_m",
    "buckets",
    "cand_merged_on_confirm",
    "cand_cancelled_by_track",
    "confirm_gate_tests",
    "cand_same_scan_kept",
    "wide_tracks_sum",
    "init_visits",
    "stale_plots_skipped",
];

/// Tracker counters that are not loss of work: traffic, filter outcomes (a rejected jump or
/// altitude is a decision of the filter), track life, clock steps and the sums.
pub const TRACKER_NOT_LOSS: &[&str] = &[
    "offered",
    "applied",
    "duplicates",
    "refiltered",
    "replayed",
    "jump_rejected",
    "resyncs",
    "alt_rejected",
    "alt_geometric_skipped",
    "started",
    "confirmed",
    "ended",
    "init_implausible",
    "init_window_expired",
    "ticks",
    "ticks_skipped",
    "rows_computed",
    "rows_uncertain",
    "velocity_gated",
    "time_held",
    "time_steps",
    "time_held_applied",
    "time_steps_back",
    "nis_sum",
    "nis_n",
    // Large moves (RT-239): outcome accounting. Every report is applied or counted as a
    // refusal elsewhere; these say what became of a move, not what was lost.
    "moves_unconfirmed",
    "moves_confirmed",
    "moves_rolled_back",
    "moves_undecided",
    "moves_decision_rejected",
    "moves_late",
    "moves_abandoned_ended",
    "moves_abandoned_restarted",
    "moves_abandoned_resync",
    "moves_abandoned_demoted",
    "moves_old_path_votes",
    "moves_old_path_unpredictable",
    "moves_resync_marked",
    "moves_peak_active",
];

/// Every counter of work lost or refused: input or output that the run dropped, refused, cut
/// short or never examined for want of room. A capacity claim needs all of them zero over the
/// measured window. The one list: a counter added to a stats struct must be put here or in
/// [`FUSION_NOT_LOSS`] / [`TRACKER_NOT_LOSS`], or the test of that name fails.
///
/// Fusion: numerical/representability forecast failures, plots refused by validation, plots late
/// for their bucket, the pending queue full, hold runs evicted, bucket plot overflow, bucket work
/// capped, anonymous plots never gated, initiation
/// cut short, the solver refusing, candidate table full, a confirmed track the tracker could not
/// hold, assignment log full, plots without a height (candidates not started, pairs skipped), scan
/// notes dropped, wide lists full (association and initiation), radar-only track records dropped,
/// keys not remembered, scratch buffers full, registration history full.
/// Tracker: no free slot, each validation refusal, numerical prediction/update refusals, late
/// measurements (window and ring), clock-step holds discarded, tracks dropped by a step back,
/// sensor table entries evicted; large moves that lost their rollback (late, or no snapshot for a
/// full pool), rows whose widened radius failed (fail-closed radius), reports refused internally.
/// Output: rows lost to a full table.
pub fn loss_counters(f: &FusionStats, t: &TrackerStats, out_overflow: u64) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = vec![
        ("fusion.plots_refused".into(), f.plots_refused),
        ("fusion.adsb_address_refused".into(), f.adsb_address_refused),
        ("fusion.late".into(), f.late),
        ("fusion.pending_overflow".into(), f.pending_overflow),
        ("fusion.hold_evicted".into(), f.hold_evicted),
        ("fusion.bucket_plot_overflow".into(), f.bucket_plot_overflow),
        ("fusion.bucket_work_capped".into(), f.bucket_work_capped),
        ("fusion.anon_plots_ungated".into(), f.anon_plots_ungated),
        (
            "fusion.assoc_incomplete_batches".into(),
            f.assoc_incomplete_batches,
        ),
        (
            "fusion.assoc_incomplete_deferred".into(),
            f.assoc_incomplete_deferred,
        ),
        (
            "fusion.assoc_refused_deferred".into(),
            f.assoc_refused_deferred,
        ),
        ("fusion.init_capped".into(), f.init_capped),
        ("fusion.init_plots_refused".into(), f.init_plots_refused),
        ("fusion.assoc_refused".into(), f.assoc_refused),
        ("fusion.cand_capacity".into(), f.cand_capacity),
        ("fusion.cand_no_height".into(), f.cand_no_height),
        ("fusion.no_height_pairs".into(), f.no_height_pairs),
        ("fusion.track_refused".into(), f.track_refused),
        ("fusion.assign_log_overflow".into(), f.assign_log_overflow),
        ("fusion.plot_scan_full".into(), f.plot_scan_full),
        ("fusion.prediction_refused".into(), f.prediction_refused),
        ("fusion.wide_full".into(), f.wide_full),
        ("fusion.init_wide_full".into(), f.init_wide_full),
        ("fusion.track_rec_overflow".into(), f.track_rec_overflow),
        ("fusion.keyinfo_overflow".into(), f.keyinfo_overflow),
        ("fusion.scratch_full".into(), f.scratch_full),
        ("fusion.reg_history_full".into(), f.reg_history_full),
        ("tracker.capacity_refused".into(), t.capacity_refused),
        ("tracker.late_window".into(), t.late_window),
        ("tracker.late_ring".into(), t.late_ring),
        (
            "tracker.time_held_discarded".into(),
            t.time_held.saturating_sub(t.time_held_applied),
        ),
        ("tracker.time_future_dropped".into(), t.time_future_dropped),
        ("tracker.time_sensor_evicted".into(), t.time_sensor_evicted),
        // A track row withheld because its position cannot be represented is output lost.
        (
            "tracker.rows_unrepresentable".into(),
            t.rows_unrepresentable,
        ),
        // A row whose position covariance is not a number is never emitted: output lost.
        ("tracker.nan_covariance".into(), t.nan_covariance),
        // A filter prediction that cannot produce a finite/usable state withholds this update.
        ("tracker.numerical_refused".into(), t.numerical_refused),
        // A move that can no longer be rolled back, or was made without a snapshot because the
        // pool was full, has lost its rollback: work lost for want of room or history.
        (
            "tracker.moves_rollback_lost_late".into(),
            t.moves_rollback_lost_late,
        ),
        ("tracker.move_no_snapshot".into(), t.move_no_snapshot),
        // Rows given the fail-closed radius instead of a computed one: output degraded.
        ("tracker.move_widen_failed".into(), t.move_widen_failed),
        // A report refused because the rule was off for it (cannot happen): input refused.
        (
            "tracker.moves_internal_refused".into(),
            t.moves_internal_refused,
        ),
    ];
    for (name, c) in REFUSE_NAMES.iter().zip(t.refused.iter()) {
        v.push((format!("tracker.refused_{name}"), *c));
    }
    v.push(("out.overflow".into(), out_overflow));
    v
}

/// The tracker's counters, by name.
pub fn tracker_counters(s: &TrackerStats) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = vec![
        ("offered".into(), s.offered),
        ("applied".into(), s.applied),
        ("duplicates".into(), s.duplicates),
        ("late_window".into(), s.late_window),
        ("late_ring".into(), s.late_ring),
        ("refiltered".into(), s.refiltered),
        ("jump_rejected".into(), s.jump_rejected),
        ("resyncs".into(), s.resyncs),
        ("alt_rejected".into(), s.alt_rejected),
        ("capacity_refused".into(), s.capacity_refused),
        ("numerical_refused".into(), s.numerical_refused),
        ("started".into(), s.started),
        ("confirmed".into(), s.confirmed),
        ("ticks".into(), s.ticks),
        ("ticks_skipped".into(), s.ticks_skipped),
        ("rows_computed".into(), s.rows_computed),
        ("rows_uncertain".into(), s.rows_uncertain),
        ("velocity_gated".into(), s.velocity_gated),
        ("time_held".into(), s.time_held),
        ("time_steps".into(), s.time_steps),
        ("time_steps_back".into(), s.time_steps_back),
    ];
    for (name, c) in REFUSE_NAMES.iter().zip(s.refused.iter()) {
        v.push((format!("refused_{name}"), *c));
    }
    v.push(("ended_total".into(), s.ended.iter().sum()));
    v
}

/// The tracker's configuration for a capacity run.
pub fn tracker_cfg(max_tracks: usize, n: usize) -> TrackerConfig {
    TrackerConfig {
        max_tracks,
        // A second of a full table is a tick row per track and the update rows: four per track is
        // room; a full table counts what it loses.
        out_capacity: n.saturating_mul(4).clamp(1 << 16, 1 << 24),
        ..DEFAULT_CONFIG
    }
}

/// Newest update time of every live track, by key, into `m` (emptied first).
fn snapshot(f: &oa_track::fusion::Fusion, m: &mut HashMap<u32, i64>) {
    m.clear();
    for (key, last) in tracker_live_iter(fusion_tracker(f)) {
        m.insert(key, last);
    }
}

/// Measured-window truth coverage. All maps and bitset storage are prepared before timing;
/// account calls only inspect offered shell inputs and never touch fusion's timed work.
// A7.2: shell bookkeeping, allocated once before the measured windows.
struct OfferedCoverage {
    by_addr: HashMap<u32, usize>,
    seen: Vec<bool>,
    unique: usize,
    unknown: u64,
}

fn offered_new(aircraft: &[crate::gen::Aircraft]) -> OfferedCoverage {
    let mut by_addr = HashMap::with_capacity(aircraft.len());
    for (i, a) in aircraft.iter().enumerate() {
        by_addr.insert(a.addr, i);
    }
    OfferedCoverage {
        by_addr,
        seen: vec![false; aircraft.len()],
        unique: 0,
        unknown: 0,
    }
}

fn offered_mark(coverage: &mut OfferedCoverage, label: u32) {
    let Some(&i) = coverage.by_addr.get(&label) else {
        coverage.unknown = coverage.unknown.saturating_add(1);
        return;
    };
    if let Some(seen) = coverage.seen.get_mut(i) {
        if !*seen {
            *seen = true;
            coverage.unique += 1;
        }
    } else {
        coverage.unknown = coverage.unknown.saturating_add(1);
    }
}

fn offered_adsb(coverage: &mut OfferedCoverage, reports: &[Measurement]) {
    for report in reports {
        offered_mark(coverage, report.addr);
    }
}

fn offered_plots(coverage: &mut OfferedCoverage, radar_plots: &[Vec<oa_track::radar::RadarPlot>]) {
    for plot in radar_plots.iter().flatten() {
        offered_mark(coverage, plot.label);
    }
}

/// Scores final confirmed keys from their cumulative assignment evidence. Work is bounded by the
/// generated aircraft count, final live-key count and `ASSIGN_LOG_MAX`.
fn truth_snapshot_score(
    fusion: &oa_track::fusion::Fusion,
    offered: &OfferedCoverage,
    first_confirmed: &HashMap<u32, i64>,
    measured_from_ns: i64,
) -> Result<TruthSnapshotScore, &'static str> {
    if fusion_stats(fusion).assign_log_overflow > 0 {
        return Err("assignment log overflowed; identity score unavailable");
    }
    if offered.unknown > 0 {
        return Err("unknown offered labels prevent a complete truth denominator");
    }
    if offered.unique == 0 {
        return Err("no known truth aircraft were offered during measurement");
    }
    let assign = fusion_assign_log(fusion);
    if assign.t_ns.len() > ASSIGN_LOG_MAX
        || assign.key.len() != assign.t_ns.len()
        || assign.label.len() != assign.t_ns.len()
        || assign.radar.len() != assign.t_ns.len()
        || assign.how.len() != assign.t_ns.len()
    {
        return Err("assignment log columns are incomplete or exceed their bound");
    }
    let keys: Vec<u32> = tracker_live_iter(fusion_tracker(fusion))
        .map(|(key, _)| key)
        .collect();
    let identified = identify_tracks(assign, &keys);
    // A separate measured-window view isolates failures after the degradation begins. This is
    // cold scoring work: labels remain write-only evidence and never enter a fusion decision.
    let mut measured_assign = AssignLog::default();
    for i in 0..assign.t_ns.len() {
        let Some((&t_ns, &key, &label, &radar, &how)) = assign
            .t_ns
            .get(i)
            .zip(assign.key.get(i))
            .zip(assign.label.get(i))
            .zip(assign.radar.get(i))
            .zip(assign.how.get(i))
            .map(|((((t_ns, key), label), radar), how)| (t_ns, key, label, radar, how))
        else {
            return Err("assignment log columns are incomplete");
        };
        if t_ns >= measured_from_ns {
            measured_assign.t_ns.push(t_ns);
            measured_assign.key.push(key);
            measured_assign.label.push(label);
            measured_assign.radar.push(radar);
            measured_assign.how.push(how);
        }
    }
    let measured_identified = identify_tracks(&measured_assign, &keys);
    let offered_truth: BTreeSet<u32> = offered
        .by_addr
        .iter()
        .filter_map(|(&addr, &index)| {
            offered
                .seen
                .get(index)
                .copied()
                .unwrap_or(false)
                .then_some(addr)
        })
        .collect();
    // The adjudicator returns one primary key per truth aircraft.
    let matched_keys: Vec<u32> = identified
        .truth_of
        .iter()
        .filter_map(|(&key, addr)| offered_truth.contains(addr).then_some(key))
        .collect();
    let first_confirmed_s = matched_keys
        .iter()
        .map(|key| first_confirmed.get(key).map(|&second| second as f64))
        .collect::<Option<Vec<_>>>()
        .map(|seconds| dist_of(&seconds));
    Ok(TruthSnapshotScore {
        snapshot_tracks: keys.len(),
        offered_aircraft: offered_truth.len(),
        matched_aircraft: matched_keys.len(),
        missing_aircraft: offered_truth.len().saturating_sub(matched_keys.len()),
        duplicate_tracks: identified.duplicates,
        false_tracks: identified.false_tracks,
        wrong_plots: identified.wrong_plots,
        false_applied: identified.false_applied,
        swaps: identified.swaps,
        measured_wrong_plots: measured_identified.wrong_plots,
        measured_swaps: measured_identified.swaps,
        measured_assignment_rows: measured_assign.t_ns.len(),
        assignment_rows: assign.t_ns.len(),
        first_confirmed_s,
    })
}

/// Builds only wrong, applied measured-window rows for the final live-key roster. This cold
/// scoring pass has a fixed input bound (`ASSOC_FORENSICS_MAX`) and fails closed on overflow,
/// incomplete columns, or disagreement with the existing truth score.
fn assignment_forensics_rows(
    fusion: &oa_track::fusion::Fusion,
    measured_from_ns: i64,
    expected_wrong: u64,
    decision_log_complete: bool,
    measured_forensics_overflow: u64,
) -> Result<Vec<ForensicWrongRow>, &'static str> {
    if !decision_log_complete {
        return Err("fusion scratch or event queue overflow makes decision rows incomplete");
    }
    if measured_forensics_overflow > 0 {
        return Err("forensics sidecar reached its row cap");
    }
    let evidence = fusion_forensics(fusion);
    let n = evidence.t_ns.len();
    if n > ASSOC_FORENSICS_MAX
        || evidence.ordinal.len() != n
        || evidence.plot_range_m.len() != n
        || evidence.plot_az_deg.len() != n
        || evidence.plot_alt_ft.len() != n
        || evidence.plot_addr.len() != n
        || evidence.label.len() != n
        || evidence.plot_mode_a.len() != n
        || evidence.radar.len() != n
        || evidence.action.len() != n
        || evidence.selected_key.len() != n
        || evidence.selected_cost.len() != n
        || evidence.alternate_key.len() != n
        || evidence.alternate_cost.len() != n
        || evidence.ambiguous.len() != n
        || evidence.hold_run.len() != n
        || evidence.held.len() != n
        || evidence.forced.len() != n
        || evidence.component_size.len() != n
        || evidence.solver_path.len() != n
        || evidence.exclusion_regret.len() != n
        || evidence.regret_status.len() != n
        || evidence.incomplete.len() != n
        || evidence.capped.len() != n
    {
        return Err("forensics sidecar columns are incomplete or exceed their bound");
    }
    let assign = fusion_assign_log(fusion);
    if assign.t_ns.len() > ASSIGN_LOG_MAX
        || assign.key.len() != assign.t_ns.len()
        || assign.label.len() != assign.t_ns.len()
        || assign.radar.len() != assign.t_ns.len()
        || assign.how.len() != assign.t_ns.len()
    {
        return Err("assignment log columns are incomplete");
    }
    let keys: Vec<u32> = tracker_live_iter(fusion_tracker(fusion))
        .map(|(key, _)| key)
        .collect();
    let mut measured = AssignLog::default();
    for i in 0..assign.t_ns.len() {
        let (Some(&t), Some(&key), Some(&label), Some(&radar), Some(&how)) = (
            assign.t_ns.get(i),
            assign.key.get(i),
            assign.label.get(i),
            assign.radar.get(i),
            assign.how.get(i),
        ) else {
            return Err("assignment log columns are incomplete");
        };
        if t >= measured_from_ns {
            measured.t_ns.push(t);
            measured.key.push(key);
            measured.label.push(label);
            measured.radar.push(radar);
            measured.how.push(how);
        }
    }
    let identity = identify_tracks(&measured, &keys);
    let mut logged = HashMap::<(i64, u32, u32, u8, u8), usize>::new();
    for i in 0..measured.t_ns.len() {
        let (Some(&t), Some(&key), Some(&label), Some(&radar), Some(&how)) = (
            measured.t_ns.get(i),
            measured.key.get(i),
            measured.label.get(i),
            measured.radar.get(i),
            measured.how.get(i),
        ) else {
            return Err("assignment log columns are incomplete");
        };
        *logged.entry((t, key, label, radar, how)).or_default() += 1;
    }
    let mut wrong = Vec::with_capacity(expected_wrong.min(n as u64) as usize);
    for i in 0..n {
        let (Some(&t_ns), Some(&action), Some(&label), Some(&key)) = (
            evidence.t_ns.get(i),
            evidence.action.get(i),
            evidence.label.get(i),
            evidence.selected_key.get(i),
        ) else {
            return Err("forensics sidecar columns are incomplete");
        };
        if t_ns < measured_from_ns
            || !matches!(action, FORENSIC_APPLIED | FORENSIC_INIT)
            || label == u32::MAX
        {
            continue;
        }
        let Some(&radar) = evidence.radar.get(i) else {
            return Err("forensics sidecar columns are incomplete");
        };
        let expected_hows: &[u8] = if action == FORENSIC_INIT {
            &[HOW_INIT]
        } else {
            &[HOW_HARD, HOW_ASSOC]
        };
        let mut matched = false;
        for &how in expected_hows {
            if let Some(left) = logged.get_mut(&(t_ns, key, label, radar, how)) {
                if *left > 0 {
                    *left -= 1;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            continue;
        }
        if identity
            .identity_of
            .get(&key)
            .is_none_or(|&truth| truth == label)
        {
            continue;
        }
        let Some(&selected_truth) = identity.identity_of.get(&key) else {
            return Err("selected key has no post-hoc truth identity");
        };
        if selected_truth == label {
            return Err("wrong-row identity check disagrees with its filter");
        }
        let (
            Some(&plot_ordinal),
            Some(&plot_addr),
            Some(&plot_range_m),
            Some(&plot_az_deg),
            Some(&plot_alt_ft),
            Some(&plot_mode_a),
            Some(&selected_cost),
            Some(&alternate_key),
            Some(&alternate_cost),
            Some(&ambiguous),
            Some(&hold_run),
            Some(&forced),
            Some(&component_size),
            Some(&solver_path),
            Some(&exclusion_regret),
            Some(&regret_status),
            Some(&incomplete),
            Some(&capped),
        ) = (
            evidence.ordinal.get(i),
            evidence.plot_addr.get(i),
            evidence.plot_range_m.get(i),
            evidence.plot_az_deg.get(i),
            evidence.plot_alt_ft.get(i),
            evidence.plot_mode_a.get(i),
            evidence.selected_cost.get(i),
            evidence.alternate_key.get(i),
            evidence.alternate_cost.get(i),
            evidence.ambiguous.get(i),
            evidence.hold_run.get(i),
            evidence.forced.get(i),
            evidence.component_size.get(i),
            evidence.solver_path.get(i),
            evidence.exclusion_regret.get(i),
            evidence.regret_status.get(i),
            evidence.incomplete.get(i),
            evidence.capped.get(i),
        )
        else {
            return Err("forensics sidecar columns are incomplete");
        };
        wrong.push(ForensicWrongRow {
            t_ns,
            plot_ordinal,
            radar,
            plot_addr,
            truth_label: label,
            action,
            plot_range_m,
            plot_az_deg,
            plot_alt_ft,
            plot_mode_a,
            selected_key: key,
            selected_truth,
            selected_cost,
            alternate_key,
            alternate_truth: identity
                .identity_of
                .get(&alternate_key)
                .copied()
                .unwrap_or(FORENSIC_NONE),
            alternate_cost,
            ambiguous,
            hold_run,
            forced,
            component_size,
            solver_path,
            exclusion_regret,
            regret_status,
            incomplete,
            capped,
        });
    }
    if wrong.len() as u64 != expected_wrong {
        return Err("wrong-row export count disagrees with measured truth score");
    }
    Ok(wrong)
}

fn forensics_source_complete(scratch_full: u64) -> bool {
    scratch_full == 0
}

/// Runs the traffic through the layer and measures it.
pub fn run_capacity(cfg: &CapCfg) -> Result<CapResult, String> {
    if cfg.warmup_s < 0 {
        return Err("capacity warm-up cannot be negative".to_string());
    }
    if cfg.assignment_forensics && !cfg.truth_score {
        return Err("assignment forensics requires truth scoring".to_string());
    }
    if cfg.adsb_dropout_s.is_some() && cfg.warmup_s < 1 {
        return Err(
            "continuity mode requires at least 1 second of mixed-sensor warm-up".to_string(),
        );
    }
    if cfg.measure_s < 1 {
        return Err("capacity measurement must be at least 1 second".to_string());
    }
    if cfg
        .adsb_dropout_s
        .is_some_and(|s| s < 0 || s > cfg.measure_s)
    {
        return Err(
            "ADS-B dropout duration must be between 0 and the measured duration".to_string(),
        );
    }
    let total_s = cfg
        .warmup_s
        .checked_add(cfg.measure_s)
        .ok_or("capacity run duration overflows")?;
    let max_s = (oa_track::TIME_LIMIT_NS - EPOCH_NS) / NS_PER_S;
    if total_s > max_s {
        return Err(format!(
            "capacity run exceeds the tracker time limit of {max_s} seconds"
        ));
    }
    let n = cfg.traffic.n;
    let load_start = loadavg3();
    let rss_before_kb = status_kb("VmRSS").unwrap_or(0);
    let mut traffic = Traffic::new(&cfg.traffic)?;
    let sites = traffic.sites();
    let base = tracker_cfg(cfg.max_tracks, n);
    let mut fcfg = fusion_config(fusion_tracker_config(&base, &sites));
    fcfg.log_assign = cfg.truth_score;
    fcfg.init_diagnostics = cfg.init_diagnostics;
    fcfg.association_diagnostics = cfg.association_diagnostics;
    fcfg.association_forensics = cfg.assignment_forensics;
    let mut f = fusion_new(&fcfg, &sites).map_err(|e| e.to_string())?;
    // OA-GEN traffic is fixed truth: prebuild the bounded address index and coverage bits before
    // any core timing. Multiple radar labels for one aircraft intentionally count once.
    let mut offered = offered_new(traffic.aircraft());
    let mut adsb_dropout_reports = 0u64;
    let mut radar_altitudes_removed = 0u64;
    let mut continuity_cohort = BTreeSet::<u32>::new();
    let mut continuity_noncoast = Vec::new();
    let rss_after_new_kb = status_kb("VmRSS").unwrap_or(0);
    let mut out = output_new(base.out_capacity);
    let buffers0 = fusion_buffers(&f);

    let (mut adsb, mut plots) = (Vec::<Measurement>::new(), Vec::new());
    let (mut before, mut after) = (HashMap::new(), HashMap::new());
    let mut first_confirmed = HashMap::new();
    let mut h_tick = Hist::new(1.0, 100_000);
    let mut h_tick_a = Hist::new(1.0, 100_000);
    let mut h_tick_r = Hist::new(1.0, 100_000);
    let mut h_upd = Hist::new(1.0, 100_000);
    let mut h_coast = Hist::new(1.0, 100_000);
    let mut h_da = Hist::new(1.0, 1_100);
    let mut h_dp = Hist::new(1.0, 1_100);
    let (mut age_unknown, mut recs_taken) = (0u64, 0u64);
    let (mut adv_ms, mut warm_ms) = (Vec::new(), Vec::new());
    let mut seconds = Vec::new();
    let (mut wall_measured, mut wall_total) = (0.0f64, 0.0f64);
    let (mut sum_adsb, mut sum_plots, mut sum_rows) = (0usize, 0usize, 0usize);
    let started = Instant::now();
    let (mut truncated, mut stopped_early) = (false, false);
    let mut measure_done = 0i64;
    let mut seconds_done = 0i64;
    let mut rss_warm_kb = 0u64;
    let mut warm = (FusionStats::default(), TrackerStats::default(), 0u64);
    for k in 0..total_s {
        if started.elapsed().as_secs_f64() > cfg.max_wall_s {
            truncated = true;
            break;
        }
        if cfg.assignment_forensics && k == cfg.warmup_s {
            fusion_clear_forensics(&mut f);
        }
        let measured = k >= cfg.warmup_s;
        adsb.clear();
        traffic.adsb_window(k, &mut adsb);
        traffic.plot_windows(k, &mut plots);
        if measured && cfg.radar_no_altitude {
            for plot in plots.iter_mut().flatten() {
                if plot.alt_ft != oa_track::radar::NONE_ALT {
                    plot.alt_ft = oa_track::radar::NONE_ALT;
                    radar_altitudes_removed = radar_altitudes_removed.saturating_add(1);
                }
            }
        }
        let dropout = cfg
            .adsb_dropout_s
            .is_some_and(|duration| k >= cfg.warmup_s && k < cfg.warmup_s.saturating_add(duration));
        if dropout {
            adsb_dropout_reports = adsb_dropout_reports.saturating_add(adsb.len() as u64);
            adsb.clear();
        }
        if measured {
            offered_adsb(&mut offered, &adsb);
            offered_plots(&mut offered, &plots);
        }
        let until = EPOCH_NS.saturating_add((k + 1).saturating_mul(NS_PER_S));
        let overflow_before = out.overflow;
        if measured {
            snapshot(&f, &mut before);
        }
        let t0 = Instant::now();
        for m in &adsb {
            fusion_adsb(&mut f, m);
        }
        for (r, p) in plots.iter().enumerate() {
            fusion_plots(&mut f, r, p);
        }
        fusion_advance(&mut f, until, &mut out);
        let ms = t0.elapsed().as_secs_f64() * 1.0e3;
        wall_total += ms * 1.0e-3;
        if cfg.truth_score {
            // Match the assignment log's existing evidence bound. If a final key is absent
            // after reaching the bound, the report marks the timing distribution unavailable.
            for (key, _) in tracker_live_iter(fusion_tracker(&f)) {
                if first_confirmed.len() == ASSIGN_LOG_MAX {
                    break;
                }
                first_confirmed.entry(key).or_insert(k + 1);
            }
        }
        let n_plots: usize = plots.iter().map(Vec::len).sum();
        if measured {
            wall_measured += ms * 1.0e-3;
            adv_ms.push(ms);
            measure_done += 1;
            snapshot(&f, &mut after);
            let mut fresh_seen = BTreeSet::<u32>::new();
            for m in &adsb {
                h_da.add((until - m.toa_ns) as f64 * 1.0e-6);
            }
            for p in plots.iter().flatten() {
                h_dp.add((until - p.time_ns) as f64 * 1.0e-6);
            }
            let rows = out.t_ns.len();
            for i in 0..rows {
                let (Some(&key), Some(&t), Some(&fl)) =
                    (out.addr.get(i), out.t_ns.get(i), out.flags.get(i))
                else {
                    continue;
                };
                if cfg.adsb_dropout_s.is_some()
                    && continuity_cohort.contains(&key)
                    && fl & oa_track::O_COAST == 0
                {
                    fresh_seen.insert(key);
                }
                if fl & (O_TICK | O_COAST) == 0 {
                    h_upd.add(0.0);
                    continue;
                }
                // The newest update the row can reflect: the track's newest at the end of the
                // advance if it is not after the row, else its newest before the advance.
                let reflected = match (after.get(&key), before.get(&key)) {
                    (Some(&a), _) if a <= t => Some(a),
                    (_, Some(&b)) if b <= t => Some(b),
                    _ => None,
                };
                let Some(r) = reflected else {
                    age_unknown += 1;
                    continue;
                };
                let age_ms = (t - r) as f64 * 1.0e-6;
                if fl & O_COAST != 0 {
                    h_coast.add(age_ms);
                } else if fl & O_TICK != 0 {
                    h_tick.add(age_ms);
                    if key >= KEY_RADAR_BASE {
                        h_tick_r.add(age_ms);
                    } else {
                        h_tick_a.add(age_ms);
                    }
                }
            }
            if cfg.adsb_dropout_s.is_some() {
                continuity_noncoast.push((
                    k,
                    (out.overflow == overflow_before).then_some(fresh_seen.len()),
                ));
            }
            seconds.push((k, ms, adsb.len(), n_plots, rows));
            sum_adsb += adsb.len();
            sum_plots += n_plots;
            sum_rows += rows;
        } else {
            warm_ms.push(ms);
        }
        output_clear(&mut out);
        if k + 1 == cfg.warmup_s {
            rss_warm_kb = status_kb("VmRSS").unwrap_or(0);
            warm = (*fusion_stats(&f), *fusion_tracker_stats(&f), out.overflow);
            if cfg.adsb_dropout_s.is_some() {
                let equipped: BTreeSet<u32> = traffic
                    .aircraft()
                    .iter()
                    .filter(|a| a.adsb)
                    .map(|a| a.addr)
                    .collect();
                for (key, _) in tracker_live_iter(fusion_tracker(&f)) {
                    if equipped.contains(&key) {
                        continuity_cohort.insert(key);
                    }
                }
            }
        }
        if cfg.early_stop
            && ((k >= 14 && wall_total > 2.0 * (k + 1) as f64)
                || (k >= 1 && wall_total > 30.0 * (k + 1) as f64))
        {
            stopped_early = true;
            seconds_done = k + 1;
            break;
        }
        seconds_done = k + 1;
        if k % 10 == 9 {
            // A long-running caller hands the radar-only track records on regularly.
            recs_taken += fusion_take_radar_tracks(&mut f).len() as u64;
        }
    }
    if seconds_done < cfg.warmup_s {
        warm = (*fusion_stats(&f), *fusion_tracker_stats(&f), out.overflow);
    }
    let tracker_table_mb = tracker_bytes(fusion_tracker(&f)) as f64 / (1024.0 * 1024.0);
    let (tent, conf) = tracker_counts(fusion_tracker(&f));
    let md = measure_done.max(1) as f64;
    let advance_ms = dist_of(&adv_ms);
    let measured_from_ns = EPOCH_NS.saturating_add(cfg.warmup_s.saturating_mul(NS_PER_S));
    let (truth_score, truth_score_unavailable) = if !cfg.truth_score {
        (None, None)
    } else if truncated || stopped_early || measure_done != cfg.measure_s {
        (
            None,
            Some("run did not complete the full measured interval"),
        )
    } else {
        match truth_snapshot_score(&f, &offered, &first_confirmed, measured_from_ns) {
            Ok(score) => (Some(score), None),
            Err(reason) => (None, Some(reason)),
        }
    };
    let (assignment_forensics_rows, assignment_forensics_unavailable) = if !cfg.assignment_forensics
    {
        (None, None)
    } else if let Some(score) = truth_score {
        match assignment_forensics_rows(
            &f,
            measured_from_ns,
            score.measured_wrong_plots,
            forensics_source_complete(
                fusion_stats(&f)
                    .scratch_full
                    .saturating_sub(warm.0.scratch_full),
            ),
            fusion_stats(&f)
                .forensics_overflow
                .saturating_sub(warm.0.forensics_overflow),
        ) {
            Ok(rows) => (Some(rows), None),
            Err(reason) => (None, Some(reason)),
        }
    } else {
        (
            None,
            Some("truth score unavailable; wrong-row export was not attempted"),
        )
    };
    let mut init_radar = [RadarInitStats::default(); MAX_RADARS];
    if cfg.init_diagnostics && measure_done > 0 {
        for ((delta, now), before) in init_radar
            .iter_mut()
            .zip(fusion_stats(&f).init_radar)
            .zip(warm.0.init_radar)
        {
            *delta = RadarInitStats {
                batches: now.batches.saturating_sub(before.batches),
                plots: now.plots.saturating_sub(before.plots),
                opening_work: now.opening_work.saturating_sub(before.opening_work),
                retirement_requested: now
                    .retirement_requested
                    .saturating_sub(before.retirement_requested),
                retirement_denied: now
                    .retirement_denied
                    .saturating_sub(before.retirement_denied),
                work: now.work.saturating_sub(before.work),
                pair_tests: now.pair_tests.saturating_sub(before.pair_tests),
                capped_batches: now.capped_batches.saturating_sub(before.capped_batches),
                refused_plots: now.refused_plots.saturating_sub(before.refused_plots),
            };
        }
    }
    let mut association_radar = [RadarAssociationStats::default(); MAX_RADARS];
    if cfg.association_diagnostics && measure_done > 0 {
        for ((delta, now), before) in association_radar
            .iter_mut()
            .zip(fusion_stats(&f).association_radar)
            .zip(warm.0.association_radar)
        {
            *delta = RadarAssociationStats {
                anonymous_offered: now
                    .anonymous_offered
                    .saturating_sub(before.anonymous_offered),
                completed: now.completed.saturating_sub(before.completed),
                incomplete: now.incomplete.saturating_sub(before.incomplete),
                coarse_candidates_visited: now
                    .coarse_candidates_visited
                    .saturating_sub(before.coarse_candidates_visited),
                exact_checks: now.exact_checks.saturating_sub(before.exact_checks),
                accepted_pairs: now.accepted_pairs.saturating_sub(before.accepted_pairs),
                capped_batches: now.capped_batches.saturating_sub(before.capped_batches),
            };
        }
    }
    Ok(CapResult {
        n,
        layout: cfg.traffic.layout,
        init_diagnostics: cfg.init_diagnostics,
        association_diagnostics: cfg.association_diagnostics,
        assignment_forensics: cfg.assignment_forensics,
        adsb_aircraft: traffic.adsb_count(),
        half_km: half_km(&cfg.traffic),
        warmup_s: cfg.warmup_s,
        measure_s_asked: cfg.measure_s,
        measure_s_done: measure_done,
        truncated,
        stopped_early,
        max_tracks: cfg.max_tracks,
        wall_measured_s: wall_measured,
        wall_total_s: wall_total,
        realtime_ratio: if measure_done > 0 {
            wall_measured / md
        } else {
            wall_total / seconds_done.max(1) as f64
        },
        seconds_over_1s: adv_ms.iter().filter(|&&m| m >= 1000.0).count(),
        advance_ms,
        warmup_advance_ms: dist_of(&warm_ms),
        adsb_per_s: sum_adsb as f64 / md,
        plots_per_s: sum_plots as f64 / md,
        rows_per_s: sum_rows as f64 / md,
        age_unknown,
        age_tick_ms: h_tick.summary(),
        age_tick_adsb_ms: h_tick_a.summary(),
        age_tick_radar_ms: h_tick_r.summary(),
        age_update_ms: h_upd.summary(),
        age_coast_ms: h_coast.summary(),
        delay_adsb_ms: h_da.summary(),
        delay_plot_ms: h_dp.summary(),
        fusion: *fusion_stats(&f),
        init_radar,
        association_radar,
        tracker: *fusion_tracker_stats(&f),
        out_overflow: out.overflow,
        fusion_warm: warm.0,
        tracker_warm: warm.1,
        out_overflow_warm: warm.2,
        tracks_tentative: tent,
        tracks_confirmed: conf,
        offered_aircraft_unique: offered.unique,
        offered_aircraft_expected: n,
        unknown_input_labels: offered.unknown,
        adsb_dropout_reports,
        adsb_dropout_s: cfg.adsb_dropout_s,
        radar_no_altitude: cfg.radar_no_altitude,
        radar_altitudes_removed,
        continuity_cohort: continuity_cohort.len(),
        continuity_noncoast,
        continuity_addresses: continuity_cohort.into_iter().collect(),
        truth_score,
        truth_score_unavailable,
        assignment_forensics_rows,
        assignment_forensics_unavailable,
        radar_track_recs_taken: recs_taken,
        bookkeeping: fusion_bookkeeping_len(&f),
        buffers_stable: fusion_buffers(&f) == buffers0,
        tracker_table_mb,
        rss_before_kb,
        rss_after_new_kb,
        rss_end_kb: status_kb("VmRSS").unwrap_or(0),
        rss_warm_kb,
        hwm_kb: status_kb("VmHWM").unwrap_or(0),
        load_start,
        load_end: loadavg3(),
        seconds,
    })
}

/// The machine-readable result: `key=value` lines.
pub fn cap_kv(r: &CapResult) -> Kv {
    let mut kv = Kv::default();
    kv.put("label", "loaded machine, preliminary");
    kv.put("n", r.n);
    kv.put("layout", r.layout.name());
    kv.put("adsb_aircraft", r.adsb_aircraft);
    kv.put("box_half_km", fmt_f(r.half_km));
    kv.put("warmup_s", r.warmup_s);
    kv.put("measure_s_asked", r.measure_s_asked);
    kv.put("measure_s_done", r.measure_s_done);
    kv.put("truncated", u8::from(r.truncated));
    kv.put("stopped_early", u8::from(r.stopped_early));
    kv.put("max_tracks", r.max_tracks);
    kv.put("wall_measured_s", fmt_f(r.wall_measured_s));
    kv.put("wall_total_s", fmt_f(r.wall_total_s));
    kv.put("realtime_ratio", format!("{:.4}", r.realtime_ratio));
    kv.put("in_time", u8::from(r.in_time()));
    kv.put("lost_work", u8::from(r.lost_work()));
    kv.put("keeps_up", u8::from(r.keeps_up()));
    kv.put("keeps_up_strict", u8::from(r.keeps_up_strict()));
    kv.put("seconds_over_1s", r.seconds_over_1s);
    kv.put_dist("advance", "ms", &r.advance_ms);
    kv.put_dist("warmup_advance", "ms", &r.warmup_advance_ms);
    kv.put("adsb_per_s", fmt_f(r.adsb_per_s));
    kv.put("plots_per_s", fmt_f(r.plots_per_s));
    kv.put("rows_per_s", fmt_f(r.rows_per_s));
    if r.init_diagnostics {
        kv.put("init_diagnostics", 1);
        for (radar, d) in r.init_radar.iter().enumerate() {
            if d.batches == 0 {
                continue;
            }
            let prefix = format!("init.radar{radar}.");
            kv.put(&format!("{prefix}batches"), d.batches);
            kv.put(&format!("{prefix}plots"), d.plots);
            kv.put(&format!("{prefix}opening_work"), d.opening_work);
            kv.put(
                &format!("{prefix}retirement_requested"),
                d.retirement_requested,
            );
            kv.put(&format!("{prefix}retirement_denied"), d.retirement_denied);
            kv.put(&format!("{prefix}work"), d.work);
            kv.put(&format!("{prefix}pair_tests"), d.pair_tests);
            kv.put(&format!("{prefix}capped_batches"), d.capped_batches);
            kv.put(&format!("{prefix}refused_plots"), d.refused_plots);
        }
    }
    if r.association_diagnostics {
        kv.put("association_diagnostics", 1);
        for (radar, d) in r.association_radar.iter().enumerate() {
            if d.anonymous_offered == 0 {
                continue;
            }
            let prefix = format!("association.radar{radar}.");
            kv.put(&format!("{prefix}anonymous_offered"), d.anonymous_offered);
            kv.put(&format!("{prefix}completed"), d.completed);
            kv.put(&format!("{prefix}incomplete"), d.incomplete);
            kv.put(
                &format!("{prefix}coarse_candidates_visited"),
                d.coarse_candidates_visited,
            );
            kv.put(&format!("{prefix}exact_checks"), d.exact_checks);
            kv.put(&format!("{prefix}accepted_pairs"), d.accepted_pairs);
            kv.put(&format!("{prefix}capped_batches"), d.capped_batches);
        }
    }
    if let Some(s) = r.adsb_dropout_s {
        kv.put("adsb_dropout_reports", r.adsb_dropout_reports);
        kv.put("adsb_dropout_start_s", r.warmup_s);
        kv.put("adsb_dropout_duration_s", s);
        kv.put("continuity_cohort", r.continuity_cohort);
        if r.continuity_cohort > 0 {
            if let Some(noncoast) = r.continuity_noncoast.iter().filter_map(|(_, n)| *n).min() {
                kv.put("continuity_noncoast_min", noncoast);
            }
        }
        kv.put(
            "continuity_uncertain_seconds",
            r.continuity_noncoast
                .iter()
                .filter(|(_, n)| n.is_none())
                .count(),
        );
    }
    if r.radar_no_altitude {
        kv.put("radar_degradation_start_s", r.warmup_s);
        kv.put("radar_no_altitude", u8::from(r.radar_no_altitude));
        kv.put("radar_altitudes_removed", r.radar_altitudes_removed);
    }
    if let Some(score) = r.truth_score {
        kv.put("truth_score.status", "available");
        kv.put(
            "truth_score.snapshot_time_s",
            r.warmup_s.saturating_add(r.measure_s_done),
        );
        kv.put("truth_score.snapshot_tracks", score.snapshot_tracks);
        kv.put("truth_score.offered_aircraft", score.offered_aircraft);
        kv.put("truth_score.matched_aircraft", score.matched_aircraft);
        kv.put("truth_score.missing_aircraft", score.missing_aircraft);
        kv.put("truth_score.duplicate_tracks", score.duplicate_tracks);
        kv.put("truth_score.false_tracks", score.false_tracks);
        kv.put("truth_score.wrong_plots", score.wrong_plots);
        kv.put("truth_score.false_applied", score.false_applied);
        kv.put("truth_score.swaps", score.swaps);
        kv.put(
            "truth_score.measured_wrong_plots",
            score.measured_wrong_plots,
        );
        kv.put("truth_score.measured_swaps", score.measured_swaps);
        kv.put(
            "truth_score.assignment_rows_measured",
            score.measured_assignment_rows,
        );
        kv.put(
            "truth_score.assignment_rows_run_cumulative",
            score.assignment_rows,
        );
        if let Some(timing) = score.first_confirmed_s {
            kv.put("truth_score.first_confirmed.status", "available");
            kv.put_dist("truth_score.first_confirmed", "s", &timing);
        } else {
            kv.put("truth_score.first_confirmed.status", "unavailable");
        }
    } else if let Some(reason) = r.truth_score_unavailable {
        kv.put("truth_score.status", "unavailable");
        kv.put("truth_score.reason", reason);
    }
    if r.assignment_forensics {
        if let Some(rows) = &r.assignment_forensics_rows {
            kv.put("assignment_forensics.status", "available");
            kv.put("assignment_forensics.wrong_rows", rows.len());
        } else {
            kv.put("assignment_forensics.status", "unavailable");
            kv.put(
                "assignment_forensics.reason",
                r.assignment_forensics_unavailable
                    .unwrap_or("unknown reason"),
            );
        }
    }
    kv.put("age_unknown_rows", r.age_unknown);
    kv.put_dist("age_tick_rows", "ms", &r.age_tick_ms);
    kv.put_dist("age_tick_adsb_keys", "ms", &r.age_tick_adsb_ms);
    kv.put_dist("age_tick_radar_keys", "ms", &r.age_tick_radar_ms);
    kv.put_dist("age_update_rows", "ms", &r.age_update_ms);
    kv.put_dist("age_coast_rows", "ms", &r.age_coast_ms);
    kv.put_dist("delay_adsb", "ms", &r.delay_adsb_ms);
    kv.put_dist("delay_plot", "ms", &r.delay_plot_ms);
    let warm = loss_counters(&r.fusion_warm, &r.tracker_warm, r.out_overflow_warm);
    for ((name, v), (_, w)) in loss_counters(&r.fusion, &r.tracker, r.out_overflow)
        .into_iter()
        .zip(warm)
    {
        kv.put(&format!("loss.{name}"), v.saturating_sub(w));
    }
    let mut measured_fusion = r.fusion;
    measured_fusion.forensics_overflow = measured_fusion
        .forensics_overflow
        .saturating_sub(r.fusion_warm.forensics_overflow);
    measured_fusion.forensics_pair_visits = measured_fusion
        .forensics_pair_visits
        .saturating_sub(r.fusion_warm.forensics_pair_visits);
    measured_fusion.forensics_plot_visits = measured_fusion
        .forensics_plot_visits
        .saturating_sub(r.fusion_warm.forensics_plot_visits);
    measured_fusion.forensics_metadata_writes = measured_fusion
        .forensics_metadata_writes
        .saturating_sub(r.fusion_warm.forensics_metadata_writes);
    measured_fusion.forensics_regret_work = measured_fusion
        .forensics_regret_work
        .saturating_sub(r.fusion_warm.forensics_regret_work);
    measured_fusion.forensics_regret_unavailable = measured_fusion
        .forensics_regret_unavailable
        .saturating_sub(r.fusion_warm.forensics_regret_unavailable);
    for (name, v) in fusion_counters(&measured_fusion) {
        kv.put(&format!("fusion.{name}"), v);
    }
    for (name, final_value, warm_value) in [
        (
            "polar_index_work",
            r.fusion.polar_index_work,
            r.fusion_warm.polar_index_work,
        ),
        (
            "polar_index_build_work",
            r.fusion.polar_index_build_work,
            r.fusion_warm.polar_index_build_work,
        ),
        (
            "polar_index_query_work",
            r.fusion.polar_index_query_work,
            r.fusion_warm.polar_index_query_work,
        ),
        (
            "polar_index_queries",
            r.fusion.polar_index_queries,
            r.fusion_warm.polar_index_queries,
        ),
        (
            "polar_index_track_visits",
            r.fusion.polar_index_track_visits,
            r.fusion_warm.polar_index_track_visits,
        ),
    ] {
        kv.put(
            &format!("fusion_measured.{name}"),
            final_value.saturating_sub(warm_value),
        );
    }
    for (name, v) in tracker_counters(&r.tracker) {
        kv.put(&format!("tracker.{name}"), v);
    }
    kv.put("out_overflow", r.out_overflow);
    kv.put("tracks_tentative", r.tracks_tentative);
    kv.put("tracks_confirmed", r.tracks_confirmed);
    kv.put("offered_aircraft_unique", r.offered_aircraft_unique);
    kv.put("offered_aircraft_expected", r.offered_aircraft_expected);
    kv.put("unknown_input_labels", r.unknown_input_labels);
    kv.put("radar_track_recs_taken", r.radar_track_recs_taken);
    kv.put("keyinfo_len", r.bookkeeping.0);
    kv.put("radar_track_recs_held", r.bookkeeping.1);
    kv.put("buffers_stable", u8::from(r.buffers_stable));
    kv.put("tracker_table_mb", fmt_f(r.tracker_table_mb));
    kv.put("rss_before_kb", r.rss_before_kb);
    kv.put("rss_after_new_kb", r.rss_after_new_kb);
    kv.put("rss_end_kb", r.rss_end_kb);
    kv.put("rss_warm_kb", r.rss_warm_kb);
    kv.put("vmhwm_kb", r.hwm_kb);
    kv.put("load_start", &r.load_start);
    kv.put("load_end", &r.load_end);
    kv
}

/// The per-second table of the measured seconds, as CSV.
pub fn seconds_csv(r: &CapResult) -> String {
    if r.continuity_noncoast.is_empty() {
        let mut s = String::from("second,wall_ms,adsb_offered,plots_offered,rows_emitted\n");
        for (k, ms, a, p, rows) in &r.seconds {
            s.push_str(&format!("{k},{ms:.3},{a},{p},{rows}\n"));
        }
        return s;
    }
    let mut s = String::from(
        "second,wall_ms,adsb_offered,plots_offered,rows_emitted,cohort_noncoast,cohort_missing\n",
    );
    for ((k, ms, a, p, rows), (_, count)) in r.seconds.iter().zip(&r.continuity_noncoast) {
        match count {
            Some(noncoast) if r.continuity_cohort > 0 => s.push_str(&format!(
                "{k},{ms:.3},{a},{p},{rows},{noncoast},{}\n",
                r.continuity_cohort.saturating_sub(*noncoast)
            )),
            Some(_) => s.push_str(&format!(
                "{k},{ms:.3},{a},{p},{rows},,empty-cutover-cohort\n"
            )),
            None => s.push_str(&format!(
                "{k},{ms:.3},{a},{p},{rows},,uncertain-output-overflow\n"
            )),
        }
    }
    s
}

/// CSV for the bounded measured-window wrong-assignment sidecar; absent when export failed closed.
pub fn assignment_forensics_csv(r: &CapResult) -> Option<String> {
    let rows = r.assignment_forensics_rows.as_ref()?;
    let mut out = String::from(
        "t_ns,plot_ordinal,radar,plot_addr,truth_label,action,selected_truth,plot_range_m,plot_az_deg,plot_alt_ft,plot_mode_a,selected_key,selected_cost,alternate_key,alternate_truth,alternate_cost,ambiguous,hold_run,forced,component_size,solver_path,exclusion_regret,regret_status,incomplete,capped\n",
    );
    for row in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{:.9},{:.9},{},{},{},{:.12},{},{},{:.12},{},{},{},{},{},{:.12},{},{},{}\n",
            row.t_ns,
            row.plot_ordinal,
            row.radar,
            row.plot_addr,
            row.truth_label,
            row.action,
            row.selected_truth,
            row.plot_range_m,
            row.plot_az_deg,
            row.plot_alt_ft,
            row.plot_mode_a,
            row.selected_key,
            row.selected_cost,
            row.alternate_key,
            row.alternate_truth,
            row.alternate_cost,
            u8::from(row.ambiguous),
            row.hold_run,
            u8::from(row.forced),
            row.component_size,
            row.solver_path,
            row.exclusion_regret,
            row.regret_status,
            u8::from(row.incomplete),
            u8::from(row.capped),
        ));
    }
    Some(out)
}

fn dist_line(name: &str, unit: &str, d: &Dist) -> String {
    format!(
        "  {name:<34} n={:<9} p50 {:>9} p99 {:>9} p99.9 {:>9} max {:>9} {unit}\n",
        d.n,
        fmt_f(d.p50),
        fmt_f(d.p99),
        fmt_f(d.p999),
        fmt_f(d.max)
    )
}

/// The plain-text summary of one run.
pub fn cap_text(r: &CapResult) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "== capacity n={} layout={} [loaded machine, preliminary; load {} -> {}] ==\n",
        r.n,
        r.layout.name(),
        r.load_start,
        r.load_end
    ));
    s.push_str(&format!(
        "  aircraft {} ({} send ADS-B), box +-{:.0} km, tracker capacity {}\n",
        r.n, r.adsb_aircraft, r.half_km, r.max_tracks
    ));
    s.push_str(&format!(
        "  traffic: warm-up {} s, measured {} of {} s{}; per second: {:.0} ADS-B, {:.0} plots, {:.0} rows out\n",
        r.warmup_s,
        r.measure_s_done,
        r.measure_s_asked,
        if r.truncated {
            " (STOPPED by the wall limit)"
        } else if r.stopped_early {
            " (STOPPED EARLY: far slower than real time)"
        } else {
            ""
        },
        r.adsb_per_s,
        r.plots_per_s,
        r.rows_per_s
    ));
    let verdict = if r.keeps_up() {
        "keeps up".to_string()
    } else if r.in_time() {
        "LOST WORK (fast enough, not a pass)".to_string()
    } else {
        "does not keep up".to_string()
    };
    s.push_str(&format!(
        "  processing time per second of traffic: {:.1} ms = {:.3} x real time; {} seconds took 1 s or more; slowest advance {:.1} ms; verdict: {} (every measured second under 1 s, no work lost; strict: {})\n",
        r.realtime_ratio * 1000.0,
        r.realtime_ratio,
        r.seconds_over_1s,
        r.advance_ms.max,
        verdict,
        r.keeps_up_strict()
    ));
    s.push_str(&dist_line("per-advance wall time", "ms", &r.advance_ms));
    s.push_str(&dist_line(
        "  (warm-up seconds, for comparison)",
        "ms",
        &r.warmup_advance_ms,
    ));
    s.push_str(&format!(
        "  measured polar-index work: {} total = {} build + {} query; {} queries; {} build track visits\n",
        r.fusion
            .polar_index_work
            .saturating_sub(r.fusion_warm.polar_index_work),
        r.fusion
            .polar_index_build_work
            .saturating_sub(r.fusion_warm.polar_index_build_work),
        r.fusion
            .polar_index_query_work
            .saturating_sub(r.fusion_warm.polar_index_query_work),
        r.fusion
            .polar_index_queries
            .saturating_sub(r.fusion_warm.polar_index_queries),
        r.fusion
            .polar_index_track_visits
            .saturating_sub(r.fusion_warm.polar_index_track_visits),
    ));
    s.push_str(&dist_line(
        "input-to-effect delay, ADS-B",
        "ms traffic",
        &r.delay_adsb_ms,
    ));
    s.push_str(&dist_line(
        "input-to-effect delay, plots",
        "ms traffic",
        &r.delay_plot_ms,
    ));
    s.push_str(&dist_line(
        "data age of tick rows",
        "ms traffic",
        &r.age_tick_ms,
    ));
    s.push_str(&dist_line(
        "  tick rows, address-keyed tracks",
        "ms traffic",
        &r.age_tick_adsb_ms,
    ));
    s.push_str(&dist_line(
        "  tick rows, radar-only tracks",
        "ms traffic",
        &r.age_tick_radar_ms,
    ));
    s.push_str(&dist_line(
        "data age of update rows (0 by design)",
        "ms traffic",
        &r.age_update_ms,
    ));
    s.push_str(&dist_line(
        "data age of coasting rows",
        "ms traffic",
        &r.age_coast_ms,
    ));
    s.push_str(&format!(
        "  rows with no findable track at the read: {}\n",
        r.age_unknown
    ));
    s.push_str(&format!(
        "  tracking hypotheses at the end: {} confirmed, {} tentative (input aircraft {})\n",
        r.tracks_confirmed, r.tracks_tentative, r.n
    ));
    s.push_str(&format!(
        "  measured input truth coverage: {}/{} unique aircraft offered; unknown labels {}\n",
        r.offered_aircraft_unique, r.offered_aircraft_expected, r.unknown_input_labels
    ));
    if let Some(dropout_s) = r.adsb_dropout_s {
        let min_noncoast = r.continuity_noncoast.iter().filter_map(|(_, n)| *n).min();
        let uncertain = r
            .continuity_noncoast
            .iter()
            .filter(|(_, n)| n.is_none())
            .count();
        let mode = if dropout_s == 0 {
            "steady-feed continuity control"
        } else {
            "ADS-B outage"
        };
        let interval = if dropout_s == 0 {
            format!(
                "continuity measured through seconds {}..{}",
                r.warmup_s,
                r.warmup_s.saturating_add(r.measure_s_done)
            )
        } else {
            format!(
                "measured seconds {}..{} ({}s)",
                r.warmup_s,
                r.warmup_s.saturating_add(dropout_s),
                dropout_s
            )
        };
        let coverage = if r.continuity_cohort == 0 {
            "empty cutover cohort (zero denominator); missing count unavailable".to_string()
        } else {
            min_noncoast.map_or_else(
                || "no complete-output seconds; cohort missing count unavailable".to_string(),
                |count| {
                    format!(
                        "minimum {count}/{} non-coasting rows in complete-output seconds ({} missing at minimum)",
                        r.continuity_cohort,
                        r.continuity_cohort.saturating_sub(count)
                    )
                },
            )
        };
        s.push_str(&format!(
            "  {mode}: {interval}, withheld {} generated ADS-B reports; cutover cohort of {} confirmed address-bearing ADS-B tracks: {coverage}; {} seconds uncertain due to output overflow. Non-coasting tracker rows are a continuity proxy, not a sensor freshness test; anonymous radar tracks are excluded.\n",
            r.adsb_dropout_reports, r.continuity_cohort, uncertain
        ));
    }
    if r.radar_no_altitude {
        s.push_str(&format!(
            "  measured-window radar degradation: removed {} Mode C altitudes from generated anonymous plots before fusion.\n",
            r.radar_altitudes_removed
        ));
    }
    if let Some(score) = r.truth_score {
        s.push_str(&format!(
            "  truth-derived final snapshot at t={} s: {} confirmed keys, {} / {} distinct known measured-window truth aircraft matched, {} missing; {} duplicate tracks, {} false tracks, {} wrong-aircraft plots, {} synthetic false plots applied, {} swaps. Identity uses cumulative assignments since run start, filtered to final snapshot keys ({} logged rows total). Timings include assignment logging.\n",
            r.warmup_s.saturating_add(r.measure_s_done),
            score.snapshot_tracks,
            score.matched_aircraft,
            score.offered_aircraft,
            score.missing_aircraft,
            score.duplicate_tracks,
            score.false_tracks,
            score.wrong_plots,
            score.false_applied,
            score.swaps,
            score.assignment_rows
        ));
        s.push_str(&format!(
            "  measured-window identity evidence: {} wrong-aircraft plots and {} swaps in {} assignment rows.\n",
            score.measured_wrong_plots, score.measured_swaps, score.measured_assignment_rows
        ));
        match score.first_confirmed_s {
            Some(timing) if timing.n > 0 => s.push_str(&format!(
                "  first confirmed appearance of matched final tracks: n={} p50={:.0} s p90={:.0} s max={:.0} s from run start\n",
                timing.n, timing.p50, timing.p90, timing.max
            )),
            Some(_) => s.push_str("  first confirmed appearance: no matched final tracks\n"),
            None => s.push_str("  first confirmed appearance: key history unavailable\n"),
        }
    } else if let Some(reason) = r.truth_score_unavailable {
        s.push_str(&format!(
            "  truth-derived final snapshot: unavailable ({reason})\n"
        ));
    }
    if r.assignment_forensics {
        if let Some(rows) = &r.assignment_forensics_rows {
            s.push_str(&format!(
                "  assignment forensics: exported {} measured-window wrong-applied rows; row count matches the measured truth score.\n",
                rows.len()
            ));
        } else {
            s.push_str(&format!(
                "  assignment forensics: unavailable ({})\n",
                r.assignment_forensics_unavailable
                    .unwrap_or("unknown reason")
            ));
        }
        if r.fusion
            .forensics_overflow
            .saturating_sub(r.fusion_warm.forensics_overflow)
            > 0
        {
            s.push_str(
                "  forensics row-cap overflow is diagnostic evidence loss, not tracker work loss; no partial CSV is written.\n",
            );
        }
    }
    let moved: Vec<String> = r
        .loss()
        .into_iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect();
    s.push_str(&format!(
        "  work-loss counters moved in the measured window: {}\n",
        if moved.is_empty() {
            "none".to_string()
        } else {
            moved.join(" ")
        },
    ));
    if r.init_diagnostics {
        s.push_str("  per-radar initiation during measurement (work in charged units):\n");
        for (radar, d) in r.init_radar.iter().enumerate() {
            if d.batches == 0 {
                continue;
            }
            s.push_str(&format!(
                "    radar {radar}: {} batches, {} plots, {} work available at batch start, {} consumed, {} retirement work requested, {} reservations denied, {} pair tests, {} capped batches, {} refused plots\n",
                d.batches, d.plots, d.opening_work, d.work, d.retirement_requested,
                d.retirement_denied, d.pair_tests, d.capped_batches, d.refused_plots
            ));
        }
    }
    if r.association_diagnostics {
        s.push_str("  per-radar anonymous association during measurement (gate completion independent of assignment):\n");
        for (radar, d) in r.association_radar.iter().enumerate() {
            if d.anonymous_offered == 0 {
                continue;
            }
            s.push_str(&format!(
                "    radar {radar}: {} anonymous offered, {} completed, {} incomplete, {} coarse candidates visited, {} exact checks, {} accepted pairs, {} capped batches\n",
                d.anonymous_offered, d.completed, d.incomplete,
                d.coarse_candidates_visited, d.exact_checks, d.accepted_pairs, d.capped_batches
            ));
        }
    }
    s.push_str(&format!(
        "  buffers unchanged across the run: {}; radar-only track records handed on: {}\n",
        r.buffers_stable, r.radar_track_recs_taken
    ));
    s.push_str(&format!(
        "  memory: RSS {:.0} MB before the layer, {:.0} MB after it is made (tracker tables {:.0} MB), {:.0} MB at the end of the warm-up, {:.0} MB at the end; peak VmHWM {:.0} MB\n",
        r.rss_before_kb as f64 / 1024.0,
        r.rss_after_new_kb as f64 / 1024.0,
        r.tracker_table_mb,
        r.rss_warm_kb as f64 / 1024.0,
        r.rss_end_kb as f64 / 1024.0,
        r.hwm_kb as f64 / 1024.0
    ));
    s
}

/// The radars of a capacity run from a list such as `e,t,t`.
pub fn parse_radars(s: &str) -> Result<Vec<Kind>, String> {
    s.split(',')
        .map(|x| match x.trim() {
            "e" | "enroute" => Ok(Kind::EnRoute),
            "t" | "terminal" => Ok(Kind::Terminal),
            other => Err(format!("radar kind '{other}' (use e or t)")),
        })
        .collect()
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

    /// Sets the counter `name` (as `loss_counters` names it) to 1 in the result's totals.
    fn set_loss(r: &mut CapResult, name: &str) {
        let (f, t) = (&mut r.fusion, &mut r.tracker);
        match name {
            "fusion.plots_refused" => f.plots_refused = 1,
            "fusion.adsb_address_refused" => f.adsb_address_refused = 1,
            "fusion.late" => f.late = 1,
            "fusion.pending_overflow" => f.pending_overflow = 1,
            "fusion.hold_evicted" => f.hold_evicted = 1,
            "fusion.bucket_plot_overflow" => f.bucket_plot_overflow = 1,
            "fusion.bucket_work_capped" => f.bucket_work_capped = 1,
            "fusion.anon_plots_ungated" => f.anon_plots_ungated = 1,
            "fusion.assoc_incomplete_batches" => f.assoc_incomplete_batches = 1,
            "fusion.assoc_incomplete_deferred" => f.assoc_incomplete_deferred = 1,
            "fusion.assoc_refused_deferred" => f.assoc_refused_deferred = 1,
            "fusion.init_capped" => f.init_capped = 1,
            "fusion.init_plots_refused" => f.init_plots_refused = 1,
            "fusion.assoc_refused" => f.assoc_refused = 1,
            "fusion.cand_capacity" => f.cand_capacity = 1,
            "fusion.cand_no_height" => f.cand_no_height = 1,
            "fusion.no_height_pairs" => f.no_height_pairs = 1,
            "fusion.track_refused" => f.track_refused = 1,
            "fusion.assign_log_overflow" => f.assign_log_overflow = 1,
            "fusion.plot_scan_full" => f.plot_scan_full = 1,
            "fusion.prediction_refused" => f.prediction_refused = 1,
            "fusion.wide_full" => f.wide_full = 1,
            "fusion.init_wide_full" => f.init_wide_full = 1,
            "fusion.track_rec_overflow" => f.track_rec_overflow = 1,
            "fusion.keyinfo_overflow" => f.keyinfo_overflow = 1,
            "fusion.scratch_full" => f.scratch_full = 1,
            "fusion.reg_history_full" => f.reg_history_full = 1,
            "tracker.capacity_refused" => t.capacity_refused = 1,
            "tracker.late_window" => t.late_window = 1,
            "tracker.late_ring" => t.late_ring = 1,
            "tracker.time_held_discarded" => t.time_held = t.time_held_applied + 1,
            "tracker.time_future_dropped" => t.time_future_dropped = 1,
            "tracker.time_sensor_evicted" => t.time_sensor_evicted = 1,
            "tracker.rows_unrepresentable" => t.rows_unrepresentable = 1,
            "tracker.nan_covariance" => t.nan_covariance = 1,
            "tracker.numerical_refused" => t.numerical_refused = 1,
            "tracker.moves_rollback_lost_late" => t.moves_rollback_lost_late = 1,
            "tracker.move_no_snapshot" => t.move_no_snapshot = 1,
            "tracker.move_widen_failed" => t.move_widen_failed = 1,
            "tracker.moves_internal_refused" => t.moves_internal_refused = 1,
            "out.overflow" => r.out_overflow = 1,
            n if n.starts_with("tracker.refused_") => t.refused[0] = 1,
            n => panic!("no setter for {n}"),
        }
    }

    /// The top-level field names of a derived `Debug` string.
    fn debug_fields(d: &str) -> Vec<String> {
        let (mut out, mut depth, mut word) = (Vec::new(), 0i32, String::new());
        let b: Vec<char> = d.chars().collect();
        for (i, &c) in b.iter().enumerate() {
            match c {
                '{' | '[' | '(' => depth += 1,
                '}' | ']' | ')' => depth -= 1,
                _ => {}
            }
            if depth == 1 && (c.is_alphanumeric() || c == '_') {
                word.push(c);
            } else {
                if depth == 1 && c == ':' && b.get(i + 1) == Some(&' ') && !word.is_empty() {
                    out.push(word.clone());
                }
                word.clear();
            }
        }
        out
    }

    fn small(n: usize, warm: i64, meas: i64) -> CapCfg {
        CapCfg {
            traffic: TrafficCfg {
                seed: 1,
                n,
                layout: Layout::Stress,
                adsb_frac: 0.5,
                radars: vec![Kind::EnRoute, Kind::Terminal, Kind::Terminal],
                horizon_s: (warm + meas) as f64 + 60.0,
            },
            warmup_s: warm,
            measure_s: meas,
            max_tracks: 4096,
            max_wall_s: 120.0,
            early_stop: false,
            adsb_dropout_s: None,
            radar_no_altitude: false,
            truth_score: false,
            init_diagnostics: false,
            association_diagnostics: false,
            assignment_forensics: false,
        }
    }

    #[test]
    fn association_diagnostics_attribute_gate_outcomes_and_work_per_radar() {
        let mut cfg = small(48, 24, 15);
        cfg.traffic.adsb_frac = 0.0;
        let default = run_capacity(&cfg).unwrap();
        assert!(!default.association_diagnostics);
        assert_eq!(cap_kv(&default).get("association_diagnostics"), None);

        cfg.association_diagnostics = true;
        let result = run_capacity(&cfg).unwrap();
        assert_eq!(result.tracks_confirmed, default.tracks_confirmed);
        let active = result.association_radar.iter().take(3).collect::<Vec<_>>();
        assert!(active.iter().all(|d| d.anonymous_offered > 0));
        assert!(active
            .iter()
            .all(|d| d.completed + d.incomplete == d.anonymous_offered));
        assert_eq!(
            result
                .association_radar
                .iter()
                .map(|d| d.coarse_candidates_visited)
                .sum::<u64>(),
            result.fusion.coarse_visits - result.fusion_warm.coarse_visits
        );
        assert_eq!(
            result
                .association_radar
                .iter()
                .map(|d| d.exact_checks)
                .sum::<u64>(),
            result.fusion.pairs_tested - result.fusion_warm.pairs_tested
        );
        assert_eq!(
            result
                .association_radar
                .iter()
                .map(|d| d.accepted_pairs)
                .sum::<u64>(),
            result.fusion.pairs_gated - result.fusion_warm.pairs_gated
        );
        assert_eq!(cap_kv(&result).get("association_diagnostics"), Some("1"));
    }

    // REQ: OA-PRF-013
    #[test]
    fn measured_window_can_remove_every_generated_radar_altitude() {
        let mut control_cfg = small(48, 24, 3);
        control_cfg.traffic.adsb_frac = 1.0;
        let control = run_capacity(&control_cfg).unwrap();

        let mut degraded_cfg = control_cfg;
        degraded_cfg.radar_no_altitude = true;
        let degraded = run_capacity(&degraded_cfg).unwrap();
        let offered_plots = degraded
            .seconds
            .iter()
            .map(|second| second.3 as u64)
            .sum::<u64>();

        assert_eq!(control.radar_altitudes_removed, 0);
        assert_eq!(degraded.radar_altitudes_removed, offered_plots);
        assert!(degraded.radar_altitudes_removed > 0);
        assert_eq!(
            control
                .seconds
                .iter()
                .map(|second| second.3)
                .collect::<Vec<_>>(),
            degraded
                .seconds
                .iter()
                .map(|second| second.3)
                .collect::<Vec<_>>()
        );
        let kv = cap_kv(&degraded);
        assert_eq!(kv.get("radar_no_altitude"), Some("1"));
        assert_eq!(kv.get("radar_degradation_start_s"), Some("24"));
    }

    #[test]
    fn initiation_diagnostics_attribute_measured_work_to_each_radar() {
        let mut cfg = small(48, 24, 15);
        cfg.traffic.adsb_frac = 0.0;
        let default = run_capacity(&cfg).unwrap();
        assert!(!default.init_diagnostics);
        assert_eq!(cap_kv(&default).get("init_diagnostics"), None);

        cfg.init_diagnostics = true;
        let result = run_capacity(&cfg).unwrap();
        assert_eq!(result.tracks_confirmed, default.tracks_confirmed);
        assert!(result.init_radar.iter().take(3).all(|radar| {
            radar.batches > 0 && radar.work <= radar.opening_work && radar.retirement_denied == 0
        }));
        assert_eq!(
            result
                .init_radar
                .iter()
                .map(|radar| radar.work)
                .sum::<u64>(),
            result.fusion.init_work - result.fusion_warm.init_work
        );
        assert_eq!(
            result
                .init_radar
                .iter()
                .map(|radar| radar.pair_tests)
                .sum::<u64>(),
            result.fusion.init_tests - result.fusion_warm.init_tests
        );
        assert_eq!(cap_kv(&result).get("init_diagnostics"), Some("1"));
    }

    #[test]
    fn paired_dropout_keeps_the_same_confirmed_cutover_roster_and_radar_schedule() {
        let mut control_cfg = small(60, 5, 6);
        control_cfg.adsb_dropout_s = Some(0);
        let control = run_capacity(&control_cfg).unwrap();
        let mut outage_cfg = control_cfg.clone();
        outage_cfg.adsb_dropout_s = Some(2);
        let outage = run_capacity(&outage_cfg).unwrap();

        assert!(!control.continuity_addresses.is_empty());
        assert_eq!(control.continuity_addresses, outage.continuity_addresses);
        assert_eq!(
            control.continuity_cohort,
            control.continuity_addresses.len()
        );
        assert_eq!(control.continuity_noncoast.len(), 6);
        assert_eq!(outage.continuity_noncoast.len(), 6);
        assert_eq!(control.adsb_dropout_reports, 0);
        assert_eq!(
            outage.adsb_dropout_reports,
            (outage.adsb_aircraft * 2) as u64
        );
        for (control_sec, outage_sec) in control.seconds.iter().zip(&outage.seconds) {
            assert_eq!(control_sec.0, outage_sec.0);
            assert_eq!(control_sec.3, outage_sec.3, "radar schedule changed");
        }
        assert!(outage.seconds[0].2 == 0 && outage.seconds[1].2 == 0);
        assert!(outage.seconds[2].2 > 0 && outage.seconds[3].2 > 0);
    }

    #[test]
    fn dropout_mode_requires_a_positive_warmup_and_bounded_interval() {
        let mut cfg = small(20, 0, 4);
        cfg.adsb_dropout_s = Some(0);
        assert!(run_capacity(&cfg).is_err());
        cfg = small(20, 2, 4);
        cfg.adsb_dropout_s = Some(5);
        assert!(run_capacity(&cfg).is_err());
    }

    #[test]
    fn opt_in_truth_score_uses_the_final_confirmed_snapshot() {
        let mut cfg = small(80, 10, 5);
        cfg.truth_score = true;
        let result = run_capacity(&cfg).unwrap();
        let score = result.truth_score.expect("complete assignment evidence");
        assert_eq!(result.truth_score_unavailable, None);
        assert!(score.snapshot_tracks > 0);
        assert!(score.assignment_rows > 0);
        assert!(score.offered_aircraft > 0);
        assert_eq!(
            score.matched_aircraft + score.missing_aircraft,
            score.offered_aircraft
        );
        let timing = score.first_confirmed_s.expect("complete first sightings");
        assert!(timing.n > 0);
        assert_eq!(timing.n, score.matched_aircraft);
        assert!(timing.min >= 1.0);
        assert!(timing.max <= (cfg.warmup_s + cfg.measure_s) as f64);
        assert_eq!(
            cap_kv(&result).get("truth_score.first_confirmed.status"),
            Some("available")
        );
        assert_eq!(result.fusion.assign_log_overflow, 0);
        assert!(cap_text(&result).contains("truth-derived final snapshot"));
        assert!(cap_kv(&result).get("truth_score.status").is_some());
        assert!(cap_kv(&result)
            .get("truth_score.measured_wrong_plots")
            .is_some());
    }

    #[test]
    fn assignment_forensics_csv_matches_measured_wrong_plot_total() {
        let mut cfg = small(24, 4, 12);
        cfg.truth_score = true;
        cfg.assignment_forensics = true;
        let mut result = run_capacity(&cfg).unwrap();
        let score = result.truth_score.expect("complete measured truth score");
        let rows = result
            .assignment_forensics_rows
            .as_ref()
            .expect("complete wrong-row export");
        assert_eq!(rows.len() as u64, score.measured_wrong_plots);
        assert!(rows.iter().all(|row| row.selected_truth != row.truth_label));
        let csv = assignment_forensics_csv(&result).expect("forensics CSV");
        assert!(
            csv.starts_with("t_ns,plot_ordinal,radar,plot_addr,truth_label,action,selected_truth")
        );
        assert!(csv.contains(",solver_path,exclusion_regret,regret_status,incomplete,"));
        assert!(rows.iter().all(|row| row.regret_status != 0));
        assert_eq!(csv.lines().count(), rows.len() + 1);
        let row_count = rows.len().to_string();
        assert_eq!(
            cap_kv(&result).get("assignment_forensics.wrong_rows"),
            Some(row_count.as_str())
        );

        result.assignment_forensics_rows = None;
        result.assignment_forensics_unavailable = Some("test cap status");
        assert_eq!(assignment_forensics_csv(&result), None);
        assert_eq!(
            cap_kv(&result).get("assignment_forensics.status"),
            Some("unavailable")
        );
        assert!(forensics_source_complete(0));
        assert!(!forensics_source_complete(1));
    }

    #[test]
    fn a_warmup_abort_has_no_measured_forensics_work() {
        let mut cfg = small(8, 3, 2);
        cfg.truth_score = true;
        cfg.assignment_forensics = true;
        cfg.max_wall_s = f64::MIN_POSITIVE;
        let result = run_capacity(&cfg).unwrap();
        assert!(result.truncated);
        assert_eq!(result.measure_s_done, 0);
        assert_eq!(result.fusion_warm, result.fusion);
        assert_eq!(result.tracker_warm, result.tracker);
        assert_eq!(result.out_overflow_warm, result.out_overflow);
        let kv = cap_kv(&result);
        for name in [
            "fusion.forensics_overflow",
            "fusion.forensics_pair_visits",
            "fusion.forensics_plot_visits",
            "fusion.forensics_metadata_writes",
            "fusion.forensics_regret_work",
            "fusion.forensics_regret_unavailable",
        ] {
            assert_eq!(kv.get(name), Some("0"), "{name}");
        }
    }

    #[test]
    fn truth_score_remains_available_during_a_bounded_adsb_dropout() {
        let mut cfg = small(80, 10, 5);
        cfg.truth_score = true;
        cfg.adsb_dropout_s = Some(3);
        cfg.radar_no_altitude = true;
        let result = run_capacity(&cfg).unwrap();
        let score = result.truth_score.expect("complete assignment evidence");
        assert_eq!(result.truth_score_unavailable, None);
        assert_eq!(result.adsb_dropout_s, Some(3));
        assert!(result.adsb_dropout_reports > 0);
        assert!(result.radar_altitudes_removed > 0);
        assert_eq!(
            score.matched_aircraft + score.missing_aircraft,
            score.offered_aircraft
        );
        let kv = cap_kv(&result);
        assert_eq!(kv.get("truth_score.status"), Some("available"));
        assert!(kv.get("truth_score.wrong_plots").is_some());
        assert!(kv.get("truth_score.measured_wrong_plots").is_some());
        assert!(score.measured_assignment_rows > 0);
    }

    // REQ: OA-PRF-012
    #[test]
    fn offered_coverage_counts_unique_truth_and_rejects_skips_or_unknown_labels() {
        // Three generated aircraft exist, but only two labels were offered; duplicate radar
        // sightings of the first aircraft count once, and an unknown label is explicit.
        let mut coverage = OfferedCoverage {
            by_addr: [(10, 0), (11, 1), (12, 2)].into_iter().collect(),
            seen: vec![false; 3],
            unique: 0,
            unknown: 0,
        };
        offered_mark(&mut coverage, 10); // ADS-B
        offered_mark(&mut coverage, 10); // same truth aircraft from another radar
        offered_mark(&mut coverage, 11);
        offered_mark(&mut coverage, 99); // not part of generated truth
        assert_eq!(coverage.unique, 2);
        assert_eq!(coverage.unknown, 1);
        assert_ne!(
            coverage.unique, 3,
            "a skipped truth aircraft must fail coverage"
        );
        assert_ne!(
            coverage.unknown, 0,
            "an unknown offered label must fail coverage"
        );
    }

    // REQ: OA-PRF-012
    #[test]
    fn incomplete_input_coverage_cannot_pass_a_fast_run() {
        let mut r = run_capacity(&small(20, 3, 5)).unwrap();
        assert!(r.keeps_up());
        r.offered_aircraft_unique -= 1;
        assert!(r.in_time(), "the control remains fast");
        assert!(!r.keeps_up());
        r.offered_aircraft_unique += 1;
        r.unknown_input_labels = 1;
        assert!(!r.keeps_up(), "an unknown offered label passed");
    }

    // REQ: OA-PRF-003
    #[test]
    fn a_small_run_processes_all_traffic_and_reports_every_figure() {
        let r = run_capacity(&small(100, 10, 20)).unwrap();
        assert_eq!(r.measure_s_done, 20);
        assert!(!r.truncated);
        assert_eq!(r.offered_aircraft_unique, r.offered_aircraft_expected);
        assert_eq!(r.offered_aircraft_expected, 100);
        assert_eq!(r.unknown_input_labels, 0);
        // All traffic was offered and processed; nothing was refused or cut.
        let f = &r.fusion;
        assert!(f.adsb_in > 0 && f.plots_in > 0, "{f:?}");
        assert_eq!(
            f.processed,
            f.adsb_in + f.plots_in - f.plots_refused - f.late
        );
        assert!(r.loss().is_empty(), "{:?}", r.loss());
        assert_eq!(r.out_overflow, 0);
        assert_eq!(r.tracker.capacity_refused, 0);
        assert!(r.buffers_stable);
        // Output has confirmed hypotheses; input coverage above establishes the offered load.
        // Unidentified aliases remain visible rather than being merged to hit an upper count.
        assert!(r.tracks_confirmed >= 95, "{}", r.tracks_confirmed);
        // Timing and age figures exist and are ordered.
        assert_eq!(r.advance_ms.n, 20);
        assert!(r.advance_ms.p50 <= r.advance_ms.p99 && r.advance_ms.p99 <= r.advance_ms.max);
        assert!(r.realtime_ratio > 0.0 && r.keeps_up());
        assert!(r.age_tick_ms.n > 0 && r.age_update_ms.n > 0);
        // A tick row is at most about a report interval old; an update row is age 0.
        assert_eq!(r.age_update_ms.max, 0.0);
        // ADS-B-keyed tracks hear 1 Hz reports: their tick rows are about a second old at most.
        assert!(
            r.age_tick_adsb_ms.n > 0 && r.age_tick_adsb_ms.p99 <= 2_000.0,
            "{:?}",
            r.age_tick_adsb_ms
        );
        // The delay from an input's time to the end of its second is under a second.
        assert!(r.delay_adsb_ms.max <= 1000.0 && r.delay_plot_ms.max <= 1000.0);
        // Linux documents VmRSS and VmHWM as approximate. Separate /proc reads do not
        // establish an exact ordering; retain and report each observed sample unchanged.
        #[cfg(target_os = "linux")]
        assert!(
            r.rss_before_kb > 0 && r.rss_after_new_kb > 0 && r.rss_end_kb > 0 && r.hwm_kb > 0,
            "memory samples: before={}, after={}, end={}, hwm={}",
            r.rss_before_kb,
            r.rss_after_new_kb,
            r.rss_end_kb,
            r.hwm_kb
        );
        let kv = cap_kv(&r);
        assert_eq!(kv.get("n"), Some("100"));
        assert!(kv.get("advance.p99_ms").is_some());
        for (name, value) in [
            ("rss_before_kb", r.rss_before_kb),
            ("rss_after_new_kb", r.rss_after_new_kb),
            ("rss_end_kb", r.rss_end_kb),
            ("vmhwm_kb", r.hwm_kb),
        ] {
            assert_eq!(kv.get(name), Some(value.to_string().as_str()));
        }
        assert!(cap_text(&r).contains("loaded machine, preliminary"));
        assert_eq!(seconds_csv(&r).lines().count(), 21);
    }

    // REQ: OA-PRF-003
    #[test]
    fn two_runs_of_one_configuration_count_the_same_things() {
        let a = run_capacity(&small(60, 5, 10)).unwrap();
        let b = run_capacity(&small(60, 5, 10)).unwrap();
        // Times differ; every counter and every row count is the same.
        assert_eq!(a.fusion, b.fusion);
        assert_eq!(a.tracker.applied, b.tracker.applied);
        assert_eq!(
            a.seconds.iter().map(|s| s.4).collect::<Vec<_>>(),
            b.seconds.iter().map(|s| s.4).collect::<Vec<_>>()
        );
        assert_eq!(a.age_tick_ms.n, b.age_tick_ms.n);
    }

    // REQ: OA-PRF-003
    #[test]
    fn radar_lists_parse() {
        assert_eq!(parse_radars("e,t,terminal").unwrap().len(), 3);
        assert!(parse_radars("e,x").is_err());
    }

    // REQ: OA-PRF-004
    #[test]
    fn an_overfull_tracker_refuses_and_counts() {
        // 100 aircraft into a 40-track tracker: the extra are refused and counted, the run goes on.
        let mut c = small(100, 5, 10);
        c.max_tracks = 40;
        let r = run_capacity(&c).unwrap();
        assert!(r.tracker.capacity_refused > 0, "{:?}", r.tracker);
        assert!(r.tracks_confirmed <= 40);
        assert!(r.buffers_stable);
    }

    // REQ: OA-PRF-009
    #[test]
    fn too_few_track_slots_is_fast_but_not_a_pass() {
        let mut c = small(100, 5, 10);
        c.max_tracks = 40;
        let r = run_capacity(&c).unwrap();
        assert!(r.tracker.capacity_refused > 0, "{:?}", r.tracker);
        assert!(r.advance_ms.max < 1000.0, "the run was meant to be fast");
        assert!(!r.keeps_up(), "a run that refused aircraft passed");
        assert!(r.lost_work() && r.in_time());
        assert!(
            !r.keeps_up_strict(),
            "a run that refused aircraft passed strict"
        );
    }

    // REQ: OA-PRF-009
    #[test]
    fn a_capped_bucket_in_the_window_is_not_a_pass() {
        let mut r = run_capacity(&small(60, 5, 10)).unwrap();
        assert!(r.keeps_up() && r.keeps_up_strict(), "the clean run passes");
        r.fusion.bucket_work_capped = 3;
        assert!(!r.keeps_up(), "a capped bucket passed");
        assert!(!r.keeps_up_strict(), "a capped bucket passed strict");
    }

    // REQ: OA-PRF-009
    #[test]
    fn every_loss_counter_fails_a_run_and_a_clean_run_passes() {
        let clean = run_capacity(&small(60, 5, 10)).unwrap();
        assert!(clean.keeps_up());
        for (name, _) in loss_counters(&Default::default(), &Default::default(), 0) {
            let mut r = clean.clone();
            set_loss(&mut r, &name);
            assert!(!r.keeps_up(), "{name} moved and the run passed");
        }
    }

    // REQ: OA-PRF-009
    #[test]
    fn loss_before_the_measured_window_does_not_fail_a_run() {
        let mut r = run_capacity(&small(60, 5, 10)).unwrap();
        r.fusion.bucket_work_capped = 4;
        r.fusion_warm.bucket_work_capped = 4;
        assert!(r.keeps_up());
        r.fusion.bucket_work_capped = 5;
        assert!(!r.keeps_up());
    }

    // REQ: OA-PRF-009
    #[test]
    fn every_counter_is_sorted_into_loss_or_not() {
        let known = |n: &str, loss: &[(String, u64)], not: &[&str], pre: &str| {
            not.contains(&n)
                || loss.iter().any(|(l, _)| *l == format!("{pre}{n}"))
                || (n == "refused" && loss.iter().any(|(l, _)| l.starts_with("tracker.refused_")))
        };
        let loss = loss_counters(&Default::default(), &Default::default(), 0);
        for n in debug_fields(&format!("{:?}", FusionStats::default())) {
            assert!(
                known(&n, &loss, FUSION_NOT_LOSS, "fusion."),
                "fusion counter {n} is neither a loss counter nor listed as not one"
            );
        }
        for n in debug_fields(&format!("{:?}", TrackerStats::default())) {
            assert!(
                known(&n, &loss, TRACKER_NOT_LOSS, "tracker."),
                "tracker counter {n} is neither a loss counter nor listed as not one"
            );
        }
    }

    // REQ: OA-PRF-009
    #[test]
    fn one_slow_second_in_a_hundred_is_not_a_pass() {
        let mut r = run_capacity(&small(60, 5, 10)).unwrap();
        let mut v = vec![10.0; 99];
        v.push(2000.0);
        r.advance_ms = dist_of(&v);
        r.realtime_ratio = r.advance_ms.mean * 1.0e-3;
        // The mean is 30 ms and the 99th percentile 10 ms: the old rule passed it.
        assert!(r.realtime_ratio < 0.05 && r.advance_ms.p99 < 20.0);
        assert!(!r.keeps_up(), "a 2 s second passed");
        assert!(!r.keeps_up_strict());
    }
}
