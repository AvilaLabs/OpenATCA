#![forbid(unsafe_code)]
//! `oa-track-run`: runs the ADS-B-only tracker on a recorded clip and writes the scored report.
//! Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-track-run --adsb <clip.oaf> --out <dir> [--no-compare]
//! ```
//!
//! The clip's reports are split into two disjoint halves (`oa_track::score::SPLIT_TEXT`); the
//! tracker runs on half A, `oa-truth` is built from half B, and the tracker's one-second output
//! is scored by `oa-bench`'s metrics. `report.txt` and `report.csv` are `oa-bench`'s report with
//! the tracker section appended; `tracks.csv` lists every track and `esassp.csv` the straight
//! and turning table. `--no-compare` skips the single-filter baselines.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use oa_adsb::reports_len;
use oa_bench::adsb_truth;
use oa_bench::input::load_adsb;
use oa_bench::report::{code_version, render_csv, render_text, Header};
use oa_bench::run::find;
use oa_bench::stats::Stats;
use oa_track::adsb::{adsb_measurements, AdsbConfig, AdsbStats};
use oa_track::score::{
    initiation_delays, interval_stats, run_tracker, score, scoring_config, split_reports, Esassp,
    IntervalStats, RowSet, Scored, SplitStats, TrackRun, BASIS_TEXT, CLASS_NAMES, R4_PER_FLIGHT_M,
    SPLIT_TEXT,
};
use oa_track::{
    output_clear, output_new, tracker_bytes, tracker_new, tracker_update, Measurement,
    TrackerConfig, DEFAULT_CONFIG, END_NAMES, O_TICK, REFUSE_NAMES,
};
use oa_truth::{DEFAULT_CONFIG as TRUTH_DEFAULT, REMOVE_NAMES};

const USAGE: &str = "usage: oa-track-run --adsb <clip.oaf> --out <dir> [--no-compare]";

fn main() -> ExitCode {
    let mut adsb: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut compare = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-compare" => compare = false,
            "--adsb" | "--out" => match args.next() {
                Some(v) if a == "--adsb" => adsb = Some(PathBuf::from(v)),
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
    match run(&adsb, &out, compare) {
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
        " per-flight position RMS (flights with at least 10 samples): {} flights, p95 {:.1} m, max {:.1} m, {} above the R4 per-flight value of {R4_PER_FLIGHT_M} m",
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

/// Per-call latency of `tracker_update` over the measurements, ns: `(p50, p99, p999, max)`.
fn latency(meas: &[Measurement], cfg: &TrackerConfig) -> Option<(f64, f64, f64, f64)> {
    let mut t = tracker_new(cfg).ok()?;
    let mut out = output_new(cfg.out_capacity);
    let mut d: Vec<f64> = Vec::with_capacity(meas.len());
    for m in meas {
        let s = Instant::now();
        tracker_update(&mut t, m, &mut out);
        d.push(s.elapsed().as_nanos() as f64);
        if out.t_ns.len() > cfg.out_capacity / 2 {
            output_clear(&mut out);
        }
    }
    d.sort_by(f64::total_cmp);
    Some((
        percentile(&d, 50),
        percentile(&d, 99),
        percentile(&d, 999 * 100 / 1000),
        d.last().copied().unwrap_or(0.0),
    ))
}

#[allow(clippy::too_many_arguments)]
fn tracker_section(
    split: &SplitStats,
    ad: &AdsbStats,
    cfg: &TrackerConfig,
    iv: &IntervalStats,
    run: &TrackRun,
    scored: &Scored,
    updates: &Scored,
    baselines: &[(String, Esassp, Esassp)],
    secs: (f64, f64, usize),
    lat: Option<(f64, f64, f64, f64)>,
    mem_bytes: usize,
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
        "  measurements offered {}, applied {}, duplicates {}, late (window) {}, late (ring) {}, re-filtered {}, jump-rejected {}, resyncs {}, altitude-rejected {}, geometric altitude skipped {}, capacity-refused {}",
        s.offered, s.applied, s.duplicates, s.late_window, s.late_ring, s.refiltered, s.jump_rejected, s.resyncs, s.alt_rejected, s.alt_geometric_skipped, s.capacity_refused
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
    d.sort_by(f64::total_cmp);
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
        "  output: {} rows ({} one-second ticks scored), digest {}, rows lost to table capacity {}",
        run.out.t_ns.len(),
        scored.tick_rows,
        run.digest,
        run.overflow
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
    let _ = writeln!(
        o,
        "\nPerformance (release build assumed; measured on this run, not estimated)"
    );
    let (secs_run, secs_upd, upd_rows) = secs;
    let _ = writeln!(
        o,
        "  {} measurements in {:.3} s: {:.0} updates/s with the one-second output ({} rows, about {:.1} rows per update; adapter excluded; includes the output digest and the accumulation of rows)",
        run.offered,
        secs_run,
        run.offered as f64 / secs_run.max(1e-9),
        run.out.t_ns.len(),
        run.out.t_ns.len() as f64 / run.offered.max(1) as f64
    );
    let _ = writeln!(
        o,
        "  the same measurements with the one-second output off: {:.3} s, {:.0} updates/s ({} update rows)",
        secs_upd,
        run.offered as f64 / secs_upd.max(1e-9),
        upd_rows
    );
    if let Some((p50, p99, p999, max)) = lat {
        let _ = writeln!(
            o,
            "  per-call latency of tracker_update (includes tick bursts): p50 {:.2} us, p99 {:.2} us, p99.9 {:.2} us, max {:.2} us",
            p50 / 1e3, p99 / 1e3, p999 / 1e3, max / 1e3
        );
    }
    let _ = writeln!(
        o,
        "  tracker tables: {:.1} MiB for {} tracks ({} stored updates per track for re-filtering)",
        mem_bytes as f64 / (1024.0 * 1024.0),
        cfg.max_tracks,
        oa_track::params::RING_DEPTH
    );
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

fn tracks_csv(run: &TrackRun) -> String {
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

fn run(clip_path: &Path, out_dir: &Path, compare: bool) -> Result<(), String> {
    let clip = load_adsb(clip_path).map_err(|e| e.to_string())?;
    let (set_a, set_b, split) = split_reports(&clip.reports);
    let (meas, ad) = adsb_measurements(&set_a, &AdsbConfig::default());
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let started = Instant::now();
    let run = run_tracker(&meas, &cfg).map_err(|e| e.to_string())?;
    let secs_run = secs(started);
    // The same measurements with no one-second output: the cost of the updates alone.
    let upd_cfg = TrackerConfig {
        tick_ns: i64::MAX / 4,
        ..cfg
    };
    let started = Instant::now();
    let upd_only = run_tracker(&meas, &upd_cfg).map_err(|e| e.to_string())?;
    let secs_upd = secs(started);
    let lat = latency(&meas, &cfg);
    let mem = tracker_bytes(&tracker_new(&cfg).map_err(|e| e.to_string())?);
    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    let scored = score(&run, &built, &tcfg, RowSet::Ticks).map_err(|e| e.to_string())?;
    let updates = score(&run, &built, &tcfg, RowSet::Updates).map_err(|e| e.to_string())?;
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
            let r = run_tracker(&meas, &c).map_err(|e| e.to_string())?;
            let s = score(&r, &built, &tcfg, RowSet::Ticks).map_err(|e| e.to_string())?;
            let u = score(&r, &built, &tcfg, RowSet::Updates).map_err(|e| e.to_string())?;
            baselines.push((name.to_string(), s.esassp, u.esassp));
        }
    }
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
        reports_len(&clip.reports),
        clip.bad_frames
    ));
    let header = Header {
        mode: "tracker (split ADS-B)".to_string(),
        code_version: code_version(),
        inputs: vec![clip.hash.clone()],
        replay_hash: "not applicable (tracker run)".to_string(),
        counts: vec![
            ("set_a_reports".to_string(), split.set_a),
            ("set_b_reports".to_string(), split.set_b),
            (
                "tracker_one_second_rows_scored".to_string(),
                scored.tick_rows as u64,
            ),
        ],
        notes,
        truth: adsb_truth::truth_section(&built, &tcfg),
    };
    let section = tracker_section(
        &split,
        &ad,
        &cfg,
        &iv,
        &run,
        &scored,
        &updates,
        &baselines,
        (secs_run, secs_upd, upd_only.out.t_ns.len()),
        lat,
        mem,
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
    let ticks = run.out.flags.iter().filter(|&&f| f & O_TICK != 0).count();
    println!(
        "wrote report.txt, report.csv, tracks.csv and esassp.csv to {}; {} measurements, {} tracks, {} one-second rows, {:.0} updates/s",
        out_dir.display(),
        run.offered,
        run.stats.started,
        ticks,
        run.offered as f64 / secs_run.max(1e-9)
    );
    Ok(())
}
