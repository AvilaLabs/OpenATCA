#![forbid(unsafe_code)]
//! `oa-track-run`: runs the ADS-B-only tracker on a recorded clip and writes the scored report.
//! Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-track-run --adsb <clip.oaf> --out <dir> [--no-compare] [--no-score]
//!                [--digest <file>] [--rows <file>]
//! oa-track-run --adsb <clip.oaf> --out <dir> --realtime [--with-output] [--max-tracks <n>]
//! ```
//!
//! The clip's reports are split into two disjoint halves (`oa_track::score::SPLIT_TEXT`); the
//! tracker runs on half A, `oa-truth` is built from half B, and the tracker's one-second output
//! is scored by `oa-bench`'s metrics. `report.txt` and `report.csv` are `oa-bench`'s report with
//! the tracker section appended; `tracks.csv` lists every track and `esassp.csv` the straight
//! and turning table. `--no-compare` skips the single-filter baselines. `--no-score` runs the
//! tracker core and the latency replay only (no truth, no scoring, no files but `core.txt`), to
//! measure the tracker's own memory and speed apart from the scoring harness. `--digest` writes
//! the per-second digest of the output rows and `--rows` the canonical rows themselves
//! (`oa_track::canon`), for `oa-replay-diff`; without them nothing else changes. `--realtime` is the M9 interim marker's measurement: every
//! report goes to the tracker (no split, no scoring) and each update is timed, per second of traffic;
//! `--with-output` also writes the rows to column files, as a display or feed would need;
//! `--max-tracks` sets the track table (the default is the per-region size).
//!
//! The tracker's output goes to column files under `<dir>/spill-<pid>` as it is produced and
//! scoring reads it back one address range at a time, so memory does not grow with the run. The
//! shell owns the clocks: wall time for throughput, a monotonic clock for the per-stage latency
//! (`oa_track::latency`), and `/proc/self/status` for the resident memory of each phase.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use oa_adsb::{decode_payload_lenient, reports_len, Reports};
use oa_bench::adsb_truth;
use oa_bench::input::{for_each_adsb_frame, load_adsb};
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::find;
use oa_bench::stats::Stats;
use oa_track::adsb::{adsb_measurements, check_adsb_config, AdsbConfig, AdsbStats};
use oa_track::canon_spill::write_canon;
use oa_track::latency::{
    latency_count, latency_new, latency_stats, update_timed, Clock, FrameStamps, LatencyLog,
    LATENCY_CAPACITY, STAGES, STAGE_NAMES,
};
use oa_track::realtime::{realtime_summary, seconds_csv, SecondLog};
use oa_track::score::{
    initiation_delays, interval_stats, run_tracker_spilled, score_source, scoring_config,
    split_reports, Esassp, IntervalStats, RowSet, Scored, SplitStats, BASIS_TEXT, CLASS_NAMES,
    PER_FLIGHT_MIN_N, R4_PER_FLIGHT_M, SPLIT_TEXT,
};
use oa_track::spill::{spill_append, spill_finish, spill_new, SpilledRun};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_bytes, tracker_counts,
    tracker_flush, tracker_log, tracker_new, tracker_stats, tracker_update, Measurement,
    TrackOutput, TrackerConfig, DEFAULT_CONFIG, END_NAMES, REFUSE_NAMES,
};
use oa_truth::{DEFAULT_CONFIG as TRUTH_DEFAULT, REMOVE_NAMES};

const USAGE: &str = "usage: oa-track-run --adsb <clip.oaf> --out <dir> [--no-compare] [--no-score] [--digest <file>] [--rows <file>] | --adsb <clip.oaf> --out <dir> --realtime [--with-output] [--max-tracks <n>]";

fn main() -> ExitCode {
    let mut adsb: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut compare = true;
    let mut score_it = true;
    let mut canon = CanonOut::default();
    let (mut realtime, mut with_output) = (false, false);
    let mut max_tracks: Option<usize> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-compare" => compare = false,
            "--no-score" => score_it = false,
            "--realtime" => realtime = true,
            "--with-output" => with_output = true,
            "--adsb" | "--out" | "--digest" | "--rows" | "--max-tracks" => match args.next() {
                Some(v) if a == "--adsb" => adsb = Some(PathBuf::from(v)),
                Some(v) if a == "--digest" => canon.digest = Some(PathBuf::from(v)),
                Some(v) if a == "--rows" => canon.rows = Some(PathBuf::from(v)),
                Some(v) if a == "--max-tracks" => match v.parse::<usize>() {
                    Ok(n) => max_tracks = Some(n),
                    Err(_) => {
                        eprintln!("oa-track-run: --max-tracks needs a whole number\n{USAGE}");
                        return ExitCode::from(2);
                    }
                },
                Some(v) => out = Some(PathBuf::from(v)),
                None => {
                    eprintln!("oa-track-run: {a} needs a value\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("oa-track-run: unknown argument {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(adsb), Some(out)) = (adsb, out) else {
        eprintln!("oa-track-run: --adsb and --out are required\n{USAGE}");
        return ExitCode::from(2);
    };
    let result = if realtime {
        run_realtime(&adsb, &out, with_output, max_tracks)
    } else {
        run(&adsb, &out, compare, score_it, &canon)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-track-run: {e}");
            ExitCode::from(1)
        }
    }
}

fn secs(a: Instant) -> f64 {
    a.elapsed().as_secs_f64()
}

fn st_line(label: &str, s: &Stats) -> String {
    format!(
        "  {label:<26} n={:<8} mean={:>9.3} rms={:>9.3} median={:>9.3} p95={:>9.3} p99={:>9.3} max={:>10.3}\n",
        s.n, s.mean, s.rms, s.median, s.p95, s.p99, s.max
    )
}

fn esassp_text(e: &Esassp) -> String {
    let mut o = String::new();
    for (name, unit, rows) in [
        ("position error (horizontal)", "m", &e.position_m),
        ("speed error", "m/s", &e.speed_mps),
        ("heading error", "deg", &e.heading_deg),
    ] {
        let _ = writeln!(o, " {name}, {unit}");
        for (c, s) in CLASS_NAMES.iter().zip(rows.iter()) {
            o.push_str(&st_line(c, s));
        }
    }
    let _ = writeln!(
        o,
        " samples not classified (truth not askable 3 s either side): {}; without a position error (truth nearly stationary): {}",
        e.unclassified, e.no_position
    );
    let _ = writeln!(
        o,
        " per-flight position RMS (one flight per truth leg of an aircraft, with at least 10 samples): {} flights, p95 {:.1} m, max {:.1} m, {} above the R4 per-flight value of {R4_PER_FLIGHT_M} m",
        e.flights, e.flight_rms_p95_m, e.flight_rms_max_m, e.flights_over_r4
    );
    o
}

fn percentile(sorted: &[f64], p: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = (p * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    sorted.get(k - 1).copied().unwrap_or(0.0)
}

/// The monotonic clock of the shell: nanoseconds since the run started.
struct SystemClock(Instant);

impl Clock for SystemClock {
    fn now_ns(&mut self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

/// `(VmRSS, VmHWM)` of this process in kB, from `/proc/self/status` (None where it is absent).
fn rss_kb() -> Option<(u64, u64)> {
    let s = fs::read_to_string("/proc/self/status").ok()?;
    let get = |k: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
    };
    Some((get("VmRSS:")?, get("VmHWM:")?))
}

/// Resident memory at the end of each phase of the run, kB: `(phase, VmRSS, VmHWM so far)`.
#[derive(Default)]
struct Phases(Vec<(String, u64, u64)>);

impl Phases {
    fn mark(&mut self, name: &str) {
        if let Some((r, h)) = rss_kb() {
            self.0.push((name.to_string(), r, h));
        }
    }
}

/// What the tracker core alone costs: the run with its rows discarded.
struct CoreRun {
    secs: f64,
    rows: usize,
}

/// Runs the tracker over `meas` and throws the output away as it goes.
fn core_run(meas: &[Measurement], cfg: &TrackerConfig) -> Result<CoreRun, String> {
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut rows = 0usize;
    let started = Instant::now();
    for m in meas {
        tracker_update(&mut t, m, &mut out);
        if output_len(&out) > cfg.out_capacity.min(1 << 17) / 2 {
            rows += output_len(&out);
            output_clear(&mut out);
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    rows += output_len(&out);
    Ok(CoreRun {
        secs: secs(started),
        rows,
    })
}

/// The replay with stamps: frames are read from the file, decoded, and the measurements of set A
/// in them offered to the tracker one by one through [`update_timed`]; the output rows go to
/// the spill as the consumer. Returns the log, the run (for its digest: it must equal the
/// scoring run's), the seconds taken, and the measurements no frame produced.
fn replay_timed(
    path: &Path,
    meas: &[Measurement],
    cfg: &TrackerConfig,
    dir: &Path,
) -> Result<(LatencyLog, SpilledRun, f64, usize), String> {
    let mut by_key: BTreeMap<(u32, i64), VecDeque<usize>> = BTreeMap::new();
    for (i, m) in meas.iter().enumerate() {
        by_key.entry((m.addr, m.toa_ns)).or_default().push_back(i);
    }
    let mut t = tracker_new(cfg).map_err(|e| e.to_string())?;
    let mut out = output_new(cfg.out_capacity);
    let mut w = spill_new(dir).map_err(|e| e.to_string())?;
    let mut log = latency_new(LATENCY_CAPACITY);
    let mut clock = SystemClock(Instant::now());
    let mut failed: Option<String> = None;
    let started = Instant::now();
    for_each_adsb_frame(path, &mut |recv_ns, payload| {
        if failed.is_some() {
            return;
        }
        let received = clock.now_ns();
        let mut fr = Reports::default();
        let ok = decode_payload_lenient(payload, &mut fr).is_ok();
        let decoded = clock.now_ns();
        if !ok {
            return;
        }
        let stamps = FrameStamps {
            recv_ns,
            received,
            decoded,
        };
        for i in 0..reports_len(&fr) {
            let key = (
                fr.icao.get(i).copied().unwrap_or(0),
                fr.time_ns.get(i).copied().unwrap_or(0),
            );
            let Some(idx) = by_key.get_mut(&key).and_then(VecDeque::pop_front) else {
                continue;
            };
            let Some(m) = meas.get(idx) else { continue };
            let mut emit = |o: &mut TrackOutput| {
                if let Err(e) = spill_append(&mut w, o) {
                    failed = Some(e.to_string());
                }
                output_clear(o);
            };
            update_timed(
                &mut t, m, &stamps, &mut out, &mut clock, &mut emit, &mut log,
            );
        }
    })
    .map_err(|e| e.to_string())?;
    if let Some(e) = failed {
        return Err(e);
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    spill_append(&mut w, &out).map_err(|e| e.to_string())?;
    let elapsed = secs(started);
    let unmatched = by_key.values().map(VecDeque::len).sum();
    let run = spill_finish(
        w,
        out.overflow,
        *tracker_stats(&t),
        tracker_log(&t).clone(),
        meas.len() as u64,
    )
    .map_err(|e| e.to_string())?;
    Ok((log, run, elapsed, unmatched))
}

/// Everything measured about speed, latency and memory, for the report.
struct Perf {
    /// Tracker core with the one-second output, rows discarded.
    with_ticks: CoreRun,
    /// Tracker core with the one-second output off.
    no_ticks: CoreRun,
    /// The scoring run: output written to the spill files, digest computed.
    spill_secs: f64,
    latency: Option<(LatencyLog, f64, bool, usize)>,
    tracker_bytes: usize,
    phases: Phases,
}

fn us(ns: i64) -> f64 {
    ns as f64 / 1e3
}

fn perf_text(o: &mut String, p: &Perf, offered: u64, rows: u64, cfg: &TrackerConfig) {
    let _ = writeln!(
        o,
        "\nPerformance (release build assumed; every number measured on this run, not estimated; wall clock of the shell)"
    );
    let rate = |n: u64, s: f64| n as f64 / s.max(1e-9);
    let _ = writeln!(
        o,
        "  tracker core, {} measurements, one-second output on, rows discarded: {:.3} s, {:.0} updates/s ({} rows, about {:.1} rows per update)",
        offered,
        p.with_ticks.secs,
        rate(offered, p.with_ticks.secs),
        p.with_ticks.rows,
        p.with_ticks.rows as f64 / offered.max(1) as f64
    );
    let _ = writeln!(
        o,
        "  tracker core, one-second output off: {:.3} s, {:.0} updates/s ({} update rows)",
        p.no_ticks.secs,
        rate(offered, p.no_ticks.secs),
        p.no_ticks.rows
    );
    let _ = writeln!(
        o,
        "  scoring run (output written to column files, digest computed): {:.3} s, {:.0} updates/s ({} rows)",
        p.spill_secs,
        rate(offered, p.spill_secs),
        rows
    );
    let _ = writeln!(
        o,
        "  tracker tables: {:.1} MiB for {} tracks ({} stored measurements and one anchor filter state per track for re-filtering)",
        p.tracker_bytes as f64 / (1024.0 * 1024.0),
        cfg.max_tracks,
        oa_track::params::RING_DEPTH
    );
    if let Some((log, secs, digest_ok, unmatched)) = &p.latency {
        let _ = writeln!(
            o,
            "\nLatency by stage, replay of the clip's frames through decode, tracker and output (monotonic clock of the shell, {} measurements, {} not stored, {} unmatched; exact nearest-rank percentiles; stamps in a core module, clock injected)",
            latency_count(log),
            log.overflow,
            unmatched
        );
        let _ = writeln!(
            o,
            "  the replay took {secs:.3} s; its output digest {} the scoring run's",
            if *digest_ok { "equals" } else { "DIFFERS FROM" }
        );
        for i in 0..STAGES {
            let s = latency_stats(log, i);
            let name = STAGE_NAMES.get(i).copied().unwrap_or("?");
            if i == 0 {
                let _ = writeln!(
                    o,
                    "  {name:<76} min {:>9.3} ms  p50 {:>9.3} ms  p95 {:>9.3} ms  p99 {:>9.3} ms  max {:>9.3} ms",
                    us(s.min) / 1e3, us(s.p50) / 1e3, us(s.p95) / 1e3, us(s.p99) / 1e3, us(s.max) / 1e3
                );
            } else {
                let _ = writeln!(
                    o,
                    "  {name:<76} p50 {:>10.2} us  p95 {:>10.2} us  p99 {:>10.2} us  max {:>10.2} us",
                    us(s.p50), us(s.p95), us(s.p99), us(s.max)
                );
            }
        }
        let _ = writeln!(
            o,
            "  stage differences that saturated a 32-bit ns field {}, monotonic differences that were negative (stored as 0) {}, received before the sensor time (clock skew) {}",
            log.saturated, log.negative, log.sensor_negative
        );
        let _ = writeln!(
            o,
            "  a replay is not live: the clip has no receive wall clock (a frame's receive stamp is its first report's time, so the first stage is frame grouping, not a network), frames come from a file at once, and the output consumer is a buffered file, not a socket"
        );
    }
    if !p.phases.0.is_empty() {
        let _ = writeln!(
            o,
            "\nResident memory (VmRSS now, VmHWM peak so far, from /proc/self/status, kB)"
        );
        for (n, r, h) in &p.phases.0 {
            let _ = writeln!(o, "  {n:<58} rss {r:>9}  peak {h:>9}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tracker_section(
    split: &SplitStats,
    ad: &AdsbStats,
    cfg: &TrackerConfig,
    iv: &IntervalStats,
    run: &SpilledRun,
    scored: &Scored,
    updates: &Scored,
    baselines: &[(String, Esassp, Esassp)],
    perf: &Perf,
) -> String {
    let mut o = String::new();
    let s = &run.stats;
    let _ = writeln!(
        o,
        "\nTracker (oa-track), ADS-B only, scored on split reports"
    );
    let _ = writeln!(o, "  basis: {BASIS_TEXT}");
    let _ = writeln!(o, "  split: {SPLIT_TEXT}");
    let _ = writeln!(
        o,
        "  split result: {} reports of {} aircraft: {} in set A (tracker), {} in set B (truth)",
        split.rows_in, split.aircraft, split.set_a, split.set_b
    );
    let _ = writeln!(
        o,
        "  split removed {} duplicate reports; {} conflicting times ({} reports) kept on one side; {} unreadable rows",
        split.duplicates_removed, split.conflict_groups, split.conflict_rows, split.rows_invalid
    );
    let _ = writeln!(
        o,
        "  adapter (oa-truth's gates, NACp >= 8, NIC >= 7, SIL 3, version >= 2): {} rows in set A, {} passed",
        ad.rows_in, ad.passed
    );
    for (n, c) in REMOVE_NAMES.iter().zip(ad.removed.iter()).take(8) {
        let _ = writeln!(o, "    removed {n}: {c}");
    }
    let _ = writeln!(
        o,
        "    with velocity {} (default NACv used {}), with altitude {}, with vertical rate {}",
        ad.with_velocity, ad.velocity_default_nacv, ad.with_altitude, ad.with_vrate
    );
    let _ = writeln!(
        o,
        "  configuration: expected interval {:.1} s (measured median of set A), initiation interval {:.1} s (measured 90th percentile), coast after {} intervals, drop after {} s, confirm {} of {}, out-of-order window {} s, tick {} s, capacity {} tracks",
        cfg.expected_interval_ns as f64 / 1e9,
        cfg.init_interval_ns as f64 / 1e9,
        cfg.coast_after_intervals,
        cfg.drop_after_ns / 1_000_000_000,
        cfg.init_m,
        cfg.init_n,
        cfg.ooo_window_ns / 1_000_000_000,
        cfg.tick_ns / 1_000_000_000,
        cfg.max_tracks
    );
    let _ = writeln!(
        o,
        "  measured interval between reports of one aircraft in set A ({} intervals under 120 s): median {:.1} s, p90 {:.1} s, p99 {:.1} s",
        iv.n,
        iv.p50_ns as f64 / 1e9,
        iv.p90_ns as f64 / 1e9,
        iv.p99_ns as f64 / 1e9
    );
    let _ = writeln!(
        o,
        "  measurements offered {}, applied {}, duplicates {}, late (window) {}, late (ring) {}, re-filtered {}, jump-rejected {}, numerical-refused {}, resyncs {}, altitude-rejected {}, geometric altitude skipped {}, capacity-refused {}",
        s.offered, s.applied, s.duplicates, s.late_window, s.late_ring, s.refiltered, s.jump_rejected, s.numerical_refused, s.resyncs, s.alt_rejected, s.alt_geometric_skipped, s.capacity_refused
    );
    for (n, c) in REFUSE_NAMES.iter().zip(s.refused.iter()) {
        let _ = writeln!(o, "    refused {n}: {c}");
    }
    let _ = writeln!(
        o,
        "  tracks: started {}, confirmed {}, ended: {}; tentative restarted for implausible second report {}, for window passed {}",
        s.started,
        s.confirmed,
        END_NAMES
            .iter()
            .zip(s.ended.iter())
            .map(|(n, c)| format!("{n} {c}"))
            .collect::<Vec<_>>()
            .join(", "),
        s.init_implausible,
        s.init_window_expired
    );
    let (mut d, never) = initiation_delays(&run.log);
    d.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let _ = writeln!(
        o,
        "  initiation delay (first report to confirmation, 2 of 3): {} confirmed tracks, median {:.1} s, p95 {:.1} s, max {:.1} s; {} tracks never confirmed",
        d.len(),
        percentile(&d, 50),
        percentile(&d, 95),
        d.last().copied().unwrap_or(0.0),
        never
    );
    let _ = writeln!(
        o,
        "  position normalised innovation squared: mean {:.3} over {} updates (expected 2.0 if the noise model is honest)",
        if s.nis_n > 0 { s.nis_sum / s.nis_n as f64 } else { 0.0 },
        s.nis_n
    );
    let _ = writeln!(
        o,
        "  output: {} rows ({} one-second ticks offered to scorer), digest {}, rows lost to table capacity {}, unrepresentable rows {}, skipped ticks {}",
        run.rows, scored.tick_rows, run.digest, run.overflow, s.rows_unrepresentable, s.ticks_skipped
    );
    let _ = writeln!(
        o,
        "  paired samples (tick rows with truth at their time): {}",
        scored.pairs
    );
    let _ = writeln!(
        o,
        "  paired samples at report times (state at each applied report): {}",
        updates.pairs
    );
    let _ = writeln!(o, "\nAt report times (the track state at each applied report, scored like the one-second output); {BASIS_TEXT}");
    for id in ["T1", "T2-raw", "T3-speed", "T3-heading", "T4"] {
        if let Some(r) = find(&updates.bench, id).and_then(|m| m.rows.first().map(|r| (m, r))) {
            o.push_str(&st_line(
                &format!("[{id}] {} ({})", r.1.label, r.0.unit),
                &r.1.stats,
            ));
        }
    }
    let _ = writeln!(
        o,
        "\nESASSP-style split, straight vs turning (ESASSP Vol 1 4.2.14: turn = transversal acceleration > 1.5 m/s^2; coasting rows left out); {BASIS_TEXT}"
    );
    let _ = writeln!(o, "AT REPORT TIMES (closest to ESASSP's per-report errors)");
    o.push_str(&esassp_text(&updates.esassp));
    let _ = writeln!(
        o,
        "ON THE ONE-SECOND OUTPUT (includes up to one report interval of extrapolation)"
    );
    o.push_str(&esassp_text(&scored.esassp));
    let _ = writeln!(
        o,
        "  note: R4 3 NM Recommended is <= 210 m global, < 230 m per flight; R17 speed <= 4 m/s straight, <= 8 m/s turning. Shown for orientation only: input here is thinned, see basis."
    );
    if !baselines.is_empty() {
        let _ = writeln!(o, "\nSingle straight-model filter instead of the interacting pair (same input, gates, scoring)");
        let line = |name: &str, e: &Esassp, u: &Esassp| {
            format!(
                "  {name:<34} report times: position rms all {:>7.1} straight {:>7.1} turning {:>7.1} m, speed rms straight {:>5.2} turning {:>5.2} m/s | one-second output: position rms all {:>7.1} straight {:>7.1} turning {:>7.1} m\n",
                u.position_m[0].rms, u.position_m[1].rms, u.position_m[2].rms, u.speed_mps[1].rms, u.speed_mps[2].rms,
                e.position_m[0].rms, e.position_m[1].rms, e.position_m[2].rms
            )
        };
        o.push_str(&line(
            "interacting pair (this tracker)",
            &scored.esassp,
            &updates.esassp,
        ));
        for (n, e, u) in baselines {
            o.push_str(&line(n, e, u));
        }
    }
    perf_text(&mut o, perf, run.offered, run.rows, cfg);
    o
}

fn esassp_csv(rows: &[(String, &Esassp)]) -> String {
    let mut o = String::from("run,rows,quantity,unit,class,n,mean,rms,median,p95,p99,max,basis\n");
    for (name, e) in rows {
        for (q, unit, v) in [
            ("position", "m", &e.position_m),
            ("speed", "m/s", &e.speed_mps),
            ("heading", "deg", &e.heading_deg),
        ] {
            for (c, s) in CLASS_NAMES.iter().zip(v.iter()) {
                let _ = writeln!(
                    o,
                    "{name},{q},{unit},{c},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},thinned input; not an ESASSP exit result",
                    s.n, s.mean, s.rms, s.median, s.p95, s.p99, s.max
                );
            }
        }
    }
    o
}

fn tracks_csv(run: &SpilledRun) -> String {
    let mut o = String::from("address_hex,first_s,confirmed_s,last_s,updates,end_reason\n");
    let l = &run.log;
    for i in 0..l.addr.len() {
        let g = |v: &[i64]| v.get(i).copied().unwrap_or(0);
        let conf = g(&l.confirmed_ns);
        let _ = writeln!(
            o,
            "{:06X},{:.3},{},{:.3},{},{}",
            l.addr.get(i).copied().unwrap_or(0) & 0xFF_FFFF,
            g(&l.first_ns) as f64 / 1e9,
            if conf == i64::MIN {
                "never".to_string()
            } else {
                format!("{:.3}", conf as f64 / 1e9)
            },
            g(&l.last_ns) as f64 / 1e9,
            l.updates.get(i).copied().unwrap_or(0),
            END_NAMES
                .get(usize::from(l.reason.get(i).copied().unwrap_or(0)))
                .copied()
                .unwrap_or("?")
        );
    }
    o
}

fn optional_f64(v: Option<f64>) -> String {
    v.filter(|x| x.is_finite())
        .map_or_else(String::new, |x| format!("{x:.6}"))
}

fn outliers_csv(rows: &[(&str, &Scored)]) -> (String, String) {
    let mut csv = String::from("row_set,rank,address_hex,truth_leg_index,leg_start_ms,leg_end_ms,paired_sample_count,noncoast_sample_count,position_sample_count,position_rms_m,position_max_m,speed_sample_count,speed_rms_mps,heading_sample_count,heading_rms_deg,window_start_ms,window_end_ms,eligible,exclusion_reason,cohort_n,cohort_over_r4\n");
    let mut meta = String::from("row_set,truth_legs,paired_rows,eligible_legs,excluded_no_pairs,excluded_all_coasting,excluded_no_finite_position,excluded_fewer_than_10_positions,cohort_over_r4,threshold_m\n");
    for (name, scored) in rows {
        let mut flights = scored.flight_errors.clone();
        flights.sort_by(|a, b| {
            let ar = a
                .position_rms_m
                .filter(|_| a.position_samples >= PER_FLIGHT_MIN_N as u64);
            let br = b
                .position_rms_m
                .filter(|_| b.position_samples >= PER_FLIGHT_MIN_N as u64);
            match (ar, br) {
                (Some(x), Some(y)) => oa_math::cmp_canon_f64(y, x)
                    .then_with(|| a.address.cmp(&b.address))
                    .then_with(|| a.leg_start_ms.cmp(&b.leg_start_ms))
                    .then_with(|| a.truth_leg_index.cmp(&b.truth_leg_index)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a
                    .address
                    .cmp(&b.address)
                    .then_with(|| a.leg_start_ms.cmp(&b.leg_start_ms))
                    .then_with(|| a.truth_leg_index.cmp(&b.truth_leg_index)),
            }
        });
        let cohort_n = flights
            .iter()
            .filter(|f| f.position_samples >= PER_FLIGHT_MIN_N as u64)
            .count();
        let over = flights
            .iter()
            .filter(|f| {
                f.position_samples >= PER_FLIGHT_MIN_N as u64
                    && f.position_rms_m.is_some_and(|v| v > R4_PER_FLIGHT_M)
            })
            .count();
        let (mut no_pairs, mut all_coasting, mut no_finite_position, mut fewer_than_10) =
            (0u64, 0u64, 0u64, 0u64);
        let mut paired = 0u64;
        let mut eligible_rank = 0usize;
        for f in &flights {
            paired += f.paired_samples;
            let eligible = f.position_samples >= PER_FLIGHT_MIN_N as u64;
            if !eligible {
                match (f.paired_samples, f.noncoast_samples, f.position_samples) {
                    (0, _, _) => no_pairs += 1,
                    (_, 0, _) => all_coasting += 1,
                    (_, _, 0) => no_finite_position += 1,
                    _ => fewer_than_10 += 1,
                }
            }
            let reason = if eligible {
                ""
            } else if f.paired_samples == 0 {
                "no_pairs"
            } else if f.noncoast_samples == 0 {
                "all_coasting"
            } else if f.position_samples == 0 {
                "no_finite_position"
            } else {
                "fewer_than_10_positions"
            };
            let rank = if eligible {
                eligible_rank += 1;
                eligible_rank.to_string()
            } else {
                String::new()
            };
            let _ = writeln!(csv,
                "{name},{rank},{:06X},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{cohort_n},{over}",
                f.address, f.truth_leg_index, f.leg_start_ms, f.leg_end_ms, f.paired_samples,
                f.noncoast_samples, f.position_samples, optional_f64(f.position_rms_m),
                optional_f64(f.position_max_m), f.speed_samples, optional_f64(f.speed_rms_mps),
                f.heading_samples, optional_f64(f.heading_rms_deg),
                f.window_start_ms.map_or_else(String::new, |v| v.to_string()),
                f.window_end_ms.map_or_else(String::new, |v| v.to_string()), eligible, reason);
        }
        let _ = writeln!(meta, "{name},{},{paired},{cohort_n},{no_pairs},{all_coasting},{no_finite_position},{fewer_than_10},{over},{R4_PER_FLIGHT_M:.1}", flights.len());
    }
    (csv, meta)
}

/// Where `--digest` and `--rows` write.
#[derive(Default)]
// A7.2: cold struct, size guard waived
struct CanonOut {
    digest: Option<PathBuf>,
    rows: Option<PathBuf>,
}

/// On-CPU nanoseconds of this process (`/proc/self/schedstat`), or from `/proc/self/stat`'s ticks
/// (10 ms each) where the first is absent. The run is one thread, so this is its CPU time.
fn cpu_ns() -> Option<u64> {
    if let Some(v) = fs::read_to_string("/proc/self/schedstat")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return Some(v);
    }
    let s = fs::read_to_string("/proc/self/stat").ok()?;
    let rest = s.get(s.rfind(')')? + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    Some(ticks * 10_000_000)
}

/// Seconds of CPU between two [`cpu_ns`] readings, as text.
fn cpu_text(a: Option<u64>, b: Option<u64>) -> Option<f64> {
    a.zip(b).map(|(a, b)| b.saturating_sub(a) as f64 / 1e9)
}

/// `--realtime`: the whole clip through the tracker, each update timed, per second of traffic
/// (OA-TRK-388). Reading, decoding and the adapter are timed apart, not in the per-second figures.
fn run_realtime(
    clip_path: &Path,
    out_dir: &Path,
    with_output: bool,
    max_tracks: Option<usize>,
) -> Result<(), String> {
    let mut phases = Phases::default();
    phases.mark("start");
    let (load_wall, load_cpu0) = (Instant::now(), cpu_ns());
    let clip = load_adsb(clip_path).map_err(|e| e.to_string())?;
    let (clip_reports, clip_bad) = (reports_len(&clip.reports), clip.bad_frames);
    phases.mark("clip decoded (all reports in memory)");
    let adsb_cfg = AdsbConfig::default();
    check_adsb_config(&adsb_cfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let (meas, ad) = adsb_measurements(&clip.reports, &adsb_cfg);
    drop(clip);
    let load_wall_s = secs(load_wall);
    let load_cpu_s = cpu_text(load_cpu0, cpu_ns());
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let mut cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    if let Some(n) = max_tracks {
        cfg.max_tracks = n;
    }
    phases.mark("measurements built");
    let mut addrs: Vec<u32> = meas.iter().map(|m| m.addr).collect();
    addrs.sort_unstable();
    addrs.dedup();
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let spill_root = out_dir.join(format!("spill-{}", std::process::id()));
    let mut t = tracker_new(&cfg).map_err(|e| e.to_string())?;
    let tracker_mem = tracker_bytes(&t);
    let mut out = output_new(cfg.out_capacity);
    let mut w = if with_output {
        Some(spill_new(&spill_root.join("run")).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let drain_rows = cfg.out_capacity.min(1 << 17) / 2;
    let mut log = SecondLog::default();
    let mut rows = 0u64;
    let mut failed: Option<String> = None;
    let mut cur_sec = i64::MIN;
    let (wall0, cpu0) = (Instant::now(), cpu_ns());
    for m in &meas {
        let sec = i64::div_euclid(m.toa_ns, 1_000_000_000);
        if sec != cur_sec {
            if cur_sec != i64::MIN {
                let (a, b) = tracker_counts(&t);
                log.set_tracks(cur_sec, (a + b) as u32);
            }
            cur_sec = sec;
        }
        let at = Instant::now();
        tracker_update(&mut t, m, &mut out);
        if output_len(&out) > drain_rows {
            rows += output_len(&out) as u64;
            if let Some(w) = w.as_mut() {
                if let Err(e) = spill_append(w, &out) {
                    failed = Some(e.to_string());
                }
            }
            output_clear(&mut out);
        }
        log.add(sec, at.elapsed().as_nanos() as u64);
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut out);
    tracker_flush(&mut t, end, &mut out);
    rows += output_len(&out) as u64;
    if let Some(w) = w.as_mut() {
        spill_append(w, &out).map_err(|e| e.to_string())?;
    }
    let feed_wall_s = secs(wall0);
    let feed_cpu_s = cpu_text(cpu0, cpu_ns());
    phases.mark("tracker run");
    if let Some(e) = failed {
        return Err(format!("spilled output: {e}"));
    }
    let overflow = out.overflow;
    let stats = *tracker_stats(&t);
    let digest = match w {
        Some(w) => {
            let run = spill_finish(
                w,
                overflow,
                stats,
                tracker_log(&t).clone(),
                meas.len() as u64,
            )
            .map_err(|e| e.to_string())?;
            Some(run.digest.clone())
        }
        None => None,
    };
    phases.mark("output digest");
    let s = realtime_summary(&log);
    let traffic_s = s.seconds as f64;
    let ms = |ns: u64| ns as f64 / 1e6;
    let cpu_or = |c: Option<f64>, f: &dyn Fn(f64) -> String| c.map_or("n/a".to_string(), f);
    let mut o = String::new();
    let _ = writeln!(
        o,
        "oa-track-run --realtime{}: clip {}",
        if with_output { " --with-output" } else { "" },
        clip_path.display()
    );
    let _ = writeln!(
        o,
        "input: {clip_reports} reports decoded ({clip_bad} frames refused), {} of {} rows passed the adapter (NACp >= 8, NIC >= 7, SIL 3, version >= 2), {} distinct addresses in the measurements; full input, no split, no thinning",
        ad.passed,
        ad.rows_in,
        addrs.len()
    );
    let _ = writeln!(
        o,
        "traffic: {} s from unix second {} ({} with measurements); measurements per second mean {:.1}, peak {} (second {})",
        s.seconds, log.first_s, s.busy_seconds, s.reports_mean, s.reports_peak, s.reports_peak_s
    );
    let _ = writeln!(
        o,
        "peak confirmed tracks at the end of a second: {}",
        s.tracks_peak
    );
    let _ = writeln!(
        o,
        "processing time of a second of traffic (the updates of that second, rows {}): p50 {:.3} ms, p99 {:.3} ms, max {:.3} ms (second {}), total {:.3} s",
        if with_output { "written to column files" } else { "discarded" },
        ms(s.work_p50_ns),
        ms(s.work_p99_ns),
        ms(s.work_max_ns),
        s.work_max_s,
        s.work_total_ns as f64 / 1e9
    );
    let _ = writeln!(
        o,
        "real time: {} seconds whose work took longer than the second, {} seconds that finished late behind queued work, longest queue {:.3} ms",
        s.over_one_second,
        s.finished_late,
        ms(s.backlog_max_ns)
    );
    let _ = writeln!(
        o,
        "tracker run (updates and output, one thread): wall {feed_wall_s:.3} s for {traffic_s:.0} s of traffic, CPU {} s; share of one core: wall {:.4}%, CPU {}",
        cpu_or(feed_cpu_s, &|c| format!("{c:.3}")),
        100.0 * feed_wall_s / traffic_s.max(1.0),
        cpu_or(feed_cpu_s, &|c| format!("{:.4}%", 100.0 * c / traffic_s.max(1.0)))
    );
    let _ = writeln!(
        o,
        "ingest (read, decode, adapter; not in the per-second figures): wall {load_wall_s:.3} s, CPU {} s",
        cpu_or(load_cpu_s, &|c| format!("{c:.3}"))
    );
    let _ = writeln!(
        o,
        "tracker tables: {} bytes ({:.1} MiB) for {} tracks",
        tracker_mem,
        tracker_mem as f64 / (1024.0 * 1024.0),
        cfg.max_tracks
    );
    let _ = writeln!(
        o,
        "counters: offered {}, applied {}, duplicates {}, late (window) {}, late (ring) {}, re-filtered {}, jump-rejected {}, numerical-refused {}, resyncs {}, altitude-rejected {}, geometric altitude skipped {}, capacity-refused {}",
        stats.offered, stats.applied, stats.duplicates, stats.late_window, stats.late_ring, stats.refiltered, stats.jump_rejected, stats.numerical_refused, stats.resyncs, stats.alt_rejected, stats.alt_geometric_skipped, stats.capacity_refused
    );
    for (n, c) in REFUSE_NAMES.iter().zip(stats.refused.iter()) {
        let _ = writeln!(o, "  refused {n}: {c}");
    }
    let _ = writeln!(
        o,
        "tracks: started {}, confirmed {}, ended: {}",
        stats.started,
        stats.confirmed,
        END_NAMES
            .iter()
            .zip(stats.ended.iter())
            .map(|(n, c)| format!("{n} {c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        o,
        "output: {rows} rows, rows lost to table capacity {overflow}, unrepresentable rows {}, nan covariance {}, skipped ticks {}, time held {} (applied {}), time steps {} back {}, log seconds dropped {}",
        stats.rows_unrepresentable, stats.nan_covariance, stats.ticks_skipped, stats.time_held, stats.time_held_applied, stats.time_steps, stats.time_steps_back, log.dropped
    );
    let _ = writeln!(
        o,
        "output digest: {}",
        digest.as_deref().unwrap_or("not computed (rows discarded)")
    );
    let _ = writeln!(o, "resident memory (kB, VmRSS now, VmHWM peak so far)");
    phases.mark("end");
    for (n, r, h) in &phases.0 {
        let _ = writeln!(o, "  {n:<44} rss {r:>9}  peak {h:>9}");
    }
    print!("{o}");
    let wr = |name: &str, body: String| {
        fs::write(out_dir.join(name), body)
            .map_err(|e| format!("{}: {e}", out_dir.join(name).display()))
    };
    wr("realtime.txt", o)?;
    wr("seconds.csv", seconds_csv(&log))?;
    let _ = fs::remove_dir_all(&spill_root);
    Ok(())
}

fn run(
    clip_path: &Path,
    out_dir: &Path,
    compare: bool,
    score_it: bool,
    canon: &CanonOut,
) -> Result<(), String> {
    let mut phases = Phases::default();
    phases.mark("start");
    let clip = load_adsb(clip_path).map_err(|e| e.to_string())?;
    phases.mark("clip decoded (all reports in memory)");
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("the split of the clip is not valid, run refused: {v:?}"))?;
    let (clip_reports, clip_bad, clip_hash) =
        (reports_len(&clip.reports), clip.bad_frames, clip.hash);
    drop(clip.reports);
    let adsb_cfg = AdsbConfig::default();
    check_adsb_config(&adsb_cfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let (meas, ad) = adsb_measurements(&set_a, &adsb_cfg);
    drop(set_a);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    phases.mark("split and measurements built");
    // The tracker core alone, rows discarded: with the one-second output and without it.
    let with_ticks = core_run(&meas, &cfg)?;
    phases.mark("tracker core run (one-second output on, rows discarded)");
    let upd_cfg = TrackerConfig {
        tick_ns: i64::MAX / 4,
        ..cfg
    };
    let no_ticks = core_run(&meas, &upd_cfg)?;
    phases.mark("tracker core run (one-second output off)");
    let tracker_mem = tracker_bytes(&tracker_new(&cfg).map_err(|e| e.to_string())?);
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let spill_root = out_dir.join(format!("spill-{}", std::process::id()));
    let started = Instant::now();
    let run =
        run_tracker_spilled(&meas, &cfg, &spill_root.join("run")).map_err(|e| e.to_string())?;
    let spill_secs = secs(started);
    phases.mark("scoring run (output spilled to files)");
    if canon.digest.is_some() || canon.rows.is_some() {
        let sum = write_canon(&run, canon.digest.as_deref(), canon.rows.as_deref())
            .map_err(|e| format!("canonical output: {e}"))?;
        println!(
            "canonical output: {} rows in {} seconds{}",
            sum.rows,
            sum.seconds,
            if sum.in_order {
                ""
            } else {
                " (emitted out of second order, grouped in the file)"
            }
        );
        phases.mark("canonical digest and rows written");
    }
    let latency = match replay_timed(clip_path, &meas, &cfg, &spill_root.join("replay")) {
        Ok((log, replay, elapsed, unmatched)) => {
            let ok = replay.digest == run.digest;
            Some((log, elapsed, ok, unmatched))
        }
        Err(e) => return Err(format!("latency replay: {e}")),
    };
    phases.mark("latency replay (frames to output)");
    if !score_it {
        let mut perf = Perf {
            with_ticks,
            no_ticks,
            spill_secs,
            latency,
            tracker_bytes: tracker_mem,
            phases,
        };
        perf.phases.mark("end");
        let mut text = format!(
            "oa-track-run --no-score: {} measurements, {} tracks, output digest {}\n",
            run.offered, run.stats.started, run.digest
        );
        perf_text(&mut text, &perf, run.offered, run.rows, &cfg);
        print!("{text}");
        fs::write(out_dir.join("core.txt"), text)
            .map_err(|e| format!("{}: {e}", out_dir.join("core.txt").display()))?;
        drop(run);
        let _ = fs::remove_dir_all(&spill_root);
        return Ok(());
    }
    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    let sc = |r: &SpilledRun, rows| {
        score_source(r, &built, &tcfg, rows, oa_track::score::SCORE_CHUNK_ROWS)
            .map_err(|e| e.to_string())
    };
    let scored = sc(&run, RowSet::Ticks)?;
    let updates = sc(&run, RowSet::Updates)?;
    phases.mark("truth built, run scored (ticks and updates)");
    let mut baselines: Vec<(String, Esassp, Esassp)> = Vec::new();
    if compare {
        for (mode, name) in [
            (1u8, "single straight filter, q 0.05"),
            (2, "single straight filter, q 0.5"),
            (3, "single straight filter, q 5"),
        ] {
            let c = TrackerConfig {
                h_mode: mode,
                ..cfg
            };
            let r = run_tracker_spilled(&meas, &c, &spill_root.join("baseline"))
                .map_err(|e| e.to_string())?;
            let s = sc(&r, RowSet::Ticks)?;
            let u = sc(&r, RowSet::Updates)?;
            baselines.push((name.to_string(), s.esassp, u.esassp));
        }
    }
    phases.mark("single-filter baselines scored");
    let mut notes = vec![
        format!("tracker: oa-track on set A, truth from set B ({SPLIT_TEXT})"),
        format!("basis: {BASIS_TEXT}"),
        "legacy stream = the tracker's one-second output (confirmed tracks, coasting flagged); T5 is one second by construction".to_string(),
        "zone for T7 and T8 = the ZAU box of the clip (no airspace model loaded)".to_string(),
        format!(
            "truth (set B): {} reports decoded of set B, built with the design's gates (10 s bracket, 50 m)",
            reports_len(&set_b)
        ),
    ];
    notes.push(format!(
        "clip: {} reports decoded, {} frames refused",
        clip_reports, clip_bad
    ));
    let header = Header {
        mode: "tracker (split ADS-B)".to_string(),
        code_version: code_version(),
        inputs: vec![clip_hash],
        replay_hash: "not applicable (tracker run)".to_string(),
        counts: vec![
            ("set_a_reports".to_string(), split.set_a),
            ("set_b_reports".to_string(), split.set_b),
            (
                "split_duplicates_removed".to_string(),
                split.duplicates_removed,
            ),
            ("split_conflict_groups".to_string(), split.conflict_groups),
            (
                "tracker_one_second_rows_offered_to_scorer".to_string(),
                scored.tick_rows as u64,
            ),
        ],
        notes,
        truth: adsb_truth::truth_section(&built, &tcfg),
    };
    let perf = Perf {
        with_ticks,
        no_ticks,
        spill_secs,
        latency,
        tracker_bytes: tracker_mem,
        phases,
    };
    let with_ticks_secs = perf.with_ticks.secs;
    let section = tracker_section(
        &split, &ad, &cfg, &iv, &run, &scored, &updates, &baselines, &perf,
    );
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut text = render_text(&header, &scored.bench);
    text.push_str(&section);
    let w = |name: &str, body: String| {
        fs::write(out_dir.join(name), body)
            .map_err(|e| format!("{}: {e}", out_dir.join(name).display()))
    };
    w("report.txt", text)?;
    w("report.csv", render_csv(&header, &scored.bench))?;
    w("tracks.csv", tracks_csv(&run))?;
    let mut rows: Vec<(String, &Esassp)> = vec![
        (
            "interacting_pair,one_second_output".to_string(),
            &scored.esassp,
        ),
        ("interacting_pair,report_times".to_string(), &updates.esassp),
    ];
    for (n, e, u) in &baselines {
        let n = n.replace(", ", "_").replace(' ', "_");
        rows.push((format!("{n},one_second_output"), e));
        rows.push((format!("{n},report_times"), u));
    }
    w("esassp.csv", esassp_csv(&rows))?;
    let (outlier_rows, outlier_meta) = outliers_csv(&[("ticks", &scored), ("updates", &updates)]);
    w("outliers.csv", outlier_rows)?;
    w("outliers-summary.csv", outlier_meta)?;
    let ticks = scored.tick_rows;
    println!(
        "wrote report.txt, report.csv, tracks.csv, esassp.csv, outliers.csv and outliers-summary.csv to {}; {} measurements, {} tracks, {} one-second rows, {:.0} updates/s",
        out_dir.display(),
        run.offered,
        run.stats.started,
        ticks,
        run.offered as f64 / with_ticks_secs.max(1e-9)
    );
    drop(run);
    let _ = fs::remove_dir_all(&spill_root);
    Ok(())
}
