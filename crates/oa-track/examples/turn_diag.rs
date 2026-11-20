//! Turn diagnostic: OpenATCA's cross-track and track-angle error binned by the reference's turn
//! rate, the turn phase and the tracker's turn-model probability.
//!
//! ```text
//! cargo run --release -p oa-track --example p002 -- --adsb <clip.oaf> --sfdps-dir <dir> --out <p002 dir> --development
//! cargo run --release -p oa-track --example turn_diag -- \
//!     --adsb <clip.oaf> --samples <p002 dir>/samples.csv --out <dir>
//! ```
//!
//! The samples are those of p002: its `samples.csv` names the common grid samples (all three
//! systems scored) and carries OpenATCA's and ERAM's cross-track and track-angle errors, so the
//! scoring, the pairing and the ERAM load are p002's and are not repeated here. This tool runs
//! p002's split and tracker (`split_reports`, `adsb_measurements`, `scoring_config`, the same
//! drain loop as `run_tracker`) over the same clip and builds the same reference
//! (`adsb_truth::build` on set B). With `--rescore` the OpenATCA errors are not read from the CSV
//! but recomputed from this build's tracker on the CSV's samples (the same keys) by the library
//! calls p002 pairs with (`associate`, `build_pairs_with`), so a change to the tracker can be
//! screened in a few minutes without the ERAM load and the bootstrap; the ERAM columns stay those
//! of the CSV, and a change worth keeping is confirmed by a p002 run. The tool adds what the CSV does not have: the tracker's turn-model
//! probability and turn rate at each sample's row, and the reference's heading rate and turn
//! phase.
//!
//! Reference heading rate: the central difference of the reference track angle over [`HALF_S`]
//! either side of the time, clockwise positive, in degrees per second; none where the reference
//! has no state at either end or its ground speed is below [`MIN_SPEED_MPS`]. A sample is
//! "turning" when the magnitude is at least [`TURN_DPS`]. Phase: turn onset (turning now, not
//! [`WINDOW_S`] ago), steady turn (turning now and then), rollout (not turning now, turning at
//! some second of the last [`WINDOW_S`]), straight (neither); no phase where the rate now or
//! [`WINDOW_S`] ago is unknown. Reported per bin: n, median, p95 and RMS of the absolute error
//! (nearest rank, p002's definitions). Aggregates only.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_bench::adsb_truth;
use oa_bench::assoc::associate;
use oa_bench::chunked::truth_index;
use oa_bench::input::{file_opener, load_adsb_counted};
use oa_bench::pair::build_pairs_with;
use oa_bench::run::RunConfig;
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::Truth;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, score_ranges, scoring_config, split_reports, IntervalStats, RowSet, RowSource,
    DRAIN_ROWS, SCORE_CHUNK_ROWS,
};
use oa_track::spill::{spill_append, spill_finish, spill_new};
use oa_track::types::{output_clear, output_len, output_new, TrackOutput, M_HAS_VEL, O_TICK};
use oa_track::DEFAULT_CONFIG;
use oa_track::{tracker_advance, tracker_flush, tracker_new, tracker_update};
use oa_track::{tracker_log, tracker_stats};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

/// Half-width of the heading-rate difference, seconds.
const HALF_S: i64 = 3;
/// Reference ground speed below which the heading is not defined, m/s (p002's fit floor).
const MIN_SPEED_MPS: f64 = 50.0;
/// Heading rate magnitude from which the reference is turning, degrees per second (the first
/// bin edge).
const TURN_DPS: f64 = 0.25;
/// The phase window, seconds.
const WINDOW_S: i64 = 10;
/// Bin edges of the reference turn rate magnitude, degrees per second.
const RATE_EDGES: [f64; 4] = [0.25, 1.0, 2.0, 3.0];
/// Bin edges of the turn-model probability.
const MU_EDGES: [f64; 3] = [0.1, 0.5, 0.9];
/// Bin edges of the time since the sample's track last had a report, and of the length of the gap
/// that report opens, seconds.
const AGE_EDGES: [f64; 4] = [1.0, 3.0, 6.0, 10.0];
const GAP_EDGES: [f64; 4] = [3.0, 6.0, 10.0, 20.0];
/// One in this many measurements enters the velocity lag table.
const LAG_EVERY: usize = 4;
const PHASES: [&str; 5] = ["onset", "steady turn", "rollout", "straight", "no phase"];

/// One common grid sample of p002.
struct Sample {
    addr: u32,
    /// Time of OpenATCA's paired row, Unix ms (the tracker row's key).
    t_ms: i64,
    oa_cross: f64,
    oa_hdg: f64,
    er_cross: f64,
    er_hdg: f64,
    band: String,
}

/// What the tracker's row says at a sample.
#[derive(Clone, Copy)]
struct Row {
    mu_turn: f64,
    /// Heading rate, degrees per second, clockwise positive.
    omega_dps: f64,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut samples, mut out) = (None, None, None);
    let mut rescore = false;
    while let Some(a) = args.next() {
        if a == "--rescore" {
            rescore = true;
            continue;
        }
        let v = args.next();
        match a.as_str() {
            "--adsb" => adsb = v.map(PathBuf::from),
            "--samples" => samples = v.map(PathBuf::from),
            "--out" => out = v.map(PathBuf::from),
            _ => {
                eprintln!("turn_diag: unknown option {a}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(adsb), Some(samples), Some(out)) = (adsb, samples, out) else {
        eprintln!(
            "usage: turn_diag --adsb <clip.oaf> --samples <p002 samples.csv> --out <dir> [--rescore]"
        );
        return ExitCode::from(2);
    };
    for p in [&adsb, &samples, &out] {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("turn_diag: {e}");
            return ExitCode::from(2);
        }
    }
    match run(&adsb, &samples, &out, rescore) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("turn_diag: {e}");
            ExitCode::from(1)
        }
    }
}

/// The common samples of a p002 `samples.csv` with finite OpenATCA and ERAM cross-track and
/// track-angle errors.
fn read_samples(path: &Path) -> Result<Vec<Sample>, String> {
    // seal-allow: reads the p002 output file named on the command line, after the sealed-path check
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_samples(&text)
}

/// [`read_samples`] of the text of the file.
fn parse_samples(text: &str) -> Result<Vec<Sample>, String> {
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    let head: BTreeMap<&str, usize> = lines
        .next()
        .ok_or("empty samples file")?
        .split(',')
        .enumerate()
        .map(|(i, h)| (h, i))
        .collect();
    let col = |n: &str| head.get(n).copied().ok_or(format!("no column {n}"));
    let (c_addr, c_t, c_band, c_common) =
        (col("addr")?, col("t_ms")?, col("band")?, col("common")?);
    let (c_cross, c_hdg, c_dt) = (
        col("oa_cross_m")?,
        col("oa_track_err_deg")?,
        col("oa_row_dt_ms")?,
    );
    let (e_cross, e_hdg) = (col("eram_cross_m")?, col("eram_track_err_deg")?);
    let mut v = Vec::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        let g = |i: usize| f.get(i).copied().unwrap_or("");
        if g(c_common) != "1" {
            continue;
        }
        let num = |i: usize| g(i).parse::<f64>().ok().filter(|x| x.is_finite());
        let (Some(addr), Some(t), Some(dt)) = (
            u32::from_str_radix(g(c_addr), 16).ok(),
            g(c_t).parse::<i64>().ok(),
            g(c_dt).parse::<i64>().ok(),
        ) else {
            continue;
        };
        let (Some(oa_cross), Some(oa_hdg), Some(er_cross), Some(er_hdg)) =
            (num(c_cross), num(c_hdg), num(e_cross), num(e_hdg))
        else {
            continue;
        };
        v.push(Sample {
            addr,
            t_ms: t + dt,
            oa_cross,
            oa_hdg,
            er_cross,
            er_hdg,
            band: g(c_band).to_string(),
        });
    }
    Ok(v)
}

/// Keeps the tick rows of `o` whose key is wanted.
fn collect(o: &TrackOutput, want: &BTreeSet<(u32, i64)>, got: &mut BTreeMap<(u32, i64), Row>) {
    let rows = o
        .addr
        .iter()
        .zip(&o.t_ns)
        .zip(&o.flags)
        .zip(&o.mu_h)
        .zip(&o.turn_rate_rps);
    for ((((&a, &t), &f), mu), &w) in rows {
        if f & O_TICK == 0 {
            continue;
        }
        let key = (a, t.div_euclid(1_000_000));
        if want.contains(&key) {
            got.entry(key).or_insert(Row {
                mu_turn: f64::from(mu[1]),
                omega_dps: f64::from(w).to_degrees(),
            });
        }
    }
}

/// `[n, median, p95, rms]` of the absolute values (nearest rank).
fn stat(v: &[f64]) -> [f64; 4] {
    let mut b: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    if b.is_empty() {
        return [0.0, f64::NAN, f64::NAN, f64::NAN];
    }
    b.sort_by(|x, y| oa_math::cmp_canon_f64(*x, *y));
    let n = b.len();
    let at = |p: usize| {
        let k = (p * n).div_ceil(100).clamp(1, n) - 1;
        b.get(k).copied().unwrap_or(f64::NAN)
    };
    let rms = (b.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt();
    [n as f64, at(50), at(95), rms]
}

fn bin_of(edges: &[f64], x: f64) -> usize {
    edges.iter().take_while(|&&e| x >= e).count()
}

/// The age of the last report at or before `t` and the gap from it to the next one, seconds
/// (none before the first report; the gap is none after the last).
fn age_gap(times: &[i64], t: i64) -> (Option<f64>, Option<f64>) {
    let i = times.partition_point(|&x| x <= t);
    let last = i.checked_sub(1).and_then(|j| times.get(j)).copied();
    let next = times.get(i).copied();
    (
        last.map(|l| (t - l) as f64 / 1000.0),
        last.zip(next).map(|(l, n)| (n - l) as f64 / 1000.0),
    )
}

/// One sample's reference heading rate and turn phase, and its tracker row.
struct Ref {
    rate: Option<f64>,
    phase: usize,
}

/// The distribution of one value in the bins of a key: one row of a table.
struct Table {
    title: String,
    labels: Vec<String>,
    /// Per bin: OpenATCA cross, ERAM cross, OpenATCA angle, ERAM angle.
    cols: Vec<[Vec<f64>; 4]>,
}

impl Table {
    fn new(title: &str, labels: Vec<String>) -> Table {
        let n = labels.len();
        Table {
            title: title.to_string(),
            labels,
            cols: (0..n).map(|_| Default::default()).collect(),
        }
    }

    fn add(&mut self, bin: usize, s: &Sample) {
        if let Some(c) = self.cols.get_mut(bin) {
            c[0].push(s.oa_cross);
            c[1].push(s.er_cross);
            c[2].push(s.oa_hdg);
            c[3].push(s.er_hdg);
        }
    }

    fn write(&self, o: &mut String) {
        let _ = writeln!(o, "\n{}", self.title);
        let _ = writeln!(
            o,
            "  {:<16} {:>7} | cross m: OA med  p95  rms | ERAM med  p95  rms | angle deg: OA med  p95  rms | ERAM med  p95  rms",
            "bin", "n"
        );
        for (l, c) in self.labels.iter().zip(&self.cols) {
            let [a, b, d, e] = [stat(&c[0]), stat(&c[1]), stat(&c[2]), stat(&c[3])];
            let _ = writeln!(
                o,
                "  {:<16} {:>7} |          {:>6.1} {:>5.1} {:>5.1} |      {:>6.1} {:>5.1} {:>5.1} |            {:>6.2} {:>5.2} {:>5.2} |      {:>6.2} {:>5.2} {:>5.2}",
                l, a[0], a[1], a[2], a[3], b[1], b[2], b[3], d[1], d[2], d[3], e[1], e[2], e[3]
            );
        }
    }
}

fn run(adsb: &Path, samples_path: &Path, out_dir: &Path, rescore: bool) -> Result<String, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut samples = read_samples(samples_path)?;
    eprintln!("{} common samples", samples.len());
    let want: BTreeSet<(u32, i64)> = samples.iter().map(|s| (s.addr, s.t_ms)).collect();

    // p002's split and tracker run; only the wanted tick rows are kept.
    let mut drops = oa_sealed::DropReport::new();
    let mut clip_sealed = 0u64;
    let clip = load_adsb_counted(adsb, &file_opener(), &mut clip_sealed);
    drops.add(clip_sealed);
    let clip = clip.map_err(|e| e.to_string())?;
    let (set_a, set_b, _) = split_reports(&clip.reports);
    drop(clip);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    // The report times of the aircraft of the samples (ms): the age of a sample's last report and
    // the gap it opens.
    let addrs: BTreeSet<u32> = samples.iter().map(|s| s.addr).collect();
    let mut times: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    // Every [`LAG_EVERY`]th reported velocity of those aircraft: (address, time ms, reported
    // compass track), for the lag of the reported velocity behind the reference.
    let mut reported: Vec<(u32, i64, f64)> = Vec::new();
    for (i, m) in meas.iter().enumerate() {
        if addrs.contains(&m.addr) {
            if i % LAG_EVERY == 0 && m.flags & M_HAS_VEL != 0 {
                let (e, n) = (f64::from(m.vel_east_mps), f64::from(m.vel_north_mps));
                reported.push((
                    m.addr,
                    m.toa_ns.div_euclid(1_000_000),
                    e.atan2(n).to_degrees(),
                ));
            }
            times
                .entry(m.addr)
                .or_default()
                .push(m.toa_ns.div_euclid(1_000_000));
        }
    }
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let mut t = tracker_new(&cfg).map_err(|e| e.to_string())?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut rows: BTreeMap<(u32, i64), Row> = BTreeMap::new();
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let mut w = if rescore {
        Some(spill_new(&spill).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let drain = |scratch: &mut TrackOutput,
                 w: &mut Option<oa_track::spill::SpillWriter>,
                 rows: &mut BTreeMap<(u32, i64), Row>|
     -> Result<(), String> {
        collect(scratch, &want, rows);
        if let Some(w) = w {
            spill_append(w, scratch).map_err(|e| e.to_string())?;
        }
        output_clear(scratch);
        Ok(())
    };
    for m in &meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            drain(&mut scratch, &mut w, &mut rows)?;
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    let overflow = scratch.overflow;
    drain(&mut scratch, &mut w, &mut rows)?;
    let run = match w {
        Some(w) => Some(
            spill_finish(
                w,
                overflow,
                *tracker_stats(&t),
                tracker_log(&t).clone(),
                meas.len() as u64,
            )
            .map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    drop(meas);
    eprintln!("tracker done, {} rows matched", rows.len());

    // The reference from set B only, as p002.
    let built = adsb_truth::build(&set_b, &TRUTH_DEFAULT).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    if let Some(run) = run {
        let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
        let truth = Truth {
            query: &q,
            legs: &built.legs,
        };
        let mut rcfg = RunConfig::default();
        rcfg.pair.max_sigma_m = TRUTH_DEFAULT.max_sigma95_h_m;
        // as p002: scored without the hint-over-plan rule
        rcfg.assoc.hint_over_plan = false;
        let tix = truth_index(&truth);
        let mut scored: BTreeMap<(u32, i64), (f64, f64)> = BTreeMap::new();
        let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
        for range in score_ranges(&counts, SCORE_CHUNK_ROWS) {
            let r = run
                .rows_in(RowSet::Ticks, range.0, range.1)
                .map_err(|e| e.0)?;
            let legacy = legacy_from_track_samples_by_leg(
                &TrackSamples {
                    addr: &r.addr,
                    t_ns: &r.t_ns,
                    lat_e7: &r.lat_e7,
                    lon_e7: &r.lon_e7,
                    alt_ft: &r.alt_ft,
                    vel_east_mps: &r.vel_east_mps,
                    vel_north_mps: &r.vel_north_mps,
                    coast: &r.coast,
                },
                truth.legs,
            )
            .map_err(|e| format!("track samples: {e:?}"))?;
            drop(r);
            let ix = build_th_index(&legacy.th, legacy.flights.gufi.len());
            let assoc = associate(&legacy, &ix, &truth, &tix, &rcfg.assoc);
            let (p, _) =
                build_pairs_with(&legacy, &ix, &assoc, &truth, &rcfg.pair, &|_, _, _| None);
            for (i, (&a, &t)) in p.addr.iter().zip(&p.t_ms).enumerate() {
                let g = |v: &[f32]| f64::from(v.get(i).copied().unwrap_or(f32::NAN));
                let (c, h) = (g(&p.cross_m), g(&p.hdg_err_deg));
                if want.contains(&(a, t)) && c.is_finite() && h.is_finite() {
                    scored.entry((a, t)).or_insert((c, h));
                }
            }
        }
        let _ = fs::remove_dir_all(&spill);
        let before = samples.len();
        samples.retain_mut(|s| match scored.get(&(s.addr, s.t_ms)) {
            Some(&(c, h)) => {
                s.oa_cross = c;
                s.oa_hdg = h;
                true
            }
            None => false,
        });
        eprintln!(
            "rescored {} of {before} samples ({} lost)",
            samples.len(),
            before - samples.len()
        );
    }
    let heading = |a: u32, t: i64| -> Option<f64> {
        let s = adsb_truth::query(&built.truth, a, t).ok()?;
        (s.vel_east_mps.hypot(s.vel_north_mps) >= MIN_SPEED_MPS)
            .then(|| s.vel_east_mps.atan2(s.vel_north_mps).to_degrees())
    };
    let rate = |a: u32, t: i64| -> Option<f64> {
        let d = heading(a, t + HALF_S * 1000)? - heading(a, t - HALF_S * 1000)?;
        let d = (d + 180.0).rem_euclid(360.0) - 180.0;
        Some(d / (2 * HALF_S) as f64)
    };
    let refs: Vec<Ref> = samples
        .iter()
        .map(|s| {
            let r0 = rate(s.addr, s.t_ms);
            let r_old = rate(s.addr, s.t_ms - WINDOW_S * 1000);
            let turning = |r: f64| r.abs() >= TURN_DPS;
            let phase = match (r0, r_old) {
                (Some(now), Some(old)) => match (turning(now), turning(old)) {
                    (true, false) => 0,
                    (true, true) => 1,
                    (false, _) => {
                        let recent = old.abs() >= TURN_DPS
                            || (1..WINDOW_S)
                                .any(|k| rate(s.addr, s.t_ms - k * 1000).is_some_and(turning));
                        if recent {
                            2
                        } else {
                            3
                        }
                    }
                },
                _ => 4,
            };
            Ref { rate: r0, phase }
        })
        .collect();
    eprintln!("reference rates done");

    // How far the reported velocity's track lags the reference's, in seconds: with the reference
    // turning at `w` (clockwise, deg/s) and the reported track `d` degrees behind it, the lag is
    // `d / w`. Bins of the reference rate magnitude.
    let mut lag: Vec<Vec<f64>> = (0..RATE_EDGES.len() + 1).map(|_| Vec::new()).collect();
    for &(a, t, h) in &reported {
        let (Some(w), Some(ht)) = (rate(a, t), heading(a, t)) else {
            continue;
        };
        if w.abs() < TURN_DPS {
            continue;
        }
        let d = ((ht - h + 180.0).rem_euclid(360.0) - 180.0) / w;
        if let Some(b) = lag.get_mut(bin_of(&RATE_EDGES, w.abs())) {
            b.push(d);
        }
    }

    let rate_labels: Vec<String> = ["0-0.25", "0.25-1", "1-2", "2-3", ">3", "no ref rate"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mu_labels: Vec<String> = ["<0.1", "0.1-0.5", "0.5-0.9", ">=0.9", "no tracker row"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut by_rate = Table::new(
        "By the reference's turn rate (deg/s, magnitude)",
        rate_labels,
    );
    let mut by_phase = Table::new(
        &format!("By turn phase (turning = |rate| >= {TURN_DPS} deg/s; window {WINDOW_S} s)"),
        PHASES.iter().map(|s| s.to_string()).collect(),
    );
    let mut by_mu = Table::new("By the IMM turn-model probability", mu_labels);
    let edge_labels = |e: &[f64]| -> Vec<String> {
        let mut v = vec![format!("<{}", e.first().copied().unwrap_or(0.0))];
        v.extend(e.windows(2).map(|w| {
            let at = |i: usize| w.get(i).copied().unwrap_or(0.0);
            format!("{}-{}", at(0), at(1))
        }));
        v.push(format!(">={}", e.last().copied().unwrap_or(0.0)));
        v.push("none".to_string());
        v
    };
    let mut by_age = Table::new(
        "By seconds since the track's last report (turning = reference |rate| >= 0.25 deg/s)",
        edge_labels(&AGE_EDGES),
    );
    let mut by_age_turn = Table::new("  ... turning samples only", edge_labels(&AGE_EDGES));
    let mut by_age_str = Table::new("  ... not turning samples only", edge_labels(&AGE_EDGES));
    let mut by_gap = Table::new(
        "By the length of the report gap holding the sample (s)",
        edge_labels(&GAP_EDGES),
    );
    let mut by_gap_turn = Table::new("  ... turning samples only", edge_labels(&GAP_EDGES));
    let mut all = Table::new("All common samples", vec!["all".to_string()]);
    let mut by_band = BTreeMap::new();
    // Tracker's heading rate against the reference's, per rate bin: signed medians.
    let mut est: Vec<(Vec<f64>, Vec<f64>)> = (0..6).map(|_| Default::default()).collect();
    let mut csv = String::from("addr,t_ms,band,oa_cross_m,eram_cross_m,oa_track_err_deg,eram_track_err_deg,ref_rate_dps,phase,mu_turn,est_rate_dps\n");
    for (s, r) in samples.iter().zip(&refs) {
        let rb = r.rate.map_or(5, |x| bin_of(&RATE_EDGES, x.abs()));
        let row = rows.get(&(s.addr, s.t_ms));
        let mb = row.map_or(4, |x| bin_of(&MU_EDGES, x.mu_turn));
        by_rate.add(rb, s);
        by_phase.add(r.phase, s);
        by_mu.add(mb, s);
        let (age, gap) = times
            .get(&s.addr)
            .map_or((None, None), |t| age_gap(t, s.t_ms));
        let ab = age.map_or(AGE_EDGES.len() + 2, |a| bin_of(&AGE_EDGES, a));
        let gb = gap.map_or(GAP_EDGES.len() + 2, |g| bin_of(&GAP_EDGES, g));
        let turning = r.rate.is_some_and(|x| x.abs() >= TURN_DPS);
        by_age.add(ab, s);
        by_gap.add(gb, s);
        if turning {
            by_age_turn.add(ab, s);
            by_gap_turn.add(gb, s);
        } else if r.rate.is_some() {
            by_age_str.add(ab, s);
        }
        all.add(0, s);
        by_band
            .entry(s.band.clone())
            .or_insert_with(|| Table::new(&format!("Band {}", s.band), vec!["all".to_string()]))
            .add(0, s);
        if let (Some(x), Some(row), Some(e)) = (r.rate, row, est.get_mut(rb)) {
            // Both signed so that the reference turns "positive": a tracker rate of the same sign
            // and size reads the same number.
            e.0.push(x.abs());
            e.1.push(row.omega_dps * oa_math::signum_canon_f64(x));
        }
        let _ = writeln!(
            csv,
            "{:06x},{},{},{},{},{},{},{},{},{},{}",
            s.addr,
            s.t_ms,
            s.band,
            s.oa_cross,
            s.er_cross,
            s.oa_hdg,
            s.er_hdg,
            r.rate.map_or(String::new(), |x| format!("{x:.3}")),
            PHASES.get(r.phase).copied().unwrap_or(""),
            row.map_or(String::new(), |x| format!("{:.4}", x.mu_turn)),
            row.map_or(String::new(), |x| format!("{:.3}", x.omega_dps)),
        );
    }
    let mut o = String::new();
    let _ = writeln!(
        o,
        "DEVELOPMENT RUN \u{2014} turn diagnostic, not a hold-out score\nclip {}\nsamples {} common; tracker rows matched {}",
        adsb.display(),
        samples.len(),
        rows.len()
    );
    all.write(&mut o);
    by_rate.write(&mut o);
    by_phase.write(&mut o);
    by_mu.write(&mut o);
    for t in [&by_age, &by_age_turn, &by_age_str, &by_gap, &by_gap_turn] {
        t.write(&mut o);
    }
    for t in by_band.values() {
        t.write(&mut o);
    }
    let _ = writeln!(
        o,
        "\nTracker heading rate against the reference's, by reference rate bin (deg/s): median of reference rate, median of tracker rate (signed, clockwise positive), median absolute difference"
    );
    for (l, (r, e)) in by_rate.labels.iter().zip(&est) {
        let signed = |v: &[f64]| {
            let mut s = v.to_vec();
            s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
            s.get(s.len() / 2).copied().unwrap_or(f64::NAN)
        };
        let diff: Vec<f64> = r.iter().zip(e).map(|(a, b)| b - a).collect();
        let d = stat(&diff);
        let _ = writeln!(
            o,
            "  {:<12} n {:>6}  ref {:>7.2}  tracker {:>7.2}  |diff| median {:>6.2}",
            l,
            r.len(),
            signed(r),
            signed(e),
            d[1]
        );
    }
    let _ = writeln!(
        o,
        "\nLag of the reported velocity's track behind the reference's, seconds (reference track minus reported, over the reference rate; every {LAG_EVERY}th report of the aircraft above)"
    );
    for (l, v) in by_rate.labels.iter().zip(&lag) {
        let mut s = v.clone();
        s.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
        let q = |p: usize| s.get(p * s.len() / 100).copied().unwrap_or(f64::NAN);
        let _ = writeln!(
            o,
            "  {:<12} n {:>7}  p25 {:>6.2}  median {:>6.2}  p75 {:>6.2}",
            l,
            s.len(),
            q(25),
            q(50),
            q(75)
        );
    }
    fs::write(out_dir.join("turn_diag.txt"), &o).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("turn_samples.csv"), &csv).map_err(|e| e.to_string())?;
    Ok(o)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // REQ: OA-TRK-396
    #[test]
    fn statistics_are_nearest_rank_of_the_absolute_values() {
        let v: Vec<f64> = (1..=100)
            .map(|i| {
                if i % 2 == 0 {
                    -f64::from(i)
                } else {
                    f64::from(i)
                }
            })
            .collect();
        let [n, med, p95, rms] = stat(&v);
        assert_eq!((n, med, p95), (100.0, 50.0, 95.0));
        assert!((rms - (338_350.0_f64 / 100.0).sqrt()).abs() < 1.0e-9);
        assert_eq!(stat(&[])[0], 0.0);
    }

    // REQ: OA-TRK-396
    #[test]
    fn a_value_goes_to_the_bin_whose_lower_edge_it_reaches() {
        assert_eq!(bin_of(&RATE_EDGES, 0.0), 0);
        assert_eq!(bin_of(&RATE_EDGES, 0.25), 1);
        assert_eq!(bin_of(&RATE_EDGES, 2.99), 3);
        assert_eq!(bin_of(&RATE_EDGES, 3.0), 4);
    }

    // REQ: OA-TRK-396
    #[test]
    fn the_age_and_gap_are_those_of_the_last_report_at_or_before_the_time() {
        let t = [1000, 4000, 9000];
        assert_eq!(age_gap(&t, 4000), (Some(0.0), Some(5.0)));
        assert_eq!(age_gap(&t, 6500), (Some(2.5), Some(5.0)));
        assert_eq!(age_gap(&t, 500), (None, None));
        assert_eq!(age_gap(&t, 9500), (Some(0.5), None));
    }

    // REQ: OA-TRK-396
    #[test]
    fn only_common_samples_with_finite_errors_are_read_from_the_p002_csv() {
        let mut csv = String::from("# DEVELOPMENT RUN\naddr,t_ms,phase,band,common,oa_miss,oa_cross_m,oa_track_err_deg,oa_row_dt_ms,eram_miss,eram_cross_m,eram_track_err_deg\n");
        csv.push_str("abc123,1000,climb,below,1,0,-3.5,0.4,20,0,7.0,0.2\n");
        csv.push_str("abc124,1000,climb,below,0,0,1.0,0.1,0,1,,\n");
        csv.push_str("abc125,2000,climb,below,1,0,2.0,,0,0,5.0,0.3\n");
        let v = parse_samples(&csv).unwrap();
        assert_eq!(v.len(), 1);
        let s = v.first().unwrap();
        assert_eq!((s.addr, s.t_ms), (0xabc123, 1020));
        assert_eq!((s.oa_cross, s.er_cross), (-3.5, 7.0));
        assert!(parse_samples("addr,t_ms\n").is_err());
    }
}
