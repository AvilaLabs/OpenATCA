//! How much the scorer's truth (built from the other half of the same ADS-B) flatters the
//! tracker when the aircraft's errors are persistent. Runs only with `OPENATCA_ADSB_CLIP` and
//! `OPT_CELLS`; every number comes from the environment, none is defaulted here.
//!
//! * `OPT_CELLS`: `;`-separated `name:bias_frac:bias_mult:lag_frac:lag_min_s:lag_max_s:seed`.
//! * `OPT_THINS`: `,`-separated thinning intervals in seconds (0 for none).
//! * `OPT_FALLBACK_M`: the bias bound used where a row has no NACp, metres.
//! * `OPT_CONTROL_TOL`: when set, a cell whose fractions are all zero must have B within this
//!   relative distance of A in every printed Ticks position metric (the Updates position and
//!   speed metrics are printed, not asserted: they sit closer to the rebuilt truth's own
//!   reconstruction error, which the control measures).
//!
//! The clip's reports (copies removed, nothing withheld) give the exact truth. Reports are
//! regenerated from it with the errors, split as the scorer does, the tracker run on one half and
//! scored (A) against the exact truth and (B) against the truth built from the other half, on the
//! rows both truths answer.
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
    RowSet, RowSource, ScoreError, ScoreRows, TrackRun, CLASS_NAMES, SCORE_CHUNK_ROWS,
    TURN_SPAN_MS,
};
use oa_track::DEFAULT_CONFIG;
use oa_truth::DEFAULT_CONFIG as TRUTH;
use std::collections::{BTreeMap, HashMap};

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
}

fn cells(fallback: f64) -> Vec<Cell> {
    let spec = std::env::var("OPT_CELLS").unwrap();
    spec.split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let f: Vec<&str> = s.split(':').collect();
            let x = |i: usize| f[i].parse::<f64>().unwrap();
            Cell {
                name: f[0].to_string(),
                p: ErrorParams {
                    seed: f[6].parse().unwrap(),
                    bias_fraction: x(1),
                    bias_mult: x(2),
                    bias_fallback_bound_m: fallback,
                    lag_fraction: x(3),
                    lag_min_s: x(4),
                    lag_max_s: x(5),
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

// REQ: OA-SIM-042
#[test]
fn optimism() {
    let Ok(path) = std::env::var("OPENATCA_ADSB_CLIP") else {
        return;
    };
    let Ok(fallback) = std::env::var("OPT_FALLBACK_M") else {
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
    let exact = adsb_truth::build(&base, &TRUTH).unwrap();
    println!(
        "exact truth built, base rows {}",
        oa_adsb::reports_len(&base)
    );
    for cell in cells(fallback.parse().unwrap()) {
        let e = adsb_error(&exact.truth, &base, &cell.p).unwrap();
        println!(
            "CELL {} rows {} dropped {} bias_aircraft {} lag_aircraft {} secs {:.0}",
            cell.name,
            oa_adsb::reports_len(&e.reports),
            e.dropped_rows,
            e.bias_addrs.len(),
            e.lag_addrs.len(),
            t0.elapsed().as_secs_f64()
        );
        let (a0, b, _) = split_reports(&e.reports);
        let built_b = adsb_truth::build(&b, &TRUTH).unwrap();
        drop(b);
        {
            // The rebuilt truth against the exact one at the generated report times.
            let mut d: Vec<f64> = Vec::new();
            for i in (0..oa_adsb::reports_len(&e.reports)).step_by(7) {
                let (ad, t) = (e.reports.icao[i], e.reports.time_ns[i] / 1_000_000);
                if let (Ok(x), Ok(y)) = (
                    adsb_truth::query(&exact.truth, ad, t),
                    adsb_truth::query(&built_b.truth, ad, t),
                ) {
                    let k = std::f64::consts::PI / 180.0 * 6_371_008.8;
                    let (de, dn) = (
                        (y.lon - x.lon) * k * (x.lat * k / 6_371_008.8).cos(),
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
        for &ts in &thins {
            let a = thin(&a0, ts);
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
            let (ma, mb) = (metrics(&au, &at), metrics(&bu, &bt));
            for ((l, x), (_, y)) in ma.iter().zip(mb.iter()) {
                println!(
                    "RESULT {} thin {ts} | {l} | A {x:.4} B {y:.4} B-A {:.4} B/A {:.4}",
                    cell.name,
                    y - x,
                    y / x
                );
                if let (Some(tol), true, true) = (
                    tol,
                    cell.p.bias_fraction == 0.0 && cell.p.lag_fraction == 0.0,
                    l.starts_with("Ticks pos"),
                ) {
                    assert!((y / x - 1.0).abs() <= tol, "control {l}: A {x} B {y}");
                }
            }
            println!("secs {:.0}", t0.elapsed().as_secs_f64());
        }
    }
}
