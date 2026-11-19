#![forbid(unsafe_code)]
//! `oa-compass`: writes one UTC day of OpenATCA tracker output and ERAM tracks as ASTERIX CAT062
//! Ed 1.21, and the ADS-B reports offered to the tracker's adapter (a superset of those it used) as CAT021 Ed 2.4, for OpenATS COMPASS. Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days]
//!             [--inputs split|all] [--openatca-rows ticks|updates]
//! ```
//!
//! Writes `openatca.ast` (from `--adsb`: the tracker is run on the clip as the scorer runs it) and
//! `adsb.ast` (the ADS-B reports offered to the tracker's adapter (a superset of those it used), deduplicated by the scorer's own code),
//! `eram.ast` (from `--sfdps-dir`: the ERAM tracks the scorer reads), `data_sources.json` and
//! `README.txt` in `<dir>`. A file holds one UTC day: rows on another day than `--date` refuse the
//! export unless `--split-days` is given, which writes one directory per day. The output
//! directory must be new or empty. `--inputs all` (the default) offers the tracker's adapter every
//! ADS-B report of the clip, like ERAM, `split` offers it set A of the scorer's split only.
//! `--openatca-rows ticks` (the default) writes the tracker's one-second tick rows, `updates` its
//! update rows (one per applied report, at the report's time). Only development Only development days may be exported; the sealed hold-out day is never read by this tool's callers.

use std::path::PathBuf;
use std::process::ExitCode;

use oa_compass::core::time::days_from_civil;
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::Layout;
use oa_compass::shell::job::{run_job, EramSource, Job};
use oa_compass::shell::run::{InputsMode, OpenatcaRows};
use oa_compass::shell::slice;

const USAGE: &str = "usage: oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days] [--inputs split|all] [--openatca-rows ticks|updates]";

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split(['.', '-']);
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    days_from_civil(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("slice") {
        return slice_main();
    }
    let (mut date, mut out, mut adsb, mut sfdps) = (None, None, None, None);
    let mut split = false;
    let mut mode = InputsMode::All;
    let mut rows = OpenatcaRows::Ticks;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--split-days" {
            split = true;
            continue;
        }
        let v = args.next();
        match a.as_str() {
            "--date" => date = v,
            "--out" => out = v.map(PathBuf::from),
            "--adsb" => adsb = v.map(PathBuf::from),
            "--sfdps-dir" => sfdps = v.map(PathBuf::from),
            "--openatca-rows" => match v.as_deref() {
                Some("ticks") => rows = OpenatcaRows::Ticks,
                Some("updates") => rows = OpenatcaRows::Updates,
                _ => {
                    eprintln!("oa-compass: --openatca-rows takes ticks or updates\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "--inputs" => match v.as_deref() {
                Some("all") => mode = InputsMode::All,
                Some("split") => mode = InputsMode::Split,
                _ => {
                    eprintln!("oa-compass: --inputs takes split or all\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("oa-compass: unknown argument {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(date), Some(out)) = (date, out) else {
        eprintln!("oa-compass: --date and --out are required\n{USAGE}");
        return ExitCode::from(2);
    };
    let Some(day) = parse_date(&date) else {
        eprintln!("oa-compass: --date {date} is not a date (YYYY.MM.DD)\n{USAGE}");
        return ExitCode::from(2);
    };
    if adsb.is_none() && sfdps.is_none() {
        eprintln!("oa-compass: give --adsb, --sfdps-dir or both\n{USAGE}");
        return ExitCode::from(2);
    }
    let job = Job {
        layout: Layout { out, day, split },
        adsb,
        sfdps: sfdps.map(EramSource::Dir),
        mode,
        rows,
    };
    match run(job) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-compass: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(job: Job) -> Result<(), ExportError> {
    let lines = run_job(job, &mut |_| Ok(()), &mut |l| eprintln!("{l}"))?;
    for l in lines {
        println!("{l}");
    }
    Ok(())
}

const SLICE_USAGE: &str =
    "usage: oa-compass slice --in <dir> --from <HH:MM:SS> --to <HH:MM:SS> --out <dir>";

/// `oa-compass slice`: a time window of the `.ast` files of a finished export.
fn slice_main() -> ExitCode {
    let (mut inp, mut out, mut from, mut to) = (None, None, None, None);
    let mut args = std::env::args().skip(2);
    while let Some(a) = args.next() {
        let v = args.next();
        match a.as_str() {
            "--in" => inp = v.map(PathBuf::from),
            "--out" => out = v.map(PathBuf::from),
            "--from" => from = v.as_deref().and_then(slice::parse_clock),
            "--to" => to = v.as_deref().and_then(slice::parse_clock),
            _ => {
                eprintln!("oa-compass slice: unknown argument {a}\n{SLICE_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(inp), Some(out), Some(from), Some(to)) = (inp, out, from, to) else {
        eprintln!(
            "oa-compass slice: --in, --out, --from and --to (HH:MM:SS) are required\n{SLICE_USAGE}"
        );
        return ExitCode::from(2);
    };
    match slice::slice_dir(&inp, &out, from, to) {
        Ok(files) => {
            for f in &files {
                println!(
                    "{}: {} in, {} kept, {} blocks, {} octets",
                    f.name, f.records_in, f.records_kept, f.blocks, f.bytes
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-compass slice: {e}");
            ExitCode::from(1)
        }
    }
}
