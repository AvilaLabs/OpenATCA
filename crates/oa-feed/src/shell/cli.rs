//! Command-line parsing for `oa-feed`. Shell (B1, B7): a function from arguments to a command that allocates (`String`, `PathBuf`); not real-time core.
//!
//! ```text
//! oa-feed (--env <file> [--jar <jar>] [--java <java>] | --stdin) [--feed <name>]
//!         [--record <dir> | --record-raw <dir>] [--sync-closed-segments]
//!         [--max-bytes <n>] [--rotate-bytes <n>]
//!         [--retain-days <n> --peak-bytes-per-hour <n> --peak-rate-source <measurement-path>]
//!         [--duration <s>] [--summary-every <s>]
//! oa-feed replay <file.oaf | file.oaf.gz>...
//! oa-feed measure-rate <raw-file.oaf>...
//! oa-feed recordings --feed <name> --from <unix-seconds> --to <unix-seconds> <dir>
//! ```
//!
//! `--record` writes gzip-compressed `.oaf.gz` files, `--record-raw` plain `.oaf`. The optional
//! `--sync-closed-segments` requests synchronous file and directory-entry sync on Linux whenever
//! a segment closes; it requires an existing recording directory and can delay capture. It does
//! not certify hardware power-loss behavior. It can be combined with measured retention; a
//! segment receives a closed-retention receipt only after both sync stages succeed. Sizes
//! (`--max-bytes`, `--rotate-bytes`) count the
//! bytes written. Replay tells the formats apart by the file's first bytes, not its name.
//! `measure-rate` is a stricter raw `.oaf`-only aggregate measurement: it refuses compressed,
//! damaged, sealed-window, or unframed input. Its hourly frame-byte sample does not prove capture
//! continuity or a future maximum; the report discloses partial edge hours.

use std::path::PathBuf;

use crate::record::{
    check_feed_label, retention_storage_budget_bytes, RetentionPolicy, MIN_RETENTION_SECS,
};

/// Default total recording cap: 2 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default size at which a recording file rotates: 256 MiB.
pub const DEFAULT_ROTATE_BYTES: u64 = 256 * 1024 * 1024;
/// Default seconds between health lines.
pub const DEFAULT_SUMMARY_SECS: u64 = 10;
/// Largest `--duration`: 10 years of 365 days, in seconds. Longer is refused (an `Instant` plus a
/// larger duration can overflow).
pub const MAX_DURATION_SECS: u64 = 10 * 365 * 24 * 3600;
/// Largest `--summary-every`: one day, in seconds.
pub const MAX_SUMMARY_SECS: u64 = 24 * 3600;

/// Where live frames come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Spawn `java -jar <jar> <env>`; the env file is passed on, never read here.
    Bridge {
        /// The SCDS env file given to the bridge.
        env: PathBuf,
        /// The bridge jar, if not the default.
        jar: Option<PathBuf>,
        /// The java launcher, if not `$JAVA_HOME/bin/java` or `java`.
        java: Option<PathBuf>,
    },
    /// Read frames from standard input.
    Stdin,
}

/// Settings for a live run (cold: A7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    /// Frame source.
    pub source: Source,
    /// Feed name for file names and health lines.
    pub feed: String,
    /// Recording directory.
    pub record: Option<PathBuf>,
    /// Record plain `.oaf` instead of gzip-compressed `.oaf.gz`.
    pub record_raw: bool,
    /// Request OS syncs for each closed recording segment.
    pub sync_closed_segments: bool,
    /// Total cap on recorded bytes.
    pub max_bytes: u64,
    /// Rotation size of one recording file.
    pub rotate_bytes: u64,
    /// Exit cleanly after this many seconds.
    pub duration_secs: Option<u64>,
    /// Seconds between health lines.
    pub summary_secs: u64,
    /// Optional measured, at-least-45-day retention policy.
    pub retention: Option<RetentionPolicy>,
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run live.
    Live(Live),
    /// Replay recorded files.
    Replay(Vec<PathBuf>),
    /// Print payload-free observed UTC-hour frame-byte aggregates for bounded OAF inputs.
    MeasureRate(Vec<PathBuf>),
    /// List segments whose known or possible interval overlaps a half-open UTC range.
    ListRecordings {
        dir: PathBuf,
        feed: String,
        from_unix_secs: i64,
        to_unix_secs: i64,
    },
    /// Print usage.
    Help,
}

/// Usage text.
pub const USAGE: &str =
    "usage:\n  oa-feed (--env <file> [--jar <jar>] [--java <java>] | --stdin) [--feed <name>] \
[--record <dir> | --record-raw <dir>] [--sync-closed-segments] [--max-bytes <n>] [--rotate-bytes <n>] [--duration <s>] \
[--summary-every <s>] [--retain-days <n> --peak-bytes-per-hour <n> --peak-rate-source <measurement-path>]\n  oa-feed replay <file.oaf | file.oaf.gz>...\n  oa-feed measure-rate <raw-file.oaf>...\n  oa-feed recordings --feed <name> --from <unix-seconds> --to <unix-seconds> <dir>";

/// Parses arguments (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter();
    let mut env = None;
    let mut jar = None;
    let mut java = None;
    let mut stdin = false;
    let mut feed = None;
    let mut record = None;
    let mut record_raw = false;
    let mut sync_closed_segments = false;
    let mut max_bytes = DEFAULT_MAX_BYTES;
    let mut rotate_bytes = DEFAULT_ROTATE_BYTES;
    let mut duration_secs = None;
    let mut summary_secs = DEFAULT_SUMMARY_SECS;
    let mut retain_days = None;
    let mut peak_bytes_per_hour = None;
    let mut peak_rate_source = None;
    let mut max_bytes_set = false;
    if args.first().map(String::as_str) == Some("replay") {
        let files: Vec<PathBuf> = args.iter().skip(1).map(PathBuf::from).collect();
        if files.is_empty() {
            return Err("replay needs at least one file".into());
        }
        return Ok(Command::Replay(files));
    }
    if args.first().map(String::as_str) == Some("measure-rate") {
        let files: Vec<PathBuf> = args.iter().skip(1).map(PathBuf::from).collect();
        if files.is_empty() {
            return Err("measure-rate needs at least one file".into());
        }
        if files.len() > 256 {
            return Err("measure-rate accepts at most 256 files".into());
        }
        return Ok(Command::MeasureRate(files));
    }
    if args.first().map(String::as_str) == Some("recordings") {
        return parse_recordings_query(args);
    }
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--stdin" => stdin = true,
            "--env" => env = Some(PathBuf::from(value("--env")?)),
            "--jar" => jar = Some(PathBuf::from(value("--jar")?)),
            "--java" => java = Some(PathBuf::from(value("--java")?)),
            "--feed" => feed = Some(value("--feed")?),
            "--record" | "--record-raw" => {
                if record.is_some() {
                    return Err("--record and --record-raw are exclusive and given once".into());
                }
                record_raw = a == "--record-raw";
                record = Some(PathBuf::from(value(a)?));
            }
            "--sync-closed-segments" => {
                if sync_closed_segments {
                    return Err("--sync-closed-segments is given more than once".into());
                }
                sync_closed_segments = true;
            }
            "--max-bytes" => {
                max_bytes = number("--max-bytes", &value("--max-bytes")?)?;
                max_bytes_set = true;
            }
            "--rotate-bytes" => rotate_bytes = number("--rotate-bytes", &value("--rotate-bytes")?)?,
            "--retain-days" => {
                retain_days = Some(bounded("--retain-days", &value("--retain-days")?, 36_500)?)
            }
            "--peak-bytes-per-hour" => {
                peak_bytes_per_hour = Some(number(
                    "--peak-bytes-per-hour",
                    &value("--peak-bytes-per-hour")?,
                )?)
            }
            "--peak-rate-source" => peak_rate_source = Some(value("--peak-rate-source")?),
            "--duration" => {
                duration_secs = Some(bounded(
                    "--duration",
                    &value("--duration")?,
                    MAX_DURATION_SECS,
                )?);
            }
            "--summary-every" => {
                summary_secs = bounded(
                    "--summary-every",
                    &value("--summary-every")?,
                    MAX_SUMMARY_SECS,
                )?;
                if summary_secs == 0 {
                    return Err("--summary-every must be at least 1".into());
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let source = match (env, stdin) {
        (Some(_), true) => return Err("--env and --stdin are exclusive".into()),
        (None, false) => return Err("give --env <file> or --stdin".into()),
        (None, true) => {
            if jar.is_some() || java.is_some() {
                return Err("--jar and --java apply only with --env".into());
            }
            Source::Stdin
        }
        (Some(env), false) => Source::Bridge { env, jar, java },
    };
    let feed = feed.unwrap_or_else(|| match &source {
        Source::Bridge { env, .. } => env
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("feed")
            .to_string(),
        Source::Stdin => "stdin".to_string(),
    });
    if sync_closed_segments && record.is_none() {
        return Err("--sync-closed-segments requires --record or --record-raw".into());
    }
    let retention = match (retain_days, peak_bytes_per_hour, peak_rate_source) {
        (None, None, None) => None,
        (Some(days), Some(rate), Some(source)) => {
            if record.is_none() {
                return Err("retention options require --record or --record-raw".into());
            }
            if max_bytes_set {
                return Err(
                    "--max-bytes cannot be combined with measured retention; its budget is derived"
                        .into(),
                );
            }
            if source.trim().is_empty()
                || source.len() > 160
                || !source.bytes().all(|b| (0x20..=0x7e).contains(&b))
            {
                return Err("--peak-rate-source must be 1 to 160 printable ASCII bytes".into());
            }
            let retention_secs = days
                .checked_mul(24 * 60 * 60)
                .ok_or("retention days overflow")?;
            if retention_secs < MIN_RETENTION_SECS {
                return Err("--retain-days must be at least 45".into());
            }
            let policy = RetentionPolicy {
                retention_secs,
                measured_peak_bytes_per_hour: rate,
                measurement_source: source,
            };
            max_bytes = retention_storage_budget_bytes(&policy).map_err(|e| e.to_string())?;
            Some(policy)
        }
        _ => return Err(
            "--retain-days, --peak-bytes-per-hour and --peak-rate-source must be supplied together"
                .into(),
        ),
    };
    check_feed_label(&feed).map_err(|e| format!("--feed: {e}"))?;
    Ok(Command::Live(Live {
        source,
        feed,
        record,
        record_raw,
        sync_closed_segments,
        max_bytes,
        rotate_bytes,
        duration_secs,
        summary_secs,
        retention,
    }))
}

fn parse_recordings_query(args: &[String]) -> Result<Command, String> {
    let mut feed = None;
    let mut from = None;
    let mut to = None;
    let mut dir = None;
    let mut i = 1;
    while i < args.len() {
        match args.get(i).map(String::as_str) {
            Some("--feed") if feed.is_none() => {
                i += 1;
                feed = Some(args.get(i).ok_or("--feed needs a value")?.clone());
            }
            Some("--from") if from.is_none() => {
                i += 1;
                from = Some(
                    args.get(i)
                        .ok_or("--from needs a value")?
                        .parse::<i64>()
                        .map_err(|_| "--from needs Unix seconds")?,
                );
            }
            Some("--to") if to.is_none() => {
                i += 1;
                to = Some(
                    args.get(i)
                        .ok_or("--to needs a value")?
                        .parse::<i64>()
                        .map_err(|_| "--to needs Unix seconds")?,
                );
            }
            Some(value) if !value.starts_with('-') && dir.is_none() => {
                dir = Some(PathBuf::from(value))
            }
            Some(other) => {
                return Err(format!(
                    "unexpected or repeated recordings argument {other}"
                ))
            }
            None => return Err("recordings query needs values".into()),
        }
        i += 1;
    }
    let feed = feed.ok_or("recordings query needs --feed")?;
    check_feed_label(&feed)?;
    let from_unix_secs = from.ok_or("recordings query needs --from")?;
    let to_unix_secs = to.ok_or("recordings query needs --to")?;
    if from_unix_secs >= to_unix_secs {
        return Err("recordings range must be nonempty and half-open".into());
    }
    Ok(Command::ListRecordings {
        dir: dir.ok_or("recordings query needs a directory")?,
        feed,
        from_unix_secs,
        to_unix_secs,
    })
}

fn number(name: &str, v: &str) -> Result<u64, String> {
    v.parse::<u64>()
        .map_err(|_| format!("{name} needs a whole number, got {v:?}"))
}

/// A whole number of seconds of at most `max`.
fn bounded(name: &str, v: &str, max: u64) -> Result<u64, String> {
    let n = number(name, v)?;
    if n > max {
        return Err(format!("{name} is at most {max} seconds, got {n}"));
    }
    Ok(n)
}
