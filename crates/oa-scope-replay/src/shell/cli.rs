//! The `oa-scope-replay` command line (**shell**, B1, B3): reads a snapshot and an ADS-B clip,
//! runs the tracker over the window asked for, and writes one PPM per step and a hashes file.
//! Every file access and clock read of the tool is here; every function in this module is shell.
//!
//! ```text
//! oa-scope-replay --snapshot <file> --adsb <clip.oaf> --centre <lat,lon> --range-nm <n>
//!     --size <WxH> --start <unix-seconds|YYYY-MM-DDTHH:MM:SSZ> --duration-s <n> --step-s <n>
//!     --out-dir <dir> [--leader-min <n>] [--trail-n <n>] [--trail-step-s <n>] [--reacq-s <n>]
//!     [--max-targets <n>] [--warmup-s <n>] [--hashes-only] [--overwrite] [--allow-loss]
//! ```
//!
//! Frame `i` shows the tracker state at `start + i * step`, for `i` from 0 while the time is
//! before `start + duration`. `hashes.txt` has one line per frame: `frame_index time sha256`, the
//! time in Unix seconds with milliseconds, the hash that of the visible pixels. The tracker is
//! started `warmup` before the start so tracks are confirmed and settled when the first frame is
//! drawn.
//!
//! # What the run says it left out, and when it fails (E6, G6)
//!
//! The run ends with a report of everything it excluded or lost, each under a named reason and
//! with its denominator, and the loss line names every loss reason on its own. Two classes:
//!
//! * **Shown or by design**, never a failure, because the system did what it is for or the
//!   screen says so: reports removed by the ADS-B quality gates, records the decoder dropped as
//!   invalid and measurements the tracker refused as invalid (input that is wrong is refused and
//!   counted by reason), jump-rejected measurements, clip frames outside the window, status
//!   frames, tracker rows before the window, tracks that ended, targets off the map, empty
//!   callsigns, callsign conflicts (shown as `CS?`), clamped or unknown values (shown, and
//!   altitudes beyond the data block as `999+` or `-99-`).
//! * **Loss**, work the run was asked to do and did not: damaged clip frames, undecodable
//!   payloads, reject frames (messages the bridge refused, so input that never arrived),
//!   tracker output rows that overflowed, tracker measurements refused for capacity or a
//!   numerical failure (valid input not used), stored rows refused, targets the display refused
//!   (full or malformed), positions off the globe or that do not project, and every part of a
//!   target that could not be drawn.
//!
//! Any loss makes the run exit with code 3, after the pictures and the report are written, unless
//! `--allow-loss` is given (the report still says what was lost).
//!
//! Exit codes (see the binary): 0 success, 1 a failure (a file, a clip, the tracker), 2 a usage
//! error, 3 work was lost (the output is written). A `--duration-s` that is not a whole number of
//! `--step-s` steps is a usage error.
//!
//! `--out-dir` must be empty or new unless `--overwrite` is given, which deletes only the files
//! this tool writes there: `frame-NNNN.ppm` and `hashes.txt`.

use crate::adapt::{callsigns_from_reports, row_columns, ColumnBuf};
use crate::frame::{replay_build, replay_new, replay_raster, ReplayConfig, ReplayError};
use crate::rows::{
    convert_check, store_addresses, store_loss, store_rows, store_stats_add, CallsignStats,
    Callsigns, ConvertConfig, RowStore, StoreStats, DEFAULT_CONVERT, MAX_CALLSIGN_REPORTS,
    NS_PER_S,
};
use crate::store::{store_add, store_finish};
use oa_adsb::{decode_payload_lenient, reports_len, Reports};
use oa_display::cli::{parse_centre, parse_size};
use oa_display::project::{MAX_RANGE_NM, MIN_RANGE_NM};
use oa_display::raster::frame_digest;
use oa_display::traffic::{TrafficStyle, DEFAULT_STYLE, MAX_LEADER_MIN, MAX_TARGETS, MAX_TRAIL};
use oa_sha256::to_hex;
use oa_track::adsb::{adsb_measurements, AdsbConfig, AdsbStats};
use oa_track::score::{interval_stats, scoring_config};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_new, tracker_stats,
    tracker_update, DEFAULT_CONFIG, REFUSE_KINDS, REFUSE_NAMES,
};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Why a report was removed by the ADS-B quality gates, indexing `AdsbStats::removed`. These are
/// the names of `oa_truth::REMOVE_NAMES`, kept here so that this crate does not link `oa-truth`;
/// the test `removal_names_match_oa_truth` holds them equal to it.
pub const REMOVE_NAMES: [&str; 13] = [
    "invalid",
    "stale_flag",
    "source_not_adsb",
    "no_quality_in_age",
    "nacp",
    "nic",
    "sil",
    "version",
    "duplicate_time",
    "jump",
    "short_leg",
    "oversized_same_time_group",
    "ambiguous_same_time_altitude",
];

/// Most frames one run writes.
pub const MAX_FRAMES: u64 = 10_000;
/// Shortest step, ns (0.1 s).
pub const MIN_STEP_NS: i64 = 100_000_000;
/// Default targets per frame.
pub const DEFAULT_MAX_TARGETS: usize = 1024;
/// Default tracker warm-up before the first frame, ns (180 s).
pub const DEFAULT_WARMUP_NS: i64 = 180 * NS_PER_S;
/// Most reports the clip window may hold in memory. Analysis: 4096 aircraft at about two reports
/// a second for about 4000 s is 3.3 * 10^7, which is 2^25; at about 80 bytes a report that is
/// 2.7 GB, the largest window this tool is sized for. Checked per payload while reading, so the
/// store never grows past the limit by more than one payload.
pub const MAX_WINDOW_REPORTS: usize = MAX_CALLSIGN_REPORTS;
/// Frames of the clip are read from this long before the warm-up start to this long after the
/// last frame, so no report near the edges is lost to the frame's own receive time.
const READ_MARGIN_NS: i64 = 2 * NS_PER_S;
const DRAIN_ROWS: usize = 1 << 15;

const USAGE: &str = "usage: oa-scope-replay --snapshot <file> --adsb <clip.oaf> \
--centre <lat,lon> --range-nm <n> --size <WxH> --start <unix-seconds|YYYY-MM-DDTHH:MM:SSZ> \
--duration-s <n> --step-s <n> --out-dir <dir> [--leader-min <n>] [--trail-n <n>] \
[--trail-step-s <n>] [--reacq-s <n>] [--max-targets <n>] [--warmup-s <n>] [--hashes-only] \
[--overwrite] [--allow-loss]";

// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Failed(String),
    /// The run finished and wrote its output, but lost work (see the module header).
    Loss(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n{USAGE}"),
            CliError::Failed(m) => write!(f, "{m}"),
            CliError::Loss(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CliError {}

fn failed(what: &str, e: impl fmt::Display) -> CliError {
    CliError::Failed(format!("{what}: {e}"))
}

impl From<ReplayError> for CliError {
    fn from(e: ReplayError) -> Self {
        failed("frame", e)
    }
}

/// The parsed arguments.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayArgs {
    pub snapshot: PathBuf,
    pub adsb: PathBuf,
    pub centre_lat_e7: i32,
    pub centre_lon_e7: i32,
    pub range_nm: f64,
    pub width: u32,
    pub height: u32,
    pub start_ns: i64,
    pub step_ns: i64,
    pub frames: u64,
    pub out_dir: PathBuf,
    pub style: TrafficStyle,
    pub convert: ConvertConfig,
    pub max_targets: usize,
    pub warmup_ns: i64,
    pub hashes_only: bool,
    /// Delete the files this tool writes from a non-empty `--out-dir`.
    pub overwrite: bool,
    /// Exit zero although work was lost (the report still says so).
    pub allow_loss: bool,
}

fn parse_finite(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Decimal seconds as ns by integer arithmetic: digits, then optionally `.` and one to nine
/// digits. No sign, exponent, `inf` or `nan`. `None` for anything else or for a value that does
/// not fit an `i64` of ns (E8).
fn parse_decimal_ns(text: &str) -> Option<i64> {
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) if !f.is_empty() => (w, f),
        Some(_) => return None,
        None => (text, ""),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs: i64 = whole.parse().ok()?;
    secs.checked_mul(NS_PER_S)?.checked_add(frac_ns(frac)?)
}

/// The ns of the digits after a decimal point, one to nine of them (none is zero).
fn frac_ns(frac: &str) -> Option<i64> {
    if frac.len() > 9 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut ns = 0i64;
    for b in frac.bytes() {
        ns = ns * 10 + i64::from(b - b'0');
    }
    for _ in frac.len()..9 {
        ns *= 10;
    }
    Some(ns)
}

/// Seconds as ns, for a number of at most a day.
fn parse_seconds(flag: &str, text: &str) -> Result<i64, CliError> {
    match parse_decimal_ns(text) {
        Some(ns) if ns <= 86_400 * NS_PER_S => Ok(ns),
        _ => Err(CliError::Usage(format!(
            "{flag} {text:?} is not a number of seconds from 0 to 86400 (digits, and a point with up to nine more)"
        ))),
    }
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// First and last year an ISO start time may name. An `i64` of nanoseconds since 1970 ends in
/// April 2262, so 2261 is the last whole year; a year before 1970 is a negative time, which no
/// clip has.
pub const MIN_YEAR: i64 = 1970;
pub const MAX_YEAR: i64 = 2261;

/// A fixed-width run of digits as a number.
fn digits(text: Option<&str>, width: usize) -> Option<i64> {
    let t = text?;
    if t.len() != width || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

/// A start time: Unix seconds (digits, and a point with up to nine more), or
/// `YYYY-MM-DDTHH:MM:SS[.fffffffff]Z` with the fields of those widths, a year from [`MIN_YEAR`] to
/// [`MAX_YEAR`], a day that the month has (leap years included), and no leap second. Unix ns.
/// All of it is integer arithmetic; a time that does not fit an `i64` of ns is refused.
pub fn parse_time(text: &str) -> Result<i64, CliError> {
    let bad = || {
        CliError::Usage(format!(
            "--start {text:?} is not Unix seconds or YYYY-MM-DDTHH:MM:SSZ (years {MIN_YEAR} to {MAX_YEAR}, a day the month has)"
        ))
    };
    if !text.contains('T') {
        return parse_decimal_ns(text).ok_or_else(bad);
    }
    let body = text.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = body.split_once('T').ok_or_else(bad)?;
    let mut d = date.split('-');
    let (y, mo, da) = (
        digits(d.next(), 4).ok_or_else(bad)?,
        digits(d.next(), 2).ok_or_else(bad)?,
        digits(d.next(), 2).ok_or_else(bad)?,
    );
    let mut t = time.split(':');
    let (h, mi) = (
        digits(t.next(), 2).ok_or_else(bad)?,
        digits(t.next(), 2).ok_or_else(bad)?,
    );
    let sec_text = t.next().ok_or_else(bad)?;
    let (sec_whole, frac) = sec_text.split_once('.').unwrap_or((sec_text, ""));
    if sec_text.contains('.') && frac.is_empty() {
        return Err(bad());
    }
    let sec = digits(Some(sec_whole), 2).ok_or_else(bad)?;
    let frac = frac_ns(frac).ok_or_else(bad)?;
    if d.next().is_some()
        || t.next().is_some()
        || !(MIN_YEAR..=MAX_YEAR).contains(&y)
        || !(1..=12).contains(&mo)
        || !(1..=days_in_month(y, mo)).contains(&da)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0..60).contains(&sec)
    {
        return Err(bad());
    }
    let secs = days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + sec;
    secs.checked_mul(NS_PER_S)
        .and_then(|ns| ns.checked_add(frac))
        .ok_or_else(bad)
}

fn set_once<T>(slot: &mut Option<T>, flag: &str, v: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::Usage(format!("{flag} given more than once")));
    }
    *slot = Some(v);
    Ok(())
}

fn parse_count(flag: &str, text: &str, max: u64) -> Result<u64, CliError> {
    match text.parse::<u64>() {
        Ok(v) if v <= max => Ok(v),
        _ => Err(CliError::Usage(format!(
            "{flag} {text:?} is not a whole number from 0 to {max}"
        ))),
    }
}

/// Parses the arguments.
pub fn parse_args(args: &[String]) -> Result<ReplayArgs, CliError> {
    let (mut snapshot, mut adsb, mut centre, mut range, mut size) = (None, None, None, None, None);
    let (mut start, mut duration, mut step, mut out_dir) = (None, None, None, None);
    let (mut leader, mut trail_n, mut trail_step, mut reacq) = (None, None, None, None);
    let (mut max_targets, mut warmup) = (None, None);
    let (mut hashes_only, mut overwrite, mut allow_loss) = (false, false, false);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| CliError::Usage(format!("missing value for {flag}")))
        };
        match flag.as_str() {
            "--snapshot" => set_once(&mut snapshot, flag, PathBuf::from(value()?))?,
            "--adsb" => set_once(&mut adsb, flag, PathBuf::from(value()?))?,
            "--centre" => set_once(&mut centre, flag, parse_centre(value()?).map_err(usage)?)?,
            "--range-nm" => {
                let text = value()?;
                match parse_finite(text) {
                    Some(v) if (MIN_RANGE_NM..=MAX_RANGE_NM).contains(&v) => {
                        set_once(&mut range, flag, v)?;
                    }
                    _ => {
                        return Err(CliError::Usage(format!(
                        "--range-nm {text:?} is not a number from {MIN_RANGE_NM} to {MAX_RANGE_NM}"
                    )))
                    }
                }
            }
            "--size" => set_once(&mut size, flag, parse_size(value()?).map_err(usage)?)?,
            "--start" => set_once(&mut start, flag, parse_time(value()?)?)?,
            "--duration-s" => set_once(&mut duration, flag, parse_seconds(flag, value()?)?)?,
            "--step-s" => set_once(&mut step, flag, parse_seconds(flag, value()?)?)?,
            "--out-dir" => set_once(&mut out_dir, flag, PathBuf::from(value()?))?,
            "--leader-min" => {
                let text = value()?;
                match parse_finite(text) {
                    Some(v) if (0.0..=MAX_LEADER_MIN).contains(&v) => {
                        set_once(&mut leader, flag, v)?;
                    }
                    _ => {
                        return Err(CliError::Usage(format!(
                            "--leader-min {text:?} is not a number from 0 to {MAX_LEADER_MIN}"
                        )))
                    }
                }
            }
            "--trail-n" => {
                let n = parse_count(flag, value()?, MAX_TRAIL as u64)?;
                set_once(&mut trail_n, flag, n as u8)?;
            }
            "--trail-step-s" => set_once(&mut trail_step, flag, parse_seconds(flag, value()?)?)?,
            "--reacq-s" => {
                let n = parse_count(flag, value()?, 3600)?;
                set_once(&mut reacq, flag, n as u16)?;
            }
            "--max-targets" => {
                let n = parse_count(flag, value()?, MAX_TARGETS as u64)?;
                set_once(&mut max_targets, flag, n as usize)?;
            }
            "--warmup-s" => set_once(&mut warmup, flag, parse_seconds(flag, value()?)?)?,
            "--hashes-only" => hashes_only = true,
            "--overwrite" => overwrite = true,
            "--allow-loss" => allow_loss = true,
            other => return Err(CliError::Usage(format!("unknown argument {other:?}"))),
        }
    }
    let need = |what: &str| CliError::Usage(format!("missing {what}"));
    let (centre_lat_e7, centre_lon_e7) = centre.ok_or_else(|| need("--centre"))?;
    let (width, height) = size.ok_or_else(|| need("--size"))?;
    let step_ns = step.ok_or_else(|| need("--step-s"))?;
    let duration_ns = duration.ok_or_else(|| need("--duration-s"))?;
    if step_ns < MIN_STEP_NS {
        return Err(CliError::Usage(format!(
            "--step-s is below the shortest step of {} s",
            MIN_STEP_NS as f64 / 1e9
        )));
    }
    if duration_ns % step_ns != 0 {
        return Err(CliError::Usage(
            "--duration-s is not a whole number of --step-s steps".into(),
        ));
    }
    let frames = u64::try_from(duration_ns / step_ns).unwrap_or(0);
    if frames == 0 || frames > MAX_FRAMES {
        return Err(CliError::Usage(format!(
            "the duration and step give {frames} frames; the limit is 1 to {MAX_FRAMES}"
        )));
    }
    let start_ns = start.ok_or_else(|| need("--start"))?;
    let warmup_ns = warmup.unwrap_or(DEFAULT_WARMUP_NS);
    // Every time the run computes from these is checked here, once, so the run itself only has to
    // carry explicit errors for what it adds.
    let span = i64::try_from(frames)
        .ok()
        .and_then(|f| f.checked_mul(step_ns))
        .and_then(|s| start_ns.checked_add(s))
        .and_then(|e| e.checked_add(READ_MARGIN_NS));
    let from = start_ns
        .checked_sub(warmup_ns)
        .and_then(|f| f.checked_sub(READ_MARGIN_NS));
    if span.is_none() || from.is_none() {
        return Err(CliError::Usage(
            "the start, duration and warm-up give a time outside the range of the clock".into(),
        ));
    }
    let style = TrafficStyle {
        leader_min: leader.unwrap_or(DEFAULT_STYLE.leader_min),
        trail_n: trail_n.unwrap_or(DEFAULT_STYLE.trail_n),
        reacquired_s: reacq.unwrap_or(DEFAULT_STYLE.reacquired_s),
    };
    let convert = ConvertConfig {
        trail_n: style.trail_n,
        trail_step_ns: trail_step.unwrap_or(DEFAULT_CONVERT.trail_step_ns),
        stale_ns: DEFAULT_CONVERT.stale_ns,
        reacq_window_s: style.reacquired_s,
    };
    convert_check(&convert).map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(ReplayArgs {
        snapshot: snapshot.ok_or_else(|| need("--snapshot"))?,
        adsb: adsb.ok_or_else(|| need("--adsb"))?,
        centre_lat_e7,
        centre_lon_e7,
        range_nm: range.ok_or_else(|| need("--range-nm"))?,
        width,
        height,
        start_ns,
        step_ns,
        frames,
        out_dir: out_dir.ok_or_else(|| need("--out-dir"))?,
        style,
        convert,
        max_targets: max_targets.unwrap_or(DEFAULT_MAX_TARGETS),
        warmup_ns,
        hashes_only,
        overwrite,
        allow_loss,
    })
}

/// A usage error from the display crate's parsers, passed on as it is.
fn usage(e: oa_display::cli::CliError) -> CliError {
    match e {
        oa_display::cli::CliError::Usage(m) => CliError::Usage(m),
        oa_display::cli::CliError::Failed(m) => CliError::Failed(m),
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`, with `.mmm` when the time is not a whole second.
pub fn stamp_of(t_ns: i64) -> String {
    let secs = t_ns.div_euclid(NS_PER_S);
    let ms = t_ns.rem_euclid(NS_PER_S) / 1_000_000;
    match oa_feed::record::utc_iso(secs) {
        Some(iso) if ms == 0 => iso,
        Some(iso) => format!("{}.{ms:03}Z", iso.trim_end_matches('Z')),
        None => format!("{secs}.{ms:03}"),
    }
}

/// Unix seconds with milliseconds, the time column of the hashes file.
pub fn unix_text(t_ns: i64) -> String {
    format!(
        "{}.{:03}",
        t_ns.div_euclid(NS_PER_S),
        t_ns.rem_euclid(NS_PER_S) / 1_000_000
    )
}

/// What the clip decoder did over the payloads in the window, summed.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeTotals {
    pub records: u64,
    pub kept: u64,
    /// Records dropped as invalid (position or flags out of range).
    pub dropped: u64,
    /// Of the dropped, those whose time was out of range.
    pub bad_time: u64,
    pub unknown_source: u64,
    pub bad_measurement: u64,
    pub reserved_set: u64,
}

/// What the tracker run over the window did, with every count by reason.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackSummary {
    pub reports: usize,
    /// Clip frames (of any kind) outside the window: not read, not a loss.
    pub frames_outside_window: u64,
    /// LOSS: frames the reader refused, and a partial frame at the end.
    pub damaged_frames: u64,
    /// LOSS: payloads in the window that could not be decoded at all.
    pub undecodable_payloads: u64,
    /// LOSS: reject frames in the window: messages the bridge refused to frame, so input that
    /// never reached the tracker.
    pub reject_frames: u64,
    /// Status frames in the window (connection state changes): counted, not loss, they carry no
    /// aircraft data.
    pub status_frames: u64,
    pub decode: DecodeTotals,
    pub adsb: AdsbStats,
    pub measurements: usize,
    pub store: StoreStats,
    pub callsigns: CallsignStats,
    /// LOSS: addresses whose rows could not be put in time order (cannot happen).
    pub sort_failed: u64,
    pub rows_kept: usize,
    pub addresses: usize,
    /// LOSS: tracker output rows lost because its table was full.
    pub overflow: u64,
    /// Tracks the tracker confirmed.
    pub confirmed: u64,
    /// Measurements the tracker refused as invalid, by reason ([`REFUSE_NAMES`]), and by the jump
    /// rule: the system working, not a loss.
    pub refused: [u64; REFUSE_KINDS],
    pub jump_rejected: u64,
    /// LOSS: measurements refused for want of a track slot, or for a numerical failure.
    pub capacity_refused: u64,
    pub numerical_refused: u64,
}

/// Reads the clip's reports with receive time in `[from_ns, to_ns]`, runs the tracker over them
/// until `end_ns`, and keeps the rows from `keep_from_ns`.
pub fn track_window(
    adsb: &Path,
    from_ns: i64,
    to_ns: i64,
    end_ns: i64,
    keep_from_ns: i64,
) -> Result<(RowStore, Callsigns, TrackSummary), CliError> {
    let mut sum = TrackSummary::default();
    let mut reports = Reports::default();
    let mut file =
        std::fs::File::open(adsb).map_err(|e| failed(&format!("read {}", adsb.display()), e))?;
    let mut window_full = false;
    let mut dec = DecodeTotals::default();
    let (mut outside, mut undecodable) = (0u64, 0u64);
    let (mut rejects, mut statuses) = (0u64, 0u64);
    let damaged = oa_feed::read_frames(&mut file, &mut |fr| {
        if !(from_ns..=to_ns).contains(&fr.recv_unix_ns) {
            outside += 1;
            return;
        }
        match fr.kind {
            oa_feed::Kind::Message => {}
            oa_feed::Kind::Reject => {
                rejects += 1;
                return;
            }
            oa_feed::Kind::Status => {
                statuses += 1;
                return;
            }
        }
        // The window is bounded by a named limit, checked before each payload is added.
        if reports_len(&reports) >= MAX_WINDOW_REPORTS {
            window_full = true;
            return;
        }
        match decode_payload_lenient(fr.payload, &mut reports) {
            Ok(c) => {
                dec.records += c.records as u64;
                dec.kept += c.kept as u64;
                dec.dropped += c.dropped as u64;
                dec.bad_time += c.bad_time as u64;
                dec.unknown_source += c.unknown_source as u64;
                dec.bad_measurement += c.bad_measurement as u64;
                dec.reserved_set += c.reserved_set as u64;
            }
            Err(_) => undecodable += 1,
        }
    })
    .map_err(|e| failed(&format!("read {}", adsb.display()), e))?;
    if window_full {
        return Err(CliError::Failed(format!(
            "{} has more than {MAX_WINDOW_REPORTS} reports in the window; narrow the window",
            adsb.display()
        )));
    }
    let n = reports_len(&reports);
    if n == 0 {
        return Err(CliError::Failed(format!(
            "{} has no reports in the window",
            adsb.display()
        )));
    }
    sum.reports = n;
    sum.frames_outside_window = outside;
    sum.damaged_frames = damaged;
    sum.undecodable_payloads = undecodable;
    sum.reject_frames = rejects;
    sum.status_frames = statuses;
    sum.decode = dec;
    let (calls, call_stats) =
        callsigns_from_reports(&reports).map_err(|e| failed("callsigns", e))?;
    sum.callsigns = call_stats;
    let (meas, adsb_stats) = adsb_measurements(&reports, &AdsbConfig::default());
    sum.adsb = adsb_stats;
    let cfg = match interval_stats(&meas) {
        Some(iv) => scoring_config(&DEFAULT_CONFIG, &iv),
        None => DEFAULT_CONFIG,
    };
    let mut t = tracker_new(&cfg).map_err(|e| failed("tracker", e))?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut store = RowStore::default();
    let mut colbuf = ColumnBuf::default();
    let mut keep = |store: &mut RowStore, out: &oa_track::TrackOutput| -> Result<(), CliError> {
        let st = store_add(store, &row_columns(out, &mut colbuf), keep_from_ns)
            .map_err(|e| failed("tracker rows", e))?;
        store_stats_add(&mut sum.store, &st);
        Ok(())
    };
    for m in &meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            keep(&mut store, &scratch)?;
            output_clear(&mut scratch);
        }
    }
    // Not flushed: a flush would end every track, which the replay is not asked to show.
    tracker_advance(&mut t, end_ns, &mut scratch);
    keep(&mut store, &scratch)?;
    let overflow = scratch.overflow;
    sum.sort_failed = store_finish(&mut store);
    let stats = *tracker_stats(&t);
    sum.measurements = meas.len();
    sum.rows_kept = store_rows(&store);
    sum.addresses = store_addresses(&store);
    sum.overflow = overflow;
    sum.confirmed = stats.confirmed;
    sum.refused = stats.refused;
    sum.jump_rejected = stats.jump_rejected;
    sum.capacity_refused = stats.capacity_refused;
    sum.numerical_refused = stats.numerical_refused;
    Ok((store, calls, sum))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// True for a file name this tool writes: `hashes.txt` or exactly `frame-` four digits `.ppm`
/// (a run writes at most [`MAX_FRAMES`] frames, so an index never needs more digits).
fn is_ours(name: &str) -> bool {
    name == "hashes.txt"
        || name
            .strip_prefix("frame-")
            .and_then(|r| r.strip_suffix(".ppm"))
            .is_some_and(|d| d.len() == 4 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Creates a new file, failing if the name exists in any form (a symlink included, never
/// followed).
fn create_new(path: &Path) -> Result<std::fs::File, CliError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| failed(&format!("create {}", path.display()), e))
}

/// Refuses an output directory that holds anything, unless `overwrite`. A directory that does not
/// exist is fine.
fn check_out_dir(dir: &Path, overwrite: bool) -> Result<(), CliError> {
    let mut entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // A directory that does not exist yet is fine; any other failure (permission, not a
        // directory) is a failure now, not later.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(failed(&format!("read {}", dir.display()), e)),
    };
    if !overwrite && entries.next().is_some() {
        return Err(CliError::Failed(format!(
            "{} is not empty; give --overwrite to delete the frame-NNNN.ppm and hashes.txt files of an earlier run, or choose another directory",
            dir.display()
        )));
    }
    Ok(())
}

/// Deletes the files of an earlier run (see [`is_ours`]) from `dir`, and nothing else. A name
/// like an output that is not a plain file (a symlink, a directory) is an error: it is never
/// followed and never deleted.
fn remove_ours(dir: &Path) -> Result<u64, CliError> {
    let mut removed = 0u64;
    let entries =
        std::fs::read_dir(dir).map_err(|e| failed(&format!("read {}", dir.display()), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| failed(&format!("read {}", dir.display()), e))?;
        let name = entry.file_name();
        if !name.to_str().is_some_and(is_ours) {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|e| failed(&format!("stat {}", entry.path().display()), e))?;
        if !kind.is_file() {
            return Err(CliError::Failed(format!(
                "{} is not a plain file (a link or a directory); remove it and run again",
                entry.path().display()
            )));
        }
        std::fs::remove_file(entry.path())
            .map_err(|e| failed(&format!("remove {}", entry.path().display()), e))?;
        removed += 1;
    }
    Ok(removed)
}

/// The frames of a run, summed, one field per reason (E6). Sums over frames of what each frame
/// counted: a track that has ended is counted in every frame it is absent from, so these are
/// target-frames, not targets.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunTotals {
    pub most_drawn: u32,
    pub frames_with_refused: u64,
    // Not loss: a scope's edge, an ended track, or something the screen shows.
    pub ended: u64,
    pub off_map: u64,
    pub off_screen: u64,
    pub cs_conflict: u64,
    pub r95_unknown: u64,
    /// Altitudes written `999+` or `-99-`: shown, not lost.
    pub alt_display_clamped: u64,
    // LOSS: targets refused, and the parts of targets not drawn.
    pub refused: u64,
    pub malformed: u64,
    pub inconsistent: u64,
    pub invalid_position: u64,
    pub unprojectable: u64,
    pub leader_bad_track: u64,
    pub leader_failed: u64,
    pub circle_failed: u64,
    pub circle_radius_clamped: u64,
    pub trail_dropped: u64,
    pub text_clipped: u64,
    pub stamp_clipped: u64,
    // LOSS: rows the conversion could not turn into targets.
    pub convert_invalid_position: u64,
    pub convert_unprojectable: u64,
    pub convert_trail_dropped: u64,
    pub reacq_age_unrepresentable: u64,
    pub time_overflow: u64,
    pub columns_inconsistent: u64,
}

/// Adds one frame's counts to the run's.
pub fn totals_add(
    tot: &mut RunTotals,
    tf: &oa_display::traffic::TrafficStats,
    cv: &crate::rows::ConvertStats,
    refused: u32,
    stamp_clipped: bool,
) {
    tot.most_drawn = tot.most_drawn.max(tf.drawn);
    tot.frames_with_refused += u64::from(refused > 0);
    tot.ended += u64::from(cv.ended);
    tot.off_map += u64::from(cv.off_map);
    tot.off_screen += u64::from(tf.off_screen);
    tot.cs_conflict += u64::from(tf.cs_conflict);
    tot.r95_unknown += u64::from(tf.r95_unknown);
    tot.alt_display_clamped += u64::from(tf.alt_display_clamped);
    tot.refused += u64::from(tf.refused);
    tot.malformed += u64::from(tf.malformed);
    tot.inconsistent += u64::from(tf.inconsistent);
    tot.invalid_position += u64::from(tf.invalid_position);
    tot.unprojectable += u64::from(tf.unprojectable);
    tot.leader_bad_track += u64::from(tf.leader_bad_track);
    tot.leader_failed += u64::from(tf.leader_failed);
    tot.circle_failed += u64::from(tf.circle_failed);
    tot.circle_radius_clamped += u64::from(tf.circle_radius_clamped);
    tot.trail_dropped += u64::from(tf.trail_dropped);
    tot.text_clipped += u64::from(tf.text_clipped);
    tot.stamp_clipped += u64::from(stamp_clipped);
    tot.convert_invalid_position += u64::from(cv.invalid_position);
    tot.convert_unprojectable += u64::from(cv.unprojectable);
    tot.convert_trail_dropped += u64::from(cv.trail_dropped);
    tot.reacq_age_unrepresentable += u64::from(cv.reacq_age_unrepresentable);
    tot.time_overflow += u64::from(cv.time_overflow);
    tot.columns_inconsistent += u64::from(cv.columns_inconsistent);
}

fn show_counts(names: &[&str], counts: &[u64]) -> String {
    let parts: Vec<String> = names
        .iter()
        .zip(counts)
        .filter(|(_, &n)| n > 0)
        .map(|(name, n)| format!("{name} {n}"))
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

/// `oa-scope-replay`: the whole run; the summary goes to `out`.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        let _ = writeln!(out, "{USAGE}");
        return Ok(());
    }
    let a = parse_args(args)?;
    check_out_dir(&a.out_dir, a.overwrite)?;
    let meta = std::fs::metadata(&a.snapshot)
        .map_err(|e| failed(&format!("read {}", a.snapshot.display()), e))?;
    if meta.len() > oa_airspace::snapshot::MAX_SNAPSHOT_BYTES as u64 {
        return Err(CliError::Failed(format!(
            "{} is larger than the snapshot limit",
            a.snapshot.display()
        )));
    }
    let bytes = std::fs::read(&a.snapshot)
        .map_err(|e| failed(&format!("read {}", a.snapshot.display()), e))?;
    let model = oa_airspace::snapshot::decode(&bytes).map_err(|e| failed("snapshot", e))?;

    // parse_args checked that these fit; they are checked again here, not trusted.
    let overflow =
        || CliError::Failed("a time of the run is outside the range of the clock".into());
    let span = i64::try_from(a.frames)
        .ok()
        .and_then(|f| f.checked_mul(a.step_ns))
        .ok_or_else(overflow)?;
    let end_ns = a.start_ns.checked_add(span).ok_or_else(overflow)?;
    let from_ns = a.start_ns.checked_sub(a.warmup_ns).ok_or_else(overflow)?;
    let history_ns = i64::from(a.convert.trail_n)
        .checked_mul(a.convert.trail_step_ns)
        .and_then(|h| h.checked_add(i64::from(a.convert.reacq_window_s) * NS_PER_S))
        .and_then(|h| h.checked_add(a.convert.stale_ns))
        .and_then(|h| h.checked_add(NS_PER_S))
        .ok_or_else(overflow)?;
    let t0 = Instant::now();
    let (store, calls, tr) = track_window(
        &a.adsb,
        from_ns.checked_sub(READ_MARGIN_NS).ok_or_else(overflow)?,
        end_ns.checked_add(READ_MARGIN_NS).ok_or_else(overflow)?,
        end_ns,
        a.start_ns.checked_sub(history_ns).ok_or_else(overflow)?,
    )?;
    let track_time = t0.elapsed();

    let cfg = ReplayConfig {
        centre_lat_e7: a.centre_lat_e7,
        centre_lon_e7: a.centre_lon_e7,
        range_nm: a.range_nm,
        width: a.width,
        height: a.height,
        max_targets: a.max_targets,
        style: a.style,
        convert: a.convert,
    };
    let mut r = replay_new(model, &cfg)?;
    std::fs::create_dir_all(&a.out_dir)
        .map_err(|e| failed(&format!("create {}", a.out_dir.display()), e))?;
    if a.overwrite {
        remove_ours(&a.out_dir)?;
    }
    let mut hashes = String::new();
    let (mut build_ms, mut raster_ms, mut hash_ms, mut write_ms) = (0.0, 0.0, 0.0, 0.0);
    let mut worst_draw_ms: f64 = 0.0;
    let mut tot = RunTotals::default();
    for i in 0..a.frames {
        let t_ns = i64::try_from(i)
            .ok()
            .and_then(|k| k.checked_mul(a.step_ns))
            .and_then(|d| a.start_ns.checked_add(d))
            .ok_or_else(overflow)?;
        let stamp = stamp_of(t_ns);
        let t1 = Instant::now();
        let info = replay_build(&mut r, &store, &calls, t_ns, &stamp)?;
        let t2 = Instant::now();
        replay_raster(&mut r)?;
        let t3 = Instant::now();
        let digest = frame_digest(&r.spec, &r.buf).map_err(|e| failed("hash", e))?;
        let t4 = Instant::now();
        if !a.hashes_only {
            let path = a.out_dir.join(format!("frame-{i:04}.ppm"));
            let file = create_new(&path)?;
            let mut w = std::io::BufWriter::new(file);
            oa_display::ppm::write_ppm(&mut w, &r.spec, &r.buf)
                .and_then(|()| w.flush())
                .map_err(|e| failed(&format!("write {}", path.display()), e))?;
        }
        let t5 = Instant::now();
        hashes.push_str(&format!("{i:04} {} {}\n", unix_text(t_ns), to_hex(&digest)));
        build_ms += ms(t2 - t1);
        raster_ms += ms(t3 - t2);
        hash_ms += ms(t4 - t3);
        write_ms += ms(t5 - t4);
        worst_draw_ms = worst_draw_ms.max(ms(t3 - t1));
        totals_add(
            &mut tot,
            &info.traffic,
            &info.convert,
            info.refused,
            info.stamp_clipped,
        );
    }
    let hashes_path = a.out_dir.join("hashes.txt");
    create_new(&hashes_path)?
        .write_all(hashes.as_bytes())
        .map_err(|e| failed(&format!("write {}", hashes_path.display()), e))?;

    let n = a.frames as f64;
    let dc = &tr.decode;
    let _ = writeln!(
        out,
        "window   {} for {} frames at {} s steps, tracker started {} s before",
        stamp_of(a.start_ns),
        a.frames,
        a.step_ns as f64 / 1e9,
        a.warmup_ns as f64 / 1e9
    );
    let _ = writeln!(
        out,
        "clip     {} payload records read in the window: {} kept, {} dropped as invalid ({} with a bad time), {} of unknown source, {} with a bad measurement; {} clip frames outside the window; in the window {} reject frames (input the bridge refused: loss), {} status frames (not loss), {} damaged frames and {} undecodable payloads (loss)",
        dc.records, dc.kept, dc.dropped, dc.bad_time, dc.unknown_source, dc.bad_measurement,
        tr.frames_outside_window, tr.reject_frames, tr.status_frames, tr.damaged_frames,
        tr.undecodable_payloads
    );
    let _ = writeln!(
        out,
        "adsb     of {} reports, {} passed the quality gates; removed by reason: {}",
        tr.adsb.rows_in,
        tr.adsb.passed,
        show_counts(&REMOVE_NAMES, &tr.adsb.removed)
    );
    let _ = writeln!(
        out,
        "tracker  {} reports, {} measurements ({} refused as invalid: {}; {} jump-rejected), {} tracks confirmed, {} addresses, {} rows kept, {} rows lost, {:.0} ms",
        tr.reports,
        tr.measurements,
        tr.refused.iter().fold(0u64, |s, &n| s.saturating_add(n)),
        show_counts(&REFUSE_NAMES, &tr.refused),
        tr.jump_rejected,
        tr.confirmed,
        tr.addresses,
        tr.rows_kept,
        tr.overflow,
        ms(track_time)
    );
    let s = &tr.store;
    let _ = writeln!(
        out,
        "rows     of {} tracker rows: {} kept, {} before the window; refused {} (address table {}, rows per address {}, missing column {}); clamped: altitude {}, speed {}; altitude not a number {}; velocity not a number {}; 95% radius unknown {}",
        s.rows_in,
        s.kept,
        s.before_window,
        store_loss(s),
        s.address_table_full,
        s.address_rows_full,
        s.missing_column,
        s.alt_clamped,
        s.speed_clamped,
        s.alt_not_finite,
        s.velocity_not_finite,
        s.r95_bad
    );
    let c = &tr.callsigns;
    let _ = writeln!(
        out,
        "calls    of {} reports, {} had no callsign, {} changes of callsign, {} instants with two different callsigns for one address",
        c.reports_in, c.empty, c.changes, c.conflicts
    );
    let _ = writeln!(
        out,
        "targets  at most {} drawn in a frame, {} frames refused targets at the capacity of {}; over {} frames: {} track-frames ended, {} target-frames off the map and {} off the screen, {} shown with a callsign conflict, {} uncertain with no 95% radius, {} altitudes beyond the data block shown as 999+ or -99-",
        tot.most_drawn,
        tot.frames_with_refused,
        a.max_targets,
        a.frames,
        tot.ended,
        tot.off_map,
        tot.off_screen,
        tot.cs_conflict,
        tot.r95_unknown,
        tot.alt_display_clamped
    );
    let _ = writeln!(
        out,
        "time     per frame: build {:.2} ms, raster {:.2} ms (draw {:.2} ms, worst {:.2} ms), hash {:.2} ms, write {:.2} ms",
        build_ms / n,
        raster_ms / n,
        (build_ms + raster_ms) / n,
        worst_draw_ms,
        hash_ms / n,
        write_ms / n
    );
    let _ = writeln!(out, "hashes   {}", hashes_path.display());

    // The loss, by reason. Everything above this line that is not here was shown or by design.
    let losses = [
        ("damaged clip frames", tr.damaged_frames),
        ("undecodable clip payloads", tr.undecodable_payloads),
        (
            "clip reject frames (input refused upstream)",
            tr.reject_frames,
        ),
        ("tracker output rows lost (table full)", tr.overflow),
        (
            "tracker measurements refused for want of a track slot",
            tr.capacity_refused,
        ),
        (
            "tracker measurements refused for a numerical failure",
            tr.numerical_refused,
        ),
        (
            "rows refused by the store: address table full",
            tr.store.address_table_full,
        ),
        (
            "rows refused by the store: rows per address full",
            tr.store.address_rows_full,
        ),
        (
            "rows refused by the store: missing column",
            tr.store.missing_column,
        ),
        ("addresses whose rows could not be ordered", tr.sort_failed),
        ("targets refused, display full (target-frames)", tot.refused),
        ("targets refused as invalid (target-frames)", tot.malformed),
        (
            "targets with unreadable columns (target-frames)",
            tot.inconsistent,
        ),
        (
            "targets with a position off the globe (target-frames)",
            tot.invalid_position,
        ),
        (
            "targets that do not project (target-frames)",
            tot.unprojectable,
        ),
        ("leaders not drawn, bad track", tot.leader_bad_track),
        (
            "leaders not drawn, geodesic or projection failed",
            tot.leader_failed,
        ),
        ("95% circles not drawn", tot.circle_failed),
        (
            "95% circles drawn smaller than true (radius clamped)",
            tot.circle_radius_clamped,
        ),
        ("history dots not drawn, did not project", tot.trail_dropped),
        ("data block texts clipped", tot.text_clipped),
        ("frame stamps clipped", tot.stamp_clipped),
        (
            "rows with a position off the globe (row-frames)",
            tot.convert_invalid_position,
        ),
        (
            "rows that do not project (row-frames)",
            tot.convert_unprojectable,
        ),
        (
            "history dots dropped, position off the globe",
            tot.convert_trail_dropped,
        ),
        (
            "reacquisition ages that do not fit",
            tot.reacq_age_unrepresentable,
        ),
        ("history times that overflowed", tot.time_overflow),
        (
            "stored rows with unreadable columns",
            tot.columns_inconsistent,
        ),
    ];
    let lost: Vec<String> = losses
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(what, n)| format!("{what}: {n}"))
        .collect();
    if lost.is_empty() {
        let _ = writeln!(out, "loss     none");
        return Ok(());
    }
    let text = format!("loss     {}", lost.join("; "));
    let _ = writeln!(out, "{text}");
    if a.allow_loss {
        let _ = writeln!(out, "loss     allowed by --allow-loss");
        return Ok(());
    }
    Err(CliError::Loss(format!(
        "work was lost ({}); the frames and hashes were written; give --allow-loss to accept it",
        lost.join("; ")
    )))
}
