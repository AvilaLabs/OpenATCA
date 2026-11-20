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

use oa_bench::input::{load_with, FeedInput};
use oa_bench::legacy::Legacy;
use oa_replay::FeedKind;

use super::adsb_export::{AdsbExport, AdsbFinished};
use super::dirs::{check_out, create_tracked, remove_created};
use super::error::{io_err, ExportError};
use super::export::{date_text, Finished, Layout, SystemExport, SystemReport};
use super::guard::refuse_holdout;
use super::notes::{
    data_sources_for, readme_with, Extras, ADSB_SOURCE, ERAM_SOURCE, OPENATCA_SOURCE,
};
use super::run::{
    export_adsb_clip, export_eram, ClipInfo, EramAddress, EramInfo, InputsMode, OpenatcaRows,
    TrackRunInfo,
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
    /// What to do with the address of an ERAM flight that contradicts it.
    pub eram_address: EramAddress,
    /// Verification self-tests: added to the latitude of every ERAM / ADS-B source position
    /// before the position check compares it with the decoded record, to show that an error over
    /// the limit fails the whole run. 0 in every real run.
    pub self_test_eram_offset_e7: i32,
    pub self_test_adsb_offset_e7: i32,
}

/// What has been produced and not yet committed.
#[derive(Default)]
struct Done {
    systems: Vec<Finished>,
    adsb: Option<AdsbFinished>,
    clip: Option<ClipInfo>,
    /// Frames and records of the ERAM (SFDPS) input left out for a sealed window (OA-SEAL-014).
    eram_sealed: u64,
}

impl Done {
    /// The rows and frames dropped for a sealed window over every input, each counted once.
    fn sealed_window_dropped(&self) -> u64 {
        self.eram_sealed + self.clip.as_ref().map_or(0, |c| c.sealed_window_dropped)
    }

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
    run_job_counted(job, hook, log).map(|(lines, _)| lines)
}

/// [`run_job`], also returning the count of rows and frames dropped because their time is in a
/// sealed window, summed over the inputs, each counted once (OA-SEAL-014).
pub fn run_job_counted(
    job: Job,
    hook: &mut dyn FnMut(Stage) -> Result<(), ExportError>,
    log: &mut dyn FnMut(String),
) -> Result<(Vec<String>, u64), ExportError> {
    let t0 = Instant::now();
    let out = job.layout.out.clone();
    // The refusals that need no output directory come first: the sealed recording, a missing
    // input, a directory that is not new or empty. `out` is created after them, and every level
    // created is recorded so that a failed run removes them all.
    if let Some(p) = &job.adsb {
        refuse_holdout(p)?;
    }
    if let Some(EramSource::Dir(p)) = &job.sfdps {
        refuse_holdout(p)?;
    }
    refuse_holdout(&out)?;
    check_out(&out)?;
    let mut run_dirs: Vec<PathBuf> = Vec::new();
    create_tracked(&out, &mut run_dirs)?;
    let mut created: Vec<PathBuf> = Vec::new();
    let mut done = Done::default();
    let has_adsb = job.adsb.is_some();
    let mut sealed = 0u64;
    let result = match export_all(&job, &mut done, t0, log) {
        Ok(()) => {
            sealed = done.sealed_window_dropped();
            let mode = has_adsb.then_some(job.mode);
            commit_staged(&job, done, mode, hook, &mut created, t0)
        }
        Err(e) => {
            done.abort();
            Err(e)
        }
    };
    match result {
        Ok(lines) => Ok((lines, sealed)),
        Err(e) => {
            // `done` was moved into the commit; its partials are removed there on failure. What
            // can remain is a partial of a system that failed in `export_all`, which that
            // function removed, and the directories of this run.
            remove_run_dirs(&out, &job.layout, &created, &run_dirs);
            Err(e)
        }
    }
}

/// Removes the directories of a failed run: the day directories, then every level of `out` the
/// run created, deepest first, with `remove_dir` only (a directory that is not empty stays).
fn remove_run_dirs(out: &Path, layout: &Layout, created: &[PathBuf], run_dirs: &[PathBuf]) {
    remove_created(created);
    if layout.split {
        // seal-allow: the output directory; only subdirectories of the tool's own run are removed
        if let Ok(rd) = fs::read_dir(out) {
            for e in rd.filter_map(Result::ok) {
                if e.path().is_dir() {
                    let _ = fs::remove_dir(e.path());
                }
            }
        }
    }
    remove_created(run_dirs);
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
                "address check: flights with a non-ICAO address (above 0xFFFFFF; nothing to compare with)"
                    .into(),
                info.flights_non_icao,
            ),
            (
                "address check: flights whose track contradicts their address".into(),
                info.flights_contradicted,
            ),
            (
                "address check: rows written without I380 address because of it (--eram-contradicted-address drop)"
                    .into(),
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
            "address check (same rule for every flight with an address): the samples used are those that are exported (not refused for position or time), one per flight and position time (copies and same-time conflicting samples are reduced to the first). Each is paired with the ADS-B report of the same address nearest in time (the earlier of two equally near), at most {} s apart (exactly that far is paired). The report set is EVERY ADS-B report of the clip (not stale, ADS-B source type, ICAO address), before the duplicate removal and under --inputs split also including set B, not only the reports offered to the tracker. With at least {} pairs, a flight whose MEDIAN distance (the lower of the two middle values when the count is even) is over {} km (exactly that is not over) contradicts its address. Reason: a pair is at most 10 s apart, so the aircraft has moved under 3 km between them, and the two sources differ by a few km; the distance between two different aircraft is tens to hundreds of km. A non-ICAO address, a flight with fewer pairs, or one with no ADS-B reports under its address is not checked and nothing is claimed for it.",
            crate::core::addrcheck::MAX_PAIR_GAP_S,
            crate::core::addrcheck::MIN_PAIRS,
            crate::core::addrcheck::MAX_MEDIAN_KM
        ));
        v.push(match info.address {
            EramAddress::Keep => "address check is REPORT ONLY (--eram-contradicted-address keep, the default): every address is exported as ERAM published it. A contradicting address is ERAM's own identification error and stays visible to the evaluator; the flights are listed here with the evidence.".to_string(),
            EramAddress::Drop => format!("ERAM FILE ALTERED (--eram-contradicted-address drop): {} rows of the {} flights listed here were written WITHOUT I380 address, which ERAM published; everything else is unchanged.", info.rows_address_withheld, info.flights_contradicted),
        });
        for c in &info.contradicted {
            v.push(format!(
                "  contradicts its address: flight {} callsign {} address {:06X}: {} samples, {} paired, median distance {} km",
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
        ax.set_source_offset(job.self_test_adsb_offset_e7);
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
        positions = std::mem::take(&mut cinfo.positions);
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
        ex.set_source_offset(job.self_test_eram_offset_e7);
        let refs = job.adsb.is_some().then_some(positions.as_slice());
        let mut eram_sealed = 0u64;
        let (info, inputs) = match src {
            EramSource::Dir(dir) => {
                let loaded = load_with(
                    &[FeedInput {
                        feed: FeedKind::Sfdps,
                        dir: dir.clone(),
                    }],
                    &super::guard::guarded_opener(),
                )
                .map_err(|e| ExportError::Input(format!("{}: {e}", dir.display())))?;
                log(format!(
                    "ERAM loaded at {:.1} s",
                    t0.elapsed().as_secs_f64()
                ));
                let info = match export_eram(&loaded.legacy, &mut ex, refs, job.eram_address) {
                    Ok(i) => i,
                    Err(e) => {
                        ex.abort();
                        return Err(e);
                    }
                };
                eram_sealed = loaded.sealed_window_dropped;
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
                let info = match export_eram(leg, &mut ex, refs, job.eram_address) {
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
        ex.counts
            .push(("sealed-window rows dropped".to_string(), eram_sealed));
        done.eram_sealed = eram_sealed;
        let mut f = ex.finish()?;
        f.report.inputs = inputs;
        f.report.details = eram_details(&info);
        f.report.details.push(
            "sealed-window rows dropped (ERAM): SFDPS frames left out by their receive time and records left out by their own time (message timestamp or position time) because they are in a sealed window".to_string(),
        );
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
            "sealed-window rows dropped".into(),
            cinfo.sealed_window_dropped,
        ),
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
