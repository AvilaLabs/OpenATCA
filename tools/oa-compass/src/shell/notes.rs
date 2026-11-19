//! Shell (B1): the text files that go beside the data: `data_sources.json` (COMPASS data source
//! definition, content version 0.2) and `README.txt` (what to give at import, what is in each
//! file and what is not). `oa-json` only decodes, so the JSON is built by hand from fixed names
//! and numbers.

use std::fmt::Write as _;

use oa_asterix::SourceId;

use super::adsb_export::{AdsbDayReport, AdsbReport, MAX_POSITION_ERROR_E7};
use super::encode::MAX_POSITION_ERROR_E7 as MAX_POSITION_ERROR_062;
use super::export::{date_text, DayReport, SystemReport};
use super::run::{ClipInfo, InputsMode, OpenatcaRows};
use crate::core::adsb::{
    ADSB_NOTES, ADSB_NOTE_NAMES, ANOTE_CATEGORY_ABSENT, ANOTE_CATEGORY_NO_CODE, ANOTE_FL_ABSENT,
    ANOTE_FL_RANGE, ANOTE_HEIGHT_ABSENT, ANOTE_HEIGHT_RANGE, ANOTE_IDENT_ABSENT, ANOTE_IDENT_CHARS,
    ANOTE_MOPS_ABSENT, ANOTE_MOPS_VERSION, ANOTE_RATE_ABSENT, ANOTE_RATE_RANGE,
    ANOTE_VECTOR_ABSENT, ANOTE_VECTOR_GROUND, ANOTE_VECTOR_RANGE,
};
use crate::core::convert::{
    NOTES, NOTE_ACCURACY_ABSENT, NOTE_ADDRESS, NOTE_ADDRESS_WITHHELD, NOTE_ALT_ABSENT,
    NOTE_ALT_QNH, NOTE_ALT_RANGE, NOTE_CLIMB_ABSENT, NOTE_CLIMB_RANGE, NOTE_IDENT, NOTE_NAMES,
    NOTE_SERVICE, NOTE_VEL_ABSENT, NOTE_VEL_RANGE, REFUSAL_NAMES,
};

/// The data source of the OpenATCA tracker output.
///
/// The ASTERIX specification (Part 1 Ed 3.1, 6.2.1) assigns SACs through the ASTERIX Manager and
/// the editions held here carry no list and no test range. This export therefore uses a pair of
/// its own, SAC 255 with SIC 11 for OpenATCA, 12 for ERAM and 13 for ADS-B (SIC 1 is left free: COMPASS writes its own reference under 255/1): a test pair chosen by us, not an
/// allocation. It must not be used on a live link; check the published SAC list before using
/// any pair outside the lab.
pub const OPENATCA_SOURCE: SourceId = SourceId { sac: 255, sic: 11 };
/// The data source of the ERAM tracks (see [`OPENATCA_SOURCE`]).
pub const ERAM_SOURCE: SourceId = SourceId { sac: 255, sic: 12 };

/// The data source of the ADS-B reports file (see [`OPENATCA_SOURCE`]).
pub const ADSB_SOURCE: SourceId = SourceId { sac: 255, sic: 13 };

/// The `ds_type` strings the COMPASS manual's data-source example uses.
pub const DS_TYPES: [&str; 6] = ["Radar", "MLAT", "ADSB", "Tracker", "RefTraj", "Other"];

/// The COMPASS data-source file with the sources `present` (`OpenATCA`, `ERAM`, `ADS-B`), in that
/// order; sources not asked for are left out.
pub fn data_sources_for(openatca: bool, eram: bool, adsb: bool) -> String {
    let one = |ty: &str, name: &str, s: SourceId| {
        format!(
            "{{\"ds_type\":\"{ty}\",\"name\":\"{name}\",\"sac\":{},\"sic\":{}}}",
            s.sac, s.sic
        )
    };
    let mut v = Vec::new();
    if openatca {
        v.push(one("Tracker", "OpenATCA", OPENATCA_SOURCE));
    }
    if eram {
        v.push(one("Tracker", "ERAM", ERAM_SOURCE));
    }
    if adsb {
        v.push(one("ADSB", "ADS-B", ADSB_SOURCE));
    }
    format!(
        "{{\"content_type\":\"data_sources\",\"content_version\":\"0.2\",\"data_sources\":[{}]}}\n",
        v.join(",")
    )
}

/// The COMPASS data-source file with all three sources.
pub fn data_sources_json() -> String {
    data_sources_for(true, true, true)
}

/// A 1/128 s count since midnight as `hh:mm:ss.mmm`.
pub fn clock_text(units: u32) -> String {
    let ms = u64::from(units) * 1000 / 128;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// One system's section of the README for one day. `report` is the system's whole run, `day` the
/// day asked for.
fn system_section(out: &mut String, report: &SystemReport, day: i64) {
    let _ = writeln!(
        out,
        "{}  (SAC {} SIC {})",
        report.name, report.source.sac, report.source.sic
    );
    for l in &report.inputs {
        let _ = writeln!(out, "  input: {l}");
    }
    let Some(d) = report.days.iter().find(|d| d.day == day) else {
        let _ = writeln!(out, "  no rows on this day; no file written\n");
        return;
    };
    day_lines(out, report, d);
    for l in &report.details {
        let _ = writeln!(out, "  {l}");
    }
}

/// What the rows of the file are, and the measured interval between the rows of one track.
fn sample_nature(report: &SystemReport, d: &DayReport) -> String {
    let what = if report.name == "OpenATCA" {
        "rows are the tracker's rows as the OpenATCA ROWS paragraph above says (ticks: its state once per second per track, coasting rows included and counted below)"
    } else {
        "rows are SFDPS track samples at their own position times, not a fixed rate; the whole-second share of those times is measured below"
    };
    let iv = &d.intervals;
    let secs = |u: u32| f64::from(u) / 128.0;
    if iv.pairs == 0 {
        // Nothing is measured, so no value is shown (E6).
        return format!(
            "sample nature: {what}. Measured interval between consecutive rows of one track in this file: no consecutive rows of one track (0 pairs)"
        );
    }
    format!(
        "sample nature: {what}. Measured interval between consecutive rows of one track in this file: median {:.3} s, 90th percentile {:.3} s, longest {:.3} s, over {} pairs of rows ({} of them at the same time)",
        secs(iv.median),
        secs(iv.p90),
        secs(iv.max),
        iv.pairs,
        iv.zero
    )
}

fn day_lines(out: &mut String, report: &SystemReport, d: &DayReport) {
    let rows = d.sink.records;
    let _ = writeln!(out, "  file: {}", d.file);
    let _ = writeln!(
        out,
        "  records written: {rows} in {} data blocks, {} octets, {} tracks (I040 numbers 1 to {})",
        d.sink.blocks, d.sink.bytes, d.tracks, d.tracks
    );
    let _ = writeln!(
        out,
        "  time span: {} to {} UTC (I070, rounded to 1/128 s)",
        clock_text(d.sink.first_time),
        clock_text(d.sink.last_time)
    );
    let _ = writeln!(out, "  {}", sample_nature(report, d));
    let _ = writeln!(
        out,
        "  verification: every block was decoded again with this repository's oa-asterix decoder and equals the records encoded; the file was read back from disk ({} records, {} blocks) and equals what was written; largest position difference between source row and decoded record {} / {} (1e-7 degree, latitude / longitude; limit {MAX_POSITION_ERROR_062}, enforced: a larger difference stops the export). This shows that our encoder and decoder agree with each other; it is not an outside conformance test (the encoder was written from, and reviewed item by item against, the Ed 1.21 text; the first outside decode is the COMPASS import)",
        d.read_back_records, d.read_back_blocks, d.sink.max_lat_error_e7, d.sink.max_lon_error_e7
    );
    let n = |i: usize| d.notes.get(i).copied().unwrap_or(0);
    let is_oa = report.name == "OpenATCA";
    let alt_name = if is_oa {
        "I135 barometric altitude (no QNH)"
    } else {
        "I136 measured flight level (ERAM at or above 18 000 ft)"
    };
    let _ = writeln!(out, "  items present (rows with the item, of {rows}):");
    let mut item = |name: &str, present: u64| {
        let _ = writeln!(out, "    {name}: {present}");
    };
    item("I010 data source", rows);
    item(
        "I015 service identification (1)",
        if is_oa {
            rows - n(NOTE_SERVICE).min(rows)
        } else {
            0
        },
    );
    item("I070 time of track information", rows);
    item("I105 position in WGS-84", rows);
    item(
        "I185 velocity",
        rows - (n(NOTE_VEL_ABSENT) + n(NOTE_VEL_RANGE)).min(rows),
    );
    item(
        "I380 target address",
        rows - (n(NOTE_ADDRESS) + n(NOTE_ADDRESS_WITHHELD)).min(rows),
    );
    item("I380 target identification", rows - n(NOTE_IDENT).min(rows));
    item("I040 track number", rows);
    item(
        "I080 track status (first octet always; coasting adds CST)",
        rows,
    );
    let qnh = n(NOTE_ALT_QNH);
    item(
        alt_name,
        rows - (n(NOTE_ALT_ABSENT) + n(NOTE_ALT_RANGE) + qnh).min(rows),
    );
    if !is_oa {
        item(
            "I135 altitude with QNH = 1 (ERAM under 18 000 ft, inferred)",
            qnh,
        );
    }
    item(
        "I220 rate of climb/descent",
        if is_oa {
            rows - (n(NOTE_CLIMB_ABSENT) + n(NOTE_CLIMB_RANGE)).min(rows)
        } else {
            0
        },
    );
    item(
        "I500 accuracies, APW only",
        rows - n(NOTE_ACCURACY_ABSENT).min(rows),
    );
    let _ = writeln!(
        out,
        "  never written: I060 Mode 3/A, I100, I130, I200, I290, I295, I210, I245, I340 and the rest"
    );
    let _ = writeln!(
        out,
        "  omitted or not encodable, by reason (rows of {rows}):"
    );
    for i in 0..NOTES {
        if let (Some(name), c) = (NOTE_NAMES.get(i), n(i)) {
            if c > 0 {
                let _ = writeln!(out, "    {c}: {name}");
            }
        }
    }
}

/// Run-wide counts of a system that are not per day.
fn run_lines(out: &mut String, report: &SystemReport) {
    let _ = writeln!(out, "  run totals for {} over all days:", report.name);
    let total: u64 = report.days.iter().map(|d| d.sink.records).sum();
    let _ = writeln!(out, "    records written: {total}");
    for (i, name) in REFUSAL_NAMES.iter().enumerate() {
        let _ = writeln!(
            out,
            "    rows refused, {name}: {}",
            report.refused.get(i).copied().unwrap_or(0)
        );
    }
    for (label, v) in &report.counts {
        let _ = writeln!(out, "    {label}: {v}");
    }
}

/// What the README says besides the CAT062 systems: the ADS-B file and the clip's split.
/// A7.2: cold.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras<'a> {
    pub adsb: Option<&'a AdsbReport>,
    pub clip: Option<&'a ClipInfo>,
    /// The names of the files this run wrote into the directory, as the README lists them.
    pub files: &'a [String],
    /// Which OpenATCA rows were exported, if the OpenATCA file was.
    pub openatca_rows: Option<OpenatcaRows>,
}

/// The paragraph that states which OpenATCA rows the file holds.
fn rows_mode_text(rows: OpenatcaRows) -> String {
    match rows {
        OpenatcaRows::Ticks => "OpenATCA ROWS: TICKS (--openatca-rows ticks, the default). The rows are the tracker's state once per second per track (fixed-rate ticks, coasting rows included and counted below). Each row's time is the tick's time, not the time of a measurement.".to_string(),
        OpenatcaRows::Updates => "OpenATCA ROWS: UPDATES (--openatca-rows updates). The rows are the tracker's UPDATE rows, the state at each applied ADS-B report, at the report's own time: the time of a row is a MEASUREMENT time, not a regular tick, and a track's rows are as far apart as its reports are. The one-second tick rows are not written. A row earlier than one already written is left out and counted below (the file is in time order).".to_string(),
    }
}

/// The README of the directory of `day`. `build` names the encoder build (commit).
pub fn readme(
    day: i64,
    systems: &[&SystemReport],
    build: &str,
    split: bool,
    mode: Option<InputsMode>,
) -> String {
    readme_with(day, systems, build, split, mode, &Extras::default())
}

/// [`readme`] with the ADS-B file and the clip's split described too.
pub fn readme_with(
    day: i64,
    systems: &[&SystemReport],
    build: &str,
    split: bool,
    mode: Option<InputsMode>,
    extras: &Extras<'_>,
) -> String {
    let mut o = String::new();
    let date = date_text(day);
    let has = |name: &str| systems.iter().any(|s| s.name == name);
    let (has_oa, has_eram, has_adsb) = (has("OpenATCA"), has("ERAM"), extras.adsb.is_some());
    // The names of the CAT062 systems present, and the title and import line that follow.
    let names: Vec<&str> = [(has_oa, "OpenATCA"), (has_eram, "ERAM")]
        .iter()
        .filter_map(|&(h, n)| h.then_some(n))
        .collect();
    let title = match (names.is_empty(), has_adsb) {
        (false, true) => format!(
            "{} tracks as ASTERIX CAT062 and the ADS-B reports as CAT021",
            names.join(" and ")
        ),
        (false, false) => format!("{} tracks as ASTERIX CAT062", names.join(" and ")),
        (true, _) => "ADS-B reports as ASTERIX CAT021".to_string(),
    };
    let _ = writeln!(o, "{title}, for evaluation in OpenATS COMPASS\n");
    let _ = writeln!(o, "UTC date to give at import (UTC Day): {date}");
    let _ = writeln!(
        o,
        "  I062/070 and I021/073 are times of day (UTC, 1/128 s since midnight) with no date; every record in this directory is on {date}."
    );
    if split {
        let _ = writeln!(
            o,
            "  The export was split by UTC day: one directory per day, each with its own files and this note; a day's files never hold another day's rows."
        );
    }
    let _ = writeln!(
        o,
        "\nFiles written by this run in this directory: {}.",
        if extras.files.is_empty() {
            "(not listed)".to_string()
        } else {
            extras.files.join(", ")
        }
    );
    if let Some(rows) = extras.openatca_rows.filter(|_| has_oa) {
        let _ = writeln!(o, "{}\n", rows_mode_text(rows));
    }
    if !names.is_empty() {
        let _ = writeln!(o, "Format: ASTERIX category 062 Edition 1.21 (EUROCONTROL-SPEC-0149-9), raw data blocks (CAT, LEN, records), no framing, records in time order, up to 256 records per block.");
    }
    let _ = writeln!(
        o,
        "Encoder: oa-asterix of this repository, written from the specification; build {build}."
    );
    let ids: Vec<String> = [
        (has_oa, "OpenATCA", OPENATCA_SOURCE),
        (has_eram, "ERAM", ERAM_SOURCE),
        (has_adsb, "ADS-B", ADSB_SOURCE),
    ]
    .iter()
    .filter_map(|&(h, n, s)| h.then(|| format!("{n} {}/{}", s.sac, s.sic)))
    .collect();
    let _ = writeln!(o, "Data sources are in data_sources.json (COMPASS content version 0.2), one entry per system in this directory. SAC 255 is a test range chosen by this export, not an allocation ({}; 255/1 is left free because COMPASS uses it for its own reference): the specification's SAC list is kept by the ASTERIX Manager and the editions held here carry no test range. Do not use it on a live link.", ids.join(", "));
    let mut imports: Vec<String> = Vec::new();
    if !names.is_empty() {
        let files: Vec<String> = names
            .iter()
            .map(|n| format!("{}.ast", n.to_lowercase()))
            .collect();
        imports.push(format!("{} as ASTERIX CAT062", files.join(" and ")));
    }
    if has_adsb {
        imports.push("adsb.ast as ASTERIX CAT021 (Ed 2.4)".to_string());
    }
    let _ = writeln!(
        o,
        "Import {}, with the UTC Day above; nothing in the files needs a time-of-day override.\n",
        imports.join(" and ")
    );
    if has_oa || has_eram {
        let mon = match (has_oa, has_eram) {
            (true, true) => "MON = 1 (monosensor) for OpenATCA because its tracker is fed ADS-B only, one kind of sensor; for ERAM MON = 0 and CNF = 0 are ASSUMPTIONS: the feed carries neither (\"not in the feed\"), so they are written as defaults and counted as a note on every ERAM row, not stated as facts about ERAM",
            (true, false) => "MON = 1 (monosensor) because the tracker is fed ADS-B only, one kind of sensor",
            _ => "MON = 0 and CNF = 0 are ASSUMPTIONS: the feed carries neither (\"not in the feed\"), so they are written as defaults and counted as a note on every ERAM row, not stated as facts about ERAM",
        };
        let cnf = if has_oa {
            " OpenATCA's CNF = 0 (confirmed): the tracker publishes confirmed tracks only. The tracker's O_IDENTITY_UNCONFIRMED flag (the key is a local radar handle, not an aircraft address) is NOT written as CNF = 1: CNF marks a tentative track, and that flag means an unconfirmed identity, which is another thing; its rows are counted below (none in an ADS-B-only run)."
        } else {
            ""
        };
        let _ = writeln!(o, "Nothing is invented: an item the source row does not have is omitted, and counted below by reason. The velocity is the track's, east and north (Vx, Vy). I080: {mon}; SPI, MRH and SRC at 0 (I130 is never sent); CST (coasting) from the source's coast flag; OpenATCA's radar-disagrees flag is AFF.{cnf}\n");
    }
    if has_oa {
        let _ = writeln!(o, "Altitude (OpenATCA): the tracker's altitude is ADS-B barometric (pressure) altitude, filtered: it is a CALCULATED altitude, I135 with QNH = 0. OpenATCA publishes no measured flight level (I136), so COMPASS's \"Mode C Present\" does not apply to it.\n");
    }
    if has_eram {
        let _ = writeln!(o, "Altitude (ERAM): the altitude is the SFDPS field enRoute/position/altitude, in feet, which the feed gives as a surveillance altitude without saying what it is referenced to; it is NOT established as pressure altitude. This project's own comparison (docs/M2_DESIGN.md, altitude reference for T4) found values under 18 000 ft altimeter-corrected (about 400 ft high against pressure altitude on this day) and values at or above 18 000 ft flight levels. The export applies that rule, an inference and not a statement of the feed: at or above 18 000 ft the value is I136 (measured flight level), below it I135 with QNH = 1.\n");
        let _ = writeln!(o, "ERAM duplicates: samples are compared on latitude, longitude, altitude, velocity east and north and the coast flag; a sample with the same flight, position time and those six values as an earlier one is a duplicate and is dropped (a row differing only in receive time is a duplicate). Same flight and position time with any difference are both kept and counted.\n");
    }
    match mode {
        Some(InputsMode::All) => {
            let _ = writeln!(o, "OpenATCA inputs: ALL (--inputs all). The tracker's ADS-B adapter was offered every ADS-B report of the day's clip (it keeps those that pass its quality gates, counted below), as ERAM gets all its inputs, so the two files are like-for-like in what they were given (ERAM additionally has radar, and ADS-B reports from its own receivers, which OpenATCA does not). Because every report was offered to it, no half of the clip is independent of this OpenATCA output: a reference built from the same ADS-B clip is not independent truth for it.\n");
        }
        Some(InputsMode::Split) => {
            let _ = writeln!(o, "OpenATCA inputs: SPLIT (--inputs split). The tracker's adapter was offered set A only, half of the ADS-B reports (the scorer's split); ERAM got all its inputs, so the two files are NOT like-for-like in what they were given. Set B, the other half, is the only half independent of this OpenATCA output: a reference built from set A would be leaked truth.\n");
        }
        None => {}
    }
    if has_oa || has_eram {
        let _ = writeln!(o, "{I500_NOTE}\n");
    }
    if let Some(c) = extras.clip {
        let _ = writeln!(o, "{}\n", dedup_text(c, mode));
    }
    for s in systems {
        system_section(&mut o, s, day);
        let _ = writeln!(o);
    }
    if let Some(a) = extras.adsb {
        adsb_section(&mut o, a, day, extras.clip);
        let _ = writeln!(o);
    }
    for s in systems {
        run_lines(&mut o, s);
    }
    if let Some(a) = extras.adsb {
        adsb_run_lines(&mut o, a);
    }
    o
}

/// The duplicate and conflict handling of the clip, with its counts.
fn dedup_text(c: &ClipInfo, mode: Option<InputsMode>) -> String {
    let st = &c.split;
    let halving = if mode == Some(InputsMode::Split) {
        "on (--inputs split)"
    } else {
        "off (--inputs all)"
    };
    format!(
        "ADS-B reports before the tracker: the clip's reports go through the same function the scorer's split uses, oa-track's split_reports_with, with the halving {halving}. It removes copies (a report with the address, time and content of one already kept), puts the quality fields through the same carry-forward handling, and keeps every report of different content at one (address, time): the conflict rule is KEEP ALL such reports (in one fixed order), also with the halving off, because nothing says which one is right. Counts: {} reports read, {} aircraft, {} duplicate reports removed, {} conflicting (address, time) groups holding {} reports, {} unreadable rows left out; {} reports offered to the tracker's adapter and to adsb.ast, {} not offered.",
        st.rows_in, st.aircraft, st.duplicates_removed, st.conflict_groups, st.conflict_rows, st.rows_invalid, c.offered, c.not_offered
    )
}

/// The ADS-B file's section for `day`.
fn adsb_section(out: &mut String, report: &AdsbReport, day: i64, clip: Option<&ClipInfo>) {
    let _ = writeln!(
        out,
        "ADS-B reports, CAT021 Ed 2.4  (SAC {} SIC {})",
        report.source.sac, report.source.sic
    );
    let Some(d) = report.days.iter().find(|d| d.day == day) else {
        let _ = writeln!(out, "  no reports on this day; no file written");
        return;
    };
    adsb_day_lines(out, d, clip);
}

fn adsb_day_lines(out: &mut String, d: &AdsbDayReport, clip: Option<&ClipInfo>) {
    let rows = d.stats.records;
    let n = |i: usize| d.notes.get(i).copied().unwrap_or(0);
    let _ = writeln!(out, "  file: {}", d.file);
    let _ = writeln!(
        out,
        "  format: ASTERIX category 021 Edition 2.4 (EUROCONTROL-SPEC-0149-12, Released Edition 2.4), the edition asked for; raw data blocks, records in time order, up to 256 records per block. The items used are the same in Ed 2.7 except for I021/090 (Ed 2.7 adds SRC and a fourth extension), I021/040 (two more extensions) and I021/210 (VN 3); the table of differences is in the oa-compass crate documentation."
    );
    let _ = writeln!(
        out,
        "  records written: {rows} in {} data blocks, {} octets",
        d.stats.blocks, d.stats.bytes
    );
    let _ = writeln!(
        out,
        "  time span: {} to {} UTC (I073, rounded to 1/128 s)",
        clock_text(d.stats.first_time),
        clock_text(d.stats.last_time)
    );
    let _ = writeln!(
        out,
        "  verification: every block was decoded again with this repository's Ed 2.4 decoder and equals the records encoded; the file was read back from disk ({} records, {} blocks) and equals what was written; largest position difference between report and decoded record {} / {} (1e-7 degree, latitude / longitude; limit {MAX_POSITION_ERROR_E7}, enforced: a larger difference stops the export). This shows that our encoder and decoder agree with each other and with the item definitions the tests were worked out from by hand; it is not an outside conformance test",
        d.read_back_records, d.read_back_blocks, d.stats.max_lat_error_e7, d.stats.max_lon_error_e7
    );
    let _ = writeln!(out, "  items present (records with the item, of {rows}):");
    let mut item = |name: &str, present: u64| {
        let _ = writeln!(out, "    {name}: {present}");
    };
    let less = |a: u64| rows - a.min(rows);
    item("I010 data source", rows);
    item("I040 target report descriptor, primary and first extension (ATP, ARC 2 unknown, GBS, CL 2 no information)", rows);
    item("I073 time of message reception of position", rows);
    item("I080 target address", rows);
    item(
        "I131 position in WGS-84, high resolution (I130 is not sent)",
        rows,
    );
    item(
        "I145 flight level",
        less(n(ANOTE_FL_ABSENT) + n(ANOTE_FL_RANGE)),
    );
    item(
        "I140 geometric height",
        less(n(ANOTE_HEIGHT_ABSENT) + n(ANOTE_HEIGHT_RANGE)),
    );
    item(
        "I155 barometric or I157 geometric vertical rate (as the report says)",
        less(n(ANOTE_RATE_ABSENT) + n(ANOTE_RATE_RANGE)),
    );
    item(
        "I160 airborne ground vector (none on the ground)",
        less(n(ANOTE_VECTOR_ABSENT) + n(ANOTE_VECTOR_RANGE) + n(ANOTE_VECTOR_GROUND)),
    );
    item(
        "I170 target identification",
        less(n(ANOTE_IDENT_ABSENT) + n(ANOTE_IDENT_CHARS)),
    );
    item(
        "I020 emitter category",
        less(n(ANOTE_CATEGORY_ABSENT) + n(ANOTE_CATEGORY_NO_CODE)),
    );
    item(
        "I090 quality indicators (in every record; the extensions only when a bit in them is 1)",
        rows,
    );
    item(
        "I210 MOPS version",
        less(n(ANOTE_MOPS_ABSENT) + n(ANOTE_MOPS_VERSION)),
    );
    let _ = writeln!(
        out,
        "  not encoded although the source has them (outside the requested item list): I070 Mode 3/A code, I150/I151 air speed, I152 magnetic heading, I230 roll angle, I165 track angle rate, I200 target status; and I130, I072, I074, I076, I077, I015, I146, I148, I110 and the rest, which the source does not have"
    );
    let _ = writeln!(
        out,
        "  omitted or not encodable, by reason (records of {rows}):"
    );
    for i in 0..ADSB_NOTES {
        if let (Some(name), c) = (ADSB_NOTE_NAMES.get(i), n(i)) {
            if c > 0 {
                let _ = writeln!(out, "    {c}: {name}");
            }
        }
    }
    let _ = writeln!(out, "  how the values were made:");
    for l in ADSB_BASIS {
        let _ = writeln!(out, "    {l}");
    }
    let _ = writeln!(out, "    {}", tracker_relation(clip, rows));
    let _ = writeln!(out, "    {INDEPENDENCE}");
}

/// How the ADS-B file's values were made and what it is (README lines).
const ADSB_BASIS: [&str; 9] = [
    "Time: I073 is the report's own time, when the receiver network stamped it (1/128 s, rounded to nearest, error at most 1/256 s). The source has no avionics time of applicability (I071) and no separate reception time for velocity (I075).",
    "Position: I131, 180/2^30 degrees, from the 1e-7 degree of the source; a longitude of +180 is written as -180.",
    "Altitude: I145 is the barometric (pressure) altitude of the report in 1/4 FL (25 ft steps, rounding at most 12.5 ft); I140 is the geometric height in 6.25 ft steps. Neither is clipped: a value outside the item's range is omitted and counted.",
    "I040: ATP is 0 for an ICAO address and 3 (anonymous) for a non-ICAO one; Ed 2.4 does not say how to code the latter, and the Ed 2.7 note recommends 3. ARC is 2 (unknown), CL is 2 (no information), and DCR, SIM, TST, SAA, RC and RAB are the specification's default 0 because the source has none of them.",
    "I090 and I210: values are those of the report after the scorer's quality handling, which carries the last known NIC, NACp, NACv, SIL and version forward within an aircraft's sequence; a field the aircraft never sent is written 0, the code for unknown. Defaults written because the format has no absent value: NICBARO is not in the source and Ed 2.4 has no unknown code for it, so it is written 0 (in ED-102A/DO-260B: barometric altitude not cross-checked, the conservative value); I210 LTT is written 0 (other, claiming neither 1090 ES, UAT nor VDL 4) because the source does not say which link carried the report; SILS, when the report has no SIL type, is written 0 (per flight-hour, the default; the bit has no unknown code); DCR, SIM, TST, SAA, RC, RAB are written 0, the specification's defaults. PIC is omitted (the extension is optional). NACv and NIC are to be read with the version in I210.",
    "Source filter: only reports of ADS-B source types (adsb_icao, adsb_icao_nt, adsb_other; or a clip without a source column) are written; other source types and positions the source marks stale are left out and counted.",
    "I040 GBS is written from the report's ground flag: 1 when the source says ground, 0 otherwise. The source sets the flag only when it says ground, so a 0 asserts \"not on ground\" without the source saying so. I160 is the airborne vector (5.2.30) and is omitted for a report on the ground.",
    "NIC under MOPS version 2: Ed 2.4 says version 2 NIC values shall be mapped accordingly to the primary subfield; no mapping table is in the documents held, so the raw NIC is written and that mapping is UNVERIFIED. NIC and NACp above 11 and NACv above 4 are not encoded.",
    "Where a field is written 0 because the report lacks it: 0 means unknown for NIC, NACp, NACv, SIL, SDA and GVA according to the MOPS (ED-102A/DO-260B), not according to Ed 2.4 itself, which gives these items no value text; for NICBARO (0 = not cross-checked), SILS (0 = per flight-hour), LTT (0 = other) and the I040 bits the format has no unknown code and the 0 is a claim, counted above.",
];

/// The relation between `adsb.ast` and what the tracker's adapter was offered.
fn tracker_relation(clip: Option<&ClipInfo>, written: u64) -> String {
    let mut t = format!(
        "RELATION TO THE TRACKER'S INPUT: adsb.ast holds the reports after the duplicate removal above, minus those left out here (positions marked stale, source types that are not ADS-B, impossible positions): {written} reports on this day. It is NOT the set the tracker used. The tracker's own adapter (oa-truth's quality gates, run by oa-track) also drops reports below its quality gates (NACp >= {}, NIC >= {}, SIL = {}, MOPS version >= {}) and leaves out TIS-B, ADS-R and MLAT reports and stale rows too, so adsb.ast is a superset of the reports the tracker used.",
        oa_track::adsb::AdsbConfig::default().gates.min_nacp,
        oa_track::adsb::AdsbConfig::default().gates.min_nic,
        oa_track::adsb::AdsbConfig::default().gates.required_sil,
        oa_track::adsb::AdsbConfig::default().gates.min_version,
    );
    if let Some(c) = clip {
        let g = &c.gate;
        let _ = write!(
            t,
            " Of the {} reports of the whole clip offered to the tracker, {} became measurements (the \"measurements offered to the tracker\" total) and {} were dropped by its adapter:",
            g.rows_in,
            g.passed,
            g.rows_in.saturating_sub(g.passed)
        );
        for (name, &n) in oa_truth::REMOVE_NAMES.iter().zip(g.removed.iter()) {
            if n > 0 {
                let _ = write!(t, " {name} {n};");
            }
        }
    }
    t
}

/// The independence statement (UK-008).
const INDEPENDENCE: &str = "INDEPENDENCE: every report in adsb.ast comes from the same clip whose reports were offered to the OpenATCA tracker. A reference or track built from it is therefore NOT independent of OpenATCA's inputs (this project's uncertainty UK-008: the ADS-B reports are not an independent truth for the OpenATCA output). ERAM also uses ADS-B: its inputs include ADS-B reports (from its own receivers, not this clip), so ADS-B is not independent of ERAM's output either.";

fn adsb_run_lines(out: &mut String, report: &AdsbReport) {
    let _ = writeln!(out, "  run totals for ADS-B over all days:");
    let total: u64 = report.days.iter().map(|d| d.stats.records).sum();
    let _ = writeln!(out, "    records written: {total}");
    for (i, name) in [
        "position beyond 90 / 180 degrees",
        "time does not fit a day",
    ]
    .iter()
    .enumerate()
    {
        let _ = writeln!(
            out,
            "    reports refused, {name}: {}",
            report.refused.get(i).copied().unwrap_or(0)
        );
    }
    for (label, v) in &report.counts {
        let _ = writeln!(out, "    {label}: {v}");
    }
}

/// How the I500 accuracy of OpenATCA is derived, as a line for the README (the full statement
/// is the module documentation of `core::convert`).
pub const I500_NOTE: &str = "I500 (OpenATCA only): subfield APW, the standard deviation of the position in latitude and in longitude, LSB 180/2^25 degrees, from the tracker's position covariance: sigma_north = sqrt(nn) and sigma_east = sqrt(ee) in metres, divided by the WGS-84 meridian radius and by (prime vertical radius * cos latitude); the east-north correlation has no place in APW and is dropped. The tracker's 95% radius r95 is 2.4477 * sqrt(larger eigenvalue), so for a circular error each component is r95 / 2.4477. ERAM publishes no accuracy: the item is omitted for ERAM.";
