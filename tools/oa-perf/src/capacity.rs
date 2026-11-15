//! The capacity run: `n` simultaneous aircraft at their real update rates, fed to the fusion
//! layer one second at a time, each second timed. Shell code (it reads the clock and /proc and
//! allocates freely, RULESET C6); the layer it drives is core.
//!
//! Per second of traffic `k` the harness (untimed) makes the ADS-B reports and plots timed in
//! that second; then (timed, one `Instant` pair) offers them all and calls `fusion_advance` to
//! the end of the second; then (untimed) reads the rows emitted and empties the table. So a
//! plot or report reaches the output at the advance that ends its second: between 0 and 1 s of
//! traffic time after its own time, which the run records as the input-to-effect delay.

use std::collections::HashMap;
use std::time::Instant;

use oa_track::fusion::{
    fusion_adsb, fusion_advance, fusion_bookkeeping_len, fusion_buffers, fusion_config, fusion_new,
    fusion_plots, fusion_stats, fusion_take_radar_tracks, fusion_tracker, fusion_tracker_config,
    fusion_tracker_stats, FusionStats,
};
use oa_track::fusion_params::KEY_RADAR_BASE;
use oa_track::fusion_score::{Kind, EPOCH_NS};
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
}

/// What a capacity run measured.
#[derive(Debug, Clone)]
pub struct CapResult {
    pub n: usize,
    pub layout: Layout,
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
    pub tracker: TrackerStats,
    pub out_overflow: u64,
    pub tracks_tentative: usize,
    pub tracks_confirmed: usize,
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
    /// Every second of the measured traffic took under a second, and the 99th percentile
    /// advance was under the advance interval (1 s). The knee is the largest `n` where this holds.
    pub fn keeps_up(&self) -> bool {
        !self.truncated
            && !self.stopped_early
            && self.realtime_ratio < 1.0
            && self.advance_ms.p99 < 1000.0
            && self.fusion.pending_overflow == 0
    }

    /// The stricter test: the slowest advance too was under a second.
    pub fn keeps_up_strict(&self) -> bool {
        self.keeps_up() && self.advance_ms.max < 1000.0
    }
}

/// The names and values of the fusion counters.
pub fn fusion_counters(s: &FusionStats) -> Vec<(&'static str, u64)> {
    vec![
        ("adsb_in", s.adsb_in),
        ("plots_in", s.plots_in),
        ("plots_refused", s.plots_refused),
        ("late", s.late),
        ("pending_overflow", s.pending_overflow),
        ("applied_hard", s.applied_hard),
        ("applied_assoc", s.applied_assoc),
        ("held", s.held),
        ("unassigned", s.unassigned),
        ("hard_no_track", s.hard_no_track),
        ("hard_mismatch", s.hard_mismatch),
        ("radar_disagree", s.radar_disagree),
        ("hard_extra", s.hard_extra),
        ("hold_evicted", s.hold_evicted),
        ("held_forced", s.held_forced),
        ("bucket_plot_overflow", s.bucket_plot_overflow),
        ("bucket_work_capped", s.bucket_work_capped),
        ("assoc_refused", s.assoc_refused),
        ("coarse_visits", s.coarse_visits),
        ("pairs_tested", s.pairs_tested),
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
    ]
}

/// The counters of the capacity refusals: the ones that say work or items were refused or cut.
pub const CAPACITY_COUNTERS: [&str; 15] = [
    "plots_refused",
    "late",
    "pending_overflow",
    "hold_evicted",
    "bucket_plot_overflow",
    "bucket_work_capped",
    "assoc_refused",
    "cand_capacity",
    "track_refused",
    "assign_log_overflow",
    "track_rec_overflow",
    "keyinfo_overflow",
    "scratch_full",
    "reg_history_full",
    "no_height_pairs",
];

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
        ("started".into(), s.started),
        ("confirmed".into(), s.confirmed),
        ("ticks".into(), s.ticks),
        ("ticks_skipped".into(), s.ticks_skipped),
        ("rows_computed".into(), s.rows_computed),
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
        out_capacity: (4 * n).clamp(1 << 16, 1 << 24),
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

/// Runs the traffic through the layer and measures it.
pub fn run_capacity(cfg: &CapCfg) -> Result<CapResult, String> {
    let n = cfg.traffic.n;
    let load_start = loadavg3();
    let rss_before_kb = status_kb("VmRSS").unwrap_or(0);
    let mut traffic = Traffic::new(&cfg.traffic)?;
    let sites = traffic.sites();
    let base = tracker_cfg(cfg.max_tracks, n);
    let fcfg = fusion_config(fusion_tracker_config(&base, &sites));
    let mut f = fusion_new(&fcfg, &sites).map_err(|e| e.to_string())?;
    let rss_after_new_kb = status_kb("VmRSS").unwrap_or(0);
    let mut out = output_new(base.out_capacity);
    let buffers0 = fusion_buffers(&f);

    let (mut adsb, mut plots) = (Vec::<Measurement>::new(), Vec::new());
    let (mut before, mut after) = (HashMap::new(), HashMap::new());
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
    let total_s = cfg.warmup_s + cfg.measure_s;
    let started = Instant::now();
    let (mut truncated, mut stopped_early) = (false, false);
    let mut measure_done = 0i64;
    let mut seconds_done = 0i64;
    let mut rss_warm_kb = 0u64;
    for k in 0..total_s {
        if started.elapsed().as_secs_f64() > cfg.max_wall_s {
            truncated = true;
            break;
        }
        let measured = k >= cfg.warmup_s;
        adsb.clear();
        traffic.adsb_window(k, &mut adsb);
        traffic.plot_windows(k, &mut plots);
        let until = EPOCH_NS + (k + 1) * NS_PER_S;
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
        let n_plots: usize = plots.iter().map(Vec::len).sum();
        if measured {
            wall_measured += ms * 1.0e-3;
            adv_ms.push(ms);
            measure_done += 1;
            snapshot(&f, &mut after);
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
    let tracker_table_mb = tracker_bytes(fusion_tracker(&f)) as f64 / (1024.0 * 1024.0);
    let (tent, conf) = tracker_counts(fusion_tracker(&f));
    let md = measure_done.max(1) as f64;
    let advance_ms = dist_of(&adv_ms);
    Ok(CapResult {
        n,
        layout: cfg.traffic.layout,
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
        tracker: *fusion_tracker_stats(&f),
        out_overflow: out.overflow,
        tracks_tentative: tent,
        tracks_confirmed: conf,
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
    kv.put("keeps_up", u8::from(r.keeps_up()));
    kv.put("keeps_up_strict", u8::from(r.keeps_up_strict()));
    kv.put("seconds_over_1s", r.seconds_over_1s);
    kv.put_dist("advance", "ms", &r.advance_ms);
    kv.put_dist("warmup_advance", "ms", &r.warmup_advance_ms);
    kv.put("adsb_per_s", fmt_f(r.adsb_per_s));
    kv.put("plots_per_s", fmt_f(r.plots_per_s));
    kv.put("rows_per_s", fmt_f(r.rows_per_s));
    kv.put("age_unknown_rows", r.age_unknown);
    kv.put_dist("age_tick_rows", "ms", &r.age_tick_ms);
    kv.put_dist("age_tick_adsb_keys", "ms", &r.age_tick_adsb_ms);
    kv.put_dist("age_tick_radar_keys", "ms", &r.age_tick_radar_ms);
    kv.put_dist("age_update_rows", "ms", &r.age_update_ms);
    kv.put_dist("age_coast_rows", "ms", &r.age_coast_ms);
    kv.put_dist("delay_adsb", "ms", &r.delay_adsb_ms);
    kv.put_dist("delay_plot", "ms", &r.delay_plot_ms);
    for (name, v) in fusion_counters(&r.fusion) {
        kv.put(&format!("fusion.{name}"), v);
    }
    for (name, v) in tracker_counters(&r.tracker) {
        kv.put(&format!("tracker.{name}"), v);
    }
    kv.put("out_overflow", r.out_overflow);
    kv.put("tracks_tentative", r.tracks_tentative);
    kv.put("tracks_confirmed", r.tracks_confirmed);
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
    let mut s = String::from("second,wall_ms,adsb_offered,plots_offered,rows_emitted\n");
    for (k, ms, a, p, rows) in &r.seconds {
        s.push_str(&format!("{k},{ms:.3},{a},{p},{rows}\n"));
    }
    s
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
    s.push_str(&format!(
        "  processing time per second of traffic: {:.1} ms = {:.3} x real time; {} seconds took 1 s or more; keeps up: {} (strict, slowest advance < 1 s: {})\n",
        r.realtime_ratio * 1000.0,
        r.realtime_ratio,
        r.seconds_over_1s,
        r.keeps_up(),
        r.keeps_up_strict()
    ));
    s.push_str(&dist_line("per-advance wall time", "ms", &r.advance_ms));
    s.push_str(&dist_line(
        "  (warm-up seconds, for comparison)",
        "ms",
        &r.warmup_advance_ms,
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
        "  tracks at the end: {} confirmed, {} tentative (aircraft {})\n",
        r.tracks_confirmed, r.tracks_tentative, r.n
    ));
    let moved: Vec<String> = fusion_counters(&r.fusion)
        .into_iter()
        .filter(|(name, v)| CAPACITY_COUNTERS.contains(name) && *v > 0)
        .map(|(name, v)| format!("{name}={v}"))
        .collect();
    let tmoved: Vec<String> = tracker_counters(&r.tracker)
        .into_iter()
        .filter(|(name, v)| {
            *v > 0
                && (name == "capacity_refused"
                    || name.starts_with("refused_")
                    || name == "late_window"
                    || name == "late_ring"
                    || name == "time_held"
                    || name == "ticks_skipped")
        })
        .map(|(name, v)| format!("{name}={v}"))
        .collect();
    s.push_str(&format!(
        "  capacity counters moved (fusion): {}\n  capacity counters moved (tracker): {}; output overflow rows {}\n",
        if moved.is_empty() { "none".to_string() } else { moved.join(" ") },
        if tmoved.is_empty() { "none".to_string() } else { tmoved.join(" ") },
        r.out_overflow
    ));
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
        }
    }

    // REQ: OA-PRF-003
    #[test]
    fn a_small_run_processes_all_traffic_and_reports_every_figure() {
        let r = run_capacity(&small(100, 10, 20)).unwrap();
        assert_eq!(r.measure_s_done, 20);
        assert!(!r.truncated);
        // All traffic was offered and processed; nothing was refused or cut.
        let f = &r.fusion;
        assert!(f.adsb_in > 0 && f.plots_in > 0, "{f:?}");
        assert_eq!(
            f.processed,
            f.adsb_in + f.plots_in - f.plots_refused - f.late
        );
        for (name, v) in fusion_counters(f) {
            if CAPACITY_COUNTERS.contains(&name) && name != "no_height_pairs" {
                assert_eq!(v, 0, "{name}");
            }
        }
        assert_eq!(r.out_overflow, 0);
        assert_eq!(r.tracker.capacity_refused, 0);
        assert!(r.buffers_stable);
        // Every aircraft has a track (a few radar-only duplicates of ADS-B aircraft may exist).
        assert!(
            r.tracks_confirmed >= 95 && r.tracks_confirmed <= 130,
            "{}",
            r.tracks_confirmed
        );
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
        assert!(r.hwm_kb >= r.rss_after_new_kb && r.rss_after_new_kb > r.rss_before_kb);
        let kv = cap_kv(&r);
        assert_eq!(kv.get("n"), Some("100"));
        assert!(kv.get("advance.p99_ms").is_some() && kv.get("vmhwm_kb").is_some());
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
}
