//! Command-line parsing for `oa-replay`. Core (pure): a function from arguments to a command.
//!
//! ```text
//! oa-replay (--sfdps | --tbfm | --tfms | --fns | --adsb <file> | --sfdps-dir | --tbfm-dir
//!           | --tfms-dir | --fns-dir | --adsb-dir <dir>)... [--checkpoint-secs <n>] [--max-age-secs <n>]
//!           [--active-age-secs <n>] [--proposed-age-secs <n>] [--terminal-age-secs <n>] [--sweep-secs <n>] [--capacity <n>] [--chunk-bytes <n>] [--progress-minutes <n>]
//!           [--quiet]
//! ```
//!
//! Each file is `.oaf` or `.oaf.gz` (told apart by its first bytes) and is given a feed kind by
//! the flag that names it. A file flag is one stream; a directory flag is one stream made of the
//! directory's recordings chained in name order (rotated files). The order of the flags is the
//! stream order used to break merge ties.

use std::path::PathBuf;

use crate::pipeline::{FeedKind, PipelineConfig};
use crate::table::NS_PER_S;

/// Default read size for recordings.
pub const DEFAULT_CHUNK: usize = 64 * 1024;

/// One input named on the command line.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// The feed the recordings are decoded as.
    pub feed: FeedKind,
    /// A recording, or a directory of rotated recordings.
    pub path: PathBuf,
    /// True when `path` is a directory.
    pub dir: bool,
}

/// A parsed command line (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Inputs in command-line order.
    pub inputs: Vec<Input>,
    /// A progress line (stderr) every this many minutes of replay time.
    pub progress_ns: i64,
    /// Pipeline settings.
    pub cfg: PipelineConfig,
    /// Read size in bytes.
    pub chunk: usize,
    /// Print only the summary, not every checkpoint.
    pub quiet: bool,
}

/// The usage text.
pub const USAGE: &str = "usage: oa-replay (--sfdps|--tbfm|--tfms|--fns|--adsb <file> | \
--sfdps-dir|--tbfm-dir|--tfms-dir|--fns-dir|--adsb-dir <dir>)... \
[--checkpoint-secs <n>] [--max-age-secs <n>] [--active-age-secs <n>] [--proposed-age-secs <n>] \
[--terminal-age-secs <n>] [--sweep-secs <n>] [--capacity <n>] \
[--chunk-bytes <n>] [--progress-minutes <n>] [--quiet]";

fn number(flag: &str, v: Option<&String>) -> Result<u64, String> {
    let v = v.ok_or_else(|| format!("missing value for {flag}"))?;
    match v.parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag} needs a positive whole number, got {v:?}")),
    }
}

/// Parses the arguments after the program name.
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut inputs = Vec::new();
    let mut progress_ns = 10 * 60 * NS_PER_S;
    let mut cfg = PipelineConfig::default();
    let mut chunk = DEFAULT_CHUNK;
    let mut quiet = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sfdps" | "--tbfm" | "--tfms" | "--fns" | "--adsb" | "--sfdps-dir" | "--tbfm-dir"
            | "--tfms-dir" | "--fns-dir" | "--adsb-dir" => {
                let v = it.next().ok_or_else(|| format!("missing value for {a}"))?;
                let name = a.trim_start_matches("--");
                let (name, dir) = match name.strip_suffix("-dir") {
                    Some(n) => (n, true),
                    None => (name, false),
                };
                let feed = match name {
                    "sfdps" => FeedKind::Sfdps,
                    "tbfm" => FeedKind::Tbfm,
                    "tfms" => FeedKind::Tfms,
                    "adsb" => FeedKind::Adsb,
                    _ => FeedKind::Fns,
                };
                inputs.push(Input {
                    feed,
                    path: PathBuf::from(v),
                    dir,
                });
            }
            "--progress-minutes" => {
                let n = number(a, it.next())?;
                progress_ns = secs_to_ns(a, n.saturating_mul(60))?;
            }
            "--checkpoint-secs" => {
                let n = number(a, it.next())?;
                cfg.checkpoint_ns = secs_to_ns(a, n)?;
            }
            "--max-age-secs" => {
                let n = number(a, it.next())?;
                cfg.table.max_age_ns = secs_to_ns(a, n)?;
            }
            "--active-age-secs" => {
                let n = number(a, it.next())?;
                cfg.table.active_age_ns = secs_to_ns(a, n)?;
            }
            "--proposed-age-secs" => {
                let n = number(a, it.next())?;
                cfg.table.proposed_age_ns = secs_to_ns(a, n)?;
            }
            "--terminal-age-secs" => {
                let n = number(a, it.next())?;
                cfg.table.terminal_age_ns = secs_to_ns(a, n)?;
            }
            "--sweep-secs" => {
                let n = number(a, it.next())?;
                cfg.table.sweep_ns = secs_to_ns(a, n)?;
            }
            "--capacity" => {
                cfg.table.capacity = usize::try_from(number(a, it.next())?)
                    .map_err(|_| "--capacity is too large".to_string())?;
            }
            "--chunk-bytes" => {
                chunk = usize::try_from(number(a, it.next())?)
                    .map_err(|_| "--chunk-bytes is too large".to_string())?;
            }
            "--quiet" => quiet = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if inputs.is_empty() {
        return Err("no recordings given".to_string());
    }
    Ok(Command {
        inputs,
        progress_ns,
        cfg,
        chunk,
        quiet,
    })
}

fn secs_to_ns(flag: &str, secs: u64) -> Result<i64, String> {
    i64::try_from(secs)
        .ok()
        .and_then(|s| s.checked_mul(NS_PER_S))
        .ok_or_else(|| format!("{flag} is too large"))
}
