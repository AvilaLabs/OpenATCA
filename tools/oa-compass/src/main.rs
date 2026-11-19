#![forbid(unsafe_code)]
//! `oa-compass`: writes one UTC day of OpenATCA tracker output and ERAM tracks as ASTERIX CAT062
//! Ed 1.21, and the ADS-B reports the tracker was fed as CAT021 Ed 2.4, for OpenATS COMPASS. Shell (B3): arguments, files, the clock and stdout.
//!
//! ```text
//! oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days]
//!             [--inputs split|all]
//! ```
//!
//! Writes `openatca.ast` (from `--adsb`: the tracker is run on the clip as the scorer runs it) and
//! `adsb.ast` (the reports fed to it, deduplicated by the scorer's own code),
//! `eram.ast` (from `--sfdps-dir`: the ERAM tracks the scorer reads), `data_sources.json` and
//! `README.txt` in `<dir>`. A file holds one UTC day: rows on another day than `--date` refuse the
//! export unless `--split-days` is given, which writes one directory per day. Only development
//! `--inputs all` (the default) feeds the tracker every ADS-B report of the clip, like ERAM, `split`
//! feeds it set A of the scorer's split only. Only development days may be exported; the sealed hold-out day is never read by this tool's callers.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Instant;

use oa_bench::input::{load, FeedInput};
use oa_compass::core::time::days_from_civil;
use oa_compass::shell::error::{io_err, ExportError};
use oa_compass::shell::adsb_export::{AdsbExport, AdsbFinished};
use oa_compass::shell::export::{date_text, Finished, Layout, SystemExport};
use oa_compass::shell::notes::{
    data_sources_json, readme_with, Extras, ADSB_SOURCE, ERAM_SOURCE, OPENATCA_SOURCE,
};
use oa_compass::shell::slice;
use oa_compass::shell::run::{export_adsb_clip, export_eram, ClipInfo, InputsMode};
use oa_replay::FeedKind;

const USAGE: &str = "usage: oa-compass --date <YYYY.MM.DD> --out <dir> [--adsb <clip.oaf>] [--sfdps-dir <dir>] [--split-days] [--inputs split|all]";

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split(['.', '-']);
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    days_from_civil(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// `commit <hash>` and whether tracked files are modified.
fn build_text() -> String {
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    format!("commit {commit}{}", if dirty { " (tracked files modified)" } else { "" })
}

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("slice") {
        return slice_main();
    }
    let (mut date, mut out, mut adsb, mut sfdps) = (None, None, None, None);
    let mut split = false;
    let mut mode = InputsMode::All;
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
    match run(Layout { out, day, split }, adsb, sfdps, mode) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-compass: {e}");
            ExitCode::from(1)
        }
    }
}

/// What the export has produced and not yet committed.
#[derive(Default)]
struct Done {
    systems: Vec<Finished>,
    adsb: Option<AdsbFinished>,
    clip: Option<ClipInfo>,
}

impl Done {
    fn abort(self) {
        for f in self.systems {
            f.abort();
        }
        if let Some(a) = self.adsb {
            a.abort();
        }
    }
}

fn run(
    layout: Layout,
    adsb: Option<PathBuf>,
    sfdps: Option<PathBuf>,
    mode: InputsMode,
) -> Result<(), ExportError> {
    let t0 = Instant::now();
    fs::create_dir_all(&layout.out).map_err(|e| io_err(&layout.out, &e))?;
    let mut done = Done::default();
    let has_adsb = adsb.is_some();
    let result = export_all(&layout, adsb, sfdps, mode, &mut done, t0);
    match result {
        Ok(()) => {
            let build = build_text();
            let adsb_mode = has_adsb.then_some(mode);
            let reports: Vec<_> = done.systems.iter().map(|f| f.report.clone()).collect();
            let adsb_report = done.adsb.as_ref().map(|a| a.report.clone());
            let clip = done.clip.take();
            let mut days: Vec<i64> = reports
                .iter()
                .flat_map(|r| r.days.iter().map(|d| d.day))
                .chain(adsb_report.iter().flat_map(|r| r.days.iter().map(|d| d.day)))
                .collect();
            days.sort_unstable();
            days.dedup();
            if !days.contains(&layout.day) {
                done.abort();
                return Err(ExportError::Input(format!(
                    "no rows at all on {}: nothing to export",
                    date_text(layout.day)
                )));
            }
            for f in done.systems {
                f.commit()?;
            }
            if let Some(a) = done.adsb {
                a.commit()?;
            }
            let refs: Vec<_> = reports.iter().collect();
            let extras = Extras {
                adsb: adsb_report.as_ref(),
                clip: clip.as_ref(),
            };
            for day in days {
                let dir = layout.day_dir(day);
                let w = |name: &str, text: String| {
                    let p = dir.join(name);
                    fs::write(&p, text).map_err(|e| io_err(&p, &e))
                };
                w("data_sources.json", data_sources_json())?;
                w(
                    "README.txt",
                    readme_with(day, &refs, &build, layout.split, adsb_mode, &extras),
                )?;
            }
            for r in &reports {
                for d in &r.days {
                    println!(
                        "{} {}: {} records, {} blocks, {} octets, {} tracks",
                        r.name,
                        date_text(d.day),
                        d.sink.records,
                        d.sink.blocks,
                        d.sink.bytes,
                        d.tracks
                    );
                }
            }
            if let Some(r) = &adsb_report {
                for d in &r.days {
                    println!(
                        "ADS-B {}: {} records, {} blocks, {} octets",
                        date_text(d.day),
                        d.stats.records,
                        d.stats.blocks,
                        d.stats.bytes
                    );
                }
            }
            println!("done in {:.1} s", t0.elapsed().as_secs_f64());
            Ok(())
        }
        Err(e) => {
            done.abort();
            Err(e)
        }
    }
}

fn export_all(
    layout: &Layout,
    adsb: Option<PathBuf>,
    sfdps: Option<PathBuf>,
    mode: InputsMode,
    done: &mut Done,
    t0: Instant,
) -> Result<(), ExportError> {
    if let Some(clip) = adsb {
        let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, layout.clone());
        let mut ax = AdsbExport::new(ADSB_SOURCE, layout.clone());
        let r = export_adsb_clip(&clip, &mut ex, Some(&mut ax), mode);
        let (info, cinfo) = match r {
            Ok(x) => x,
            Err(e) => {
                ex.abort();
                ax.abort();
                return Err(e);
            }
        };
        eprintln!("OpenATCA done at {:.1} s: {} tick rows", t0.elapsed().as_secs_f64(), info.tick_rows);
        for (l, v) in [
            ("ADS-B reports fed to the tracker", cinfo.fed),
            ("ADS-B reports not fed to the tracker (the other half under --inputs split)", cinfo.left),
            ("measurements offered to the tracker", info.measurements),
            ("tick rows offered (one per second per confirmed track)", info.tick_rows),
            ("update rows not exported (tick rows only)", info.update_rows_not_exported),
            ("tick rows with O_COAST (written with CST)", info.coasting),
            ("tick rows with O_UNCERTAIN (written; flag not representable)", info.uncertain),
            ("tick rows with O_REACQUIRED (written; flag not representable)", info.reacquired),
            ("tracker output rows lost to a full table", info.overflow),
        ] {
            ex.counts.push((l.to_string(), v));
        }
        if let Some(a) = &cinfo.adsb {
            for (l, v) in [
                ("ADS-B reports offered to the file", a.rows_in),
                ("reports left out: position marked stale by the source", a.stale),
                ("reports written", a.converted),
            ] {
                ax.counts.push((l.to_string(), v));
            }
            for (name, v) in &a.other_sources {
                ax.counts
                    .push((format!("reports left out: source type {name}, not ADS-B"), *v));
            }
        }
        let mut f = ex.finish()?;
        f.report.inputs.push(format!("ADS-B clip {}", clip.display()));
        done.systems.push(f);
        done.adsb = Some(ax.finish()?);
        done.clip = Some(cinfo);
    }
    if let Some(dir) = sfdps {
        let mut ex = SystemExport::new("ERAM", "eram.ast", ERAM_SOURCE, layout.clone());
        let loaded = load(&[FeedInput {
            feed: FeedKind::Sfdps,
            dir: dir.clone(),
        }])
        .map_err(|e| ExportError::Input(format!("{}: {e}", dir.display())))?;
        eprintln!("ERAM loaded at {:.1} s", t0.elapsed().as_secs_f64());
        let info = match export_eram(&loaded.legacy, &mut ex) {
            Ok(i) => i,
            Err(e) => {
                ex.abort();
                return Err(e);
            }
        };
        let inputs: Vec<String> = loaded
            .inputs
            .iter()
            .map(|h| format!("{} files={} bytes={} sha256={}", h.label, h.files, h.bytes, h.sha256))
            .collect();
        for (l, v) in [
            ("ERAM track samples (TH) in the input", info.rows_in),
            ("samples dropped: same flight, position time and content as an earlier one", info.exact_duplicates),
            ("samples kept: same flight and position time as another, different content", info.conflicting_same_time),
            ("samples refused: flight not in the flight table", info.unknown_flight),
        ] {
            ex.counts.push((l.to_string(), v));
        }
        let mut f = ex.finish()?;
        f.report.inputs = inputs;
        done.systems.push(f);
    }
    Ok(())
}

const SLICE_USAGE: &str = "usage: oa-compass slice --in <dir> --from <HH:MM:SS> --to <HH:MM:SS> --out <dir>";

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
        eprintln!("oa-compass slice: --in, --out, --from and --to (HH:MM:SS) are required\n{SLICE_USAGE}");
        return ExitCode::from(2);
    };
    match slice::slice_dir(&inp, &out, from, to) {
        Ok(files) => {
            for f in &files {
                println!("{}: {} in, {} kept, {} blocks, {} octets", f.name, f.records_in, f.records_kept, f.blocks, f.bytes);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oa-compass slice: {e}");
            ExitCode::from(1)
        }
    }
}
