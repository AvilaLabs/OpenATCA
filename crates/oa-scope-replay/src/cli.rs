//! The `oa-scope-replay` command line (**shell**, B1, B3): reads a snapshot and an ADS-B clip,
//! runs the tracker over the window asked for, and writes one PPM per step and a hashes file.
//! Every file access and clock read of the tool is here.
//!
//! ```text
//! oa-scope-replay --snapshot <file> --adsb <clip.oaf> --centre <lat,lon> --range-nm <n>
//!     --size <WxH> --start <unix-seconds|YYYY-MM-DDTHH:MM:SSZ> --duration-s <n> --step-s <n>
//!     --out-dir <dir> [--leader-min <n>] [--trail-n <n>] [--trail-step-s <n>] [--reacq-s <n>]
//!     [--max-targets <n>] [--warmup-s <n>] [--hashes-only]
//! ```
//!
//! Frame `i` shows the tracker state at `start + i * step`, for `i` from 0 while the time is
//! before `start + duration`. `hashes.txt` has one line per frame: `frame_index time sha256`, the
//! time in Unix seconds with milliseconds, the hash that of the visible pixels. The tracker is
//! started `warmup` before the start so tracks are confirmed and settled when the first frame is
//! drawn.

use crate::frame::{replay_build, replay_new, replay_raster, ReplayConfig, ReplayError};
use crate::rows::{
    callsigns_from_reports, store_add, store_finish, Callsigns, ConvertConfig, RowStore,
    DEFAULT_CONVERT,
};
use oa_adsb::{decode_payload_lenient, reports_len, Reports};
use oa_display::cli::{parse_centre, parse_size};
use oa_display::project::{MAX_RANGE_NM, MIN_RANGE_NM};
use oa_display::raster::frame_digest;
use oa_display::traffic::{TrafficStyle, DEFAULT_STYLE, MAX_LEADER_MIN, MAX_TARGETS, MAX_TRAIL};
use oa_sha256::to_hex;
use oa_track::adsb::{adsb_measurements, AdsbConfig};
use oa_track::score::{interval_stats, scoring_config};
use oa_track::{
    output_clear, output_len, output_new, tracker_advance, tracker_new, tracker_stats,
    tracker_update, DEFAULT_CONFIG,
};
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NS_PER_S: i64 = 1_000_000_000;
/// Most frames one run writes.
pub const MAX_FRAMES: u64 = 100_000;
/// Shortest step, seconds.
pub const MIN_STEP_S: f64 = 0.1;
/// Default targets per frame.
pub const DEFAULT_MAX_TARGETS: usize = 1024;
/// Default tracker warm-up before the first frame, seconds.
pub const DEFAULT_WARMUP_S: f64 = 180.0;
/// Frames of the clip are read from this long before the warm-up start to this long after the
/// last frame, so no report near the edges is lost to the frame's own receive time.
const READ_MARGIN_NS: i64 = 2 * NS_PER_S;
const DRAIN_ROWS: usize = 1 << 15;

const USAGE: &str = "usage: oa-scope-replay --snapshot <file> --adsb <clip.oaf> \
--centre <lat,lon> --range-nm <n> --size <WxH> --start <unix-seconds|YYYY-MM-DDTHH:MM:SSZ> \
--duration-s <n> --step-s <n> --out-dir <dir> [--leader-min <n>] [--trail-n <n>] \
[--trail-step-s <n>] [--reacq-s <n>] [--max-targets <n>] [--warmup-s <n>] [--hashes-only]";

// A7.2: cold struct, size guard waived (errors are rare).
#[derive(Debug)]
pub enum CliError {
    Usage(String),
    Failed(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n{USAGE}"),
            CliError::Failed(m) => write!(f, "{m}"),
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
}

fn parse_finite(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Seconds as ns, for a finite non-negative number of at most a day.
fn parse_seconds(flag: &str, text: &str) -> Result<i64, CliError> {
    match parse_finite(text) {
        Some(v) if (0.0..=86_400.0).contains(&v) => Ok((v * 1e9).round() as i64),
        _ => Err(CliError::Usage(format!(
            "{flag} {text:?} is not a number of seconds from 0 to 86400"
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

/// A start time: Unix seconds (decimals allowed), or `YYYY-MM-DDTHH:MM:SS[.fff]Z`. Unix ns.
pub fn parse_time(text: &str) -> Result<i64, CliError> {
    let bad = || {
        CliError::Usage(format!(
            "--start {text:?} is not Unix seconds or YYYY-MM-DDTHH:MM:SSZ"
        ))
    };
    if let Some(v) = parse_finite(text) {
        return if (0.0..=253_402_300_799.0).contains(&v) {
            Ok((v * 1e9).round() as i64)
        } else {
            Err(bad())
        };
    }
    let body = text.strip_suffix('Z').ok_or_else(bad)?;
    let (date, time) = body.split_once('T').ok_or_else(bad)?;
    let mut d = date.split('-');
    let mut t = time.split(':');
    let num = |it: &mut std::str::Split<'_, char>| -> Result<i64, CliError> {
        it.next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(bad)
    };
    let (y, mo, da) = (num(&mut d)?, num(&mut d)?, num(&mut d)?);
    let (h, mi) = (num(&mut t)?, num(&mut t)?);
    let sec_text = t.next().ok_or_else(bad)?;
    let sec = parse_finite(sec_text).ok_or_else(bad)?;
    if d.next().is_some()
        || t.next().is_some()
        || !(1..=9999).contains(&y)
        || !(1..=12).contains(&mo)
        || !(1..=31).contains(&da)
        || !(0..24).contains(&h)
        || !(0..60).contains(&mi)
        || !(0.0..60.0).contains(&sec)
    {
        return Err(bad());
    }
    let days = days_from_civil(y, mo, da);
    Ok(((days * 86_400 + h * 3600 + mi * 60) as f64 * 1e9 + sec * 1e9).round() as i64)
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
    let mut hashes_only = false;
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
            other => return Err(CliError::Usage(format!("unknown argument {other:?}"))),
        }
    }
    let need = |what: &str| CliError::Usage(format!("missing {what}"));
    let (centre_lat_e7, centre_lon_e7) = centre.ok_or_else(|| need("--centre"))?;
    let (width, height) = size.ok_or_else(|| need("--size"))?;
    let step_ns = step.ok_or_else(|| need("--step-s"))?;
    let duration_ns = duration.ok_or_else(|| need("--duration-s"))?;
    if step_ns < (MIN_STEP_S * 1e9) as i64 {
        return Err(CliError::Usage(format!(
            "--step-s is below the shortest step of {MIN_STEP_S} s"
        )));
    }
    let frames = (duration_ns / step_ns) as u64;
    if frames == 0 || frames > MAX_FRAMES {
        return Err(CliError::Usage(format!(
            "the duration and step give {frames} frames; the limit is 1 to {MAX_FRAMES}"
        )));
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
    Ok(ReplayArgs {
        snapshot: snapshot.ok_or_else(|| need("--snapshot"))?,
        adsb: adsb.ok_or_else(|| need("--adsb"))?,
        centre_lat_e7,
        centre_lon_e7,
        range_nm: range.ok_or_else(|| need("--range-nm"))?,
        width,
        height,
        start_ns: start.ok_or_else(|| need("--start"))?,
        step_ns,
        frames,
        out_dir: out_dir.ok_or_else(|| need("--out-dir"))?,
        style,
        convert,
        max_targets: max_targets.unwrap_or(DEFAULT_MAX_TARGETS),
        warmup_ns: warmup.unwrap_or((DEFAULT_WARMUP_S * 1e9) as i64),
        hashes_only,
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

/// What the tracker run over the window did.
// A7.2: cold struct, size guard waived (one per run).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackSummary {
    pub reports: usize,
    pub bad_frames: u64,
    pub measurements: usize,
    pub rows_kept: usize,
    pub addresses: usize,
    pub overflow: u64,
    /// Tracks the tracker confirmed, and measurements it refused or did not apply.
    pub confirmed: u64,
    pub refused: u64,
}

/// Reads the clip's reports with receive time in `[from_ns, to_ns]`, runs the tracker over them
/// until `end_ns`, and keeps the rows from `keep_from_ns`.
pub fn track_window(
    adsb: &std::path::Path,
    from_ns: i64,
    to_ns: i64,
    end_ns: i64,
    keep_from_ns: i64,
) -> Result<(RowStore, Callsigns, TrackSummary), CliError> {
    let mut reports = Reports::default();
    let mut file =
        std::fs::File::open(adsb).map_err(|e| failed(&format!("read {}", adsb.display()), e))?;
    let mut undecodable = 0u64;
    let damaged = oa_feed::read_frames(&mut file, &mut |fr| {
        if fr.kind == oa_feed::Kind::Message
            && (from_ns..=to_ns).contains(&fr.recv_unix_ns)
            && decode_payload_lenient(fr.payload, &mut reports).is_err()
        {
            undecodable += 1;
        }
    })
    .map_err(|e| failed(&format!("read {}", adsb.display()), e))?;
    let n = reports_len(&reports);
    if n == 0 {
        return Err(CliError::Failed(format!(
            "{} has no reports in the window",
            adsb.display()
        )));
    }
    let calls = callsigns_from_reports(&reports);
    let (meas, _) = adsb_measurements(&reports, &AdsbConfig::default());
    let cfg = match interval_stats(&meas) {
        Some(iv) => scoring_config(&DEFAULT_CONFIG, &iv),
        None => DEFAULT_CONFIG,
    };
    let mut t = tracker_new(&cfg).map_err(|e| failed("tracker", e))?;
    let mut scratch = output_new(cfg.out_capacity);
    let mut store = RowStore::default();
    for m in &meas {
        tracker_update(&mut t, m, &mut scratch);
        if output_len(&scratch) > DRAIN_ROWS.min(cfg.out_capacity / 2) {
            store_add(&mut store, &scratch, keep_from_ns);
            output_clear(&mut scratch);
        }
    }
    // Not flushed: a flush would end every track, which the replay is not asked to show.
    tracker_advance(&mut t, end_ns, &mut scratch);
    store_add(&mut store, &scratch, keep_from_ns);
    let overflow = scratch.overflow;
    store_finish(&mut store);
    let stats = *tracker_stats(&t);
    let summary = TrackSummary {
        reports: n,
        bad_frames: damaged + undecodable,
        measurements: meas.len(),
        rows_kept: store.by_addr.values().map(Vec::len).sum(),
        addresses: store.by_addr.len(),
        overflow,
        confirmed: stats.confirmed,
        refused: stats.refused.iter().sum::<u64>() + stats.jump_rejected + stats.capacity_refused,
    };
    Ok((store, calls, summary))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// `oa-scope-replay`: the whole run; the summary goes to `out`.
pub fn run(args: &[String], out: &mut dyn Write) -> Result<(), CliError> {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        let _ = writeln!(out, "{USAGE}");
        return Ok(());
    }
    let a = parse_args(args)?;
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
    std::fs::create_dir_all(&a.out_dir)
        .map_err(|e| failed(&format!("create {}", a.out_dir.display()), e))?;

    let end_ns = a.start_ns.saturating_add(
        i64::try_from(a.frames)
            .unwrap_or(0)
            .saturating_mul(a.step_ns),
    );
    let from_ns = a.start_ns.saturating_sub(a.warmup_ns);
    let history_ns = i64::from(a.convert.trail_n)
        .saturating_mul(a.convert.trail_step_ns)
        .saturating_add(i64::from(a.convert.reacq_window_s) * NS_PER_S)
        .saturating_add(a.convert.stale_ns)
        .saturating_add(NS_PER_S);
    let t0 = Instant::now();
    let (store, calls, tr) = track_window(
        &a.adsb,
        from_ns.saturating_sub(READ_MARGIN_NS),
        end_ns.saturating_add(READ_MARGIN_NS),
        end_ns,
        a.start_ns.saturating_sub(history_ns),
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
    let mut hashes = String::new();
    let (mut build_ms, mut raster_ms, mut hash_ms, mut write_ms) = (0.0, 0.0, 0.0, 0.0);
    let mut worst_draw_ms: f64 = 0.0;
    let (mut most, mut refused_frames) = (0u32, 0u64);
    for i in 0..a.frames {
        let t_ns = a
            .start_ns
            .saturating_add(i64::try_from(i).unwrap_or(0).saturating_mul(a.step_ns));
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
            let file = std::fs::File::create(&path)
                .map_err(|e| failed(&format!("write {}", path.display()), e))?;
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
        most = most.max(info.traffic.drawn);
        refused_frames += u64::from(info.refused > 0);
    }
    let hashes_path = a.out_dir.join("hashes.txt");
    std::fs::write(&hashes_path, hashes)
        .map_err(|e| failed(&format!("write {}", hashes_path.display()), e))?;

    let n = a.frames as f64;
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
        "tracker  {} reports, {} measurements ({} refused), {} tracks confirmed, {} addresses, {} rows kept, {} rows lost, {} bad frames, {:.0} ms",
        tr.reports,
        tr.measurements,
        tr.refused,
        tr.confirmed,
        tr.addresses,
        tr.rows_kept,
        tr.overflow,
        tr.bad_frames,
        ms(track_time)
    );
    let _ = writeln!(
        out,
        "targets  at most {most} drawn in a frame, {refused_frames} frames refused targets at the capacity of {}",
        a.max_targets
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
    Ok(())
}
