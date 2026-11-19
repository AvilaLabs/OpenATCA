//! Shell (B1): one export run, from the inputs to the committed files. The exports of the systems
//! are written as `.partial` files; the README and the data-source file of each day are written
//! as `.partial` files too, and then every file is renamed in one pass. Any error, before or
//! during that pass, removes every file this run created (partial or already renamed) and the
//! directories it made, so a failed run leaves the output directory as it found it (empty or
//! absent).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use oa_bench::input::{load, FeedInput};
use oa_bench::legacy::Legacy;
use oa_replay::FeedKind;

use super::adsb_export::{AdsbExport, AdsbFinished};
use super::error::{io_err, ExportError};
use super::export::{date_text, Finished, Layout, SystemExport, SystemReport};
use super::notes::{
    data_sources_for, readme_with, Extras, ADSB_SOURCE, ERAM_SOURCE, OPENATCA_SOURCE,
};
use super::run::{
    export_adsb_clip, export_eram, ClipInfo, EramInfo, InputsMode, OpenatcaRows, TrackRunInfo,
};
use super::slice::Stage;

/// Where the ERAM tracks come from.
// A7.2: cold enum, size guard waived
pub enum EramSource {
    /// A directory of SFDPS recordings, loaded the way the scorer loads them.
    Dir(PathBuf),
    /// Tables already loaded, with the description lines of their inputs (tests).
    Loaded(Box<Legacy>, Vec<String>),
}

/// What to export. A7.2: cold.
// A7.2: cold struct, size guard waived
pub struct Job {
    pub layout: Layout,
    pub adsb: Option<PathBuf>,
    pub sfdps: Option<EramSource>,
    pub mode: InputsMode,
    pub rows: OpenatcaRows,
}

/// What has been produced and not yet committed.
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

/// Runs the job. `log` receives progress lines (stderr in the program); the lines of the result
/// are returned. `hook` is called at each [`Stage`] of the final rename pass (`Verified`: every
/// file written as a partial; `Renamed(k)`: `k` files renamed); an error from it fails the run
/// like any other.
pub fn run_job(
    job: Job,
    hook: &mut dyn FnMut(Stage) -> Result<(), ExportError>,
    log: &mut dyn FnMut(String),
) -> Result<Vec<String>, ExportError> {
    let t0 = Instant::now();
    let out = job.layout.out.clone();
    // A new or empty directory only: nothing of an earlier run is mixed in or overwritten.
    let existed = out.exists();
    if existed {
        let occupied = fs::read_dir(&out)
            .map_err(|e| io_err(&out, &e))?
            .next()
            .is_some();
        if occupied {
            return Err(ExportError::Input(format!(
                "{}: the output directory is not empty; the export writes into a new or empty directory only",
                out.display()
            )));
        }
    }
    fs::create_dir_all(&out).map_err(|e| io_err(&out, &e))?;
    let mut created: Vec<PathBuf> = Vec::new();
    let mut done = Done::default();
    let has_adsb = job.adsb.is_some();
    let result = match export_all(&job, &mut done, t0, log) {
        Ok(()) => {
            let mode = has_adsb.then_some(job.mode);
            commit_staged(&job, done, mode, hook, &mut created, t0)
        }
        Err(e) => {
            done.abort();
            Err(e)
        }
    };
    match result {
        Ok(lines) => Ok(lines),
        Err(e) => {
            // `done` was moved into the commit; its partials are removed there on failure. What
            // can remain is a partial of a system that failed in `export_all`, which that
            // function removed, and the directories of this run.
            remove_run_dirs(&out, &job.layout, existed, &created);
            Err(e)
        }
    }
}

/// Removes the day directories of a failed run, and the output directory if the run made it.
/// Directories that are not empty are left.
fn remove_run_dirs(out: &Path, layout: &Layout, existed: bool, created: &[PathBuf]) {
    for d in created {
        let _ = fs::remove_dir(d);
    }
    if layout.split {
        if let Ok(rd) = fs::read_dir(out) {
            for e in rd.filter_map(Result::ok) {
                if e.path().is_dir() {
                    let _ = fs::remove_dir(e.path());
                }
            }
        }
    }
    if !existed {
        let _ = fs::remove_dir(out);
    }
}

fn eram_counts(info: &EramInfo) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = vec![
        ("ERAM track samples (TH) in the input".into(), info.rows_in),
        (
            "samples dropped: same flight, position time and content as an earlier one".into(),
            info.exact_duplicates,
        ),
        (
            "samples kept: same flight and position time as another, different content".into(),
            info.conflicting_same_time,
        ),
        (
            "samples refused: flight not in the flight table".into(),
            info.unknown_flight,
        ),
        (
            "samples written with a position time on a whole second".into(),
            info.whole_second,
        ),
        ("flights with samples".into(), info.flights),
        (
            "flights with samples and an aircraft address".into(),
            info.flights_with_address,
        ),
        (
            "addresses carried by more than one flight (more than one ERAM track)".into(),
            info.addresses_on_several_flights,
        ),
    ];
    if info.checked {
        v.extend([
            (
                "address check: flights whose track agrees with their address".into(),
                info.flights_agree,
            ),
            (
                "address check: flights not checked (too few ADS-B reports of the address to compare)"
                    .into(),
                info.flights_not_checked,
            ),
            (
                "address check: flights whose track contradicts their address (exported without I380 address)"
                    .into(),
                info.flights_contradicted,
            ),
            (
                "address check: rows written without I380 address because of it".into(),
                info.rows_address_withheld,
            ),
        ]);
    }
    v
}

/// The README lines about the ERAM run that are not counts.
fn eram_details(info: &EramInfo) -> Vec<String> {
    let mut v = vec![
        "ERAM track number (I040): the feed's track samples carry no track number in the tables this tool reads (flight, position time, position, altitude, velocity, coast flag), so the export numbers each SFDPS flight (GUFI) 1, 2, ... in order of first appearance: one flight is one track number, and no flight has more than one by construction. An aircraft address carried by several flights therefore appears as several tracks (counted above).".to_string(),
    ];
    if !info.several.is_empty() {
        let list: Vec<String> = info
            .several
            .iter()
            .map(|(a, c)| format!("{a:06X} ({c} flights)"))
            .collect();
        v.push(format!(
            "addresses on more than one flight{}: {}",
            if info.addresses_on_several_flights as usize > info.several.len() {
                format!(
                    " (first {} of {})",
                    info.several.len(),
                    info.addresses_on_several_flights
                )
            } else {
                String::new()
            },
            list.join(", ")
        ));
    }
    if info.checked {
        v.push(format!(
            "address check (same rule for every flight with an address): each track sample is paired with the ADS-B report of the same address nearest in time, at most {} s apart; with at least {} pairs, a flight whose MEDIAN distance is over {} km contradicts its address. Reason: a pair is at most 10 s apart, so the aircraft has moved under 3 km between them, and the two sources differ by a few km; the distance between two different aircraft is tens to hundreds of km. A flight that contradicts its address is exported WITHOUT I380 address (everything else unchanged), and listed here; a flight with fewer pairs is not checked and keeps its address.",
            crate::core::addrcheck::MAX_PAIR_GAP_S,
            crate::core::addrcheck::MIN_PAIRS,
            crate::core::addrcheck::MAX_MEDIAN_KM
        ));
        for c in &info.contradicted {
            v.push(format!(
                "  address withheld: flight {} callsign {} address {:06X}: {} samples, {} paired, median distance {} km",
                c.flight,
                if c.callsign.is_empty() { "(none)" } else { &c.callsign },
                c.address,
                c.samples,
                c.pairs,
                c.median_m / 1000
            ));
        }
        if info.contradicted.is_empty() {
            v.push("  no flight contradicts its address.".to_string());
        }
        let nearest = info.contradicted.iter().map(|c| c.median_m).min();
        v.push(format!(
            "address check, measured separation in this run: {} flights agree, the largest median distance among them is {} m{}.",
            info.flights_agree,
            info.agree_max_median_m,
            nearest.map_or(String::new(), |m| format!("; the smallest among the {} that contradict is {} m", info.flights_contradicted, m))
        ));
    } else {
        v.push("address check: NOT run (no ADS-B clip was given, so there are no reports of the same address to compare with); every address is exported as the feed gave it.".to_string());
    }
    v
}

fn export_all(
    job: &Job,
    done: &mut Done,
    t0: Instant,
    log: &mut dyn FnMut(String),
) -> Result<(), ExportError> {
    let layout = &job.layout;
    let mut positions = Vec::new();
    if let Some(clip) = &job.adsb {
        let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, layout.clone());
        let mut ax = AdsbExport::new(ADSB_SOURCE, layout.clone());
        let r = export_adsb_clip(clip, &mut ex, Some(&mut ax), job.mode, job.rows);
        let (info, mut cinfo) = match r {
            Ok(x) => x,
            Err(e) => {
                ex.abort();
                ax.abort();
                return Err(e);
            }
        };
        log(format!(
            "OpenATCA done at {:.1} s: {} rows",
            t0.elapsed().as_secs_f64(),
            info.tick_rows + info.update_rows
        ));
        for (l, v) in openatca_counts(&info, job.rows, &cinfo) {
            ex.counts.push((l, v));
        }
        if let Some(a) = &mut cinfo.adsb {
            positions = std::mem::take(&mut a.positions);
        }
        if let Some(a) = &cinfo.adsb {
            for (l, v) in [
                ("ADS-B reports offered to the file", a.rows_in),
                (
                    "reports left out: position marked stale by the source",
                    a.stale,
                ),
                ("reports written", a.converted),
            ] {
                ax.counts.push((l.to_string(), v));
            }
            for (name, v) in &a.other_sources {
                ax.counts.push((
                    format!("reports left out: source type {name}, not ADS-B"),
                    *v,
                ));
            }
        }
        let mut f = match ex.finish() {
            Ok(f) => f,
            Err(e) => {
                ax.abort();
                return Err(e);
            }
        };
        f.report
            .inputs
            .push(format!("ADS-B clip {}", clip.display()));
        done.systems.push(f);
        done.adsb = Some(ax.finish()?);
        done.clip = Some(cinfo);
    }
    if let Some(src) = &job.sfdps {
        let mut ex = SystemExport::new("ERAM", "eram.ast", ERAM_SOURCE, layout.clone());
        let refs = job.adsb.is_some().then_some(positions.as_slice());
        let (info, inputs) = match src {
            EramSource::Dir(dir) => {
                let loaded = load(&[FeedInput {
                    feed: FeedKind::Sfdps,
                    dir: dir.clone(),
                }])
                .map_err(|e| ExportError::Input(format!("{}: {e}", dir.display())))?;
                log(format!(
                    "ERAM loaded at {:.1} s",
                    t0.elapsed().as_secs_f64()
                ));
                let info = match export_eram(&loaded.legacy, &mut ex, refs) {
                    Ok(i) => i,
                    Err(e) => {
                        ex.abort();
                        return Err(e);
                    }
                };
                let inputs: Vec<String> = loaded
                    .inputs
                    .iter()
                    .map(|h| {
                        format!(
                            "{} files={} bytes={} sha256={}",
                            h.label, h.files, h.bytes, h.sha256
                        )
                    })
                    .collect();
                (info, inputs)
            }
            EramSource::Loaded(leg, inputs) => {
                let info = match export_eram(leg, &mut ex, refs) {
                    Ok(i) => i,
                    Err(e) => {
                        ex.abort();
                        return Err(e);
                    }
                };
                (info, inputs.clone())
            }
        };
        for (l, v) in eram_counts(&info) {
            ex.counts.push((l, v));
        }
        let mut f = ex.finish()?;
        f.report.inputs = inputs;
        f.report.details = eram_details(&info);
        done.systems.push(f);
    }
    Ok(())
}

/// The run counts of the OpenATCA file, as README lines.
fn openatca_counts(
    info: &TrackRunInfo,
    rows: OpenatcaRows,
    cinfo: &ClipInfo,
) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = vec![
        (
            "ADS-B reports offered to the tracker's adapter".into(),
            cinfo.offered,
        ),
        (
            "ADS-B reports not offered to the tracker (the other half under --inputs split)".into(),
            cinfo.not_offered,
        ),
        (
            "measurements offered to the tracker".into(),
            info.measurements,
        ),
    ];
    match rows {
        OpenatcaRows::Ticks => v.extend([
            (
                "tick rows exported (one per second per confirmed track)".to_string(),
                info.tick_rows,
            ),
            (
                "update rows not exported (--openatca-rows ticks)".to_string(),
                info.update_rows_not_exported,
            ),
        ]),
        OpenatcaRows::Updates => v.extend([
            (
                "update rows exported (--openatca-rows updates)".to_string(),
                info.update_rows,
            ),
            (
                "tick rows not exported (--openatca-rows updates)".to_string(),
                info.tick_rows_not_exported,
            ),
            (
                "update rows left out: earlier than a row already written".to_string(),
                info.updates_out_of_order,
            ),
        ]),
    }
    v.extend([
        ("exported rows with O_COAST (written with CST)".to_string(), info.coasting),
        (
            "exported rows with O_UNCERTAIN (written; flag not representable)".to_string(),
            info.uncertain,
        ),
        (
            "exported rows with O_REACQUIRED (written; flag not representable)".to_string(),
            info.reacquired,
        ),
        (
            "exported rows with O_IDENTITY_UNCONFIRMED (written with CNF = 0; not a tentative track)"
                .to_string(),
            info.unconfirmed_identity,
        ),
        ("tracker output rows lost to a full table".to_string(), info.overflow),
    ]);
    v
}

/// Renames every file of the run in one pass after writing the README and data-source file of
/// each day as partials. On any error, removes everything the run created.
fn commit_staged(
    job: &Job,
    done: Done,
    mode: Option<InputsMode>,
    hook: &mut dyn FnMut(Stage) -> Result<(), ExportError>,
    created: &mut Vec<PathBuf>,
    t0: Instant,
) -> Result<Vec<String>, ExportError> {
    let layout = &job.layout;
    let reports: Vec<SystemReport> = done.systems.iter().map(|f| f.report.clone()).collect();
    let adsb_report = done.adsb.as_ref().map(|a| a.report.clone());
    let mut pending: Vec<(PathBuf, PathBuf)> = done
        .systems
        .iter()
        .flat_map(|f| f.renames().iter().cloned())
        .chain(done.adsb.iter().flat_map(|a| a.renames().iter().cloned()))
        .collect();
    let clip = done.clip.clone();
    let mut days: Vec<i64> = reports
        .iter()
        .flat_map(|r| r.days.iter().map(|d| d.day))
        .chain(
            adsb_report
                .iter()
                .flat_map(|r| r.days.iter().map(|d| d.day)),
        )
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
    let has = |n: &str| reports.iter().any(|r| r.name == n);
    let (has_oa, has_eram, has_adsb) = (has("OpenATCA"), has("ERAM"), adsb_report.is_some());
    let mut renamed: Vec<PathBuf> = Vec::new();
    let result = (|| {
        let refs: Vec<&SystemReport> = reports.iter().collect();
        for day in &days {
            let dir = layout.day_dir(*day);
            if layout.split && !dir.exists() {
                created.push(dir.clone());
            }
            fs::create_dir_all(&dir).map_err(|e| io_err(&dir, &e))?;
            // The files of this day's directory: the data files and the two written here.
            let mut files: BTreeSet<String> = pending
                .iter()
                .filter(|(_, to)| to.parent() == Some(dir.as_path()))
                .filter_map(|(_, to)| to.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            files.insert("data_sources.json".into());
            files.insert("README.txt".into());
            let files: Vec<String> = files.into_iter().collect();
            let extras = Extras {
                adsb: adsb_report.as_ref(),
                clip: clip.as_ref(),
                files: &files,
                openatca_rows: has_oa.then_some(job.rows),
            };
            for (name, text) in [
                (
                    "data_sources.json",
                    data_sources_for(has_oa, has_eram, has_adsb),
                ),
                (
                    "README.txt",
                    readme_with(
                        *day,
                        &refs,
                        oa_build_info::BUILD,
                        layout.split,
                        mode,
                        &extras,
                    ),
                ),
            ] {
                let (partial, fin) = (dir.join(format!("{name}.partial")), dir.join(name));
                // Listed before it is written: a failed write leaves nothing behind.
                pending.push((partial.clone(), fin));
                fs::write(&partial, text).map_err(|e| io_err(&partial, &e))?;
            }
        }
        hook(Stage::Verified)?;
        for (k, (from, to)) in pending.iter().enumerate() {
            fs::rename(from, to).map_err(|e| io_err(to, &e))?;
            renamed.push(to.clone());
            hook(Stage::Renamed(k + 1))?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        for (from, _) in &pending {
            let _ = fs::remove_file(from);
        }
        for f in &renamed {
            let _ = fs::remove_file(f);
        }
        return Err(e);
    }
    let mut lines = Vec::new();
    for r in &reports {
        for d in &r.days {
            lines.push(format!(
                "{} {}: {} records, {} blocks, {} octets, {} tracks",
                r.name,
                date_text(d.day),
                d.sink.records,
                d.sink.blocks,
                d.sink.bytes,
                d.tracks
            ));
        }
    }
    if let Some(r) = &adsb_report {
        for d in &r.days {
            lines.push(format!(
                "ADS-B {}: {} records, {} blocks, {} octets",
                date_text(d.day),
                d.stats.records,
                d.stats.blocks,
                d.stats.bytes
            ));
        }
    }
    lines.push(format!("done in {:.1} s", t0.elapsed().as_secs_f64()));
    Ok(lines)
}
