//! OpenATCA's tracker against ERAM's published track, on the same aircraft at the same times.
//!
//! ```text
//! cargo run --release -p oa-track --example vs_eram -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--turn-onset]
//! ```
//!
//! The ADS-B clip is split into two disjoint halves (`oa_track::score::SPLIT_TEXT`). Set A feeds
//! the tracker; set B alone builds the reference trajectories. ERAM's published track (SWIM SFDPS)
//! and the tracker's one-second output are both paired with that one reference by the benchmark
//! code (`oa-bench`), so association, pairing and error definitions are shared.
//!
//! Comparison: at each ERAM sample time, the tracker's sample nearest in time (within 0.5 s) for
//! the same truth aircraft. Each system's primary flight per aircraft is used (most paired
//! samples), never the one with the smaller error. Reported three ways:
//! - horizontal error as published;
//! - horizontal error with each system's mean along-track error per aircraft removed (removes a
//!   constant timestamp offset, which ERAM's published track has);
//! - ground speed error.
//!
//! Inputs differ: ERAM fuses FAA radar and ADS-B; the tracker sees only set A of a community
//! ADS-B feed. The reference comes from the same feed as the tracker's input (disjoint reports,
//! correlated GNSS error), which favours the tracker by an amount this run does not measure.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use oa_adsb::ZAU_BOX;
use oa_bench::adsb_truth;
use oa_bench::assoc::associate;
use oa_bench::chunked::{benchmark_chunk, chunk_add, chunk_finish, truth_index, ChunkAcc};
use oa_bench::common_time::{common_time_compare, CommonTimeConfig};
use oa_bench::input::{file_opener, load_adsb_counted, load_counted, msg_limit, FeedInput};
use oa_bench::pair::{build_pairs_with, PairTable};
use oa_bench::run::RunConfig;
use oa_bench::stats::{stats_of, Stats};
use oa_bench::thindex::build_th_index;
use oa_bench::tracker_input::{legacy_from_track_samples_by_leg, TrackSamples};
use oa_bench::truth::{empty_env, Env, Truth};
use oa_replay::FeedKind;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{
    interval_stats, run_tracker_spilled, score_ranges, scoring_config, split_reports,
    IntervalStats, RowSet, RowSource, SCORE_CHUNK_ROWS, SPLIT_TEXT,
};
use oa_track::{TrackerConfig, DEFAULT_CONFIG};
use oa_truth::DEFAULT_CONFIG as TRUTH_DEFAULT;

const KT_MPS: f64 = 0.514_444_444_444;
const TOL_MS: i64 = 500;
const TURN_EXPORT_ROWS: usize = 250_000;

fn reference_course_deg(east: f64, north: f64) -> Option<f64> {
    let speed = east.hypot(north);
    (east.is_finite() && north.is_finite() && speed > 0.1)
        .then(|| east.atan2(north).to_degrees().rem_euclid(360.0))
}

fn output_course_deg(east: f64, north: f64) -> Option<f64> {
    let speed = east.hypot(north);
    (east.is_finite() && north.is_finite() && speed > 0.1)
        .then(|| east.atan2(north).to_degrees().rem_euclid(360.0))
}

fn leg_key(addr: u32, leg: u32) -> String {
    format!("{addr:06x}-{leg}")
}

fn append_tracker_updates<R: RowSource>(
    out: &mut String,
    count: &mut usize,
    no_course: &mut u64,
    near_zero: &mut u64,
    no_truth: &mut u64,
    run: &R,
    truth: &Truth<'_>,
    system: &str,
) -> Result<(), String> {
    let rows = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    for range in score_ranges(&rows, SCORE_CHUNK_ROWS) {
        let r = run
            .rows_in(RowSet::Ticks, range.0, range.1)
            .map_err(|e| e.0)?;
        for i in 0..r.addr.len() {
            let (Some(&addr), Some(&time_ns), Some(&east), Some(&north)) = (
                r.addr.get(i),
                r.t_ns.get(i),
                r.vel_east_mps.get(i),
                r.vel_north_mps.get(i),
            ) else {
                continue;
            };
            let east = f64::from(east);
            let north = f64::from(north);
            let speed = east.hypot(north);
            if east.is_finite() && north.is_finite() && speed <= 0.1 {
                *near_zero = near_zero.saturating_add(1);
            }
            let Some(course) = output_course_deg(east, north) else {
                *no_course = no_course.saturating_add(1);
                continue;
            };
            let time_ms = time_ns.div_euclid(1_000_000);
            let Ok(reference) = (truth.query)(addr, time_ms) else {
                *no_truth = no_truth.saturating_add(1);
                continue;
            };
            *count = count.saturating_add(1);
            if *count > TURN_EXPORT_ROWS {
                return Err(format!(
                    "turn-onset tracker rows exceed bounded export limit {TURN_EXPORT_ROWS}"
                ));
            }
            let _ = writeln!(
                out,
                "{},{time_ms},{system},{course:.9},",
                leg_key(addr, reference.leg)
            );
        }
    }
    Ok(())
}

fn truth_csv(truth: &Truth<'_>) -> Result<String, String> {
    let mut out = String::from("leg,time_ms,truth_course_deg,truth_speed_kt\n");
    let mut count = 0usize;
    for (leg_ix, leg) in truth.legs.iter().enumerate() {
        let mut t = leg.start_ms.div_euclid(1000) * 1000;
        let end = leg.end_ms.div_euclid(1000) * 1000;
        while t <= end {
            count = count.saturating_add(1);
            if count > TURN_EXPORT_ROWS {
                return Err(format!(
                    "turn-onset truth rows exceed bounded export limit {TURN_EXPORT_ROWS}"
                ));
            }
            match (truth.query)(leg.addr, t) {
                Ok(sample) if sample.leg as usize == leg_ix => {
                    let speed = sample.vel_east_mps.hypot(sample.vel_north_mps) / KT_MPS;
                    if let Some(course) =
                        reference_course_deg(sample.vel_east_mps, sample.vel_north_mps)
                    {
                        let _ = writeln!(
                            out,
                            "{},{t},{course:.9},{speed:.6}",
                            leg_key(leg.addr, leg_ix as u32)
                        );
                    } else {
                        let _ =
                            writeln!(out, "{},{t},,{speed:.6}", leg_key(leg.addr, leg_ix as u32));
                    }
                }
                _ => {
                    let _ = writeln!(out, "{},{t},,", leg_key(leg.addr, leg_ix as u32));
                }
            }
            t = t.saturating_add(1000);
            if t <= end && t == i64::MAX {
                return Err("turn-onset truth time overflow".into());
            }
        }
    }
    Ok(out)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut sfdps, mut out) = (None, None, None);
    let mut turn_onset = false;
    while let Some(a) = args.next() {
        if a == "--turn-onset" {
            turn_onset = true;
            continue;
        }
        let v = args.next().map(PathBuf::from);
        match a.as_str() {
            "--adsb" => adsb = v,
            "--sfdps-dir" => sfdps = v,
            "--out" => out = v,
            _ => {
                eprintln!("vs_eram: unknown option {a}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(adsb), Some(sfdps), Some(out)) = (adsb, sfdps, out) else {
        eprintln!("usage: vs_eram --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--turn-onset]");
        return ExitCode::from(2);
    };
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in [&adsb, &sfdps, &out] {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("vs_eram: {e}");
            return ExitCode::from(2);
        }
    }
    match run(&adsb, &sfdps, &out, turn_onset) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("vs_eram: {e}");
            ExitCode::from(1)
        }
    }
}

/// One system's samples per truth aircraft: (time ms, horizontal m, horizontal with the
/// aircraft's mean along-track error removed m, speed error m/s), sorted by time.
type Sample = (i64, f64, f64, f64, u8);
type Samples = BTreeMap<u32, Vec<Sample>>;

fn samples(p: &PairTable) -> Samples {
    // One row per pair, read with checked access. The chunked scorer leaves the flight column
    // empty: all rows of an address are then one flight.
    struct R {
        addr: u32,
        flight: u32,
        t: i64,
        cross: f64,
        along: f64,
        speed: f64,
        coast: u8,
    }
    let nan = f32::NAN;
    let rows: Vec<R> = (0..p.t_ms.len())
        .filter_map(|i| {
            Some(R {
                addr: *p.addr.get(i)?,
                flight: p.flight.get(i).copied().unwrap_or(0),
                t: *p.t_ms.get(i)?,
                cross: f64::from(p.cross_m.get(i).copied().unwrap_or(nan)),
                along: f64::from(p.along_m.get(i).copied().unwrap_or(nan)),
                speed: f64::from(p.speed_err_kt.get(i).copied().unwrap_or(nan)) * KT_MPS,
                coast: p.coast.get(i).copied().unwrap_or(0),
            })
        })
        .collect();
    // Primary flight per aircraft: the most finite position pairs, lowest flight on a tie.
    let mut count: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for r in rows
        .iter()
        .filter(|r| r.cross.is_finite() && r.along.is_finite())
    {
        *count.entry((r.addr, r.flight)).or_default() += 1;
    }
    let mut primary: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    for (&(addr, flight), &n) in &count {
        let e = primary.entry(addr).or_insert((flight, n));
        if n > e.1 {
            *e = (flight, n);
        }
    }
    let is_primary = |r: &R| primary.get(&r.addr).map(|e| e.0) == Some(r.flight);
    let mut along_sum: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    for r in rows.iter().filter(|r| is_primary(r) && r.along.is_finite()) {
        let e = along_sum.entry(r.addr).or_insert((0.0, 0.0));
        e.0 += r.along;
        e.1 += 1.0;
    }
    let mut out: Samples = BTreeMap::new();
    for r in rows
        .iter()
        .filter(|r| is_primary(r) && r.cross.is_finite() && r.along.is_finite())
    {
        let bias = along_sum.get(&r.addr).map_or(0.0, |s| s.0 / s.1);
        out.entry(r.addr).or_default().push((
            r.t,
            r.cross.hypot(r.along),
            r.cross.hypot(r.along - bias),
            r.speed,
            r.coast,
        ));
    }
    for v in out.values_mut() {
        v.sort_by_key(|s| s.0);
        v.dedup_by_key(|s| s.0);
    }
    out
}

fn nearest(v: &[Sample], t: i64) -> Option<&Sample> {
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
    ((pick.0 - t).abs() <= TOL_MS).then_some(pick)
}

struct Side {
    raw: Vec<f64>,
    debiased: Vec<f64>,
    speed: Vec<f64>,
    per_flight_raw: Vec<f64>,
    per_flight_debiased: Vec<f64>,
}

fn side() -> Side {
    Side {
        raw: Vec::new(),
        debiased: Vec::new(),
        speed: Vec::new(),
        per_flight_raw: Vec::new(),
        per_flight_debiased: Vec::new(),
    }
}

fn rms(v: &[f64]) -> f64 {
    stats_of(v).rms
}

fn line(o: &mut String, label: &str, eram: &Stats, ours: &Stats) {
    let ratio = if ours.rms > 0.0 {
        eram.rms / ours.rms
    } else {
        f64::NAN
    };
    let _ = writeln!(
        o,
        "  {label:<44} ERAM rms {:>8.2} med {:>7.2} p95 {:>8.2} | OpenATCA rms {:>7.2} med {:>6.2} p95 {:>7.2} | ERAM/OpenATCA rms {:>5.2}x  (n={})",
        eram.rms, eram.median, eram.p95, ours.rms, ours.median, ours.p95, ratio, eram.n
    );
}

fn run(clip_path: &Path, sfdps: &Path, out_dir: &Path, turn_onset: bool) -> Result<String, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut o = String::new();
    let t_start = Instant::now();

    // Tracker on set A.
    // The clip's count and the SFDPS loader's, each input once, printed when this function
    // returns, an error included.
    let mut drops = oa_sealed::DropReport::new();
    let mut clip_sealed = 0u64;
    let clip = load_adsb_counted(clip_path, &file_opener(), &mut clip_sealed);
    drops.add(clip_sealed);
    let clip = clip.map_err(|e| e.to_string())?;
    let clip_hash = clip.hash.sha256.clone();
    let (set_a, set_b, split) = split_reports(&clip.reports);
    oa_bench::split_check::check_split(&set_a, &set_b)
        .map_err(|v| format!("split refused: {v:?}"))?;
    drop(clip);
    let (meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    let iv = interval_stats(&meas).unwrap_or(IntervalStats {
        n: 0,
        p50_ns: 1_000_000_000,
        p90_ns: 1_000_000_000,
        p99_ns: 1_000_000_000,
    });
    let cfg = scoring_config(&DEFAULT_CONFIG, &iv);
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let t_track = Instant::now();
    let run = run_tracker_spilled(&meas, &cfg, &spill).map_err(|e| e.to_string())?;
    // The existing single straight-model baseline is a separate linear CV Kalman instance:
    // h_mode 1 disables mixing and the coordinated-turn model; its fixed process density is
    // CV_ONLY_Q[0]. The measurement vector and scoring config are exactly the OA run's.
    let track_secs = t_track.elapsed().as_secs_f64();
    let cv_cfg = TrackerConfig { h_mode: 1, ..cfg };
    let cv_spill =
        turn_onset.then(|| out_dir.join(format!("turn-onset-cv-spill-{}", std::process::id())));
    let cv_run = if let Some(cv_spill) = cv_spill.as_ref() {
        let t_cv = Instant::now();
        let result = run_tracker_spilled(&meas, &cv_cfg, cv_spill).map_err(|e| e.to_string())?;
        eprintln!(
            "turn-onset CV baseline done in {:.1} s",
            t_cv.elapsed().as_secs_f64()
        );
        Some(result)
    } else {
        None
    };
    let traffic_secs = match (meas.first(), meas.last()) {
        (Some(a), Some(b)) => (b.toa_ns - a.toa_ns) as f64 / 1e9,
        _ => 0.0,
    };
    let n_meas = meas.len();
    let uncertain = run.stats.rows_uncertain;
    drop(meas);
    eprintln!("tracker done in {track_secs:.1} s");

    // Reference from set B only.
    let tcfg = TRUTH_DEFAULT;
    let built = adsb_truth::build(&set_b, &tcfg).map_err(|e| format!("truth: {e}"))?;
    drop(set_b);
    let q = |a: u32, t: i64| adsb_truth::query(&built.truth, a, t);
    let truth = Truth {
        query: &q,
        legs: &built.legs,
    };
    let turn_truth = if turn_onset {
        Some(truth_csv(&truth)?)
    } else {
        None
    };
    let mut turn_estimates =
        turn_onset.then(|| String::from("leg,state_time_ms,system,course_deg,availability_ms\n"));
    let mut turn_exported = 0usize;
    let (
        mut oa_no_course,
        mut oa_near_zero,
        mut oa_no_truth,
        mut cv_no_course,
        mut cv_near_zero,
        mut cv_no_truth,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let in_zone = |la: f64, lo: f64| {
        la >= f64::from(ZAU_BOX.lat_min_e7) * 1e-7
            && la <= f64::from(ZAU_BOX.lat_max_e7) * 1e-7
            && lo >= f64::from(ZAU_BOX.lon_min_e7) * 1e-7
            && lo <= f64::from(ZAU_BOX.lon_max_e7) * 1e-7
    };
    let mut env: Env<'_> = empty_env();
    env.in_zone = &in_zone;
    let mut rcfg = RunConfig::default();
    rcfg.pair.max_sigma_m = tcfg.max_sigma95_h_m;
    let tix = truth_index(&truth);
    eprintln!("reference built");

    // OpenATCA pairs: the one-second output, scored exactly as score_source does.
    let counts = run.row_counts(RowSet::Ticks).map_err(|e| e.0)?;
    let mut acc = ChunkAcc::default();
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
        let c = benchmark_chunk(&legacy, &truth, &tix, &env, &rcfg, range);
        chunk_add(&mut acc, c).map_err(|e| e.0)?;
    }
    let (_, ours_pairs) = chunk_finish(acc, &rcfg).map_err(|e| e.0)?;
    if turn_onset {
        let estimates = turn_estimates
            .as_mut()
            .ok_or_else(|| "turn-onset export buffer missing".to_owned())?;
        append_tracker_updates(
            estimates,
            &mut turn_exported,
            &mut oa_no_course,
            &mut oa_near_zero,
            &mut oa_no_truth,
            &run,
            &truth,
            "oa",
        )?;
        let cv_rows = cv_run
            .as_ref()
            .ok_or_else(|| "turn-onset CV run missing".to_owned())?;
        append_tracker_updates(
            estimates,
            &mut turn_exported,
            &mut cv_no_course,
            &mut cv_near_zero,
            &mut cv_no_truth,
            cv_rows,
            &truth,
            "cv",
        )?;
    }
    drop(run);
    drop(cv_run);
    let _ = fs::remove_dir_all(&spill);
    if let Some(cv_spill) = cv_spill {
        let _ = fs::remove_dir_all(cv_spill);
    }
    eprintln!("OpenATCA paired: {}", ours_pairs.t_ms.len());

    // ERAM pairs: SFDPS published track against the same reference.
    // Every file of the directory is checked against the sealed list as it is opened.
    let open: oa_bench::input::Opener = std::rc::Rc::new(|p: &Path| {
        oa_sealed::refuse_io(p)?;
        oa_bench::input::file_opener()(p)
    });
    let mut sfdps_sealed = 0u64;
    let loaded = load_counted(
        &[FeedInput {
            feed: FeedKind::Sfdps,
            dir: sfdps.to_path_buf(),
        }],
        &open,
        msg_limit(),
        &mut sfdps_sealed,
    );
    drops.add(sfdps_sealed);
    let loaded = loaded.map_err(|e| e.to_string())?;
    let leg = &loaded.legacy;
    let ix = build_th_index(&leg.th, leg.flights.gufi.len());
    let assoc = associate(leg, &ix, &truth, &tix, &rcfg.assoc);
    let (eram_pairs, _) = build_pairs_with(leg, &ix, &assoc, &truth, &rcfg.pair, &|_, _, _| None);
    eprintln!("ERAM paired: {}", eram_pairs.t_ms.len());

    // Export the identity/leg-mapped SFDPS output table directly, independently of position
    // error-pair exclusions. Source state time and recorder availability remain separate.
    let mut sfdps_updates: BTreeMap<(u32, u32, i64), (Option<i64>, f64)> = BTreeMap::new();
    let mut sfdps_no_receive_time = 0u64;
    let mut sfdps_no_position_time = 0u64;
    let mut sfdps_receive_before_position = 0u64;
    let mut sfdps_candidate_rows = 0u64;
    let mut sfdps_unassociated_rows = 0u64;
    let mut sfdps_no_truth_rows = 0u64;
    let mut sfdps_no_velocity_rows = 0u64;
    let mut sfdps_near_zero_direction_rows = 0u64;
    let mut sfdps_duplicate_state_rows = 0u64;
    let mut sfdps_conflicting_state_rows = 0u64;
    if turn_onset {
        for j in 0..leg.th.pos_ms.len() {
            let (Some(&flight), Some(&pos_ms)) = (leg.th.flight.get(j), leg.th.pos_ms.get(j))
            else {
                continue;
            };
            let Some(&addr) = assoc.addr.get(flight as usize) else {
                continue;
            };
            if addr == oa_bench::legacy::NONE {
                sfdps_unassociated_rows = sfdps_unassociated_rows.saturating_add(1);
                continue;
            }
            if pos_ms == oa_bench::legacy::NO_TIME {
                sfdps_no_position_time = sfdps_no_position_time.saturating_add(1);
                continue;
            }
            let (Some(&recv_ms), Some(&vx), Some(&vy)) = (
                leg.th.recv_ms.get(j),
                leg.th.vx_dkt.get(j),
                leg.th.vy_dkt.get(j),
            ) else {
                continue;
            };
            if recv_ms == oa_bench::legacy::NO_TIME {
                sfdps_no_receive_time = sfdps_no_receive_time.saturating_add(1);
            }
            let availability_ms = if recv_ms == oa_bench::legacy::NO_TIME {
                None
            } else if recv_ms < pos_ms {
                sfdps_receive_before_position = sfdps_receive_before_position.saturating_add(1);
                None
            } else {
                Some(recv_ms)
            };
            if vx == oa_bench::legacy::NO_VEL || vy == oa_bench::legacy::NO_VEL {
                sfdps_no_velocity_rows = sfdps_no_velocity_rows.saturating_add(1);
                continue;
            }
            let east = f64::from(vx) * 0.1 * KT_MPS;
            let north = f64::from(vy) * 0.1 * KT_MPS;
            if east.is_finite() && north.is_finite() && east.hypot(north) <= 0.1 {
                sfdps_near_zero_direction_rows = sfdps_near_zero_direction_rows.saturating_add(1);
            }
            let Some(course) = output_course_deg(east, north) else {
                continue;
            };
            let Ok(reference) = (truth.query)(addr, pos_ms) else {
                sfdps_no_truth_rows = sfdps_no_truth_rows.saturating_add(1);
                continue;
            };
            sfdps_candidate_rows = sfdps_candidate_rows.saturating_add(1);
            let key = (addr, reference.leg, pos_ms);
            if let Some(entry) = sfdps_updates.get_mut(&key) {
                sfdps_duplicate_state_rows = sfdps_duplicate_state_rows.saturating_add(1);
                if ((entry.1 - course + 180.0).rem_euclid(360.0) - 180.0).abs() > 1e-9 {
                    sfdps_conflicting_state_rows = sfdps_conflicting_state_rows.saturating_add(1);
                    // Keep the state ambiguous; an arbitrary duplicate must not become a scored output.
                    entry.0 = None;
                    entry.1 = f64::NAN;
                } else if entry.1.is_finite() {
                    if let Some(received) = availability_ms {
                        if entry.0.is_none_or(|old| received < old) {
                            entry.0 = Some(received);
                        }
                    }
                }
            } else {
                sfdps_updates.insert(key, (availability_ms, course));
            }
            if sfdps_updates.len() > TURN_EXPORT_ROWS {
                return Err(format!(
                    "turn-onset SFDPS rows exceed bounded export limit {TURN_EXPORT_ROWS}"
                ));
            }
        }
    }
    let sfdps_deduplicated_updates = sfdps_updates.len();
    if turn_onset {
        for (&(addr, truth_leg, state_ms), &(availability_ms, course)) in &sfdps_updates {
            if !course.is_finite() {
                continue;
            }
            turn_exported = turn_exported.saturating_add(1);
            if turn_exported > TURN_EXPORT_ROWS {
                return Err(format!("turn-onset combined estimate rows exceed bounded export limit {TURN_EXPORT_ROWS}"));
            }
            let availability = availability_ms.map_or_else(String::new, |t| t.to_string());
            let estimates = turn_estimates
                .as_mut()
                .ok_or_else(|| "turn-onset export buffer missing".to_owned())?;
            let _ = writeln!(
                estimates,
                "{},{state_ms},sfdps,{course:.9},{availability}",
                leg_key(addr, truth_leg)
            );
        }
    }

    // Library common-time table (horizontal as published), 1 s grid.
    let c1 = common_time_compare(
        &eram_pairs,
        &ours_pairs,
        &CommonTimeConfig {
            grid_ms: 1000,
            tol_ms: TOL_MS,
        },
    );

    // At ERAM's sample times.
    let (es, os) = (samples(&eram_pairs), samples(&ours_pairs));
    let (mut e, mut ours) = (side(), side());
    let (mut aircraft, mut ours_better_raw, mut ours_better_deb) = (0u64, 0u64, 0u64);
    let mut eram_without_ours = 0u64;
    let mut csv = String::from("addr,t_ms,eram_h_m,ours_h_m,eram_coast,ours_coast,dt_ms\n");
    for (addr, ev) in &es {
        let Some(ov) = os.get(addr) else {
            eram_without_ours += ev.len() as u64;
            continue;
        };
        let (mut er, mut ed, mut or, mut od) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for s in ev {
            let Some(m) = nearest(ov, s.0) else {
                eram_without_ours += 1;
                continue;
            };
            let _ = writeln!(
                csv,
                "{addr:06x},{},{:.2},{:.2},{},{},{}",
                s.0,
                s.1,
                m.1,
                s.4,
                m.4,
                m.0 - s.0
            );
            er.push(s.1);
            ed.push(s.2);
            or.push(m.1);
            od.push(m.2);
            if s.3.is_finite() && m.3.is_finite() {
                e.speed.push(s.3);
                ours.speed.push(m.3);
            }
        }
        if er.is_empty() {
            continue;
        }
        aircraft += 1;
        let (a, b, c, d) = (rms(&er), rms(&or), rms(&ed), rms(&od));
        ours_better_raw += u64::from(b < a);
        ours_better_deb += u64::from(d < c);
        e.per_flight_raw.push(a);
        ours.per_flight_raw.push(b);
        e.per_flight_debiased.push(c);
        ours.per_flight_debiased.push(d);
        e.raw.extend(er);
        e.debiased.extend(ed);
        ours.raw.extend(or);
        ours.debiased.extend(od);
    }

    let _ = writeln!(
        o,
        "OpenATCA vs ERAM published track (SFDPS), same aircraft, same times"
    );
    let _ = writeln!(
        o,
        "  ADS-B clip: {} (digest {clip_hash})",
        clip_path.display()
    );
    let _ = writeln!(o, "  SFDPS: {}", sfdps.display());
    let _ = writeln!(o, "  split: {SPLIT_TEXT}");
    let _ = writeln!(
        o,
        "  set A {} reports -> {} tracker measurements; set B {} reports -> reference ({} legs)",
        split.set_a,
        n_meas,
        split.set_b,
        built.legs.len()
    );
    let _ = writeln!(
        o,
        "  paired samples: ERAM {}, OpenATCA {} (one-second output); OpenATCA rows flagged uncertain (95% radius over 926 m, scored): {uncertain}",
        eram_pairs.t_ms.len(),
        ours_pairs.t_ms.len()
    );
    let _ = writeln!(
        o,
        "  compared at ERAM sample times: {aircraft} aircraft, {} samples; ERAM samples with no OpenATCA sample within 0.5 s: {eram_without_ours}",
        e.raw.len()
    );
    let _ = writeln!(o);
    line(
        &mut o,
        "horizontal error, as published (m)",
        &stats_of(&e.raw),
        &stats_of(&ours.raw),
    );
    line(
        &mut o,
        "horizontal, per-aircraft along bias removed (m)",
        &stats_of(&e.debiased),
        &stats_of(&ours.debiased),
    );
    line(
        &mut o,
        "per-aircraft RMS, as published (m)",
        &stats_of(&e.per_flight_raw),
        &stats_of(&ours.per_flight_raw),
    );
    line(
        &mut o,
        "per-aircraft RMS, along bias removed (m)",
        &stats_of(&e.per_flight_debiased),
        &stats_of(&ours.per_flight_debiased),
    );
    line(
        &mut o,
        "ground speed error (m/s)",
        &stats_of(&e.speed),
        &stats_of(&ours.speed),
    );
    let _ = writeln!(
        o,
        "  aircraft where OpenATCA has the lower RMS: {ours_better_raw} of {aircraft} as published, {ours_better_deb} of {aircraft} with along bias removed"
    );
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Library common-time table (C1, 1 s grid, 0.5 s tolerance; A = ERAM, B = OpenATCA):"
    );
    for r in &c1.rows {
        let s = &r.stats;
        let _ = writeln!(
            o,
            "  {:<28} n={:<8} rms {:>8.2} median {:>7.2} p95 {:>8.2}",
            r.label, s.n, s.rms, s.median, s.p95
        );
    }
    for (k, v) in &c1.scalars {
        let _ = writeln!(o, "  {k}: {v}");
    }
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "Speed: tracker processed {n_meas} measurements covering {:.0} s of traffic in {track_secs:.1} s ({:.0}x real time, one core); total run {:.0} s",
        traffic_secs,
        if track_secs > 0.0 { traffic_secs / track_secs } else { f64::NAN },
        t_start.elapsed().as_secs_f64()
    );
    let _ = writeln!(
        o,
        "Inputs differ: ERAM fuses FAA radar and ADS-B; OpenATCA sees only set A of a community ADS-B feed. The reference shares that feed (disjoint reports, correlated GNSS error)."
    );
    if turn_onset {
        let mut turn_provenance = String::from(
        "metric=turn-onset-dev-v2\ntruth=smoothed-split-B-reference; labels may use future reference only\ncourse=ground_track_from_east_north_velocity\nprimary_time=source/state applicability time, an algorithmic proxy only; not observed detector or wall-clock latency\nOA/CV state_time=tracker tick time on measurement applicability clock; recording has no per-report receive clock and offline ticks are not emission timestamps\nSFDPS state_time=ThTable.pos_ms; availability_time=ThTable.recv_ms where valid; receive cadence reported separately\nNo result compares physical detector latency or claims ERAM internal sensor/update/processing latency\nSFDPS_dedup=per (address, truth leg, pos_ms); omit conflicting course states, retain identical course and earliest valid recv_ms\ncv=separate oa-track TrackerConfig h_mode=1 straight-only CV; no mixing/CT model; CV_ONLY_Q[0]=H_Q_CV=1.0 m^2/s^3\ncv_input=same split-A measurements, measurement timestamps and scoring_config as OA\n",
    );
        let config_text = format!("oa={cfg:?}\ncv={cv_cfg:?}\n");
        let config_hash = oa_sha256::to_hex(&oa_sha256::sha256(config_text.as_bytes()));
        let _ = writeln!(turn_provenance, "tracker_config_sha256={config_hash}");
        let _ = writeln!(turn_provenance, "tracker_config_debug={config_text:?}");
        let _ = writeln!(turn_provenance, "adsb_clip_sha256={clip_hash}");
        let truth_csv = turn_truth
            .as_ref()
            .ok_or_else(|| "turn-onset truth buffer missing".to_owned())?;
        let estimate_csv = turn_estimates
            .as_ref()
            .ok_or_else(|| "turn-onset estimate buffer missing".to_owned())?;
        let _ = writeln!(
            turn_provenance,
            "truth_rows={}",
            truth_csv.lines().count().saturating_sub(1)
        );
        let _ = writeln!(turn_provenance, "all_estimate_rows={turn_exported}");
        let _ = writeln!(turn_provenance, "oa_no_course_rows={oa_no_course}");
        let _ = writeln!(
            turn_provenance,
            "oa_near_zero_direction_rows={oa_near_zero}"
        );
        let _ = writeln!(turn_provenance, "oa_no_truth_rows={oa_no_truth}");
        let _ = writeln!(turn_provenance, "cv_no_course_rows={cv_no_course}");
        let _ = writeln!(
            turn_provenance,
            "cv_near_zero_direction_rows={cv_near_zero}"
        );
        let _ = writeln!(turn_provenance, "cv_no_truth_rows={cv_no_truth}");
        let _ = writeln!(
            turn_provenance,
            "sfdps_no_receive_time_rows={sfdps_no_receive_time}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_no_position_time_rows={sfdps_no_position_time}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_receive_before_position_rows={sfdps_receive_before_position}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_unassociated_rows={sfdps_unassociated_rows}"
        );
        let _ = writeln!(turn_provenance, "sfdps_no_truth_rows={sfdps_no_truth_rows}");
        let _ = writeln!(
            turn_provenance,
            "sfdps_no_velocity_rows={sfdps_no_velocity_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_near_zero_direction_rows={sfdps_near_zero_direction_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_velocity_and_truth_paired_rows={sfdps_candidate_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_deduplicated_updates={sfdps_deduplicated_updates}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_duplicate_state_rows={sfdps_duplicate_state_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_conflicting_state_rows={sfdps_conflicting_state_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_deduplicated_rows={}",
            sfdps_candidate_rows.saturating_sub(sfdps_deduplicated_updates as u64)
        );
        for input in &loaded.inputs {
            let _ = writeln!(
                turn_provenance,
                "input={} files={} bytes={} sha256={}",
                input.label, input.files, input.bytes, input.sha256
            );
        }
        for (i, name) in oa_bench::assoc::astatus::NAMES.iter().enumerate() {
            let _ = writeln!(
                turn_provenance,
                "sfdps_association_{}={}",
                name.replace(' ', "_"),
                assoc.counts.get(i).copied().unwrap_or(0)
            );
        }
        fs::write(out_dir.join("turn-onset-truth.csv"), truth_csv).map_err(|e| e.to_string())?;
        fs::write(out_dir.join("turn-onset-estimates.csv"), estimate_csv)
            .map_err(|e| e.to_string())?;
        fs::write(out_dir.join("turn-onset-provenance.txt"), &turn_provenance)
            .map_err(|e| e.to_string())?;
    }
    fs::write(out_dir.join("compared.csv"), &csv).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("vs_eram.txt"), &o).map_err(|e| e.to_string())?;
    Ok(o)
}
