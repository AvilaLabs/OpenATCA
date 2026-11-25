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
//! (`adsb_truth::build` on set B). `--vel-lag S` sets every velocity's lag to S seconds (0: before
//! OA-TRK-401); `--thin N` keeps every Nth report of each aircraft (with `--rescore`). With `--rescore` the OpenATCA errors are not read from the CSV
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
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use oa_bench::adsb_truth;
use oa_bench::assoc::associate;
use oa_bench::chunked::truth_index;
use oa_bench::input::{load_adsb_counted, Opener};
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
/// Report age beyond which a sample is "far" from a report, seconds (the gap diagnostic).
const FAR_S: f64 = 5.0;
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

/// The two development clips are 388,031,016 and 466,613,280 bytes. A 512 MiB ceiling admits
/// both without an unbounded loader. Valid records use at least 64 encoded bytes, so decoding
/// this capped stream can retain fewer than 8,388,608 reports before the count check below.
const MAX_ADSB_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// The larger development clip contains 5,775,231 reports. Eight million leaves 38% headroom
/// and bounds downstream scans/maps; retained Sample/Ref/Cand arrays have their separate cap.
/// Loading/splitting is below roughly 2 GiB of report columns at this bound. Keyed diagnostic
/// maps, tracker state, truth and tables also remain subject to the coordinator's 6 GiB guard.
const MAX_DIAG_MEASUREMENTS: usize = 8_000_000;
/// Byte bound for CSV scanning/allocation before parsing; row/column/field bounds below cap vectors.
const MAX_SAMPLES_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Caps parallel sample/ref/candidate arrays and each sample-driven output loop.
const MAX_SAMPLES: usize = 500_000;
/// Bounds header-map growth before constructing the column index.
const MAX_SAMPLE_COLUMNS: usize = 64;
/// Bounds each line split and field storage before allocating its field vector.
const MAX_SAMPLE_ROW_BYTES: usize = 4096;

/// A per-file byte cap enforced on the same verified descriptor used by the decoder.
// D5 shell input-adapter subsystem: its state is the live reader and remaining byte budget, not a record.
struct CappedInput {
    file: fs::File,
    left: u64,
    path: PathBuf,
}

impl Read for CappedInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            let mut probe = [0u8; 1];
            return match self.file.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} exceeds the {MAX_ADSB_FILE_BYTES} byte diagnostic limit",
                        self.path.display()
                    ),
                )),
            };
        }
        let n = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let part = buf.get_mut(..n).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "read buffer shorter than bounded request",
            )
        })?;
        let read = self.file.read(part)?;
        let read = u64::try_from(read)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "read count exceeds u64"))?;
        self.left = self.left.checked_sub(read).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "read exceeds byte budget")
        })?;
        usize::try_from(read)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "read count exceeds usize"))
    }
}

fn bounded_adsb_opener() -> Opener {
    Rc::new(|path| {
        let (file, len) = oa_sealed::open_regular(path)?;
        if len > MAX_ADSB_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} is {len} bytes; diagnostic limit is {MAX_ADSB_FILE_BYTES}",
                    path.display()
                ),
            ));
        }
        Ok(Box::new(CappedInput {
            file,
            left: MAX_ADSB_FILE_BYTES,
            path: path.to_path_buf(),
        }) as Box<dyn Read>)
    })
}

/// One common grid sample of p002.
// A3 exception: shell join row capped by MAX_SAMPLES; keyed AoS avoids parallel-map joins, and is
// not tracker update state. A9: f64 preserves units in this offline analysis.
#[repr(C)]
struct Sample {
    addr: u32,
    /// Time of OpenATCA's paired row, Unix ms (the tracker row's key).
    t_ms: i64,
    oa_cross: f64,
    oa_hdg: f64,
    er_cross: f64,
    er_hdg: f64,
    band: String,
    /// p002's flight phase (climb, cruise, descent).
    flight: String,
}
const _: () = assert!(core::mem::size_of::<Sample>() == 96);

/// What the tracker's row says at a sample.
// A3 exception: keyed shell join value capped by MAX_DIAG_MEASUREMENTS, never tracker state;
// A9: f64 is offline input.
#[derive(Clone, Copy, PartialEq)]
#[repr(C)]
struct Row {
    mu_turn: f64,
    /// Heading rate, degrees per second, clockwise positive.
    omega_dps: f64,
    /// The row's position (degrees) and velocity (east, north, m/s).
    lat: f64,
    lon: f64,
    ve: f64,
    vn: f64,
}
const _: () = assert!(core::mem::size_of::<Row>() == 48);

/// One raw report of an aircraft: position (degrees) and the reported velocity (east, north, m/s).
// A3 exception: keyed shell join value capped by MAX_DIAG_MEASUREMENTS, never tracker state;
// A9: f64 is offline input.
#[derive(Clone, Copy, PartialEq)]
#[repr(C)]
struct Raw {
    lat: f64,
    lon: f64,
    ve: f64,
    vn: f64,
}
const _: () = assert!(core::mem::size_of::<Raw>() == 32);

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (mut adsb, mut samples, mut out) = (None, None, None);
    let mut rescore = false;
    let mut thin = 1usize;
    let mut vel_lag = None;
    while let Some(a) = args.next() {
        if a == "--rescore" {
            rescore = true;
            continue;
        }
        let v = args.next();
        match a.as_str() {
            "--vel-lag" => vel_lag = v.and_then(|x| x.parse::<f32>().ok()),
            "--thin" => {
                thin = v
                    .and_then(|x| x.parse().ok())
                    .filter(|&n| n >= 1)
                    .unwrap_or(0)
            }
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
            "usage: turn_diag --adsb <clip.oaf> --samples <p002 samples.csv> --out <dir> [--rescore [--thin N]] [--vel-lag S]"
        );
        return ExitCode::from(2);
    };
    if thin == 0 || (thin > 1 && !rescore) {
        eprintln!("turn_diag: --thin N needs N >= 1 and --rescore");
        return ExitCode::from(2);
    }
    for p in [&adsb, &samples, &out] {
        if let Err(e) = oa_sealed::refuse(p) {
            eprintln!("turn_diag: {e}");
            return ExitCode::from(2);
        }
    }
    match run(&adsb, &samples, &out, rescore, thin, vel_lag) {
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
    let (file, len) =
        oa_sealed::open_regular(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if len > MAX_SAMPLES_FILE_BYTES {
        return Err(format!(
            "{}: samples file exceeds {MAX_SAMPLES_FILE_BYTES} byte limit",
            path.display()
        ));
    }
    let mut text = String::new();
    file.take(MAX_SAMPLES_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if u64::try_from(text.len()).map_or(true, |n| n > MAX_SAMPLES_FILE_BYTES) {
        return Err(format!(
            "{}: samples file exceeds {MAX_SAMPLES_FILE_BYTES} byte limit",
            path.display()
        ));
    }
    parse_samples(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// [`read_samples`] of the text of the file.
fn parse_samples(text: &str) -> Result<Vec<Sample>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.starts_with('#'));
    let (header_ix, header) = lines.next().ok_or("line 1: empty samples file")?;
    if header.len() > MAX_SAMPLE_ROW_BYTES {
        return Err(format!(
            "line {} exceeds {MAX_SAMPLE_ROW_BYTES} byte header limit",
            header_ix + 1
        ));
    }
    let headers: Vec<&str> = header.split(',').collect();
    if headers.len() > MAX_SAMPLE_COLUMNS {
        return Err(format!(
            "line {} exceeds {MAX_SAMPLE_COLUMNS} column limit",
            header_ix + 1
        ));
    }
    let mut head = BTreeMap::new();
    for (i, h) in headers.into_iter().enumerate() {
        if head.insert(h, i).is_some() {
            return Err(format!(
                "line {} has duplicate samples column {h}",
                header_ix + 1
            ));
        }
    }
    let col = |n: &str| head.get(n).copied().ok_or(format!("no column {n}"));
    let (c_addr, c_t, c_band, c_common) =
        (col("addr")?, col("t_ms")?, col("band")?, col("common")?);
    let c_flight = col("phase")?;
    let (c_cross, c_hdg, c_dt) = (
        col("oa_cross_m")?,
        col("oa_track_err_deg")?,
        col("oa_row_dt_ms")?,
    );
    let (e_cross, e_hdg) = (col("eram_cross_m")?, col("eram_track_err_deg")?);
    let mut v = Vec::new();
    let mut grid_keys = BTreeSet::new();
    for (line_ix, l) in lines {
        if l.len() > MAX_SAMPLE_ROW_BYTES {
            return Err(format!(
                "line {} exceeds {MAX_SAMPLE_ROW_BYTES} byte row limit",
                line_ix + 1
            ));
        }
        let f: Vec<&str> = l.split(',').collect();
        let g = |i: usize| f.get(i).copied().unwrap_or("");
        if g(c_common) != "1" {
            continue;
        }
        if v.len() >= MAX_SAMPLES {
            return Err(format!(
                "line {} exceeds {MAX_SAMPLES} common-row limit",
                line_ix + 1
            ));
        }
        let num = |i: usize| g(i).parse::<f64>().ok().filter(|x| x.is_finite());
        let (Some(addr), Some(t), Some(dt)) = (
            u32::from_str_radix(g(c_addr), 16).ok(),
            g(c_t).parse::<i64>().ok(),
            g(c_dt).parse::<i64>().ok(),
        ) else {
            return Err(format!(
                "line {} has an invalid address or timestamp",
                line_ix + 1
            ));
        };
        let (Some(oa_cross), Some(oa_hdg), Some(er_cross), Some(er_hdg)) =
            (num(c_cross), num(c_hdg), num(e_cross), num(e_hdg))
        else {
            return Err(format!(
                "line {} has a missing or non-finite common error",
                line_ix + 1
            ));
        };
        if !grid_keys.insert((addr, t)) {
            return Err(format!(
                "line {} duplicates p002 grid sample {addr:06x},{t}",
                line_ix + 1
            ));
        }
        let t_ms = t.checked_add(dt).ok_or_else(|| {
            format!(
                "line {} timestamp plus row offset overflows i64",
                line_ix + 1
            )
        })?;
        v.push(Sample {
            addr,
            t_ms,
            oa_cross,
            oa_hdg,
            er_cross,
            er_hdg,
            band: g(c_band).to_string(),
            flight: g(c_flight).to_string(),
        });
    }
    Ok(v)
}

/// Keeps the tick rows of `o` whose key is wanted.
fn collect(
    o: &TrackOutput,
    want: &BTreeSet<(u32, i64)>,
    got: &mut BTreeMap<(u32, i64), Row>,
    addrs: &BTreeSet<u32>,
    got_upd: &mut BTreeMap<(u32, i64), Row>,
    conflicts: &mut BTreeSet<(u32, i64)>,
    update_conflicts: &mut BTreeSet<(u32, i64)>,
) {
    for i in 0..o.addr.len() {
        let (Some(&a), Some(&t), Some(&f)) = (o.addr.get(i), o.t_ns.get(i), o.flags.get(i)) else {
            continue;
        };
        let key = (a, t.div_euclid(1_000_000));
        let tick = f & O_TICK != 0;
        if !(tick && want.contains(&key) || !tick && addrs.contains(&a)) {
            continue;
        }
        let g = |v: &[f32]| v.get(i).copied().map_or(f64::NAN, f64::from);
        let e7 = |v: &[i32]| v.get(i).map_or(f64::NAN, |&x| f64::from(x) * 1e-7);
        let row = Row {
            mu_turn: o.mu_h.get(i).map_or(f64::NAN, |m| f64::from(m[1])),
            omega_dps: g(&o.turn_rate_rps).to_degrees(),
            lat: e7(&o.lat_e7),
            lon: e7(&o.lon_e7),
            ve: g(&o.vel_east_mps),
            vn: g(&o.vel_north_mps),
        };
        if tick {
            insert_unambiguous(got, conflicts, key, row);
        } else {
            insert_unambiguous(got_upd, update_conflicts, key, row);
        }
    }
}

/// Keep equivalent duplicate rows, but remove and remember conflicting rows independent of order.
fn insert_unambiguous<K: Ord + Copy, V: PartialEq + Copy>(
    map: &mut BTreeMap<K, V>,
    conflicts: &mut BTreeSet<K>,
    key: K,
    value: V,
) {
    if conflicts.contains(&key) {
        return;
    }
    match map.get(&key).copied() {
        None => {
            map.insert(key, value);
        }
        Some(old) if old == value => {}
        Some(_) => {
            map.remove(&key);
            conflicts.insert(key);
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

/// The time of the last report at or before `t`, Unix ms.
fn last_report(times: &[i64], t: i64) -> Option<i64> {
    let i = times.partition_point(|&x| x <= t);
    i.checked_sub(1).and_then(|j| times.get(j)).copied()
}

/// The age of the last report at or before `t` and the gap from it to the next one, seconds
/// (none before the first report; the gap is none after the last).
fn age_gap(times: &[i64], t: i64) -> (Option<f64>, Option<f64>) {
    let i = times.partition_point(|&x| x <= t);
    let last = i.checked_sub(1).and_then(|j| times.get(j)).copied();
    let next = times.get(i).copied();
    // i64 endpoints differ by at most 2^64-1, so the widened i128 subtraction cannot overflow.
    (
        last.map(|l| (i128::from(t) - i128::from(l)) as f64 / 1000.0),
        last.zip(next)
            .map(|(l, n)| (i128::from(n) - i128::from(l)) as f64 / 1000.0),
    )
}

/// Estimate the signed time offset represented by a reported heading during a reference turn.
/// Positive means the reported heading trails the reference; negative means it leads. This is a
/// diagnostic proxy, not an estimate of the tracker's internal state or proof of causality.
fn heading_lag_s(reference_deg: f64, reported_deg: f64, reference_rate_dps: f64) -> Option<f64> {
    if !reference_deg.is_finite()
        || !reported_deg.is_finite()
        || !reference_rate_dps.is_finite()
        || reference_rate_dps.abs() < TURN_DPS
    {
        return None;
    }
    let signed_delta = (reference_deg - reported_deg + 180.0).rem_euclid(360.0) - 180.0;
    Some(signed_delta / reference_rate_dps)
}

/// Index of the report-age cohort, with a final bin for no report at or before the sample.
fn age_bin(age_s: Option<f64>) -> usize {
    age_s.map_or(AGE_EDGES.len() + 1, |age| bin_of(&AGE_EDGES, age))
}

/// Reference turn status: turning, straight, or unavailable.
fn turn_bin(rate_dps: Option<f64>) -> usize {
    match rate_dps {
        Some(rate) if rate.abs() >= TURN_DPS => 0,
        Some(_) => 1,
        None => 2,
    }
}

// A3 exception: exactly 18 fixed diagnostic cells, not tracker state; A9: f64 is report-only.
#[derive(Default)]
#[repr(C)]
struct RateCell {
    samples: u64,
    matched: u64,
    reference_dps: Vec<f64>,
    tracker_dps: Vec<f64>,
    absolute_error_dps: Vec<f64>,
}
const _: () = assert!(core::mem::size_of::<RateCell>() == 88);

fn signed_median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    sorted.get(sorted.len() / 2).copied().unwrap_or(f64::NAN)
}

fn record_rate_sample(cell: &mut RateCell, reference_dps: Option<f64>, tracker: Option<Row>) {
    cell.samples += 1;
    if let (Some(reference), Some(tracker)) = (reference_dps, tracker) {
        if reference.is_finite() && tracker.omega_dps.is_finite() {
            cell.matched += 1;
            cell.reference_dps.push(reference);
            cell.tracker_dps.push(tracker.omega_dps);
            cell.absolute_error_dps
                .push((tracker.omega_dps - reference).abs());
        }
    }
}

/// One sample's reference heading rate and turn phase, and its tracker row.
// A3 exception: parallel sample record capped by MAX_SAMPLES, not tracker state;
// A9: f64 holds diagnostic values, not core state.
#[repr(C)]
struct Ref {
    rate: Option<f64>,
    phase: usize,
}
const _: () = assert!(core::mem::size_of::<Ref>() == 24);

/// The distribution of one value in the bins of a key: one row of a table.
// A3 exception: shell table accumulator keyed by sampled band (at most MAX_SAMPLES keys), not
// tracker state. A9: f64 values are diagnostic summaries.
#[repr(C)]
struct Table {
    title: String,
    labels: Vec<String>,
    /// Per bin: OpenATCA cross, ERAM cross, OpenATCA angle, ERAM angle.
    cols: Vec<[Vec<f64>; 4]>,
}
const _: () = assert!(core::mem::size_of::<Table>() == 72);

fn table_new(title: &str, labels: Vec<String>) -> Table {
    let n = labels.len();
    Table {
        title: title.to_string(),
        labels,
        cols: (0..n).map(|_| Default::default()).collect(),
    }
}

fn table_add(table: &mut Table, bin: usize, s: &Sample) {
    if let Some(c) = table.cols.get_mut(bin) {
        c[0].push(s.oa_cross);
        c[1].push(s.er_cross);
        c[2].push(s.oa_hdg);
        c[3].push(s.er_hdg);
    }
}

fn table_write(table: &Table, o: &mut String) {
    let _ = writeln!(o, "\n{}", table.title);
    let _ = writeln!(
        o,
        "  {:<16} {:>7} | cross m: OA med  p95  rms | ERAM med  p95  rms | angle deg: OA med  p95  rms | ERAM med  p95  rms",
        "bin", "n"
    );
    for (l, c) in table.labels.iter().zip(&table.cols) {
        let [a, b, d, e] = [stat(&c[0]), stat(&c[1]), stat(&c[2]), stat(&c[3])];
        let _ = writeln!(
            o,
            "  {:<16} {:>7} |          {:>6.1} {:>5.1} {:>5.1} |      {:>6.1} {:>5.1} {:>5.1} |            {:>6.2} {:>5.2} {:>5.2} |      {:>6.2} {:>5.2} {:>5.2}",
            l, a[0], a[1], a[2], a[3], b[1], b[2], b[3], d[1], d[2], d[3], e[1], e[2], e[3]
        );
    }
}

fn run(
    adsb: &Path,
    samples_path: &Path,
    out_dir: &Path,
    rescore: bool,
    thin: usize,
    vel_lag: Option<f32>,
) -> Result<String, String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut samples = read_samples(samples_path)?;
    eprintln!("{} common samples", samples.len());
    let want: BTreeSet<(u32, i64)> = samples.iter().map(|s| (s.addr, s.t_ms)).collect();

    // p002's split and tracker run; only the wanted tick rows are kept.
    let mut drops = oa_sealed::DropReport::new();
    let mut clip_sealed = 0u64;
    let clip = load_adsb_counted(adsb, &bounded_adsb_opener(), &mut clip_sealed);
    drops.add(clip_sealed);
    let clip = clip.map_err(|e| e.to_string())?;
    if oa_adsb::reports_len(&clip.reports) > MAX_DIAG_MEASUREMENTS {
        return Err(format!(
            "{}: decoded ADS-B reports exceed {MAX_DIAG_MEASUREMENTS} diagnostic limit",
            adsb.display()
        ));
    }
    let (set_a, set_b, _) = split_reports(&clip.reports);
    drop(clip);
    let (mut meas, _) = adsb_measurements(&set_a, &AdsbConfig::default());
    drop(set_a);
    if meas.len() > MAX_DIAG_MEASUREMENTS {
        return Err(format!(
            "{}: ADS-B measurements exceed {MAX_DIAG_MEASUREMENTS} diagnostic limit",
            adsb.display()
        ));
    }
    if let Some(l) = vel_lag {
        // Every velocity says it lags by `l` s whatever the source set: 0 is the behaviour before
        // OA-TRK-401.
        for m in meas.iter_mut().filter(|m| m.flags & M_HAS_VEL != 0) {
            m.vel_lag_s = l;
        }
    }
    if thin > 1 {
        // Every `thin`th report of each aircraft goes to the tracker; the grid and the reference
        // are unchanged.
        let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
        meas.retain(|m| {
            let k = seen.entry(m.addr).or_insert(0);
            *k += 1;
            (*k - 1) % thin == 0
        });
    }
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
    for report_times in times.values_mut() {
        report_times.sort_unstable();
        report_times.dedup();
    }
    // The raw position and velocity of every report of the sample aircraft (the gap diagnostic's
    // dead reckoning).
    let mut raw: BTreeMap<(u32, i64), Raw> = BTreeMap::new();
    let mut raw_conflicts = BTreeSet::new();
    let mut raw_seen = BTreeSet::new();
    for m in &meas {
        if addrs.contains(&m.addr) {
            let key = (m.addr, m.toa_ns.div_euclid(1_000_000));
            if !raw_seen.insert(key) {
                raw.remove(&key);
                raw_conflicts.insert(key);
                continue;
            }
            if m.flags & M_HAS_VEL != 0 {
                raw.insert(
                    key,
                    Raw {
                        lat: f64::from(m.lat_e7) * 1e-7,
                        lon: f64::from(m.lon_e7) * 1e-7,
                        ve: f64::from(m.vel_east_mps),
                        vn: f64::from(m.vel_north_mps),
                    },
                );
            }
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
    let mut upd_rows: BTreeMap<(u32, i64), Row> = BTreeMap::new();
    let mut row_conflicts = BTreeSet::new();
    let mut upd_conflicts = BTreeSet::new();
    let spill = out_dir.join(format!("spill-{}", std::process::id()));
    let mut w = if rescore {
        Some(spill_new(&spill).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let drain = |scratch: &mut TrackOutput,
                 w: &mut Option<oa_track::spill::SpillWriter>,
                 rows: &mut BTreeMap<(u32, i64), Row>,
                 upd: &mut BTreeMap<(u32, i64), Row>,
                 row_conflicts: &mut BTreeSet<(u32, i64)>,
                 upd_conflicts: &mut BTreeSet<(u32, i64)>|
     -> Result<(), String> {
        collect(
            scratch,
            &want,
            rows,
            &addrs,
            upd,
            row_conflicts,
            upd_conflicts,
        );
        if let Some(w) = w {
            spill_append(w, scratch).map_err(|e| e.to_string())?;
        }
        output_clear(scratch);
        Ok(())
    };
    for m in &meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            drain(
                &mut scratch,
                &mut w,
                &mut rows,
                &mut upd_rows,
                &mut row_conflicts,
                &mut upd_conflicts,
            )?;
        }
    }
    let end = meas.last().map_or(0, |m| m.toa_ns);
    tracker_advance(&mut t, end, &mut scratch);
    tracker_flush(&mut t, end, &mut scratch);
    let st = tracker_stats(&t);
    eprintln!(
        "tracker stats: offered {} applied {} velocity_gated {} jump_rejected {}",
        st.offered, st.applied, st.velocity_gated, st.jump_rejected
    );
    let overflow = scratch.overflow;
    drain(
        &mut scratch,
        &mut w,
        &mut rows,
        &mut upd_rows,
        &mut row_conflicts,
        &mut upd_conflicts,
    )?;
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
        let mut scored_conflicts = BTreeSet::new();
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
                    insert_unambiguous(&mut scored, &mut scored_conflicts, (a, t), (c, h));
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
            "rescored {} of {before} samples ({} lost; {} conflicting OA row keys excluded)",
            samples.len(),
            before - samples.len(),
            scored_conflicts.len()
        );
    }
    let heading = |a: u32, t: i64| -> Option<f64> {
        let s = adsb_truth::query(&built.truth, a, t).ok()?;
        (s.vel_east_mps.hypot(s.vel_north_mps) >= MIN_SPEED_MPS)
            .then(|| s.vel_east_mps.atan2(s.vel_north_mps).to_degrees())
    };
    let rate = |a: u32, t: i64| -> Option<f64> {
        let delta_ms = HALF_S.checked_mul(1000)?;
        let ahead = t.checked_add(delta_ms)?;
        let behind = t.checked_sub(delta_ms)?;
        let d = heading(a, ahead)? - heading(a, behind)?;
        let d = (d + 180.0).rem_euclid(360.0) - 180.0;
        Some(d / (2 * HALF_S) as f64)
    };
    let refs: Vec<Ref> = samples
        .iter()
        .map(|s| {
            let r0 = rate(s.addr, s.t_ms);
            let r_old = WINDOW_S
                .checked_mul(1000)
                .and_then(|d| s.t_ms.checked_sub(d))
                .and_then(|old_t| rate(s.addr, old_t));
            let turning = |r: f64| r.abs() >= TURN_DPS;
            let phase = match (r0, r_old) {
                (Some(now), Some(old)) => match (turning(now), turning(old)) {
                    (true, false) => 0,
                    (true, true) => 1,
                    (false, _) => {
                        let recent = old.abs() >= TURN_DPS
                            || (1..WINDOW_S).any(|k| {
                                k.checked_mul(1000)
                                    .and_then(|d| s.t_ms.checked_sub(d))
                                    .and_then(|at| rate(s.addr, at))
                                    .is_some_and(turning)
                            });
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
        if let Some(seconds) = heading_lag_s(ht, h, w) {
            if let Some(b) = lag.get_mut(bin_of(&RATE_EDGES, w.abs())) {
                b.push(seconds);
            }
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
    let mut by_rate = table_new(
        "By the reference's turn rate (deg/s, magnitude)",
        rate_labels,
    );
    let mut by_phase = table_new(
        &format!("By turn phase (turning = |rate| >= {TURN_DPS} deg/s; window {WINDOW_S} s)"),
        PHASES.iter().map(|s| s.to_string()).collect(),
    );
    let mut by_mu = table_new("By the IMM turn-model probability", mu_labels);
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
    let mut by_age = table_new(
        "By seconds since the track's last report (turning = reference |rate| >= 0.25 deg/s)",
        edge_labels(&AGE_EDGES),
    );
    let mut by_age_turn = table_new("  ... turning samples only", edge_labels(&AGE_EDGES));
    let mut by_age_str = table_new("  ... not turning samples only", edge_labels(&AGE_EDGES));
    let mut by_age_turn_rate: Vec<RateCell> = (0..(AGE_EDGES.len() + 2) * 3)
        .map(|_| RateCell::default())
        .collect();
    let mut by_gap = table_new(
        "By the length of the report gap holding the sample (s)",
        edge_labels(&GAP_EDGES),
    );
    let mut by_gap_turn = table_new("  ... turning samples only", edge_labels(&GAP_EDGES));
    let mut all = table_new("All common samples", vec!["all".to_string()]);
    let flight_labels: Vec<String> = ["all", "climb", "cruise", "descent", "steady turn"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut by_flight_near = table_new(
        "By flight phase, samples within 2 s of a report",
        flight_labels.clone(),
    );
    let mut by_flight_far = table_new(
        "By flight phase, samples more than 5 s from a report",
        flight_labels.clone(),
    );
    let mut by_flight_any = table_new("By flight phase, any report age", flight_labels);
    let mut by_band = BTreeMap::new();
    // Tracker's heading rate against the reference's, per rate bin: signed medians.
    let mut est: Vec<(Vec<f64>, Vec<f64>)> = (0..6).map(|_| Default::default()).collect();
    let mut csv = String::from(
        "addr,t_ms,band,oa_cross_m,eram_cross_m,oa_track_err_deg,eram_track_err_deg,ref_rate_dps,phase,mu_turn,est_rate_dps\n",
    );
    for (s, r) in samples.iter().zip(&refs) {
        let rb = r.rate.map_or(5, |x| bin_of(&RATE_EDGES, x.abs()));
        let row = rows.get(&(s.addr, s.t_ms));
        let mb = row
            .filter(|x| x.mu_turn.is_finite())
            .map_or(4, |x| bin_of(&MU_EDGES, x.mu_turn));
        table_add(&mut by_rate, rb, s);
        table_add(&mut by_phase, r.phase, s);
        table_add(&mut by_mu, mb, s);
        let (age, gap) = times
            .get(&s.addr)
            .map_or((None, None), |t| age_gap(t, s.t_ms));
        let ab = age.map_or(AGE_EDGES.len() + 2, |a| bin_of(&AGE_EDGES, a));
        let gb = gap.map_or(GAP_EDGES.len() + 2, |g| bin_of(&GAP_EDGES, g));
        let turning = r.rate.is_some_and(|x| x.abs() >= TURN_DPS);
        let turn_class = turn_bin(r.rate);
        let cell_index = age_bin(age) * 3 + turn_class;
        if let Some(cell) = by_age_turn_rate.get_mut(cell_index) {
            record_rate_sample(cell, r.rate, row.copied());
        }
        table_add(&mut by_age, ab, s);
        table_add(&mut by_gap, gb, s);
        if turning {
            table_add(&mut by_age_turn, ab, s);
            table_add(&mut by_gap_turn, gb, s);
        } else if r.rate.is_some() {
            table_add(&mut by_age_str, ab, s);
        }
        table_add(&mut all, 0, s);
        let fb = [
            Some(0),
            match s.flight.as_str() {
                "climb" => Some(1),
                "cruise" => Some(2),
                "descent" => Some(3),
                _ => None,
            },
            (r.phase == 1).then_some(4),
        ];
        let near = age.is_some_and(|a| a <= 2.0);
        let far = age.is_some_and(|a| a > 5.0);
        for b in fb.into_iter().flatten() {
            table_add(&mut by_flight_any, b, s);
            if near {
                table_add(&mut by_flight_near, b, s);
            }
            if far {
                table_add(&mut by_flight_far, b, s);
            }
        }
        by_band
            .entry(s.band.clone())
            .or_insert_with(|| table_new(&format!("Band {}", s.band), vec!["all".to_string()]));
        if let Some(table) = by_band.get_mut(&s.band) {
            table_add(table, 0, s);
        }
        if let (Some(x), Some(row), Some(e)) = (r.rate, row, est.get_mut(rb)) {
            if row.omega_dps.is_finite() {
                // Both signed so that the reference turns "positive": a tracker rate of the same sign
                // and size reads the same number.
                e.0.push(x.abs());
                e.1.push(row.omega_dps * oa_math::signum_canon_f64(x));
            }
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
            row.filter(|x| x.mu_turn.is_finite())
                .map_or(String::new(), |x| format!("{:.4}", x.mu_turn)),
            row.filter(|x| x.omega_dps.is_finite())
                .map_or(String::new(), |x| format!("{:.3}", x.omega_dps)),
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
    let mut gaps: Vec<f64> = times
        .values()
        .flat_map(|v| {
            v.windows(2).filter_map(|w| match w {
                // Both endpoints are i64; their widened difference is bounded by 2^64-1.
                [a, b] => Some((i128::from(*b) - i128::from(*a)) as f64 / 1000.0),
                _ => None,
            })
        })
        .collect();
    gaps.sort_by(|a, b| oa_math::cmp_canon_f64(*a, *b));
    let _ = writeln!(
        o,
        "thin {thin}: input reports of the sample aircraft {}, gaps {}, median input gap {:.2} s",
        times.values().map(Vec::len).sum::<usize>(),
        gaps.len(),
        gaps.get(gaps.len() / 2).copied().unwrap_or(f64::NAN)
    );
    table_write(&all, &mut o);
    for t in [&by_flight_any, &by_flight_near, &by_flight_far] {
        table_write(t, &mut o);
    }
    table_write(&by_rate, &mut o);
    table_write(&by_phase, &mut o);
    table_write(&by_mu, &mut o);
    for t in [&by_age, &by_age_turn, &by_age_str, &by_gap, &by_gap_turn] {
        table_write(t, &mut o);
    }
    let _ = writeln!(
        o,
        "\nTracker turn-rate agreement by actual ADS-B report age and reference turn status (rate error = |tracker minus reference|; report-age bin is the last set-A report at or before the p002 sample; coverage requires a reference rate and matching tracker tick; descriptive association, not causal)\n  {:<16} {:<12} {:>8} {:>8} {:>9} {:>9} {:>9} {:>9}",
        "age", "reference", "samples", "rate rows", "coverage", "ref med", "trk med", "err p95"
    );
    let age_labels = edge_labels(&AGE_EDGES);
    let turn_labels = ["turning", "straight", "no reference rate"];
    for (ai, age_label) in age_labels.iter().enumerate() {
        for (ti, turn_label) in turn_labels.iter().enumerate() {
            let Some(cell) = by_age_turn_rate.get(ai * 3 + ti) else {
                continue;
            };
            let err = stat(&cell.absolute_error_dps);
            let coverage = if cell.samples == 0 {
                f64::NAN
            } else {
                100.0 * cell.matched as f64 / cell.samples as f64
            };
            let _ = writeln!(
                o,
                "  {:<16} {:<12} {:>8} {:>8} {:>8.1}% {:>9.3} {:>9.3} {:>9.3}",
                age_label,
                turn_label,
                cell.samples,
                cell.matched,
                coverage,
                signed_median(&cell.reference_dps),
                signed_median(&cell.tracker_dps),
                err[2]
            );
        }
    }
    for t in by_band.values() {
        table_write(t, &mut o);
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
        "\nReported-velocity heading lag proxy, seconds (wrapped reference-minus-reported heading divided by reference turn rate; positive trails, negative leads; every {LAG_EVERY}th report of the aircraft above; descriptive, not causal)"
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
    let tq = |a: u32, t: i64| -> Option<[f64; 4]> {
        let s = adsb_truth::query(&built.truth, a, t).ok()?;
        Some([s.lat, s.lon, s.vel_east_mps, s.vel_north_mps])
    };
    let rate_q = |a: u32, t: i64| rate(a, t);
    let acc_times: BTreeMap<u32, Vec<i64>> = {
        let mut m: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
        for &(a, t) in upd_rows.keys().chain(upd_conflicts.iter()) {
            m.entry(a).or_default().push(t);
        }
        for accepted_times in m.values_mut() {
            accepted_times.sort_unstable();
            accepted_times.dedup();
        }
        m
    };
    gap_report(
        &mut o,
        &GapIn {
            samples: &samples,
            rows: &rows,
            upd: &upd_rows,
            raw: &raw,
            raw_conflicts: &raw_conflicts,
            row_conflicts: &row_conflicts,
            update_conflicts: &upd_conflicts,
            offered: &times,
            accepted: &acc_times,
            truth: &tq,
            rate: &rate_q,
        },
    );
    fs::write(out_dir.join("turn_diag.txt"), &o).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("turn_samples.csv"), &csv).map_err(|e| e.to_string())?;
    Ok(o)
}

/// WGS-84 equatorial radius, metres, and first eccentricity squared.
const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 0.006_694_379_990_141_317;
/// Longest gap (seconds) over which the reference's turn rate is scanned; a longer one is
/// "unknown rate".
const SCAN_MAX_S: f64 = 120.0;

/// Meridian and prime-vertical radii of curvature at a latitude (degrees), metres.
fn radii(lat_deg: f64) -> (f64, f64) {
    let s = lat_deg.to_radians().sin();
    let w = 1.0 - WGS84_E2 * s * s;
    (
        WGS84_A * (1.0 - WGS84_E2) / (w * w.sqrt()),
        WGS84_A / w.sqrt(),
    )
}

/// The dead-reckoning baseline: the position (degrees) after flying `age_s` seconds in a straight
/// line at the report's reported velocity from its position. The step is in metres east and
/// north, turned into degrees with the WGS-84 radii at the report's latitude. (The reported
/// velocity lags the aircraft's by about a second; for a straight line that does not matter.)
fn dead_reckon(raw: &Raw, age_s: f64) -> (f64, f64) {
    let (m, n) = radii(raw.lat);
    (
        raw.lat + (raw.vn * age_s / m).to_degrees(),
        raw.lon + (raw.ve * age_s / (n * raw.lat.to_radians().cos())).to_degrees(),
    )
}

/// The cross-track distance of the point (degrees) from the reference's track line at the
/// reference state `r` (latitude, longitude, east velocity, north velocity), metres. Sign:
/// positive when the point is to the right of the direction of travel (south of an eastbound
/// track); the diagnostic uses the magnitude. None below the fit speed floor.
fn cross_of(lat: f64, lon: f64, r: &[f64; 4]) -> Option<f64> {
    if !lat.is_finite() || !lon.is_finite() || r.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let sp = r[2].hypot(r[3]);
    if !sp.is_finite() || sp < MIN_SPEED_MPS {
        return None;
    }
    let (m, n) = radii(r[0]);
    let dn = (lat - r[0]).to_radians() * m;
    let de = (lon - r[1]).to_radians() * n * r[0].to_radians().cos();
    let result = (de * r[3] - dn * r[2]) / sp;
    result.is_finite().then_some(result)
}

fn wrap_deg(d: f64) -> f64 {
    (d + 180.0).rem_euclid(360.0) - 180.0
}

/// What the reference did between the last report and the sample.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    /// Its turn rate stayed under [`TURN_DPS`] at every second of the interval.
    Level,
    /// It reached [`TURN_DPS`] or more at some second.
    Turned,
    /// Its turn rate is not defined at some second (or the interval is over [`SCAN_MAX_S`]).
    Unknown,
}

const CLASSES: [(&str, Class); 3] = [
    ("level throughout gap", Class::Level),
    ("turned in gap", Class::Turned),
    ("unknown rate", Class::Unknown),
];

/// Why a sample is not in the dead-reckoning comparison; the index into [`DROPS`].
const DROPS: [&str; 8] = [
    "no accepted report",
    "ambiguous duplicate key",
    "no velocity on it",
    "no reference",
    "invalid geometry",
    "slow reference",
    "no tick row",
    "timestamp arithmetic overflow",
];

/// One common sample of the gap diagnostic.
// A3 exception: shell aggregate row capped by MAX_SAMPLES, not tracker state;
// A9: f64 retains diagnostic units.
#[repr(C)]
struct Cand {
    addr: u32,
    flight: String,
    /// Seconds since the last report the tracker accepted; none if it has accepted none.
    age: Option<f64>,
    class: Class,
    drop: Option<usize>,
    /// Cross-track error magnitudes: dead reckoning, OpenATCA by the same definition, OpenATCA
    /// by p002's, ERAM by p002's (ERAM positions are not in `samples.csv`).
    dr: f64,
    oa: f64,
    oa_p002: f64,
    er: f64,
    /// The track's turn rate and turn probability at the sample, the reference's turn rate.
    omega: f64,
    mu: f64,
    ref_rate: f64,
    /// Heading error (degrees) of the track's velocity at the last accepted report's update row
    /// and of the raw reported velocity, against the reference's heading then.
    upd_err: Option<f64>,
    raw_err: Option<f64>,
}
const _: () = assert!(core::mem::size_of::<Cand>() == 160);

/// What the gap diagnostic reads.
struct GapIn<'a> {
    samples: &'a [Sample],
    rows: &'a BTreeMap<(u32, i64), Row>,
    /// The tracker's update rows (one per accepted report) of the sample aircraft.
    upd: &'a BTreeMap<(u32, i64), Row>,
    /// Raw reports with a velocity.
    raw: &'a BTreeMap<(u32, i64), Raw>,
    raw_conflicts: &'a BTreeSet<(u32, i64)>,
    row_conflicts: &'a BTreeSet<(u32, i64)>,
    update_conflicts: &'a BTreeSet<(u32, i64)>,
    /// Times of every report offered to the tracker, and of those it accepted (update rows).
    offered: &'a BTreeMap<u32, Vec<i64>>,
    accepted: &'a BTreeMap<u32, Vec<i64>>,
    /// The reference at a time: latitude, longitude, east and north velocity.
    truth: &'a dyn Fn(u32, i64) -> Option<[f64; 4]>,
    /// The reference's turn rate, degrees per second, clockwise positive.
    rate: &'a dyn Fn(u32, i64) -> Option<f64>,
}

fn candidates(g: &GapIn) -> (Vec<Cand>, usize) {
    let mut out = Vec::new();
    let mut differs = 0usize;
    for s in g.samples {
        let acc = g.accepted.get(&s.addr).and_then(|v| last_report(v, s.t_ms));
        let off = g.offered.get(&s.addr).and_then(|v| last_report(v, s.t_ms));
        if acc != off {
            differs += 1;
        }
        let mut c = Cand {
            addr: s.addr,
            flight: s.flight.clone(),
            age: None,
            class: Class::Unknown,
            drop: None,
            dr: f64::NAN,
            oa: f64::NAN,
            oa_p002: s.oa_cross,
            er: s.er_cross,
            omega: f64::NAN,
            mu: f64::NAN,
            ref_rate: f64::NAN,
            upd_err: None,
            raw_err: None,
        };
        let Some(last) = acc else {
            c.drop = Some(0);
            out.push(c);
            continue;
        };
        let Some(age_ms) = s.t_ms.checked_sub(last) else {
            c.drop = Some(7);
            out.push(c);
            continue;
        };
        let age = age_ms as f64 / 1000.0;
        c.age = Some(age);
        if g.raw_conflicts.contains(&(s.addr, last))
            || g.row_conflicts.contains(&(s.addr, s.t_ms))
            || g.update_conflicts.contains(&(s.addr, last))
        {
            c.drop = Some(1);
            out.push(c);
            continue;
        }
        // The reference's largest turn rate at the seconds from the last report to the sample.
        if age <= SCAN_MAX_S {
            let (mut mx, mut unknown) = (0.0f64, false);
            let mut t = last;
            loop {
                match (g.rate)(s.addr, t.min(s.t_ms)) {
                    Some(r) => mx = mx.max(r.abs()),
                    None => unknown = true,
                }
                if t >= s.t_ms {
                    break;
                }
                let Some(next) = t.checked_add(1000) else {
                    unknown = true;
                    break;
                };
                t = next;
            }
            c.class = if unknown {
                Class::Unknown
            } else if mx < TURN_DPS {
                Class::Level
            } else {
                Class::Turned
            };
        }
        c.ref_rate = (g.rate)(s.addr, s.t_ms).unwrap_or(f64::NAN);
        let Some(raw) = g.raw.get(&(s.addr, last)) else {
            c.drop = Some(2);
            out.push(c);
            continue;
        };
        let Some(rf) = (g.truth)(s.addr, s.t_ms) else {
            c.drop = Some(3);
            out.push(c);
            continue;
        };
        if rf.iter().any(|x| !x.is_finite()) {
            c.drop = Some(4);
            out.push(c);
            continue;
        }
        if rf[2].hypot(rf[3]) < MIN_SPEED_MPS {
            c.drop = Some(5);
            out.push(c);
            continue;
        }
        if ![raw.lat, raw.lon, raw.ve, raw.vn]
            .iter()
            .all(|x| x.is_finite())
        {
            c.drop = Some(4);
            out.push(c);
            continue;
        }
        let (lat, lon) = dead_reckon(raw, age);
        let Some(dr) = cross_of(lat, lon, &rf) else {
            c.drop = Some(4);
            out.push(c);
            continue;
        };
        let Some(row) = g.rows.get(&(s.addr, s.t_ms)) else {
            c.drop = Some(6);
            out.push(c);
            continue;
        };
        if ![row.lat, row.lon, row.ve, row.vn, row.omega_dps, row.mu_turn]
            .iter()
            .all(|x| x.is_finite())
        {
            c.drop = Some(4);
            out.push(c);
            continue;
        }
        let Some(oa) = cross_of(row.lat, row.lon, &rf) else {
            c.drop = Some(4);
            out.push(c);
            continue;
        };
        let at_last = (g.truth)(s.addr, last);
        let hdg = |ve: f64, vn: f64| ve.atan2(vn).to_degrees();
        c.upd_err = g.upd.get(&(s.addr, last)).zip(at_last).and_then(|(u, t)| {
            (t.iter().all(|x| x.is_finite())
                && [u.ve, u.vn].iter().all(|x| x.is_finite())
                && t[2].hypot(t[3]) >= MIN_SPEED_MPS)
                .then(|| wrap_deg(hdg(u.ve, u.vn) - hdg(t[2], t[3])))
                .filter(|x| x.is_finite())
        });
        c.raw_err = at_last.and_then(|t| {
            (t.iter().all(|x| x.is_finite())
                && [raw.ve, raw.vn].iter().all(|x| x.is_finite())
                && t[2].hypot(t[3]) >= MIN_SPEED_MPS)
                .then(|| wrap_deg(hdg(raw.ve, raw.vn) - hdg(t[2], t[3])))
                .filter(|x| x.is_finite())
        });
        c.dr = dr;
        c.oa = oa;
        c.omega = row.omega_dps;
        c.mu = row.mu_turn;
        out.push(c);
    }
    (out, differs)
}

/// The gap diagnostic: how a dead-reckoning baseline from the last accepted report compares with
/// the tracker and ERAM by phase, by what the reference did in the gap, and by age; what the
/// track carries after a long gap; how the error grows with the age. Appended to `o`.
fn gap_report(o: &mut String, g: &GapIn) {
    let (all, differs) = candidates(g);
    let ok: Vec<&Cand> = all.iter().filter(|c| c.drop.is_none()).collect();
    let mut drops = [0usize; DROPS.len()];
    for c in &all {
        if let Some(d) = c.drop {
            if let Some(x) = drops.get_mut(d) {
                *x += 1;
            }
        }
    }
    let _ = writeln!(
        o,
        "\nGap diagnostic. Last report = the last the tracker accepted (an update row); it differs from the last report offered for {differs} of {} samples. Dead reckoning (DR) = that report's position plus its reported velocity times the age (WGS-84 radii at its latitude). Cross-track magnitudes, m, median p95 rms; the ERAM column is p002's (ERAM positions are not in samples.csv). Cross-track is the distance from the reference's track line; DR and OpenATCA (same definition) use cross_of, OpenATCA (p002) and ERAM are p002's. Duplicate-key exclusions are independent of input order: {} repeated raw keys (all repeated millisecond keys are ambiguous), {} conflicting tracker tick keys, {} conflicting update keys.\nsamples {}: compared {}; dropped: {}",
        all.len(),
        g.raw_conflicts.len(),
        g.row_conflicts.len(),
        g.update_conflicts.len(),
        all.len(),
        ok.len(),
        DROPS
            .iter()
            .zip(&drops)
            .map(|(n, c)| format!("{n} {c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let cell = |v: &[&Cand], f: &dyn Fn(&Cand) -> f64| -> String {
        let x: Vec<f64> = v.iter().map(|a| f(a)).collect();
        let t = stat(&x);
        format!("{:>6.1} {:>6.1} {:>6.1}", t[1], t[2], t[3])
    };
    let ages: [(&str, f64, f64); 4] = [
        ("under 5 s", 0.0, FAR_S),
        ("5-8 s", FAR_S, 8.0),
        ("over 8 s", 8.0, f64::INFINITY),
        ("over 5 s", FAR_S, f64::INFINITY),
    ];
    let in_group = |fl: &str, c: &Cand| -> bool {
        match fl {
            "all" => true,
            "level throughout gap" => c.class == Class::Level,
            "turned in gap" => c.class == Class::Turned,
            "unknown rate" => c.class == Class::Unknown,
            _ => c.flight == fl,
        }
    };
    let _ = writeln!(
        o,
        "  {:<34} {:>6} {:<26} | dead reckoning        | OpenATCA (same definition) | OpenATCA (p002)      | ERAM (p002)",
        "group, age", "n", "dropped (acc/vel/ref/slow/tick)"
    );
    for fl in [
        "all",
        "climb",
        "cruise",
        "descent",
        "level throughout gap",
        "turned in gap",
        "unknown rate",
    ] {
        for (name, lo, hi) in ages {
            let cand: Vec<&Cand> = all
                .iter()
                .filter(|a| in_group(fl, a) && a.age.is_some_and(|x| x >= lo && x < hi))
                .collect();
            let v: Vec<&Cand> = cand.iter().copied().filter(|a| a.drop.is_none()).collect();
            let mut d = [0usize; DROPS.len()];
            for c in &cand {
                if let Some(k) = c.drop.and_then(|k| d.get_mut(k)) {
                    *k += 1;
                }
            }
            let _ = writeln!(
                o,
                "  {:<34} {:>6} {:<26} | {} | {}     | {} | {}",
                format!("{fl}, {name}"),
                v.len(),
                format!("{}/{}/{}/{}/{}", d[0], d[1], d[2], d[3], d[4]),
                cell(&v, &|a| a.dr),
                cell(&v, &|a| a.oa),
                cell(&v, &|a| a.oa_p002),
                cell(&v, &|a| a.er)
            );
        }
    }
    // The velocity and turn rate the track carries over a long gap.
    let _ = writeln!(
        o,
        "\nSamples more than {FAR_S} s from the last accepted report. Heading errors in degrees (absolute: n, median, p95), turn rates in deg/s"
    );
    let show = |o: &mut String, name: &str, v: &[&Cand]| {
        let ae: Vec<f64> = v.iter().filter_map(|a| a.upd_err).collect();
        let re: Vec<f64> = v.iter().filter_map(|a| a.raw_err).collect();
        let w: Vec<f64> = v
            .iter()
            .map(|a| a.omega)
            .filter(|x| x.is_finite())
            .collect();
        let rr: Vec<f64> = v
            .iter()
            .map(|a| a.ref_rate)
            .filter(|x| x.is_finite())
            .collect();
        let mu: Vec<f64> = v.iter().map(|a| a.mu).filter(|x| x.is_finite()).collect();
        let (e, r, w, rr, m) = (stat(&ae), stat(&re), stat(&w), stat(&rr), stat(&mu));
        let hi = mu.iter().filter(|&&x| x >= 0.5).count();
        let _ = writeln!(
            o,
            "  {:<44} n {:>5} | track heading at last report n {:>5} med {:>5.2} p95 {:>5.2} | raw heading n {:>5} med {:>5.2} p95 {:>5.2} | track turn rate |w| med {:>5.2} p95 {:>5.2} | ref |rate| at sample med {:>5.2} p95 {:>5.2} | mu_turn med {:>5.3} p95 {:>5.3}, share >= 0.5 {:>5.3}",
            name, v.len(), e[0], e[1], e[2], r[0], r[1], r[2], w[1], w[2], rr[1], rr[2], m[1], m[2],
            hi as f64 / v.len().max(1) as f64
        );
    };
    for (cname, class) in CLASSES {
        let v: Vec<&Cand> = ok
            .iter()
            .copied()
            .filter(|a| a.class == class && a.age.is_some_and(|x| x > FAR_S))
            .collect();
        show(o, cname, &v);
        let big: Vec<&Cand> = v.iter().copied().filter(|a| a.oa.abs() > 50.0).collect();
        let small: Vec<&Cand> = v.iter().copied().filter(|a| a.oa.abs() <= 50.0).collect();
        show(o, &format!("{cname}, OpenATCA error over 50 m"), &big);
        show(o, &format!("{cname}, OpenATCA error 50 m or less"), &small);
    }
    // Growth of the error with the age, by what the reference did, fixed bins on every day.
    let edges = [0.0, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 20.0, f64::INFINITY];
    for (cname, class) in [CLASSES[0], CLASSES[1]] {
        let _ = writeln!(
            o,
            "\nError against the age of the last accepted report, {cname}: cross-track m, median p95 and p95/age (m/s). A coast with a heading error grows linearly with the age, a carried turn rate or a turn begun in the gap with its square"
        );
        let _ = writeln!(
            o,
            "  {:<10} {:>6} {:>8} | OpenATCA med  p95  p95/age | dead reckoning med  p95  p95/age | ERAM med  p95",
            "age s", "n", "mean age"
        );
        let mut fit: Vec<FitPoint> = Vec::new();
        for w in edges.windows(2) {
            let (lo, hi) = match w {
                [a, b] => (*a, *b),
                _ => continue,
            };
            let v: Vec<&Cand> = ok
                .iter()
                .copied()
                .filter(|a| a.class == class && a.age.is_some_and(|x| x >= lo && x < hi))
                .collect();
            if v.is_empty() {
                continue;
            }
            let mean = v.iter().filter_map(|a| a.age).sum::<f64>() / v.len() as f64;
            let oa: Vec<f64> = v.iter().map(|a| a.oa).collect();
            let dr: Vec<f64> = v.iter().map(|a| a.dr).collect();
            let er: Vec<f64> = v.iter().map(|a| a.er).collect();
            let (a, d, e) = (stat(&oa), stat(&dr), stat(&er));
            let _ = writeln!(
                o,
                "  {:<10} {:>6} {:>8.2} | {:>12.1} {:>5.1} {:>8.2} | {:>20.1} {:>5.1} {:>8.2} | {:>8.1} {:>5.1}",
                format!("{lo}-{hi}"),
                v.len(),
                mean,
                a[1],
                a[2],
                a[2] / mean,
                d[1],
                d[2],
                d[2] / mean,
                e[1],
                e[2]
            );
            if lo >= 3.0 && hi <= 20.0 {
                fit.push((mean, a[1], a[2], d[2]));
            }
        }
        // Log-log slope of the error against the mean age over the bins 3-4, 4-6, 6-8, 8-12 and
        // 12-20 s (the same bins on every day).
        let slope = |pick: &dyn Fn(&FitPoint) -> f64| -> f64 {
            let xs: Vec<f64> = fit.iter().map(|f| f.0.ln()).collect();
            let ys: Vec<f64> = fit.iter().map(|f| pick(f).ln()).collect();
            let n = xs.len() as f64;
            let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
            let num: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
            let den: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
            num / den
        };
        if fit.len() == 5 {
            let _ = writeln!(
                o,
                "  log-log slope over the bins 3-4, 4-6, 6-8, 8-12, 12-20 s: OpenATCA median {:.2}, OpenATCA p95 {:.2}, dead reckoning p95 {:.2} (1 is linear, 2 is quadratic)",
                slope(&|f| f.1),
                slope(&|f| f.2),
                slope(&|f| f.3)
            );
        } else {
            let _ = writeln!(
                o,
                "  slope not computed: only {} of the 5 bins are present",
                fit.len()
            );
        }
    }
    // Why the dead-reckoning median in cruise is high at 5-8 s and lower beyond: by what the
    // reference did, and the flights that make it.
    let _ = writeln!(
        o,
        "\nCruise, dead reckoning against OpenATCA by what the reference did in the gap: n, median p95 (DR | OA)"
    );
    for (name, lo, hi) in [ages[1], ages[2]] {
        for (cname, class) in CLASSES {
            let v: Vec<&Cand> = ok
                .iter()
                .copied()
                .filter(|a| {
                    a.flight == "cruise"
                        && a.class == class
                        && a.age.is_some_and(|x| x >= lo && x < hi)
                })
                .collect();
            let (d, a) = (
                stat(&v.iter().map(|c| c.dr).collect::<Vec<_>>()),
                stat(&v.iter().map(|c| c.oa).collect::<Vec<_>>()),
            );
            let _ = writeln!(
                o,
                "  cruise, {name}, {cname:<22} n {:>4} | DR {:>6.1} {:>6.1} | OA {:>6.1} {:>6.1}",
                v.len(),
                d[1],
                d[2],
                a[1],
                a[2]
            );
        }
        let v: Vec<&Cand> = ok
            .iter()
            .copied()
            .filter(|a| a.flight == "cruise" && a.age.is_some_and(|x| x >= lo && x < hi))
            .collect();
        let mut by: BTreeMap<u32, Vec<&Cand>> = BTreeMap::new();
        for c in &v {
            by.entry(c.addr).or_default().push(c);
        }
        let mut order: Vec<(&u32, &Vec<&Cand>)> = by.iter().collect();
        order.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        let top3: usize = order.iter().take(3).map(|x| x.1.len()).sum();
        let _ = writeln!(
            o,
            "  cruise, {name}: {} samples from {} flights; the top three flights hold {top3}; flights ranked by samples (labels are ranks):",
            v.len(),
            order.len()
        );
        for (i, (_, cs)) in order.iter().take(6).enumerate() {
            let n = |cl: Class| cs.iter().filter(|c| c.class == cl).count();
            let mean = cs.iter().filter_map(|c| c.age).sum::<f64>() / cs.len() as f64;
            let (d, a, e) = (
                stat(&cs.iter().map(|c| c.dr).collect::<Vec<_>>()),
                stat(&cs.iter().map(|c| c.oa).collect::<Vec<_>>()),
                stat(&cs.iter().map(|c| c.er).collect::<Vec<_>>()),
            );
            let _ = writeln!(
                o,
                "    flight {}: n {:>3} (level {}, turned {}, unknown {}), mean age {:>5.2} s | DR median {:>6.1} | OA median {:>6.1} | ERAM median {:>6.1}",
                i + 1,
                cs.len(),
                n(Class::Level),
                n(Class::Turned),
                n(Class::Unknown),
                mean,
                d[1],
                a[1],
                e[1]
            );
        }
    }
}

/// A bin of the growth fit: mean age, OpenATCA median, OpenATCA p95, dead-reckoning p95.
type FitPoint = (f64, f64, f64, f64);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // REQ: OA-TRK-402
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

    // REQ: OA-TRK-402
    #[test]
    fn a_value_goes_to_the_bin_whose_lower_edge_it_reaches() {
        assert_eq!(bin_of(&RATE_EDGES, 0.0), 0);
        assert_eq!(bin_of(&RATE_EDGES, 0.25), 1);
        assert_eq!(bin_of(&RATE_EDGES, 2.99), 3);
        assert_eq!(bin_of(&RATE_EDGES, 3.0), 4);
    }

    // REQ: OA-TRK-402
    #[test]
    fn the_age_and_gap_are_those_of_the_last_report_at_or_before_the_time() {
        let t = [1000, 4000, 9000];
        assert_eq!(age_gap(&t, 4000), (Some(0.0), Some(5.0)));
        assert_eq!(age_gap(&t, 6500), (Some(2.5), Some(5.0)));
        assert_eq!(age_gap(&t, 500), (None, None));
        assert_eq!(age_gap(&t, 9500), (Some(0.5), None));
    }

    // REQ: OA-TRK-402
    #[test]
    fn heading_lag_proxy_handles_wrap_sign_and_non_turning_inputs() {
        assert_eq!(heading_lag_s(3.0, 0.0, 1.5), Some(2.0));
        assert_eq!(heading_lag_s(0.0, 3.0, 1.5), Some(-2.0));
        assert_eq!(heading_lag_s(1.0, 359.0, 1.0), Some(2.0));
        assert_eq!(heading_lag_s(0.0, 1.0, 0.0), None);
        assert_eq!(heading_lag_s(0.0, 1.0, TURN_DPS - f64::EPSILON), None);
        assert_eq!(heading_lag_s(f64::NAN, 1.0, 1.0), None);
    }

    // REQ: OA-TRK-402
    #[test]
    fn age_turn_strata_separate_stale_turning_from_stale_straight_samples() {
        let stale_turning = age_bin(Some(12.0)) * 3 + turn_bin(Some(1.5));
        let stale_straight = age_bin(Some(12.0)) * 3 + turn_bin(Some(0.0));
        assert_eq!(stale_turning, stale_straight - 1);
        assert_eq!(age_bin(Some(0.5)), 0);
        assert_eq!(turn_bin(None), 2);

        let mut cells: Vec<RateCell> = (0..18).map(|_| RateCell::default()).collect();
        let turning_row = Row {
            mu_turn: 0.8,
            omega_dps: 1.0,
            lat: 0.0,
            lon: 0.0,
            ve: 0.0,
            vn: 0.0,
        };
        let straight_row = Row {
            mu_turn: 0.1,
            omega_dps: 0.0,
            lat: 0.0,
            lon: 0.0,
            ve: 0.0,
            vn: 0.0,
        };
        record_rate_sample(&mut cells[stale_turning], Some(1.5), Some(turning_row));
        record_rate_sample(&mut cells[stale_straight], Some(0.0), Some(straight_row));
        record_rate_sample(&mut cells[stale_turning], Some(1.5), None);
        record_rate_sample(
            &mut cells[stale_turning],
            Some(1.5),
            Some(Row {
                omega_dps: f64::NAN,
                ..turning_row
            }),
        );
        let turning = &cells[stale_turning];
        let straight = &cells[stale_straight];
        assert_eq!((turning.samples, turning.matched), (3, 1));
        assert_eq!((straight.samples, straight.matched), (1, 1));
        assert_eq!(stat(&turning.absolute_error_dps)[2], 0.5);
        assert_eq!(stat(&straight.absolute_error_dps)[2], 0.0);
    }

    // REQ: OA-TRK-402
    #[test]
    fn only_common_samples_with_finite_errors_are_read_from_the_p002_csv() {
        let mut csv = String::from(
            "# DEVELOPMENT RUN\naddr,t_ms,phase,band,common,oa_miss,oa_cross_m,oa_track_err_deg,oa_row_dt_ms,eram_miss,eram_cross_m,eram_track_err_deg\n",
        );
        csv.push_str("abc123,1000,climb,below,1,0,-3.5,0.4,20,0,7.0,0.2\n");
        csv.push_str("abc124,1000,climb,below,0,0,1.0,0.1,0,1,,\n");
        csv.push_str("abc125,2000,climb,below,1,0,2.0,,0,0,5.0,0.3\n");
        let v = parse_samples(&csv).unwrap();
        assert_eq!(v.len(), 1);
        let s = v.first().unwrap();
        assert_eq!((s.addr, s.t_ms), (0xabc123, 1020));
        assert_eq!((s.oa_cross, s.er_cross), (-3.5, 7.0));
        assert_eq!(s.flight, "climb");
        assert!(parse_samples("addr,t_ms\n").is_err());
    }

    // REQ: OA-TRK-402
    #[test]
    fn the_cross_track_sign_and_size_hold_for_east_north_and_oblique_headings() {
        // Positive is to the right of the direction of travel. WGS-84 radii at 45 N.
        let (m, n) = radii(45.0);
        let north_deg = |d: f64| (d / m).to_degrees();
        let east_deg = |d: f64| (d / (n * 45f64.to_radians().cos())).to_degrees();
        // East at 200 m/s: 100 m north is on the left (-100), 100 m south on the right (+100).
        let r = [45.0, 10.0, 200.0, 0.0];
        assert!((cross_of(45.0 + north_deg(100.0), 10.0, &r).unwrap() + 100.0).abs() < 1e-6);
        assert!((cross_of(45.0 - north_deg(100.0), 10.0, &r).unwrap() - 100.0).abs() < 1e-6);
        assert!(cross_of(45.0, 10.0 + east_deg(5000.0), &r).unwrap().abs() < 1e-6);
        // North: 100 m east is on the right.
        let r = [45.0, 10.0, 0.0, 200.0];
        assert!((cross_of(45.0, 10.0 + east_deg(100.0), &r).unwrap() - 100.0).abs() < 1e-6);
        assert!(cross_of(45.0 + north_deg(5000.0), 10.0, &r).unwrap().abs() < 1e-6);
        // Heading 045 (north-east): a point 100 m east and 100 m north of it is on the line; one
        // 100 m east and 100 m south is 100 sqrt 2 on the right.
        let r = [45.0, 10.0, 150.0, 150.0];
        assert!(
            cross_of(45.0 + north_deg(100.0), 10.0 + east_deg(100.0), &r)
                .unwrap()
                .abs()
                < 1e-6
        );
        let off = cross_of(45.0 - north_deg(100.0), 10.0 + east_deg(100.0), &r).unwrap();
        assert!((off - 200.0 / 2f64.sqrt()).abs() < 1e-6, "{off}");
        assert!(cross_of(45.0, 10.0, &[45.0, 10.0, 10.0, 0.0]).is_none());
        assert!(cross_of(f64::NAN, 10.0, &[45.0, 10.0, 200.0, 0.0]).is_none());
        assert!(cross_of(45.0, 10.0, &[45.0, 10.0, f64::NAN, 0.0]).is_none());
        assert_eq!(wrap_deg(190.0), -170.0);
        assert_eq!(wrap_deg(-190.0), 170.0);
        let t = [1000, 2000, 5000];
        assert_eq!(last_report(&t, 1999), Some(1000));
        assert_eq!(last_report(&t, 5000), Some(5000));
        assert_eq!(last_report(&t, 999), None);
    }

    // REQ: OA-TRK-402
    #[test]
    fn dead_reckoning_steps_the_velocity_times_the_age_from_the_last_accepted_report() {
        let (m, n) = radii(60.0);
        assert!((m - 6_383_453.86).abs() < 1.0 && (n - 6_394_209.17).abs() < 1.0);
        // North-east at 100 m/s each way, 20 s on from the report: 2 km each way.
        let raw = Raw {
            lat: 60.0,
            lon: 5.0,
            ve: 100.0,
            vn: 100.0,
        };
        let (lat, lon) = dead_reckon(&raw, 20.0);
        assert!(((lat - 60.0).to_radians() * m - 2000.0).abs() < 1e-6);
        assert!(((lon - 5.0).to_radians() * n * 60f64.to_radians().cos() - 2000.0).abs() < 1e-6);
        // The reference flies that line: the dead-reckoned point is on it.
        let r = [60.0, 5.0, 100.0, 100.0];
        assert!(cross_of(lat, lon, &r).unwrap().abs() < 1e-6);
        // The last report used is the last accepted one, not the last offered.
        let offered: BTreeMap<u32, Vec<i64>> = BTreeMap::from([(1, vec![1000, 2000, 3000])]);
        let accepted: BTreeMap<u32, Vec<i64>> = BTreeMap::from([(1, vec![1000, 2000])]);
        let samples = vec![Sample {
            addr: 1,
            t_ms: 6000,
            oa_cross: 0.0,
            oa_hdg: 0.0,
            er_cross: 0.0,
            er_hdg: 0.0,
            band: String::new(),
            flight: "cruise".to_string(),
        }];
        let rows = BTreeMap::new();
        let upd = BTreeMap::new();
        let raws = BTreeMap::from([(
            (1u32, 2000i64),
            Raw {
                lat: 60.0,
                lon: 5.0,
                ve: 0.0,
                vn: 100.0,
            },
        )]);
        let truth = |_: u32, _: i64| Some([60.0, 5.0, 0.0, 100.0]);
        let rate = |_: u32, _: i64| Some(0.0);
        let conflicts = BTreeSet::new();
        let g = GapIn {
            samples: &samples,
            rows: &rows,
            upd: &upd,
            raw: &raws,
            raw_conflicts: &conflicts,
            row_conflicts: &conflicts,
            update_conflicts: &conflicts,
            offered: &offered,
            accepted: &accepted,
            truth: &truth,
            rate: &rate,
        };
        let (c, differs) = candidates(&g);
        assert_eq!(differs, 1);
        let c = c.first().unwrap();
        assert_eq!(c.age, Some(4.0));
        assert!(c.class == Class::Level);
        // The reference has no tick row here: dropped for that reason, after the DR was computed.
        assert_eq!(c.drop, Some(6));

        let invalid_raws = BTreeMap::from([(
            (1u32, 2000i64),
            Raw {
                lat: f64::NAN,
                lon: 5.0,
                ve: 0.0,
                vn: 100.0,
            },
        )]);
        let invalid = GapIn {
            raw: &invalid_raws,
            ..g
        };
        let (invalid_rows, _) = candidates(&invalid);
        assert_eq!(invalid_rows.first().and_then(|x| x.drop), Some(4));
    }

    // REQ: OA-TRK-402
    #[test]
    fn conflicting_duplicate_keys_are_removed_independent_of_arrival_order() {
        let a = Row {
            mu_turn: 0.2,
            omega_dps: 0.1,
            lat: 1.0,
            lon: 2.0,
            ve: 3.0,
            vn: 4.0,
        };
        let b = Row {
            mu_turn: 0.8,
            omega_dps: 0.9,
            lat: 5.0,
            lon: 6.0,
            ve: 7.0,
            vn: 8.0,
        };
        let key = (1u32, 2i64);
        let build = |values: [Row; 2]| {
            let mut rows = BTreeMap::new();
            let mut conflicts = BTreeSet::new();
            for row in values {
                insert_unambiguous(&mut rows, &mut conflicts, key, row);
            }
            (rows, conflicts)
        };
        let (forward, forward_conflicts) = build([a, b]);
        let (reverse, reverse_conflicts) = build([b, a]);
        assert!(forward.is_empty() && reverse.is_empty());
        assert_eq!(forward_conflicts, reverse_conflicts);
        assert!(forward_conflicts.contains(&key));
    }

    // REQ: OA-TRK-402
    #[test]
    fn sample_timestamp_overflow_and_duplicate_keys_are_refused() {
        let header = "addr,t_ms,phase,band,common,oa_miss,oa_cross_m,oa_track_err_deg,oa_row_dt_ms,eram_miss,eram_cross_m,eram_track_err_deg\n";
        let overflow = format!("{header}abc123,{},cruise,below,1,0,1,1,1,0,1,1\n", i64::MAX);
        assert!(parse_samples(&overflow).is_err());
        let duplicate = format!("{header}abc123,1000,cruise,below,1,0,1,1,0,0,1,1\nabc123,1000,cruise,below,1,0,1,1,0,0,1,1\n");
        assert!(parse_samples(&duplicate).is_err());
    }

    // REQ: OA-TRK-402
    #[test]
    fn distinct_p002_grid_rows_may_reuse_one_oa_lookup_key() {
        let csv = concat!(
            "addr,t_ms,phase,band,common,oa_miss,oa_cross_m,oa_track_err_deg,oa_row_dt_ms,eram_miss,eram_cross_m,eram_track_err_deg\n",
            "abc123,1000,cruise,below,1,0,1,1,20,0,1,1\n",
            "abc123,1010,cruise,below,1,0,2,2,10,0,2,2\n",
        );
        let samples = parse_samples(csv).unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!((samples[0].t_ms, samples[1].t_ms), (1020, 1020));
    }
}
