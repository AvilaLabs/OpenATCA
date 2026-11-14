//! Command-line parsing for `oa-feed`. Core (pure): a function from arguments to a command.
//!
//! ```text
//! oa-feed (--env <file> [--jar <jar>] [--java <java>] | --stdin) [--feed <name>]
//!         [--record <dir> | --record-raw <dir>] [--max-bytes <n>] [--rotate-bytes <n>]
//!         [--duration <s>] [--summary-every <s>]
//! oa-feed replay <file.oaf | file.oaf.gz>...
//! ```
//!
//! `--record` writes gzip-compressed `.oaf.gz` files, `--record-raw` plain `.oaf`. Sizes
//! (`--max-bytes`, `--rotate-bytes`) count the bytes on disk. Replay tells the two apart by the
//! file's first bytes, not its name.

use std::path::PathBuf;

/// Default total recording cap: 2 GiB.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default size at which a recording file rotates: 256 MiB.
pub const DEFAULT_ROTATE_BYTES: u64 = 256 * 1024 * 1024;
/// Default seconds between health lines.
pub const DEFAULT_SUMMARY_SECS: u64 = 10;

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
    /// Total cap on recorded bytes.
    pub max_bytes: u64,
    /// Rotation size of one recording file.
    pub rotate_bytes: u64,
    /// Exit cleanly after this many seconds.
    pub duration_secs: Option<u64>,
    /// Seconds between health lines.
    pub summary_secs: u64,
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run live.
    Live(Live),
    /// Replay recorded files.
    Replay(Vec<PathBuf>),
    /// Print usage.
    Help,
}

/// Usage text.
pub const USAGE: &str =
    "usage:\n  oa-feed (--env <file> [--jar <jar>] [--java <java>] | --stdin) [--feed <name>] \
[--record <dir> | --record-raw <dir>] [--max-bytes <n>] [--rotate-bytes <n>] [--duration <s>] \
[--summary-every <s>]\n  oa-feed replay <file.oaf | file.oaf.gz>...";

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
    let mut max_bytes = DEFAULT_MAX_BYTES;
    let mut rotate_bytes = DEFAULT_ROTATE_BYTES;
    let mut duration_secs = None;
    let mut summary_secs = DEFAULT_SUMMARY_SECS;
    if args.first().map(String::as_str) == Some("replay") {
        let files: Vec<PathBuf> = args.iter().skip(1).map(PathBuf::from).collect();
        if files.is_empty() {
            return Err("replay needs at least one file".into());
        }
        return Ok(Command::Replay(files));
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
            "--max-bytes" => max_bytes = number("--max-bytes", &value("--max-bytes")?)?,
            "--rotate-bytes" => rotate_bytes = number("--rotate-bytes", &value("--rotate-bytes")?)?,
            "--duration" => duration_secs = Some(number("--duration", &value("--duration")?)?),
            "--summary-every" => {
                summary_secs = number("--summary-every", &value("--summary-every")?)?;
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
    Ok(Command::Live(Live {
        source,
        feed,
        record,
        record_raw,
        max_bytes,
        rotate_bytes,
        duration_secs,
        summary_secs,
    }))
}

fn number(name: &str, v: &str) -> Result<u64, String> {
    v.parse::<u64>()
        .map_err(|_| format!("{name} needs a whole number, got {v:?}"))
}
