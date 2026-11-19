//! Velocity accuracy diagnostic: raw ADS-B velocity against the tracker's output, both scored
//! against the benchmark's reference velocity.
//!
//! ```text
//! cargo run --release -p oa-track --example velocity_diag -- --adsb <clip.oaf> --out <dir>
//!     [--bound-table] [--vel-sigma a,b,c,d] [--vel-scale x] [--no-vel-gate] [--vel-gate-chi2 x]
//!     [--gate-manoeuvre a] [--resync-k k] [--drop-withheld]
//! ```
//!
//! `--bound-table` uses the NACv-bound velocity noise table (the behaviour before calibration),
//! `--vel-sigma` overrides the table for NACv 1 to 4 (m/s per axis), `--vel-scale` multiplies it,
//! `--no-vel-gate` turns the tracker's velocity gate off, `--vel-gate-chi2` sets its limit.
//! Build with `--features gate-trace` for the tail breakdown by time since the last update and
//! track age and for the gated velocities by flight condition. The output also gives the east and north
//! component errors of the raw velocity by NACv (the calibration of `NACV_VEL_SIGMA_MPS`) and
//! the tracker's horizontal position error.
//!
//! Same split and reference as `vs_eram`: set A feeds the tracker, set B alone builds the
//! reference (`oa_bench::adsb_truth`, the RTS-smoothed velocity state of set-B positions). Raw
//! errors are the set-A measurements' reported velocity (M_HAS_VEL) against the reference at the
//! measurement time; tracker errors are the one-second output scored by the benchmark's pairing
//! (`PairTable::speed_err_kt`, `hdg_err_deg`). Flight condition comes from finite differences of
//! the reference velocity over +-5 s (the reference exposes no turn rate or acceleration).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oa_adsb::{reports_row, NONE_U16};
use oa_bench::adsb_truth;
use oa_bench::chunked::{benchmark_chunk, chunk_add, chunk_finish, truth_index, ChunkAcc};
use oa_bench::geo::{heading_deg, wrap180, KT_MPS};
use oa_bench::input::load_adsb;
use oa_bench::run::RunConfig;
use oa_bench::stats::stats_of;
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::TruthSample;
use oa_bench::truth::{empty_env, Env, Truth};
use oa_track::adsb::check_adsb_config;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::params::NACV_BOUND_VEL_SIGMA_MPS;
use oa_track::score::{
    interval_stats, run_tracker_spilled_observed, score_ranges, scoring_config, split_reports,
    IntervalStats, RowSet, RowSource, SCORE_CHUNK_ROWS, SPLIT_TEXT,
};
use oa_track::{Measurement, TrackerStats, DEFAULT_CONFIG, M_HAS_VEL, REFUSE_NAMES};
use oa_truth::{quality_check, QualityState, DEFAULT_CONFIG as TRUTH_DEFAULT};

const TOL_MS: i64 = 500;
/// Half-width of the finite-difference window, ms.
const HALF_MS: i64 = 5_000;
const STEADY_TURN_DPS: f64 = 0.5;
const STEADY_ACCEL: f64 = 0.2;
const TURNING_DPS: f64 = 1.5;
const ACCEL_MPS2: f64 = 0.5;

const CONDS: [&str; 5] = ["steady", "turning", "accel", "other", "no-ref"];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut out) = (None, None);
    let mut opts = Opts::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bound-table" => {
                opts.bound = true;
                continue;
            }
            "--drop-withheld" => {
                opts.drop_withheld = true;
                continue;
            }
            "--no-vel-gate" => {
                opts.no_gate = true;
                continue;
            }
            _ => {}
        }
        let v = args.next();
        match a.as_str() {
            "--adsb" => adsb = v.map(PathBuf::from),
            "--out" => out = v.map(PathBuf::from),
            "--gate-manoeuvre" => match v.and_then(|x| x.parse::<f64>().ok()) {
                Some(x) => opts.manoeuvre = x,
                None => {
                    eprintln!("velocity_diag: --gate-manoeuvre needs a number");
                    return ExitCode::from(2);
                }
            },
            "--resync-k" => match v.and_then(|x| x.parse::<u8>().ok()) {
                Some(x) => opts.resync_k = x,
                None => {
                    eprintln!("velocity_diag: --resync-k needs a number");
                    return ExitCode::from(2);
                }
            },
            "--vel-gate-chi2" => match v.and_then(|x| x.parse::<f64>().ok()) {
                Some(x) => opts.gate = Some(x),
                None => {
                    eprintln!("velocity_diag: --vel-gate-chi2 needs a number");
                    return ExitCode::from(2);
                }
            },
            "--vel-scale" => match v.and_then(|x| x.parse::<f64>().ok()) {
                Some(x) => opts.scale = x,
                None => {
                    eprintln!("velocity_diag: --vel-scale needs a number");
                    return ExitCode::from(2);
                }
            },
            "--vel-sigma" => {
                let p: Vec<f64> = v
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|x| x.parse().ok())
                    .collect();
                match <[f64; 4]>::try_from(p) {
                    Ok(a4) => opts.sigma = Some(a4),
                    Err(_) => {
                        eprintln!("velocity_diag: --vel-sigma needs four numbers a,b,c,d");
                        return ExitCode::from(2);
                    }
                }
            }
            _ => {
                eprintln!("velocity_diag: unknown option {a}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(adsb), Some(out)) = (adsb, out) else {
        eprintln!("usage: velocity_diag --adsb <clip.oaf> --out <dir> [--bound-table] [--vel-sigma a,b,c,d] [--vel-scale x] [--no-vel-gate] [--vel-gate-chi2 x]");
        return ExitCode::from(2);
    };
    match run(&adsb, &out, &opts) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("velocity_diag: {e}");
            ExitCode::from(1)
        }
    }
}

/// What became of one offered measurement, from the counter differences around it.
fn outcome(b: &TrackerStats, a: &TrackerStats) -> &'static str {
    for (k, name) in REFUSE_NAMES.iter().enumerate() {
        if a.refused.get(k) > b.refused.get(k) {
            return name;
        }
    }
    if a.duplicates > b.duplicates {
        "duplicate"
    } else if a.late_window > b.late_window {
        "late_window"
    } else if a.late_ring > b.late_ring {
        "late_ring"
    } else if a.capacity_refused > b.capacity_refused {
        "capacity_refused"
    } else if a.numerical_refused > b.numerical_refused {
        "numerical_refused"
    } else if a.time_held > b.time_held {
        "time_held"
    } else if a.resyncs > b.resyncs {
        "jump_resync"
    } else if a.jump_rejected > b.jump_rejected {
        "jump_rejected"
    } else if a.init_implausible > b.init_implausible {
        "init_implausible"
    } else if a.init_window_expired > b.init_window_expired {
        "init_window_expired"
    } else if a.started > b.started {
        "track_started"
    } else if a.applied > b.applied || a.refiltered > b.refiltered {
        "applied"
    } else {
        "no_counter_moved"
    }
}

/// Age bands of an uncertain row, seconds since the track's last applied update.
const UNC_BANDS_S: [i64; 4] = [15, 30, 60, 120];
/// Margin of the clip box, 1e-7 degrees (0.1 degree, about 11 km), for a last report to count as
/// at the box edge.
const EDGE_E7: i32 = 1_000_000;

/// Tick rows flagged uncertain: live or coasting, by age since the last applied update, and by
/// whether the aircraft's last applied report was at the box edge or low (left or landed).
#[derive(Default)]
struct Uncertain {
    rows: u64,
    uncertain: u64,
    /// [live, coasting] by age band (five bands).
    by_age: [[u64; 5]; 2],
    gone: u64,
    gone_edge: u64,
    gone_low: u64,
}

impl Uncertain {
    fn add(
        &mut self,
        r: &oa_track::score::ScoreRows,
        applied: &BTreeMap<u32, Vec<i64>>,
        last: &BTreeMap<u32, (i64, i32, i32, i32)>,
        ext: (i32, i32, i32, i32),
    ) {
        self.rows += r.addr.len() as u64;
        for i in 0..r.addr.len() {
            if !r.uncertain.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (Some(&a), Some(&t)) = (r.addr.get(i), r.t_ns.get(i)) else {
                continue;
            };
            self.uncertain += 1;
            let coast = usize::from(r.coast.get(i).copied().unwrap_or(false));
            let since_s = applied
                .get(&a)
                .and_then(|v| v.get(v.partition_point(|&x| x <= t).checked_sub(1)?))
                .map_or(i64::MAX, |&u| (t - u) / 1_000_000_000);
            let band = UNC_BANDS_S.iter().position(|&b| since_s < b).unwrap_or(4);
            if let Some(c) = self.by_age.get_mut(coast).and_then(|x| x.get_mut(band)) {
                *c += 1;
            }
            if let Some(&(_, la, lo, alt)) = last.get(&a) {
                let edge = la - ext.0 < EDGE_E7
                    || ext.1 - la < EDGE_E7
                    || lo - ext.2 < EDGE_E7
                    || ext.3 - lo < EDGE_E7;
                let low = alt != oa_track::NONE_I32 && alt < 1000;
                self.gone += u64::from(edge || low);
                self.gone_edge += u64::from(edge);
                self.gone_low += u64::from(low);
            }
        }
    }

    fn report(&self, ext: (i32, i32, i32, i32)) -> String {
        let mut o = format!(
            "12. Uncertain tick rows (95% radius over 926 m): {} of {} tick rows; clip extent lat {:.2}..{:.2} lon {:.2}..{:.2}\n",
            self.uncertain,
            self.rows,
            f64::from(ext.0) / 1e7,
            f64::from(ext.1) / 1e7,
            f64::from(ext.2) / 1e7,
            f64::from(ext.3) / 1e7
        );
        for (name, row) in ["live", "coasting"].iter().zip(&self.by_age) {
            o.push_str(&format!(
                "    {name:<9} 0-15 s {} 15-30 s {} 30-60 s {} 60-120 s {} >120 s {}\n",
                row[0], row[1], row[2], row[3], row[4]
            ));
        }
        o.push_str(&format!(
            "    aircraft whose last applied report was at the box edge or below 1000 ft: {} rows (edge {}, low {})\n",
            self.gone, self.gone_edge, self.gone_low
        ));
        o
    }
}

/// Outcomes that put the measurement into a track (the track's last applied update moves).
fn is_applied(o: &str) -> bool {
    matches!(o, "applied" | "track_started" | "jump_resync")
}

/// Settings of the sweep.
struct Opts {
    /// Start from the NACv-bound table instead of the calibrated one.
    bound: bool,
    /// Override of the table for NACv 1 to 4.
    sigma: Option<[f64; 4]>,
    /// Multiplier on the table.
    scale: f64,
    no_gate: bool,
    /// Limit of the velocity gate (overrides the default; `no_gate` wins).
    gate: Option<f64>,
    /// Manoeuvre allowance of the position gate, m/s^2 (0 off).
    manoeuvre: f64,
    /// Consistent rejected reports that re-anchor the track (0 off).
    resync_k: u8,
    /// Score without the rows the tracker used to withhold (coasting and uncertain).
    drop_withheld: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            bound: false,
            sigma: None,
            scale: 1.0,
            no_gate: false,
            gate: None,
            manoeuvre: oa_track::params::GATE_MANOEUVRE_MPS2,
            resync_k: 0,
            drop_withheld: false,
        }
    }
}

/// NACv group of a velocity: 0 default used, 1 to 4 the NACv, 5 no velocity.
fn nacv_groups(
    reports: &oa_adsb::Reports,
    cfg: &AdsbConfig,
    n_meas: usize,
) -> Result<Vec<u8>, String> {
    // Replays the adapter's row loop (same order, same quality state, same pass and velocity
    // rules) and records which NACv each passing row's velocity was built with.
    let mut order: Vec<(i64, u32)> = reports
        .time_ns
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, i as u32))
        .collect();
    order.sort_unstable();
    let mut quality: BTreeMap<u32, QualityState> = BTreeMap::new();
    let mut out = Vec::with_capacity(n_meas);
    for &(t, i) in &order {
        let Some(row) = reports_row(reports, i as usize) else {
            continue;
        };
        let q = quality.entry(row.icao).or_default();
        let v = quality_check(q, &row, t, &cfg.gates);
        if v.removal.is_some() {
            continue;
        }
        if !(oa_truth::nacp_epu_m(v.nacp.unwrap_or(0)).is_finite()) {
            continue;
        }
        let g = if row.gs_dkt != NONE_U16 && row.track_ddeg != NONE_U16 {
            match v.nacv {
                Some(k) if (1..=4).contains(&k) => k,
                _ => 0,
            }
        } else {
            5
        };
        out.push(g);
    }
    if out.len() != n_meas {
        return Err(format!(
            "NACv replay gave {} rows, adapter gave {n_meas}",
            out.len()
        ));
    }
    Ok(out)
}

fn group_name(g: u8) -> String {
    match g {
        0 => "default NACv".to_string(),
        k => format!("NACv {k}"),
    }
}

/// Flight condition from the reference at `t_ms`: index into [`CONDS`].
fn condition(q: &dyn Fn(i64) -> Result<TruthSample, oa_bench::truth::NoTruth>, t_ms: i64) -> usize {
    let (Ok(a), Ok(b)) = (q(t_ms - HALF_MS), q(t_ms + HALF_MS)) else {
        return 4;
    };
    let (sa, sb) = (
        a.vel_east_mps.hypot(a.vel_north_mps),
        b.vel_east_mps.hypot(b.vel_north_mps),
    );
    let min_hdg = RunConfig::default().pair.min_heading_mps;
    if sa < min_hdg || sb < min_hdg {
        return 4;
    }
    let dt = 2.0 * HALF_MS as f64 / 1000.0;
    let turn = wrap180(
        heading_deg(b.vel_east_mps, b.vel_north_mps) - heading_deg(a.vel_east_mps, a.vel_north_mps),
    ) / dt;
    let acc = (sb - sa) / dt;
    if turn.abs() >= TURNING_DPS {
        1
    } else if acc.abs() >= ACCEL_MPS2 {
        2
    } else if turn.abs() < STEADY_TURN_DPS && acc.abs() < STEADY_ACCEL {
        0
    } else {
        3
    }
}

/// Error samples keyed by (series label, group label, condition label).
#[derive(Default)]
struct Acc {
    speed: BTreeMap<(String, String, usize), Vec<f64>>,
    track: BTreeMap<(String, String, usize), Vec<f64>>,
}

impl Acc {
    fn add(&mut self, series: &str, group: &str, cond: usize, spd: f64, trk: f64) {
        for c in [cond, 5] {
            for g in [group, "all"]
                .into_iter()
                .take(if group == "all" { 1 } else { 2 })
            {
                let k = (series.to_string(), g.to_string(), c);
                if spd.is_finite() {
                    self.speed.entry(k.clone()).or_default().push(spd);
                }
                if trk.is_finite() {
                    self.track.entry(k).or_default().push(trk);
                }
            }
        }
    }
}

fn table(
    o: &mut String,
    title: &str,
    unit: &str,
    m: &BTreeMap<(String, String, usize), Vec<f64>>,
    series: &[&str],
) {
    let _ = writeln!(o, "{title} ({unit}; mean signed, others of |err|)");
    let _ = writeln!(
        o,
        "  {:<18} {:<14} {:<8} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "series", "group", "cond", "n", "mean", "med|e|", "p95|e|", "rms"
    );
    for s in series {
        let mut groups: Vec<&String> = m
            .keys()
            .filter(|k| k.0 == *s)
            .map(|k| &k.1)
            .collect::<Vec<_>>();
        groups.sort();
        groups.dedup();
        for g in groups {
            for c in 0..=5usize {
                let Some(v) = m.get(&(s.to_string(), g.clone(), c)) else {
                    continue;
                };
                let st = stats_of(v);
                let cname = if c == 5 {
                    "ALL"
                } else {
                    CONDS.get(c).copied().unwrap_or("?")
                };
                let _ = writeln!(
                    o,
                    "  {s:<18} {g:<14} {cname:<8} {:>9} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
                    st.n, st.mean, st.median, st.p95, st.rms
                );
            }
        }
    }
    let _ = writeln!(o);
}

/// Tracker ticks of one aircraft: (time ms, speed error m/s, track error deg), by time.
type Ticks = BTreeMap<u32, Vec<(i64, f64, f64)>>;

fn nearest_tick(v: &[(i64, f64, f64)], t: i64) -> Option<(i64, f64, f64)> {
    let i = v.partition_point(|s| s.0 < t);
    let before = i.checked_sub(1).and_then(|j| v.get(j));
    let pick = match (before, v.get(i)) {
        (Some(b), Some(a)) => {
            if t - b.0 <= a.0 - t {
                b
            } else {
                a
            }
        }
        (Some(b), None) => b,
        (None, Some(a)) => a,
        (None, None) => return None,
    };
    ((pick.0 - t).abs() <= TOL_MS).then_some(*pick)
}

fn run(clip_path: &Path, out_dir: &Path, opts: &Opts) -> Result<String, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let clip = load_adsb(clip_path).map_err(|e| e.to_string())?;
    let clip_hash = clip.hash.sha256.clone();
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(clip);
    let mut acfg = AdsbConfig::default();
    if opts.bound {
        acfg.vel_sigma_mps = NACV_BOUND_VEL_SIGMA_MPS;
    }
    if let Some(a4) = opts.sigma {
        for (k, x) in a4.iter().enumerate() {
            if let Some(slot) = acfg.vel_sigma_mps.get_mut(k + 1) {
                *slot = *x;
            }
        }
    }
    for x in acfg.vel_sigma_mps.iter_mut().skip(1) {
        *x *= opts.scale;
    }
    check_adsb_config(&acfg).map_err(|e| format!("adapter configuration: {e}"))?;
    let (meas, astats) = adsb_measurements(&set_a, &acfg);
    let groups = nacv_groups(&set_a, &acfg, meas.len())?;
    drop(set_a);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let mut cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    if let Some(x) = opts.gate {
        cfg.vel_gate_chi2 = x;
    }
    if opts.no_gate {
        cfg.vel_gate_chi2 = f64::INFINITY;
    }
    cfg.gate_manoeuvre_mps2 = opts.manoeuvre;
    cfg.resync_consistent_k = opts.resync_k;
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let mut events: Vec<(u32, i64, &'static str)> = Vec::with_capacity(meas.len());
    // Applied reports: (addr, time, lat, lon, alt) for the uncertain-row breakdown (section 12).
    let mut applied_at: Vec<(u32, i64, i32, i32, i32)> = Vec::new();
    let extent = meas.iter().fold(
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN),
        |(a, b, c, d), m| {
            (
                a.min(m.lat_e7),
                b.max(m.lat_e7),
                c.min(m.lon_e7),
                d.max(m.lon_e7),
            )
        },
    );
    let run = run_tracker_spilled_observed(&meas, &cfg, &spill, &mut |i, b, a| {
        if let Some(m) = meas.get(i) {
            let o = outcome(b, a);
            if is_applied(o) {
                applied_at.push((m.addr, m.toa_ns, m.lat_e7, m.lon_e7, m.alt_ft));
            }
            events.push((m.addr, m.toa_ns, o));
        }
    })
    .map_err(|e| e.to_string())?;
    let velocity_gated = run.stats.velocity_gated;
    let (jump_rejected, resyncs) = (run.stats.jump_rejected, run.stats.resyncs);
    let (rows_uncertain, nan_covariance) = (run.stats.rows_uncertain, run.stats.nan_covariance);
    let trace = oa_track::gate_trace::gate_trace_take();
    let drop_after_ns = cfg.drop_after_ns;
    eprintln!("tracker done");

    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let env: Env<'_> = empty_env();
    let mut rcfg = RunConfig::default();
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    let tix = truth_index(&truth);
    eprintln!("reference built");

    // Tracker pairs, scored exactly as vs_eram does.
    let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    let tick_rows: u64 = counts.values().sum();
    let mut acc = ChunkAcc::default();
    let mut unc = Uncertain::default();
    let mut applied_by: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    let mut last_of: BTreeMap<u32, (i64, i32, i32, i32)> = BTreeMap::new();
    for &(a, t, la, lo, alt) in &applied_at {
        applied_by.entry(a).or_default().push(t);
        let e = last_of.entry(a).or_insert((t, la, lo, alt));
        if t >= e.0 {
            *e = (t, la, lo, alt);
        }
    }
    for v in applied_by.values_mut() {
        v.sort_unstable();
    }
    for range in score_ranges(&counts, SCORE_CHUNK_ROWS) {
        let mut r = run
            .rows_in(RowSet::Ticks, range.0, range.1)
            .map_err(|e| e.0)?;
        unc.add(&r, &applied_by, &last_of, extent);
        if opts.drop_withheld {
            // The rows the tracker withheld before it flagged them: coasting and uncertain.
            let keep: Vec<bool> = r
                .coast
                .iter()
                .zip(&r.uncertain)
                .map(|(&c, &u)| !(c && u))
                .collect();
            fn pick<T: Copy>(v: &[T], keep: &[bool]) -> Vec<T> {
                v.iter()
                    .zip(keep)
                    .filter_map(|(x, &k)| k.then_some(*x))
                    .collect()
            }
            r = oa_track::score::ScoreRows {
                addr: pick(&r.addr, &keep),
                t_ns: pick(&r.t_ns, &keep),
                lat_e7: pick(&r.lat_e7, &keep),
                lon_e7: pick(&r.lon_e7, &keep),
                alt_ft: pick(&r.alt_ft, &keep),
                vel_east_mps: pick(&r.vel_east_mps, &keep),
                vel_north_mps: pick(&r.vel_north_mps, &keep),
                coast: pick(&r.coast, &keep),
                uncertain: pick(&r.uncertain, &keep),
            };
        }
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
        let c = benchmark_chunk(&legacy, &truth, &tix, &env, &rcfg, range);
        chunk_add(&mut acc, c).map_err(|e| e.0)?;
    }
    let (_, pairs) = chunk_finish(acc, &rcfg).map_err(|e| e.0)?;
    drop(run);
    let _ = fs::remove_dir_all(&spill);
    eprintln!("tracker paired: {}", pairs.t_ms.len());

    let pos_err: Vec<f64> = pairs
        .cross_m
        .iter()
        .zip(&pairs.along_m)
        .map(|(&c, &l)| f64::from(c).hypot(f64::from(l)))
        .collect();
    let mut comps: BTreeMap<(String, &'static str, bool), Vec<f64>> = BTreeMap::new();
    let mut a = Acc::default();
    let mut ticks: Ticks = BTreeMap::new();
    let nan = f32::NAN;
    for i in 0..pairs.t_ms.len() {
        let (Some(&addr), Some(&t)) = (pairs.addr.get(i), pairs.t_ms.get(i)) else {
            continue;
        };
        let spd = f64::from(pairs.speed_err_kt.get(i).copied().unwrap_or(nan)) * KT_MPS;
        let trk = f64::from(pairs.hdg_err_deg.get(i).copied().unwrap_or(nan));
        let qa = |tt: i64| adsb_truth::query(&built.truth, addr, tt);
        let c = condition(&qa, t);
        a.add("OURS (1 Hz ticks)", "all", c, spd, trk);
        ticks.entry(addr).or_default().push((t, spd, trk));
    }
    for v in ticks.values_mut() {
        v.sort_by(|x, y| x.0.cmp(&y.0));
        v.dedup_by_key(|s| s.0);
    }

    // Raw ADS-B velocity at each set-A measurement with a velocity.
    let (mut n_vel, mut n_noref, mut n_unc, mut n_slow) = (0u64, 0u64, 0u64, 0u64);
    for (m, &g) in meas.iter().zip(&groups) {
        let m: &Measurement = m;
        if m.flags & M_HAS_VEL == 0 || g == 5 {
            continue;
        }
        n_vel += 1;
        let t_ms = m.toa_ns.div_euclid(1_000_000);
        let ts = match adsb_truth::query(&built.truth, m.addr, t_ms) {
            Ok(ts) => ts,
            Err(_) => {
                n_noref += 1;
                continue;
            }
        };
        if ts.sigma95_h_m > rcfg.pair.max_sigma_m {
            n_unc += 1;
            continue;
        }
        let tv = ts.vel_east_mps.hypot(ts.vel_north_mps);
        if tv < rcfg.pair.min_speed_mps {
            n_slow += 1;
            continue;
        }
        let (ve, vn) = (f64::from(m.vel_east_mps), f64::from(m.vel_north_mps));
        let spd = ve.hypot(vn) - tv;
        let trk = if tv >= rcfg.pair.min_heading_mps {
            wrap180(heading_deg(ve, vn) - heading_deg(ts.vel_east_mps, ts.vel_north_mps))
        } else {
            f64::NAN
        };
        let qa = |tt: i64| adsb_truth::query(&built.truth, m.addr, tt);
        let c = condition(&qa, t_ms);
        let gn = group_name(g);
        let (ee, en) = (ve - ts.vel_east_mps, vn - ts.vel_north_mps);
        for (axis, e) in [("east", ee), ("north", en)] {
            comps.entry((gn.clone(), axis, false)).or_default().push(e);
            if c == 0 {
                comps.entry((gn.clone(), axis, true)).or_default().push(e);
            }
        }
        a.add("RAW ADS-B", &gn, c, spd, trk);
        if let Some((_, os, ot)) = ticks.get(&m.addr).and_then(|v| nearest_tick(v, t_ms)) {
            a.add("RAW (matched)", &gn, c, spd, trk);
            a.add("OURS (matched)", &gn, c, os, ot);
        }
    }

    // Tail breakdown of the tracker's 1 Hz errors (needs the gate trace).
    let mut brk = String::new();
    {
        // Applied updates per aircraft: (time ns, start of the track it belongs to).
        let mut applied: BTreeMap<u32, Vec<(i64, i64)>> = BTreeMap::new();
        for r in &trace {
            let v = applied.entry(r.addr).or_default();
            let start = match v.last() {
                Some(&(t, st)) if r.toa_ns - t <= drop_after_ns => st,
                _ => r.toa_ns,
            };
            v.push((r.toa_ns, start));
        }
        let mut b_since: BTreeMap<&'static str, [Vec<f64>; 2]> = BTreeMap::new();
        let mut b_age: BTreeMap<&'static str, [Vec<f64>; 2]> = BTreeMap::new();
        let mut b_coast: BTreeMap<&'static str, [Vec<f64>; 2]> = BTreeMap::new();
        let mut b_cond: BTreeMap<&'static str, [Vec<f64>; 2]> = BTreeMap::new();
        let mut per_addr: BTreeMap<u32, (u64, f64)> = BTreeMap::new();
        for i in 0..pairs.t_ms.len() {
            let (Some(&addr), Some(&t)) = (pairs.addr.get(i), pairs.t_ms.get(i)) else {
                continue;
            };
            let spd = f64::from(pairs.speed_err_kt.get(i).copied().unwrap_or(nan)) * KT_MPS;
            let pos = f64::from(pairs.cross_m.get(i).copied().unwrap_or(nan))
                .hypot(f64::from(pairs.along_m.get(i).copied().unwrap_or(nan)));
            let t_ns = t * 1_000_000;
            let (since, age) = match applied.get(&addr) {
                Some(v) => {
                    let j = v.partition_point(|x| x.0 <= t_ns);
                    match j.checked_sub(1).and_then(|k| v.get(k)) {
                        Some(&(ta, st)) => (
                            Some((t_ns - ta) as f64 / 1e9),
                            Some((t_ns - st) as f64 / 1e9),
                        ),
                        None => (None, None),
                    }
                }
                None => (None, None),
            };
            let since_l = match since {
                None => "no update yet",
                Some(x) if x < 1.0 => "0-1 s",
                Some(x) if x < 2.0 => "1-2 s",
                Some(x) if x < 5.0 => "2-5 s",
                Some(x) if x < 15.0 => "5-15 s",
                Some(_) => ">15 s",
            };
            let age_l = match age {
                None => "no update yet",
                Some(x) if x < 30.0 => "<30 s",
                Some(x) if x < 120.0 => "30-120 s",
                Some(_) => ">120 s",
            };
            let coast_l = if pairs.coast.get(i).copied().unwrap_or(0) != 0 {
                "coast"
            } else {
                "not coast"
            };
            let qa = |tt: i64| adsb_truth::query(&built.truth, addr, tt);
            let cond_l = CONDS.get(condition(&qa, t)).copied().unwrap_or("?");
            for (m, l) in [
                (&mut b_since, since_l),
                (&mut b_age, age_l),
                (&mut b_coast, coast_l),
                (&mut b_cond, cond_l),
            ] {
                let e = m.entry(l).or_default();
                if spd.is_finite() {
                    e[0].push(spd);
                }
                if pos.is_finite() {
                    e[1].push(pos);
                }
            }
            if spd.is_finite() {
                let e = per_addr.entry(addr).or_insert((0, 0.0));
                e.0 += 1;
                e.1 += spd * spd;
            }
        }
        let tot = |m: &BTreeMap<&'static str, [Vec<f64>; 2]>, k: usize| -> f64 {
            m.values()
                .map(|v| {
                    v.get(k)
                        .map_or(0.0, |c| c.iter().map(|x| x * x).sum::<f64>())
                })
                .sum()
        };
        let _ = writeln!(
            brk,
            "9. Tail breakdown of OUR 1 Hz errors (n, median |e|, RMS, share of total squared error). Time since the last applied update and track age come from the gate trace (a track is a run of applied updates with gaps up to {} s); coast flag from the benchmark pair; condition as above.",
            drop_after_ns / 1_000_000_000
        );
        if trace.is_empty() {
            let _ = writeln!(
                brk,
                "   (gate trace empty: build with --features gate-trace)"
            );
        }
        for (title, m) in [
            ("(a) time since last applied update", &b_since),
            ("(b) track age", &b_age),
            ("(c) coast flag", &b_coast),
            ("(d) flight condition", &b_cond),
        ] {
            let _ = writeln!(brk, "  {title}");
            let _ = writeln!(
                brk,
                "    {:<14} | {:>9} {:>8} {:>8} {:>7} | {:>9} {:>8} {:>8} {:>7}",
                "bucket", "spd n", "med|e|", "rms", "share", "pos n", "med|e|", "rms", "share"
            );
            let (ts, tp) = (tot(m, 0), tot(m, 1));
            for (l, v) in m {
                let cell = |x: &Vec<f64>, total: f64| {
                    let st = stats_of(x);
                    let sq: f64 = x.iter().map(|e| e * e).sum();
                    format!(
                        "{:>9} {:>8.3} {:>8.3} {:>6.1}%",
                        st.n,
                        st.median,
                        st.rms,
                        if total > 0.0 { 100.0 * sq / total } else { 0.0 }
                    )
                };
                let _ = writeln!(
                    brk,
                    "    {l:<14} | {} | {}",
                    cell(&v[0], ts),
                    cell(&v[1], tp)
                );
            }
        }
        let total_sq: f64 = per_addr.values().map(|x| x.1).sum();
        let mut top: Vec<(&u32, &(u64, f64))> = per_addr.iter().collect();
        top.sort_by(|a, b| b.1 .1.total_cmp(&a.1 .1));
        let _ = writeln!(brk, "  Top 10 aircraft by squared ground-speed error");
        for (a, (n, sq)) in top.into_iter().take(10) {
            let _ = writeln!(
                brk,
                "    {a:06X} n {n:>8} rms {:>8.3} share {:>5.1}%",
                (sq / *n as f64).sqrt(),
                100.0 * sq / total_sq
            );
        }
        let _ = writeln!(brk);

        // Gated velocities by flight condition and their distance.
        let _ = writeln!(
            brk,
            "10. Gated velocities (first applications; gate limit {}): applied {}, with velocity {}, gated {}",
            cfg.vel_gate_chi2,
            trace.len(),
            trace.iter().filter(|r| !r.d2.is_nan() || r.gated).count(),
            trace.iter().filter(|r| r.gated).count()
        );
        let mut by_cond = [0u64; 5];
        let mut d2s: Vec<f64> = Vec::new();
        let mut by_cond_d2: [Vec<f64>; 5] = Default::default();
        for r in trace.iter().filter(|r| r.gated) {
            let t_ms = r.toa_ns.div_euclid(1_000_000);
            let qa = |tt: i64| adsb_truth::query(&built.truth, r.addr, tt);
            let c = condition(&qa, t_ms).min(4);
            if let Some(x) = by_cond.get_mut(c) {
                *x += 1;
            }
            if r.d2.is_finite() {
                d2s.push(r.d2);
                if let Some(v) = by_cond_d2.get_mut(c) {
                    v.push(r.d2);
                }
            }
        }
        let med = |v: &mut Vec<f64>| -> f64 {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
        };
        for (c, name) in CONDS.iter().enumerate() {
            let mut v = by_cond_d2.get(c).cloned().unwrap_or_default();
            let _ = writeln!(
                brk,
                "    {name:<8} gated {:>8} median d2 {:>10.3}",
                by_cond.get(c).copied().unwrap_or(0),
                med(&mut v)
            );
        }
        let _ = writeln!(
            brk,
            "    all      gated {:>8} median d2 {:>10.3} (finite d2 {}; non-finite d2 count as gated)",
            by_cond.iter().sum::<u64>(),
            med(&mut d2s),
            d2s.len()
        );
        let _ = writeln!(brk);
    }

    // Section 11: stale-track ticks, input gaps against refused input.
    let mut brk2 = String::new();
    {
        type Ev = Vec<(i64, &'static str)>;
        let mut per: BTreeMap<u32, Ev> = BTreeMap::new();
        let mut outcomes: BTreeMap<&'static str, u64> = BTreeMap::new();
        for &(a, t, o) in &events {
            per.entry(a).or_default().push((t, o));
            *outcomes.entry(o).or_default() += 1;
        }
        let mut resync_at: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
        for &(a, t, o) in &events {
            if o == "jump_resync" {
                resync_at.entry(a).or_default().push(t);
            }
        }
        drop(events);
        let _ = writeln!(brk2, "11. Stale ticks: input gaps against refused input");
        let _ = writeln!(brk2, "  Outcome of every offered measurement (counter differences around tracker_update; applied-like: applied, track_started, jump_resync):");
        for (k, v) in &outcomes {
            let _ = writeln!(brk2, "    {k:<26} {v:>9}");
        }
        // Gaps between consecutive applied updates of one aircraft.
        const GAP_LABELS: [&str; 5] = ["0-1 s", "1-2 s", "2-5 s", "5-15 s", ">15 s"];
        let gl = |s: f64| -> usize {
            if s < 1.0 {
                0
            } else if s < 2.0 {
                1
            } else if s < 5.0 {
                2
            } else if s < 15.0 {
                3
            } else {
                4
            }
        };
        let mut gaps = [(0u64, 0u64, 0u64); 5]; // gaps, with no offered input, with offered input
        let mut gap_reasons: BTreeMap<(usize, &'static str), u64> = BTreeMap::new();
        let mut per_ac: BTreeMap<u32, (u64, BTreeMap<&'static str, u64>)> = BTreeMap::new();
        for (addr, ev) in &per {
            let mut prev: Option<i64> = None;
            let mut acc: Vec<&'static str> = Vec::new();
            for &(t, o) in ev {
                if !is_applied(o) {
                    acc.push(o);
                    continue;
                }
                if let Some(p) = prev {
                    let k = gl((t - p) as f64 / 1e9);
                    if let Some(g) = gaps.get_mut(k) {
                        g.0 += 1;
                        if acc.is_empty() {
                            g.1 += 1;
                        } else {
                            g.2 += 1;
                        }
                    }
                    for r in &acc {
                        *gap_reasons.entry((k, r)).or_default() += 1;
                        if k >= 3 {
                            let e = per_ac.entry(*addr).or_default();
                            e.0 += 1;
                            *e.1.entry(r).or_default() += 1;
                        }
                    }
                }
                prev = Some(t);
                acc.clear();
            }
        }
        let _ = writeln!(brk2, "  Gaps between consecutive applied updates of one aircraft, by gap length (open final gaps not counted):");
        let _ = writeln!(
            brk2,
            "    {:<8} {:>9} {:>14} {:>16}",
            "gap", "gaps", "no offered", "offered, refused"
        );
        for (k, g) in gaps.iter().enumerate() {
            let _ = writeln!(
                brk2,
                "    {:<8} {:>9} {:>14} {:>16}",
                GAP_LABELS.get(k).copied().unwrap_or("?"),
                g.0,
                g.1,
                g.2
            );
        }
        let _ = writeln!(
            brk2,
            "  Offered-but-not-applied measurements inside those gaps, by gap length and reason:"
        );
        for ((k, r), v) in &gap_reasons {
            let _ = writeln!(
                brk2,
                "    {:<8} {r:<26} {v:>9}",
                GAP_LABELS.get(*k).copied().unwrap_or("?")
            );
        }
        let mut top: Vec<_> = per_ac.iter().collect();
        top.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        let _ = writeln!(
            brk2,
            "  5 aircraft with most refused measurements in gaps of 5 s or more:"
        );
        for (a, (n, rs)) in top.into_iter().take(5) {
            let _ = writeln!(brk2, "    {a:06X} refused {n:>7}  {rs:?}");
        }
        // Ticks: time since last applied, and whether input was offered since.
        let mut cells: BTreeMap<(usize, usize), [Vec<f64>; 2]> = BTreeMap::new();
        let mut offered_since: BTreeMap<(usize, usize), u64> = BTreeMap::new();
        for i in 0..pairs.t_ms.len() {
            let (Some(&addr), Some(&t)) = (pairs.addr.get(i), pairs.t_ms.get(i)) else {
                continue;
            };
            let Some(ev) = per.get(&addr) else { continue };
            let t_ns = t * 1_000_000;
            let upto = ev.partition_point(|x| x.0 <= t_ns);
            let Some(head) = ev.get(..upto) else { continue };
            let Some(la) = head.iter().rev().find(|x| is_applied(x.1)).map(|x| x.0) else {
                continue;
            };
            let lo = head.last().map_or(la, |x| x.0);
            let k = gl((t_ns - la) as f64 / 1e9);
            let cls = usize::from(lo > la);
            let spd = f64::from(pairs.speed_err_kt.get(i).copied().unwrap_or(nan)) * KT_MPS;
            let pos = f64::from(pairs.cross_m.get(i).copied().unwrap_or(nan))
                .hypot(f64::from(pairs.along_m.get(i).copied().unwrap_or(nan)));
            let e = cells.entry((k, cls)).or_default();
            if spd.is_finite() {
                e[0].push(spd);
            }
            if pos.is_finite() {
                e[1].push(pos);
            }
            if cls == 1 {
                *offered_since.entry((k, cls)).or_default() += 1;
            }
        }
        let tot: [f64; 2] = [0, 1].map(|c| {
            cells
                .values()
                .map(|v| {
                    v.get(c)
                        .map_or(0.0, |x| x.iter().map(|e| e * e).sum::<f64>())
                })
                .sum()
        });
        let _ = writeln!(brk2, "  Ticks by time since last applied update and whether input was offered since (n, median |e|, RMS, share of total squared error):");
        let _ = writeln!(
            brk2,
            "    {:<8} {:<17} | {:>9} {:>8} {:>8} {:>7} | {:>9} {:>8} {:>8} {:>7}",
            "since", "class", "spd n", "med|e|", "rms", "share", "pos n", "med|e|", "rms", "share"
        );
        for ((k, cls), v) in &cells {
            let cell = |x: &Vec<f64>, total: f64| {
                let st = stats_of(x);
                let sq: f64 = x.iter().map(|e| e * e).sum();
                format!(
                    "{:>9} {:>8.3} {:>8.3} {:>6.1}%",
                    st.n,
                    st.median,
                    st.rms,
                    if total > 0.0 { 100.0 * sq / total } else { 0.0 }
                )
            };
            let _ = writeln!(
                brk2,
                "    {:<8} {:<17} | {} | {}",
                GAP_LABELS.get(*k).copied().unwrap_or("?"),
                if *cls == 0 {
                    "no offered input"
                } else {
                    "offered, refused"
                },
                cell(&v[0], tot[0]),
                cell(&v[1], tot[1])
            );
        }
        // Errors in the 30 s after each resync, against the other ticks.
        let (mut after, mut other): ([Vec<f64>; 2], [Vec<f64>; 2]) = Default::default();
        for i in 0..pairs.t_ms.len() {
            let (Some(&addr), Some(&t)) = (pairs.addr.get(i), pairs.t_ms.get(i)) else {
                continue;
            };
            let t_ns = t * 1_000_000;
            let recent = resync_at.get(&addr).is_some_and(|v| {
                let j = v.partition_point(|&x| x <= t_ns);
                j.checked_sub(1)
                    .and_then(|k| v.get(k))
                    .is_some_and(|&r| t_ns - r <= 30_000_000_000)
            });
            let spd = f64::from(pairs.speed_err_kt.get(i).copied().unwrap_or(nan)) * KT_MPS;
            let pos = f64::from(pairs.cross_m.get(i).copied().unwrap_or(nan))
                .hypot(f64::from(pairs.along_m.get(i).copied().unwrap_or(nan)));
            let dst = if recent { &mut after } else { &mut other };
            if spd.is_finite() {
                dst[0].push(spd);
            }
            if pos.is_finite() {
                dst[1].push(pos);
            }
        }
        let _ = writeln!(brk2, "  Error in the 30 s after each re-anchoring on a report (jump_resync), against all other ticks:");
        for (name, d) in [("30 s after a resync", &after), ("all other ticks", &other)] {
            let (sp, po) = (stats_of(&d[0]), stats_of(&d[1]));
            let _ = writeln!(
                brk2,
                "    {name:<20} speed n {:>8} med|e| {:>7.3} p95|e| {:>8.3} | position n {:>8} med|e| {:>8.3} p95|e| {:>9.3}",
                sp.n, sp.median, sp.p95, po.n, po.median, po.p95
            );
        }
        let _ = writeln!(brk2);
    }

    let mut o = String::new();
    let _ = writeln!(
        o,
        "Velocity diagnostic: raw ADS-B velocity vs tracker output"
    );
    let _ = writeln!(
        o,
        "  ADS-B clip: {} (digest {clip_hash})",
        clip_path.display()
    );
    let _ = writeln!(o, "  split: {SPLIT_TEXT}");
    let _ = writeln!(
        o,
        "  set A {} reports -> {} measurements ({} with velocity, {} of them default NACv); set B {} reports -> reference ({} legs)",
        split.set_a,
        meas.len(),
        astats.with_velocity,
        astats.velocity_default_nacv,
        split.set_b,
        built.legs.len()
    );
    let _ = writeln!(
        o,
        "  raw velocities without reference {n_noref}, reference too uncertain (> {} m) {n_unc}, reference speed under {} m/s {n_slow}, scored {}",
        rcfg.pair.max_sigma_m,
        rcfg.pair.min_speed_mps,
        n_vel - n_noref - n_unc - n_slow
    );
    let _ = writeln!(o, "  tracker pairs: {}", pairs.t_ms.len());
    let _ = writeln!(
        o,
        "  velocity sigma table (NACv 0..4, m/s per axis): {:?}; velocity gate chi2: {}",
        acfg.vel_sigma_mps, cfg.vel_gate_chi2
    );
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "1. Reference velocity: the Rauch-Tung-Striebel smoother's velocity state (nearly-constant-acceleration model, position-only measurements weighted by NACp) over set-B positions, interpolated between the bracketing reports (oa-truth); set-B reported velocity is not used. A smoothed reference can favour smoothed estimates."
    );
    let _ = writeln!(
        o,
        "   Flight condition: not exposed by the reference; computed by finite differences of reference velocity over +-5 s (turn rate = change of track / 10 s, accel = change of ground speed / 10 s). steady: turn < {STEADY_TURN_DPS} deg/s and |accel| < {STEADY_ACCEL} m/s^2; turning: turn >= {TURNING_DPS} deg/s; accel: |accel| >= {ACCEL_MPS2} m/s^2 (turning takes precedence); other: the rest; no-ref: reference missing at +-5 s or speed below the heading gate. Row cond ALL covers every sample."
    );
    let _ = writeln!(
        o,
        "   Errors are measured minus reference. Ground speed in m/s; track angle in degrees, only where the reference speed is at least {:.2} m/s (30 kt).",
        rcfg.pair.min_heading_mps
    );
    let _ = writeln!(o);
    let series = [
        "RAW ADS-B",
        "OURS (1 Hz ticks)",
        "RAW (matched)",
        "OURS (matched)",
    ];
    let _ = writeln!(o, "2-5. RAW = every set-A measurement with a velocity, at its own time, by NACv. OURS (1 Hz ticks) = benchmark pairs of the tracker output. (matched) = raw measurements that have a tracker tick for the same aircraft within 0.5 s, raw and tracker error on the same samples.");
    let _ = writeln!(o);
    table(&mut o, "Ground-speed error", "m/s", &a.speed, &series);
    table(&mut o, "Track-angle error", "deg", &a.track, &series);
    let _ = writeln!(
        o,
        "6. Raw velocity component errors by NACv (m/s, measured minus reference; sigma = p95|e| / 1.96, the robust per-axis sigma; the reference carries its own error, so this is conservative)"
    );
    let _ = writeln!(
        o,
        "  {:<14} {:<6} {:<7} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "group", "axis", "cond", "n", "med|e|", "p95|e|", "rms", "sigma"
    );
    for ((g, axis, steady), v) in &comps {
        let st = stats_of(v);
        let _ = writeln!(
            o,
            "  {g:<14} {axis:<6} {:<7} {:>9} {:>9.4} {:>9.4} {:>9.4} {:>9.4}",
            if *steady { "steady" } else { "ALL" },
            st.n,
            st.median,
            st.p95,
            st.rms,
            st.p95 / 1.96
        );
    }
    let _ = writeln!(o);
    let ps = stats_of(&pos_err);
    let _ = writeln!(
        o,
        "7. Tracker horizontal position error against the reference (m, hypot of cross and along): n {} median {:.3} p95 {:.3} rms {:.3}",
        ps.n, ps.median, ps.p95, ps.rms
    );
    let _ = writeln!(
        o,
        "8. velocity_gated {velocity_gated} jump_rejected {jump_rejected} resyncs {resyncs} (gate manoeuvre allowance {} m/s^2, consistent resync k {})",
        cfg.gate_manoeuvre_mps2, cfg.resync_consistent_k
    );
    let _ = writeln!(
        o,
        "8b. tick rows emitted {tick_rows} rows_uncertain {rows_uncertain} nan_covariance {nan_covariance} (scored without coasting-and-uncertain rows: {})",
        opts.drop_withheld
    );
    o.push_str(&unc.report(extent));
    o.push_str(&brk);
    o.push_str(&brk2);
    fs::write(out_dir.join("velocity_diag.txt"), &o).map_err(|e| e.to_string())?;
    Ok(o)
}
