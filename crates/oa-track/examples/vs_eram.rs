//! OpenATCA's tracker against ERAM's published track, on the same aircraft at the same times.
//!
//! ```text
//! cargo run --release -p oa-track --example vs_eram -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir>
//! cargo run --release -p oa-track --example vs_eram -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> \
//!     --turn-onset --turn-from-ms <utc-ms> --turn-to-ms <utc-ms>
//! cargo run --release -p oa-track --example vs_eram -- \
//!     --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> \
//!     --turn-onset --turn-onset-extended-budget \
//!     --turn-from-ms <utc-ms> --turn-to-ms <utc-ms>
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
const TURN_EXPORT_BYTES: usize = 32 * 1024 * 1024;
const TURN_EXPORT_EXTENDED_ROWS: usize = 1_000_000;
const TURN_EXPORT_EXTENDED_BYTES: usize = 128 * 1024 * 1024;
const TURN_EXPORT_MAX_ROW_BYTES: usize = 512;

// A7.2: cold command-level export configuration, copied once per run and never stored per row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TurnExportLimits {
    name: &'static str,
    max_rows: usize,
    max_bytes: usize,
}

const STANDARD_TURN_EXPORT: TurnExportLimits = TurnExportLimits {
    name: "standard",
    max_rows: TURN_EXPORT_ROWS,
    max_bytes: TURN_EXPORT_BYTES,
};
const EXTENDED_TURN_EXPORT: TurnExportLimits = TurnExportLimits {
    name: "extended",
    max_rows: TURN_EXPORT_EXTENDED_ROWS,
    max_bytes: TURN_EXPORT_EXTENDED_BYTES,
};

fn select_turn_export_limits(
    turn_onset: bool,
    extended: bool,
) -> Result<Option<TurnExportLimits>, String> {
    match (turn_onset, extended) {
        (true, false) => Ok(Some(STANDARD_TURN_EXPORT)),
        (true, true) => Ok(Some(EXTENDED_TURN_EXPORT)),
        (false, false) => Ok(None),
        (false, true) => Err("--turn-onset-extended-budget requires --turn-onset".into()),
    }
}

// A1/D5: fixed-layout, cold offline shell formatter adapter; this is not tracker state or a core DTO.
#[repr(C)]
struct BoundedCsvRow {
    bytes: [u8; TURN_EXPORT_MAX_ROW_BYTES],
    len: u16,
}
const _: () = assert!(core::mem::size_of::<BoundedCsvRow>() == 514);

impl std::fmt::Write for BoundedCsvRow {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let start = usize::from(self.len);
        let Some(next_len) = start.checked_add(value.len()) else {
            return Err(std::fmt::Error);
        };
        if next_len > TURN_EXPORT_MAX_ROW_BYTES {
            return Err(std::fmt::Error);
        }
        self.bytes
            .get_mut(start..next_len)
            .ok_or(std::fmt::Error)?
            .copy_from_slice(value.as_bytes());
        self.len = u16::try_from(next_len).map_err(|_| std::fmt::Error)?;
        Ok(())
    }
}

fn append_bounded_csv_row(
    output: &mut String,
    limits: TurnExportLimits,
    format_row: impl FnOnce(&mut BoundedCsvRow) -> std::fmt::Result,
) -> Result<(), String> {
    let mut row = BoundedCsvRow {
        bytes: [0; TURN_EXPORT_MAX_ROW_BYTES],
        len: 0,
    };
    format_row(&mut row).map_err(|_| {
        format!(
            "turn-onset formatted row exceeds bounded {}-byte row capacity",
            TURN_EXPORT_MAX_ROW_BYTES
        )
    })?;
    let bytes = row
        .bytes
        .get(..usize::from(row.len))
        .ok_or_else(|| "turn-onset formatted row length is invalid".to_owned())?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "turn-onset formatted row is not valid UTF-8".to_owned())?;
    let next_len = output
        .len()
        .checked_add(text.len())
        .ok_or_else(|| "turn-onset CSV byte count overflow".to_owned())?;
    if next_len > limits.max_bytes {
        return Err(format!(
            "turn-onset CSV exceeds bounded byte limit {}",
            limits.max_bytes
        ));
    }
    output.push_str(text);
    Ok(())
}

fn increment_export_count(count: &mut usize, limits: TurnExportLimits) -> Result<(), String> {
    let next = count
        .checked_add(1)
        .ok_or_else(|| "turn-onset export row count overflow".to_owned())?;
    if next > limits.max_rows {
        return Err(format!(
            "turn-onset rows exceed bounded export limit {}",
            limits.max_rows
        ));
    }
    *count = next;
    Ok(())
}

fn bounded_csv_header(header: &str, limits: TurnExportLimits) -> Result<String, String> {
    if header.len() > limits.max_bytes {
        return Err(format!(
            "turn-onset CSV header exceeds bounded byte limit {}",
            limits.max_bytes
        ));
    }
    Ok(String::from(header))
}

// A3: one fixed offline export counter record per system, outside tracker state.
#[repr(C)]
#[derive(Default)]
struct TurnExportCounters {
    no_course: u64,
    near_zero: u64,
    no_truth: u64,
    selected_window: u64,
    outside_window: u64,
}
const _: () = assert!(core::mem::size_of::<TurnExportCounters>() == 40);

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

fn in_turn_window(t_ms: i64, window: (i64, i64)) -> bool {
    window.0 <= t_ms && t_ms < window.1
}

fn checked_turn_window(from_ms: i64, to_ms: i64) -> Result<(i64, i64), String> {
    if from_ms >= to_ms || from_ms.rem_euclid(1000) != 0 || to_ms.rem_euclid(1000) != 0 {
        return Err("turn window must be increasing and aligned to whole UTC seconds".into());
    }
    let (Some(from_ns), Some(to_ns)) =
        (from_ms.checked_mul(1_000_000), to_ms.checked_mul(1_000_000))
    else {
        return Err("turn window endpoint overflows nanoseconds".into());
    };
    if oa_sealed::sealed_windows()
        .iter()
        .any(|w| from_ns < w.to_ns && w.from_ns < to_ns)
    {
        return Err("turn window intersects a sealed timestamp window".into());
    }
    Ok((from_ms, to_ms))
}

fn selected_truth_grid_bounds(
    start_ms: i64,
    end_ms: i64,
    window: (i64, i64),
) -> Result<Option<(i64, i64, u64, u64)>, String> {
    if start_ms > end_ms {
        return Ok(None);
    }
    let first = i128::from(start_ms).div_euclid(1000) * 1000;
    let last = i128::from(end_ms).div_euclid(1000) * 1000;
    if first > last {
        return Ok(None);
    }
    let full_count = (last - first).div_euclid(1000) + 1;
    let selected_first = first.max(i128::from(window.0));
    let selected_last = last.min(i128::from(window.1) - 1000);
    let selected_count = if selected_first <= selected_last {
        (selected_last - selected_first).div_euclid(1000) + 1
    } else {
        0
    };
    let outside_count = full_count - selected_count;
    let selected_bounds = if selected_count > 0 {
        Some((
            i64::try_from(selected_first)
                .map_err(|_| "truth grid start outside i64 milliseconds")?,
            i64::try_from(selected_last).map_err(|_| "truth grid end outside i64 milliseconds")?,
            u64::try_from(selected_count).map_err(|_| "truth selected row count overflow")?,
            u64::try_from(outside_count).map_err(|_| "truth outside row count overflow")?,
        ))
    } else {
        Some((
            0,
            -1,
            0,
            u64::try_from(outside_count).map_err(|_| "truth outside row count overflow")?,
        ))
    };
    Ok(selected_bounds)
}

// Keep the offline writer's output, counters, capacity, window and row source explicit.
#[allow(clippy::too_many_arguments)]
fn append_tracker_updates<R: RowSource>(
    out: &mut String,
    count: &mut usize,
    counters: &mut TurnExportCounters,
    limits: TurnExportLimits,
    window: (i64, i64),
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
            let (Some(&addr), Some(&time_ns)) = (r.addr.get(i), r.t_ns.get(i)) else {
                continue;
            };
            let time_ms = time_ns.div_euclid(1_000_000);
            if !in_turn_window(time_ms, window) {
                counters.outside_window = counters.outside_window.saturating_add(1);
                continue;
            }
            counters.selected_window = counters.selected_window.saturating_add(1);
            let (Some(&east), Some(&north)) = (r.vel_east_mps.get(i), r.vel_north_mps.get(i))
            else {
                continue;
            };
            let east = f64::from(east);
            let north = f64::from(north);
            let speed = east.hypot(north);
            if east.is_finite() && north.is_finite() && speed <= 0.1 {
                counters.near_zero = counters.near_zero.saturating_add(1);
            }
            let Some(course) = output_course_deg(east, north) else {
                counters.no_course = counters.no_course.saturating_add(1);
                continue;
            };
            let Ok(reference) = (truth.query)(addr, time_ms) else {
                counters.no_truth = counters.no_truth.saturating_add(1);
                continue;
            };
            increment_export_count(count, limits)?;
            append_bounded_csv_row(out, limits, |row| {
                writeln!(
                    row,
                    "{},{time_ms},{system},{course:.9},",
                    leg_key(addr, reference.leg)
                )
            })?;
        }
    }
    Ok(())
}

fn truth_csv(
    truth: &Truth<'_>,
    window: (i64, i64),
    limits: TurnExportLimits,
) -> Result<(String, u64, u64), String> {
    let mut out = bounded_csv_header("leg,time_ms,truth_course_deg,truth_speed_kt\n", limits)?;
    let mut selected_rows = 0u64;
    let mut outside_rows = 0u64;
    for leg in truth.legs {
        let Some((_, _, selected_count, outside_count)) =
            selected_truth_grid_bounds(leg.start_ms, leg.end_ms, window)?
        else {
            continue;
        };
        selected_rows = selected_rows
            .checked_add(selected_count)
            .ok_or_else(|| "truth selected row count overflow".to_owned())?;
        outside_rows = outside_rows
            .checked_add(outside_count)
            .ok_or_else(|| "truth outside row count overflow".to_owned())?;
    }
    if selected_rows > limits.max_rows as u64 {
        return Err(format!(
            "turn-onset truth rows exceed bounded export limit {}",
            limits.max_rows
        ));
    }
    for (leg_ix, leg) in truth.legs.iter().enumerate() {
        let Some((first, last, _, _)) =
            selected_truth_grid_bounds(leg.start_ms, leg.end_ms, window)?
        else {
            continue;
        };
        let mut t = first;
        while t <= last {
            let time_ms = t;
            match (truth.query)(leg.addr, time_ms) {
                Ok(sample) if sample.leg as usize == leg_ix => {
                    let speed = sample.vel_east_mps.hypot(sample.vel_north_mps) / KT_MPS;
                    if let Some(course) =
                        reference_course_deg(sample.vel_east_mps, sample.vel_north_mps)
                    {
                        append_bounded_csv_row(&mut out, limits, |row| {
                            writeln!(
                                row,
                                "{},{time_ms},{course:.9},{speed:.6}",
                                leg_key(leg.addr, leg_ix as u32)
                            )
                        })?;
                    } else {
                        append_bounded_csv_row(&mut out, limits, |row| {
                            writeln!(
                                row,
                                "{},{time_ms},,{speed:.6}",
                                leg_key(leg.addr, leg_ix as u32)
                            )
                        })?;
                    }
                }
                _ => {
                    append_bounded_csv_row(&mut out, limits, |row| {
                        writeln!(row, "{},{time_ms},,", leg_key(leg.addr, leg_ix as u32))
                    })?;
                }
            }
            t = t
                .checked_add(1000)
                .ok_or_else(|| "truth grid timestamp overflow".to_owned())?;
        }
    }
    Ok((out, selected_rows, outside_rows))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut sfdps, mut out) = (None, None, None);
    let mut turn_onset = false;
    let mut turn_onset_extended_budget = false;
    let mut turn_from_ms = None;
    let mut turn_to_ms = None;
    let mut turn_from_seen = false;
    let mut turn_to_seen = false;
    while let Some(a) = args.next() {
        if a == "--turn-onset" {
            turn_onset = true;
            continue;
        }
        if a == "--turn-onset-extended-budget" {
            turn_onset_extended_budget = true;
            continue;
        }
        if a == "--turn-from-ms" || a == "--turn-to-ms" {
            let Some(raw) = args.next() else {
                eprintln!("vs_eram: {a} requires an integer UTC millisecond value");
                return ExitCode::from(2);
            };
            let parsed = match raw.parse::<i64>() {
                Ok(v) => v,
                Err(_) => {
                    eprintln!(
                        "vs_eram: {a} must be an in-range signed integer UTC millisecond value"
                    );
                    return ExitCode::from(2);
                }
            };
            if a == "--turn-from-ms" {
                if turn_from_seen {
                    eprintln!("vs_eram: duplicate --turn-from-ms");
                    return ExitCode::from(2);
                }
                turn_from_seen = true;
                turn_from_ms = Some(parsed);
            } else {
                if turn_to_seen {
                    eprintln!("vs_eram: duplicate --turn-to-ms");
                    return ExitCode::from(2);
                }
                turn_to_seen = true;
                turn_to_ms = Some(parsed);
            }
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
        eprintln!("usage: vs_eram --adsb <clip.oaf> --sfdps-dir <dir> --out <dir> [--turn-onset [--turn-onset-extended-budget] --turn-from-ms <utc-ms> --turn-to-ms <utc-ms>]");
        return ExitCode::from(2);
    };
    let turn_window = match (turn_onset, turn_from_ms, turn_to_ms) {
        (true, Some(from), Some(to)) => match checked_turn_window(from, to) {
            Ok(window) => Some(window),
            Err(e) => {
                eprintln!("vs_eram: {e}");
                return ExitCode::from(2);
            }
        },
        (false, None, None) => None,
        _ => {
            eprintln!("vs_eram: --turn-onset requires both --turn-from-ms and --turn-to-ms; window flags are invalid without --turn-onset");
            return ExitCode::from(2);
        }
    };
    let turn_export_limits = match select_turn_export_limits(turn_onset, turn_onset_extended_budget)
    {
        Ok(limits) => limits,
        Err(e) => {
            eprintln!("vs_eram: {e}");
            return ExitCode::from(2);
        }
    };
    // Every path is checked, as given and resolved, before anything is opened or created.
    for p in [&adsb, &sfdps, &out] {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("vs_eram: {e}");
            return ExitCode::from(2);
        }
    }
    match run(&adsb, &sfdps, &out, turn_window, turn_export_limits) {
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

fn run(
    clip_path: &Path,
    sfdps: &Path,
    out_dir: &Path,
    turn_window: Option<(i64, i64)>,
    turn_export_limits: Option<TurnExportLimits>,
) -> Result<String, String> {
    let turn_onset = turn_window.is_some();
    let export_limits = turn_export_limits;
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
    let (turn_truth, truth_window_rows, truth_outside_rows) = if let Some(window) = turn_window {
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        let (csv, selected, outside) = truth_csv(&truth, window, limits)?;
        (Some(csv), selected, outside)
    } else {
        (None, 0, 0)
    };
    let mut turn_estimates = if turn_onset {
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        Some(bounded_csv_header(
            "leg,state_time_ms,system,course_deg,availability_ms\n",
            limits,
        )?)
    } else {
        None
    };
    let mut turn_exported = 0usize;
    let mut oa_counts = TurnExportCounters::default();
    let mut cv_counts = TurnExportCounters::default();
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
        let window = turn_window.ok_or_else(|| "turn-onset window missing".to_owned())?;
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        let estimates = turn_estimates
            .as_mut()
            .ok_or_else(|| "turn-onset export buffer missing".to_owned())?;
        append_tracker_updates(
            estimates,
            &mut turn_exported,
            &mut oa_counts,
            limits,
            window,
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
            &mut cv_counts,
            limits,
            window,
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
    let mut sfdps_window_selected_rows = 0u64;
    let mut sfdps_window_outside_rows = 0u64;
    let mut sfdps_duplicate_state_rows = 0u64;
    let mut sfdps_conflicting_state_rows = 0u64;
    if turn_onset {
        let window = turn_window.ok_or_else(|| "turn-onset window missing".to_owned())?;
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        for j in 0..leg.th.pos_ms.len() {
            let (Some(&flight), Some(&pos_ms)) = (leg.th.flight.get(j), leg.th.pos_ms.get(j))
            else {
                continue;
            };
            if pos_ms == oa_bench::legacy::NO_TIME {
                sfdps_no_position_time = sfdps_no_position_time.saturating_add(1);
                continue;
            }
            if !in_turn_window(pos_ms, window) {
                sfdps_window_outside_rows = sfdps_window_outside_rows.saturating_add(1);
                continue;
            }
            sfdps_window_selected_rows = sfdps_window_selected_rows.saturating_add(1);
            let Some(&addr) = assoc.addr.get(flight as usize) else {
                continue;
            };
            if addr == oa_bench::legacy::NONE {
                sfdps_unassociated_rows = sfdps_unassociated_rows.saturating_add(1);
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
            if !sfdps_updates.contains_key(&key) && sfdps_updates.len() >= limits.max_rows {
                return Err(format!(
                    "turn-onset SFDPS rows exceed bounded export limit {}",
                    limits.max_rows
                ));
            }
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
        }
    }
    let sfdps_deduplicated_updates = sfdps_updates.len();
    if turn_onset {
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        for (&(addr, truth_leg, state_ms), &(availability_ms, course)) in &sfdps_updates {
            if !course.is_finite() {
                continue;
            }
            increment_export_count(&mut turn_exported, limits)?;
            let availability = availability_ms.map_or_else(String::new, |t| t.to_string());
            let estimates = turn_estimates
                .as_mut()
                .ok_or_else(|| "turn-onset export buffer missing".to_owned())?;
            append_bounded_csv_row(estimates, limits, |row| {
                writeln!(
                    row,
                    "{},{state_ms},sfdps,{course:.9},{availability}",
                    leg_key(addr, truth_leg)
                )
            })?;
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
        "export=turn-onset-input-v1\nmetric_protocol=selected_by_downstream_scorer\ntruth=smoothed-split-B-reference; labels may use future reference only\ncourse=ground_track_from_east_north_velocity\nprimary_time=source/state applicability time, an algorithmic proxy only; not observed detector or wall-clock latency\nOA/CV state_time=tracker tick time on measurement applicability clock; recording has no per-report receive clock and offline ticks are not emission timestamps\nSFDPS state_time=ThTable.pos_ms; availability_time=ThTable.recv_ms where valid; receive cadence reported separately\nNo result compares physical detector latency or claims ERAM internal sensor/update/processing latency\nSFDPS_dedup=per (address, truth leg, pos_ms); omit conflicting course states, retain identical course and earliest valid recv_ms\ncv=separate oa-track TrackerConfig h_mode=1 straight-only CV; no mixing/CT model; CV_ONLY_Q[0]=H_Q_CV=1.0 m^2/s^3\ncv_input=same split-A measurements, measurement timestamps and scoring_config as OA\n",
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
        let (window_from_ms, window_to_ms) =
            turn_window.ok_or_else(|| "turn-onset window missing".to_owned())?;
        let limits = export_limits.ok_or_else(|| "turn-onset export limits missing".to_owned())?;
        let _ = writeln!(turn_provenance, "export_capacity_profile={}", limits.name);
        let _ = writeln!(
            turn_provenance,
            "export_max_rows_per_csv={}",
            limits.max_rows
        );
        let _ = writeln!(
            turn_provenance,
            "export_max_bytes_per_csv={}",
            limits.max_bytes
        );
        let _ = writeln!(turn_provenance, "truth_csv_bytes={}", truth_csv.len());
        let _ = writeln!(turn_provenance, "estimate_csv_bytes={}", estimate_csv.len());
        let _ = writeln!(
            turn_provenance,
            "turn_window_utc_ms_half_open={window_from_ms},{window_to_ms}"
        );
        let _ = writeln!(
            turn_provenance,
            "truth_window_selected_rows={truth_window_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "truth_window_outside_rows={truth_outside_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "oa_window_selected_rows={}",
            oa_counts.selected_window
        );
        let _ = writeln!(
            turn_provenance,
            "oa_window_outside_rows={}",
            oa_counts.outside_window
        );
        let _ = writeln!(
            turn_provenance,
            "cv_window_selected_rows={}",
            cv_counts.selected_window
        );
        let _ = writeln!(
            turn_provenance,
            "cv_window_outside_rows={}",
            cv_counts.outside_window
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_window_selected_rows={sfdps_window_selected_rows}"
        );
        let _ = writeln!(
            turn_provenance,
            "sfdps_window_outside_rows={sfdps_window_outside_rows}"
        );
        let _ = writeln!(turn_provenance, "all_estimate_rows={turn_exported}");
        let _ = writeln!(turn_provenance, "oa_no_course_rows={}", oa_counts.no_course);
        let _ = writeln!(
            turn_provenance,
            "oa_near_zero_direction_rows={}",
            oa_counts.near_zero
        );
        let _ = writeln!(turn_provenance, "oa_no_truth_rows={}", oa_counts.no_truth);
        let _ = writeln!(turn_provenance, "cv_no_course_rows={}", cv_counts.no_course);
        let _ = writeln!(
            turn_provenance,
            "cv_near_zero_direction_rows={}",
            cv_counts.near_zero
        );
        let _ = writeln!(turn_provenance, "cv_no_truth_rows={}", cv_counts.no_truth);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod turn_window_tests {
    use super::{
        append_bounded_csv_row, checked_turn_window, in_turn_window, increment_export_count,
        select_turn_export_limits, selected_truth_grid_bounds, TurnExportLimits,
        EXTENDED_TURN_EXPORT, STANDARD_TURN_EXPORT,
    };
    use std::fmt::Write as _;

    #[test]
    fn export_capacity_profiles_are_explicit_and_opt_in() {
        assert_eq!(
            select_turn_export_limits(true, false).unwrap(),
            Some(STANDARD_TURN_EXPORT)
        );
        assert_eq!(
            select_turn_export_limits(true, true).unwrap(),
            Some(EXTENDED_TURN_EXPORT)
        );
        assert_eq!(STANDARD_TURN_EXPORT.max_rows, 250_000);
        assert_eq!(STANDARD_TURN_EXPORT.max_bytes, 32 * 1024 * 1024);
        assert_eq!(EXTENDED_TURN_EXPORT.max_rows, 1_000_000);
        assert_eq!(EXTENDED_TURN_EXPORT.max_bytes, 128 * 1024 * 1024);
        assert_eq!(select_turn_export_limits(false, false).unwrap(), None);
        assert!(select_turn_export_limits(false, true).is_err());
    }

    #[test]
    fn byte_cap_refuses_before_appending_formatted_row() {
        let limits = TurnExportLimits {
            name: "test",
            max_rows: 3,
            max_bytes: 4,
        };
        let mut output = String::from("abc");
        let result = append_bounded_csv_row(&mut output, limits, |row| writeln!(row, "x"));
        assert!(result.is_err());
        assert_eq!(output, "abc");
    }

    #[test]
    fn oversized_formatted_row_refuses_before_appending() {
        let limits = TurnExportLimits {
            name: "test",
            max_rows: 3,
            max_bytes: 4096,
        };
        let mut output = String::from("header\n");
        let too_long = "x".repeat(TURN_EXPORT_MAX_ROW_BYTES + 1);
        let result = append_bounded_csv_row(&mut output, limits, |row| write!(row, "{too_long}"));
        assert!(result.is_err());
        assert_eq!(output, "header\n");
    }

    #[test]
    fn row_cap_refuses_without_incrementing_past_bound() {
        let limits = TurnExportLimits {
            name: "test",
            max_rows: 2,
            max_bytes: 128,
        };
        let mut count = 2;
        assert!(increment_export_count(&mut count, limits).is_err());
        assert_eq!(count, 2);
    }

    // REQ: OA-TRK-404
    #[test]
    fn half_open_window_and_checked_whole_seconds() {
        let window = checked_turn_window(1_000, 3_000).expect("synthetic unsealed interval");
        assert!(!in_turn_window(999, window));
        assert!(in_turn_window(1_000, window));
        assert!(in_turn_window(2_999, window));
        assert!(!in_turn_window(3_000, window));
        assert!(checked_turn_window(2_000, 2_000).is_err());
        assert!(checked_turn_window(1_001, 3_000).is_err());
        assert!(checked_turn_window(9_223_372_036_854_774_000, 9_223_372_036_854_775_000).is_err());
    }

    // REQ: OA-TRK-404
    #[test]
    fn selected_truth_grid_handles_extremes_and_inverted_legs_without_overflow() {
        let bounds = selected_truth_grid_bounds(i64::MIN, i64::MAX, (1_000, 3_000))
            .unwrap()
            .unwrap();
        assert_eq!((bounds.0, bounds.1, bounds.2), (1_000, 2_000, 2));
        assert_eq!(
            bounds.3,
            u64::try_from(
                ((i128::from(i64::MAX).div_euclid(1000) * 1000)
                    - (i128::from(i64::MIN).div_euclid(1000) * 1000))
                    .div_euclid(1000)
                    - 1
            )
            .unwrap()
        );
        assert_eq!(
            selected_truth_grid_bounds(4_000, 3_000, (1_000, 3_000)).unwrap(),
            None
        );
        assert_eq!(
            selected_truth_grid_bounds(1_999, 1_000, (1_000, 3_000)).unwrap(),
            None
        );
        assert_eq!(
            selected_truth_grid_bounds(1_000, 2_000, (1_000, 3_000)).unwrap(),
            Some((1_000, 2_000, 2, 0))
        );
        assert_eq!(
            selected_truth_grid_bounds(1_000, 2_000, (3_000, 4_000)).unwrap(),
            Some((0, -1, 0, 2))
        );
    }

    // REQ: OA-TRK-404
    #[test]
    fn sealed_middle_interval_is_refused_and_adjacent_boundaries_are_allowed() {
        let sealed = oa_sealed::sealed_windows()
            .first()
            .expect("compiled seal metadata");
        let middle_ns = (i128::from(sealed.from_ns) + i128::from(sealed.to_ns)) / 2;
        let middle_from = i64::try_from(middle_ns.div_euclid(1_000_000_000) * 1_000).unwrap();
        assert!(checked_turn_window(middle_from, middle_from + 1_000).is_err());
        let before_to =
            i64::try_from(i128::from(sealed.from_ns).div_euclid(1_000_000_000) * 1_000).unwrap();
        assert!(checked_turn_window(before_to - 1_000, before_to).is_ok());
        let after_from = i64::try_from(
            (i128::from(sealed.to_ns) + 999_999_999).div_euclid(1_000_000_000) * 1_000,
        )
        .unwrap();
        assert!(checked_turn_window(after_from, after_from + 1_000).is_ok());
    }
}
