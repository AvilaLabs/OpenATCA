//! Command-line parsing for `oa-replay`. Core (pure): a function from arguments to a command.
//!
//! ```text
//! oa-replay (--sfdps <file> | --tbfm <file>)... [--checkpoint-secs <n>] [--max-age-secs <n>]
//!           [--sweep-secs <n>] [--capacity <n>] [--chunk-bytes <n>] [--quiet]
//! ```
//!
//! Each file is `.oaf` or `.oaf.gz` (told apart by its first bytes) and is given a feed kind by
//! the flag that names it; the order of the flags is the file order used to break merge ties.

use std::path::PathBuf;

use crate::pipeline::{FeedKind, PipelineConfig};
use crate::table::NS_PER_S;

/// Default read size for recordings.
pub const DEFAULT_CHUNK: usize = 64 * 1024;

/// A parsed command line (cold: A7.2).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Recordings with their feed kind, in command-line order.
    pub files: Vec<(FeedKind, PathBuf)>,
    /// Pipeline settings.
    pub cfg: PipelineConfig,
    /// Read size in bytes.
    pub chunk: usize,
    /// Print only the summary, not every checkpoint.
    pub quiet: bool,
}

/// The usage text.
pub const USAGE: &str = "usage: oa-replay (--sfdps <file> | --tbfm <file>)... \
[--checkpoint-secs <n>] [--max-age-secs <n>] [--sweep-secs <n>] [--capacity <n>] \
[--chunk-bytes <n>] [--quiet]";

fn number(flag: &str, v: Option<&String>) -> Result<u64, String> {
    let v = v.ok_or_else(|| format!("missing value for {flag}"))?;
    match v.parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag} needs a positive whole number, got {v:?}")),
    }
}

/// Parses the arguments after the program name.
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut files = Vec::new();
    let mut cfg = PipelineConfig::default();
    let mut chunk = DEFAULT_CHUNK;
    let mut quiet = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sfdps" | "--tbfm" => {
                let v = it.next().ok_or_else(|| format!("missing value for {a}"))?;
                let feed = if a == "--sfdps" {
                    FeedKind::Sfdps
                } else {
                    FeedKind::Tbfm
                };
                files.push((feed, PathBuf::from(v)));
            }
            "--checkpoint-secs" => {
                let n = number(a, it.next())?;
                cfg.checkpoint_ns = secs_to_ns(a, n)?;
            }
            "--max-age-secs" => {
                let n = number(a, it.next())?;
                cfg.table.max_age_ns = secs_to_ns(a, n)?;
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
    if files.is_empty() {
        return Err("no recordings given".to_string());
    }
    Ok(Command {
        files,
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
