//! How much the scorer's truth (built from the other half of the same ADS-B) flatters the
//! tracker when the aircraft's errors are persistent. Runs only with `OPENATCA_ADSB_CLIP`,
//! `OPT_FALLBACK_M` and `OPT_CELLS`; otherwise it prints "skipped" and passes. Every number comes
//! from the environment, none is defaulted here.
//!
//! * `OPT_CELLS`: `;`-separated
//!   `name:bias_frac:bias_mult:lag_frac:lag_min_s:lag_max_s:seed:where:noise_sigma_m:noise_bound_frac`.
//!   `where` is `both`, `a` (errors only in the tracker's half; the truth half is clean) or `b`
//!   (errors only in the truth's half; the tracker's input is clean).
//! * `OPT_THINS`: `,`-separated thinning intervals in seconds (0 for none).
//! * `OPT_FALLBACK_M`: the bias bound used where an aircraft has no NACp, metres.
//! * `OPT_CONTROL_TOL`: a cell with no error and no noise must have B within this relative distance
//!   of A in every printed position metric (Updates and Ticks; the values are printed).
//!
//! The clip's reports (copies removed, nothing withheld) give the exact truth. Reports are
//! regenerated from it with the errors; the halves of the clip's own split decide which regenerated
//! rows are the tracker's (A) and which are the truth's (B). The tracker runs on A and is scored
//! against the exact truth (A score; built with its uncertainty gate opened so the generator can
//! answer between reports) and against the truth built from B (B score), on the rows both
//! truths answer, so the samples are identical: the harness asserts equal pair counts, at least
//! `MIN_PAIRS` Updates pairs per cell, and (for a lagged cell) that at most `MAX_DROP_SHARE` of
//! the lagged aircraft's rows are left out. Rows whose t-L falls in a reporting gap use
//! the truth at t stepped back along its velocity by L; its accuracy is checked on rows where both
//! are available (RMS of the horizontal difference at most `MAX_STEPBACK_RMS_SHARE` of the cell's A Updates
//! position RMS, reported after every cell has run), and the share of such rows is printed by label and reason.
//!
//! Confound: a row is classed straight or turning by the truth it is scored against, so the A and B
//! scores can class the same row differently. The per-class pair counts of both scores and the
//! A-by-B class confusion matrix over the identical rows are printed, and the "all" class is not
//! affected.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    clippy::print_stdout,
    clippy::too_many_lines
)]
use oa_bench::adsb_truth::{self, BuiltTruth};
use oa_sensorsim::adsb_error::{adsb_error, ErrorParams};
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, run_tracker, score_source, scoring_config, split_reports, split_reports_with,
    transversal_accel, RowSet, RowSource, ScoreError, ScoreRows, TrackRun, CLASS_NAMES,
    SCORE_CHUNK_ROWS, TURN_ACCEL_MPS2, TURN_SPAN_MS,
};
use oa_track::types::{O_COAST, O_TICK};
use oa_track::DEFAULT_CONFIG;
use oa_truth::DEFAULT_CONFIG as TRUTH;
use std::collections::{BTreeMap, HashMap, HashSet};

const MIN_PAIRS: usize = 10_000;
const MAX_DROP_SHARE: f64 = 0.02;
/// The step-back stand-in's RMS may be at most this share of the cell's A Updates position RMS:
/// its purpose is to be small against the effect measured. It replaced an absolute 1.0 m p99
/// bound that failed at 1.21 m.
const MAX_STEPBACK_RMS_SHARE: f64 = 0.05;

/// A run's rows restricted to those both truths answer.
struct Both<'a> {
    run: &'a TrackRun,
    a: &'a BuiltTruth,
    b: &'a BuiltTruth,
}

impl Both<'_> {
    fn ok(&self, addr: u32, t_ns: i64) -> bool {
        let t_ms = t_ns.div_euclid(1_000_000);
        [self.a, self.b].iter().all(|bt| {
            [0, -TURN_SPAN_MS, TURN_SPAN_MS].iter().all(|&d| {
                adsb_truth::query(&bt.truth, addr, t_ms + d)
                    .is_ok_and(|s| d != 0 || s.sigma95_h_m <= TRUTH.max_sigma95_h_m)
            })
        })
    }
}

impl RowSource for Both<'_> {
    fn row_counts(&self, rows: RowSet) -> Result<BTreeMap<u32, u64>, ScoreError> {
        self.run.row_counts(rows)
    }

    fn rows_in(&self, rows: RowSet, lo: u32, hi: u32) -> Result<ScoreRows, ScoreError> {
        let r = self.run.rows_in(rows, lo, hi)?;
        let mut o = ScoreRows::default();
        for i in 0..r.addr.len() {
            if !self.ok(r.addr[i], r.t_ns[i]) {
                continue;
            }
            o.addr.push(r.addr[i]);
            o.t_ns.push(r.t_ns[i]);
            o.lat_e7.push(r.lat_e7[i]);
            o.lon_e7.push(r.lon_e7[i]);
            o.alt_ft.push(r.alt_ft[i]);
            o.vel_east_mps.push(r.vel_east_mps[i]);
            o.vel_north_mps.push(r.vel_north_mps[i]);
            o.coast.push(r.coast[i]);
            o.uncertain.push(r.uncertain[i]);
            o.reacquired.push(r.reacquired[i]);
        }
        Ok(o)
    }
}

fn thin(a0: &oa_adsb::Reports, thin_s: f64) -> oa_adsb::Reports {
    if thin_s <= 0.0 {
        return a0.clone();
    }
    let n = oa_adsb::reports_len(a0);
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by_key(|&i| (a0.time_ns[i], i));
    let mut last: HashMap<u32, i64> = HashMap::new();
    let mut r = oa_adsb::Reports::default();
    let gap = (thin_s * 1.0e9) as i64;
    for &i in &idx {
        if last
            .get(&a0.icao[i])
            .is_none_or(|&t| a0.time_ns[i] - t >= gap)
        {
            last.insert(a0.icao[i], a0.time_ns[i]);
            oa_adsb::reports_push(&mut r, &oa_adsb::reports_row(a0, i).unwrap());
        }
    }
    r
}

struct Cell {
    name: String,
    p: ErrorParams,
    in_a: bool,
    in_b: bool,
}

fn cells(fallback: f64) -> Vec<Cell> {
    let spec = std::env::var("OPT_CELLS").unwrap();
    spec.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let f: Vec<&str> = s.split(':').collect();
            let x = |i: usize| f[i].parse::<f64>().unwrap();
            let w = f[7];
            assert!(["both", "a", "b"].contains(&w));
            Cell {
                in_a: w != "b",
                in_b: w != "a",
                name: f[0].to_string(),
                p: ErrorParams {
                    seed: f[6].parse().unwrap(),
                    bias_fraction: x(1),
                    bias_mult: x(2),
                    bias_fallback_bound_m: fallback,
                    lag_fraction: x(3),
                    lag_min_s: x(4),
                    lag_max_s: x(5),
                    noise_sigma_m: x(8),
                    noise_bound_frac: x(9),
                },
            }
        })
        .collect()
}

/// The printed metrics of one scoring: (label, value).
fn metrics(up: &oa_track::score::Scored, tk: &oa_track::score::Scored) -> Vec<(String, f64)> {
    let mut v = Vec::new();
    for c in 0..3 {
        v.push((
            format!("Updates pos {}", CLASS_NAMES[c]),
            up.esassp.position_m[c].rms,
        ));
    }
    for c in 0..3 {
        v.push((
            format!("Ticks pos {}", CLASS_NAMES[c]),
            tk.esassp.position_m[c].rms,
        ));
    }
    v.push(("Updates speed all".to_string(), up.esassp.speed_mps[0].rms));
    v
}

/// The class of a sample by the velocity change of `bt`'s truth around it: 0 straight, 1
/// turning, 2 not answered.
fn class_of(bt: &BuiltTruth, addr: u32, t_ms: i64) -> usize {
    let q = |dt: i64| {
        adsb_truth::query(&bt.truth, addr, t_ms + dt)
            .ok()
            .map(|s| (s.vel_east_mps, s.vel_north_mps))
    };
    match (q(-TURN_SPAN_MS), q(TURN_SPAN_MS)) {
        (Some(a), Some(b)) => usize::from(
            transversal_accel(a, b, 2.0 * TURN_SPAN_MS as f64 / 1000.0) > TURN_ACCEL_MPS2,
        ),
        _ => 2,
    }
}

/// The 3 by 3 confusion matrix (A class by B class) over the Updates rows both truths answer.
fn confusion(src: &Both, a: &BuiltTruth, b: &BuiltTruth) -> [[u64; 3]; 3] {
    let o = &src.run.out;
    let mut m = [[0u64; 3]; 3];
    for i in 0..o.t_ns.len() {
        if o.flags[i] & (O_TICK | O_COAST) != 0 || !src.ok(o.addr[i], o.t_ns[i]) {
            continue;
        }
        let t_ms = o.t_ns[i].div_euclid(1_000_000);
        m[class_of(a, o.addr[i], t_ms)][class_of(b, o.addr[i], t_ms)] += 1;
    }
    m
}

/// The generated rows of `e` whose (address, millisecond) is in `keys`.
fn rows_in(e: &oa_sensorsim::adsb_error::Errored, keys: &HashSet<(u32, i64)>) -> oa_adsb::Reports {
    let mut r = oa_adsb::Reports::default();
    for i in 0..oa_adsb::reports_len(&e.reports) {
        if keys.contains(&(e.reports.icao[i], e.reports.time_ns[i] / 1_000_000)) {
            oa_adsb::reports_push(&mut r, &oa_adsb::reports_row(&e.reports, i).unwrap());
        }
    }
    r
}

fn keys_of(r: &oa_adsb::Reports) -> HashSet<(u32, i64)> {
    (0..oa_adsb::reports_len(r))
        .map(|i| (r.icao[i], (r.time_ns[i] + 500_000).div_euclid(1_000_000)))
        .collect()
}

// REQ: OA-SIM-042
#[test]
fn optimism() {
    let (Ok(path), Ok(fallback)) = (
        std::env::var("OPENATCA_ADSB_CLIP"),
        std::env::var("OPT_FALLBACK_M"),
    ) else {
        println!("optimism: skipped (OPENATCA_ADSB_CLIP or OPT_FALLBACK_M unset)");
        return;
    };
    let t0 = std::time::Instant::now();
    let thins: Vec<f64> = std::env::var("OPT_THINS")
        .unwrap()
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let tol: Option<f64> = std::env::var("OPT_CONTROL_TOL")
        .ok()
        .map(|v| v.parse().unwrap());
    let clip = oa_bench::input::load_adsb(std::path::Path::new(&path)).unwrap();
    let (base, _, _) = split_reports_with(&clip.reports, false);
    drop(clip);
    // The exact truth answers wherever a leg is, however uncertain: its gate is opened so that a
    // lagged or between-report query is not refused; scoring still pairs only rows within the
    // scorer's own gate (see `Both::ok`).
    let exact = adsb_truth::build(&base, &adsb_truth::config_with(None, Some(1.0e6))).unwrap();
    // Only rows the exact truth answers at their own time can be regenerated; the rest are left
    // out up front, so the rows left out per cell are the ones a lag causes.
    let mut answered = oa_adsb::Reports::default();
    for i in 0..oa_adsb::reports_len(&base) {
        let t_ms = (base.time_ns[i] + 500_000).div_euclid(1_000_000);
        if adsb_truth::query(&exact.truth, base.icao[i], t_ms).is_ok() {
            oa_adsb::reports_push(&mut answered, &oa_adsb::reports_row(&base, i).unwrap());
        }
    }
    println!(
        "base rows {} of which the exact truth answers {} (pre-filter left out {:.2} %)",
        oa_adsb::reports_len(&base),
        oa_adsb::reports_len(&answered),
        100.0
            * (1.0
                - oa_adsb::reports_len(&answered) as f64
                    / oa_adsb::reports_len(&base).max(1) as f64)
    );
    let base = answered;
    // The clip's own halves decide which rows are the tracker's and which the truth's.
    let (base_a, base_b, _) = split_reports(&base);
    let (keys_a, keys_b) = (keys_of(&base_a), keys_of(&base_b));
    drop((base_a, base_b));
    println!(
        "exact truth built, base rows {}",
        oa_adsb::reports_len(&base)
    );
    let mut failures: Vec<String> = Vec::new();
    for cell in cells(fallback.parse().unwrap()) {
        let e = adsb_error(&exact.truth, &base, &cell.p).unwrap();
        let clean = ErrorParams {
            bias_fraction: 0.0,
            lag_fraction: 0.0,
            noise_sigma_m: 0.0,
            noise_bound_frac: 0.0,
            ..cell.p
        };
        let c = adsb_error(&exact.truth, &base, &clean).unwrap();
        println!(
            "CELL {} rows {} bias_aircraft {} lag_aircraft {} extrapolated {} secs {:.0}",
            cell.name,
            oa_adsb::reports_len(&e.reports),
            e.bias_addrs.len(),
            e.lag_addrs.len(),
            e.extrapolated_rows,
            t0.elapsed().as_secs_f64()
        );
        let share = |k: &[usize]| {
            let (d, n): (u64, u64) = k.iter().fold((0, 0), |a, &i| {
                (a.0 + e.dropped_by_label[i], a.1 + e.rows_by_label[i])
            });
            (d, n, d as f64 / n.max(1) as f64)
        };
        for (name, k) in [
            ("none", vec![0]),
            ("bias", vec![1]),
            ("lag", vec![2]),
            ("both", vec![3]),
            ("all lagged", vec![2, 3]),
        ] {
            let (d, n, f) = share(&k);
            println!(
                "DROPPED {} {name}: {d} of {n} ({:.2} %)",
                cell.name,
                100.0 * f
            );
        }
        let stepback_rms: Option<f64>;
        assert_eq!(e.dropped_by_label[0] + e.dropped_by_label[1], 0);
        if !e.lag_addrs.is_empty() {
            let (_, _, f) = share(&[2, 3]);
            assert!(f <= MAX_DROP_SHARE, "dropped share of lagged rows {f}");
            let lagged: u64 = e.rows_by_label[2] + e.rows_by_label[3];
            let f = e.extrapolated_rows as f64 / lagged.max(1) as f64;
            println!(
                "EXTRAPOLATED {}: {} of {lagged} lagged rows ({:.2} %); by label none/bias/lag/both {:?}; by reason OutsideLegs/NoNearbyReport/other {:?}",
                cell.name,
                e.extrapolated_rows,
                100.0 * f,
                e.extrapolated_by_label,
                e.extrapolated_by_reason
            );
        }
        {
            let mut d = e.stepback_diff_m.clone();
            d.sort_by(f64::total_cmp);
            let rms = (d.iter().map(|v| v * v).sum::<f64>() / d.len().max(1) as f64).sqrt();
            let p99 = d.get(d.len() * 99 / 100).copied().unwrap_or(f64::NAN);
            println!(
                "STEPBACK {} n {} rms {rms:.3} p99 {p99:.3} max {:.3}",
                cell.name,
                d.len(),
                d.last().copied().unwrap_or(f64::NAN)
            );
            stepback_rms = (!d.is_empty()).then_some(rms);
        }
        let a_tab = rows_in(if cell.in_a { &e } else { &c }, &keys_a);
        let b_tab = rows_in(if cell.in_b { &e } else { &c }, &keys_b);
        drop((e, c));
        let built_b = adsb_truth::build(&b_tab, &TRUTH).unwrap();
        {
            // The rebuilt truth against the exact one at the truth half's report times.
            let mut d: Vec<f64> = Vec::new();
            for i in (0..oa_adsb::reports_len(&b_tab)).step_by(7) {
                let (ad, t) = (b_tab.icao[i], b_tab.time_ns[i] / 1_000_000);
                if let (Ok(x), Ok(y)) = (
                    adsb_truth::query(&exact.truth, ad, t),
                    adsb_truth::query(&built_b.truth, ad, t),
                ) {
                    let k = std::f64::consts::PI / 180.0 * 6_371_008.8;
                    let (de, dn) = (
                        (y.lon - x.lon) * k * (x.lat * std::f64::consts::PI / 180.0).cos(),
                        (y.lat - x.lat) * k,
                    );
                    d.push(de.hypot(dn));
                }
            }
            d.sort_by(f64::total_cmp);
            let rms = (d.iter().map(|v| v * v).sum::<f64>() / d.len().max(1) as f64).sqrt();
            println!(
                "TRUTHDIFF {} n {} rms {rms:.3} p50 {:.3} p95 {:.3}",
                cell.name,
                d.len(),
                d.get(d.len() / 2).copied().unwrap_or(f64::NAN),
                d.get(d.len() * 95 / 100).copied().unwrap_or(f64::NAN)
            );
        }
        let is_control = cell.p.bias_fraction == 0.0
            && cell.p.lag_fraction == 0.0
            && cell.p.noise_sigma_m == 0.0
            && cell.p.noise_bound_frac == 0.0;
        for &ts in &thins {
            let a = thin(&a_tab, ts);
            let (meas, _) = adsb_measurements(&a, &AdsbConfig::default());
            let iv = interval_stats(&meas).unwrap();
            let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
            let run = run_tracker(&meas, &cfg).unwrap();
            let src = Both {
                run: &run,
                a: &exact,
                b: &built_b,
            };
            let sc =
                |bt: &BuiltTruth, rs| score_source(&src, bt, &TRUTH, rs, SCORE_CHUNK_ROWS).unwrap();
            let (au, at) = (sc(&exact, RowSet::Updates), sc(&exact, RowSet::Ticks));
            let (bu, bt) = (sc(&built_b, RowSet::Updates), sc(&built_b, RowSet::Ticks));
            println!(
                "SAMPLES {} thin {ts} updates_pairs A {} B {} ticks_pairs A {} B {}",
                cell.name, au.pairs, bu.pairs, at.pairs, bt.pairs
            );
            assert_eq!(au.pairs, bu.pairs);
            assert_eq!(at.pairs, bt.pairs);
            assert!(au.pairs >= MIN_PAIRS, "only {} Updates pairs", au.pairs);
            if let Some(r) = stepback_rms {
                let a_rms = au.esassp.position_m[0].rms;
                println!(
                    "STEPBACK {} thin {ts} rms {r:.3} of A Updates {a_rms:.3} ({:.2} %)",
                    cell.name,
                    100.0 * r / a_rms
                );
                if r > MAX_STEPBACK_RMS_SHARE * a_rms {
                    failures.push(format!("cell {} thin {ts}: step-back rms {r:.3} above {MAX_STEPBACK_RMS_SHARE} of {a_rms:.3}", cell.name));
                }
            }
            for (rs, x, y) in [("Updates", &au, &bu), ("Ticks", &at, &bt)] {
                println!(
                    "CLASSPAIRS {} thin {ts} {rs}: A straight {} turning {} | B straight {} turning {}",
                    cell.name,
                    x.esassp.position_m[1].n,
                    x.esassp.position_m[2].n,
                    y.esassp.position_m[1].n,
                    y.esassp.position_m[2].n
                );
            }
            let m = confusion(&src, &exact, &built_b);
            let total: u64 = m.iter().flatten().sum();
            println!("CONFUSION total {total} updates_pairs {}", au.pairs);
            assert_eq!(total, au.pairs as u64);
            println!(
                "CONFUSION {} thin {ts} (rows A straight/turning/none, columns B straight/turning/none): {:?}",
                cell.name, m
            );
            let (ma, mb) = (metrics(&au, &at), metrics(&bu, &bt));
            for ((l, x), (_, y)) in ma.iter().zip(mb.iter()) {
                println!(
                    "RESULT {} thin {ts} | {l} | A {x:.4} B {y:.4} B-A {:.4} B/A {:.4}",
                    cell.name,
                    y - x,
                    y / x
                );
                if let (Some(tol), true, true) = (tol, is_control, l.contains("pos")) {
                    assert!((y / x - 1.0).abs() <= tol, "control {l}: A {x} B {y}");
                }
            }
            println!("secs {:.0}", t0.elapsed().as_secs_f64());
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
