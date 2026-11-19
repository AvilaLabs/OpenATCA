//! Shell (B1): the text files that go beside the data: `data_sources.json` (COMPASS data source
//! definition, content version 0.2) and `README.txt` (what to give at import, what is in each
//! file and what is not). `oa-json` only decodes, so the JSON is built by hand from fixed names
//! and numbers.

use std::fmt::Write as _;

use oa_asterix::SourceId;

use super::export::{date_text, DayReport, SystemReport};
use super::run::InputsMode;
use crate::core::convert::{
    NOTES, NOTE_ACCURACY_ABSENT, NOTE_ADDRESS, NOTE_ALT_ABSENT, NOTE_ALT_QNH, NOTE_ALT_RANGE, NOTE_CLIMB_ABSENT,
    NOTE_CLIMB_RANGE, NOTE_IDENT, NOTE_NAMES, NOTE_SERVICE, NOTE_VEL_ABSENT, NOTE_VEL_RANGE,
    REFUSAL_NAMES,
};

/// The data source of the OpenATCA tracker output.
///
/// The ASTERIX specification (Part 1 Ed 3.1, 6.2.1) assigns SACs through the ASTERIX Manager and
/// the editions held here carry no list and no test range. This export therefore uses a pair of
/// its own, SAC 255 with SIC 1 for OpenATCA and SIC 2 for ERAM: a test pair chosen by us, not an
/// allocation. It must not be used on a live link; check the published SAC list before using
/// any pair outside the lab.
pub const OPENATCA_SOURCE: SourceId = SourceId { sac: 255, sic: 1 };
/// The data source of the ERAM tracks (see [`OPENATCA_SOURCE`]).
pub const ERAM_SOURCE: SourceId = SourceId { sac: 255, sic: 2 };

/// The COMPASS data-source file for the two systems.
pub fn data_sources_json() -> String {
    let one = |name: &str, s: SourceId| {
        format!(
            "{{\"ds_type\":\"Tracker\",\"name\":\"{name}\",\"sac\":{},\"sic\":{}}}",
            s.sac, s.sic
        )
    };
    format!(
        "{{\"content_type\":\"data_sources\",\"content_version\":\"0.2\",\"data_sources\":[{},{}]}}\n",
        one("OpenATCA", OPENATCA_SOURCE),
        one("ERAM", ERAM_SOURCE)
    )
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
    let _ = writeln!(
        out,
        "  verification: every block was decoded again with this repository's oa-asterix decoder and equals the records encoded; the file was read back from disk ({} records, {} blocks) and equals what was written; largest position difference between source row and decoded record {} / {} (1e-7 degree, latitude / longitude; limit 27). This shows that our encoder and decoder agree with each other; it is not an outside conformance test (the encoder was written from, and reviewed item by item against, the Ed 1.21 text; the first outside decode is the COMPASS import)",
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
    item("I015 service identification (1)", if is_oa { rows - n(NOTE_SERVICE).min(rows) } else { 0 });
    item("I070 time of track information", rows);
    item("I105 position in WGS-84", rows);
    item("I185 velocity", rows - (n(NOTE_VEL_ABSENT) + n(NOTE_VEL_RANGE)).min(rows));
    item("I380 target address", rows - n(NOTE_ADDRESS).min(rows));
    item("I380 target identification", rows - n(NOTE_IDENT).min(rows));
    item("I040 track number", rows);
    item("I080 track status (first octet always; coasting adds CST)", rows);
    let qnh = n(NOTE_ALT_QNH);
    item(alt_name, rows - (n(NOTE_ALT_ABSENT) + n(NOTE_ALT_RANGE) + qnh).min(rows));
    if !is_oa {
        item("I135 altitude with QNH = 1 (ERAM under 18 000 ft, inferred)", qnh);
    }
    item(
        "I220 rate of climb/descent",
        if is_oa {
            rows - (n(NOTE_CLIMB_ABSENT) + n(NOTE_CLIMB_RANGE)).min(rows)
        } else {
            0
        },
    );
    item("I500 accuracies, APW only", rows - n(NOTE_ACCURACY_ABSENT).min(rows));
    let _ = writeln!(
        out,
        "  never written: I060 Mode 3/A, I100, I130, I200, I290, I295, I210, I245, I340 and the rest"
    );
    let _ = writeln!(out, "  omitted or not encodable, by reason (rows of {rows}):");
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

/// The README of the directory of `day`. `build` names the encoder build (commit).
pub fn readme(
    day: i64,
    systems: &[&SystemReport],
    build: &str,
    split: bool,
    mode: Option<InputsMode>,
) -> String {
    let mut o = String::new();
    let date = date_text(day);
    let _ = writeln!(o, "OpenATCA and ERAM tracks as ASTERIX CAT062, for evaluation in OpenATS COMPASS\n");
    let _ = writeln!(o, "UTC date to give at import (UTC Day): {date}");
    let _ = writeln!(
        o,
        "  I062/070 is a time of day (UTC, 1/128 s since midnight) with no date; every record in this directory is on {date}."
    );
    if split {
        let _ = writeln!(
            o,
            "  The export was split by UTC day: one directory per day, each with its own files and this note; a day's files never hold another day's rows."
        );
    }
    let _ = writeln!(o, "\nFormat: ASTERIX category 062 Edition 1.21 (EUROCONTROL-SPEC-0149-9), raw data blocks (CAT, LEN, records), no framing, records in time order, up to 256 records per block.");
    let _ = writeln!(o, "Encoder: oa-asterix of this repository, written from the specification; build {build}.");
    let _ = writeln!(o, "Data sources are in data_sources.json (COMPASS content version 0.2). SAC 255 is a test pair chosen by this export, not an allocation: the specification's SAC list is kept by the ASTERIX Manager and the editions held here carry no test range. Do not use it on a live link.");
    let _ = writeln!(o, "Import each .ast file as ASTERIX CAT062 with the UTC Day above; nothing in the files needs a time-of-day override.\n");
    let _ = writeln!(o, "Nothing is invented: an item the source row does not have is omitted, and counted below by reason. The velocity is the track's, east and north (Vx, Vy). I080: CNF = 0 (confirmed: the tracker publishes confirmed tracks only); MON = 1 (monosensor) for OpenATCA because its tracker is fed ADS-B only, one kind of sensor, and MON = 0 for ERAM because it fuses radar and ADS-B; SPI, MRH and SRC at 0 (I130 is never sent); CST (coasting) from the source's coast flag; OpenATCA's radar-disagrees flag is AFF.\n");
    let _ = writeln!(o, "Altitude: OpenATCA's altitude is ADS-B barometric (pressure) altitude, filtered: I135 with QNH = 0. ERAM's altitude is the SFDPS field enRoute/position/altitude, in feet, which the feed gives as a surveillance altitude without saying what it is referenced to; it is NOT established as pressure altitude. This project's own comparison (docs/M2_DESIGN.md, altitude reference for T4) found values under 18 000 ft altimeter-corrected (about 400 ft high against pressure altitude on this day) and values at or above 18 000 ft flight levels. The export applies that rule, an inference and not a statement of the feed: at or above 18 000 ft the value is I136 (measured flight level), below it I135 with QNH = 1.\n");
    let _ = writeln!(o, "ERAM duplicates: samples are compared on latitude, longitude, altitude, velocity east and north and the coast flag; a sample with the same flight, position time and those six values as an earlier one is a duplicate and is dropped (a row differing only in receive time is a duplicate). Same flight and position time with any difference are both kept and counted.\n");
    match mode {
        Some(InputsMode::All) => {
            let _ = writeln!(o, "OpenATCA inputs: ALL (--inputs all). The tracker was fed every ADS-B report of the day's clip, as ERAM gets all its inputs, so the two files are like-for-like in what they were given (ERAM additionally has radar, and ADS-B reports from its own receivers, which OpenATCA does not). Because every report was used, no half of the clip is independent of this OpenATCA output: a reference built from the same ADS-B clip is not independent truth for it.\n");
        }
        Some(InputsMode::Split) => {
            let _ = writeln!(o, "OpenATCA inputs: SPLIT (--inputs split). The tracker was fed set A only, half of the ADS-B reports (the scorer's split); ERAM got all its inputs, so the two files are NOT like-for-like in what they were given. Set B, the other half, is the only half independent of this OpenATCA output: a reference built from set A would be leaked truth.\n");
        }
        None => {}
    }
    let _ = writeln!(o, "{I500_NOTE}\n");
    for s in systems {
        system_section(&mut o, s, day);
        let _ = writeln!(o);
    }
    for s in systems {
        run_lines(&mut o, s);
    }
    o
}

/// How the I500 accuracy of OpenATCA is derived, as a line for the README (the full statement
/// is the module documentation of `core::convert`).
pub const I500_NOTE: &str = "I500 (OpenATCA only): subfield APW, the standard deviation of the position in latitude and in longitude, LSB 180/2^25 degrees, from the tracker's position covariance: sigma_north = sqrt(nn) and sigma_east = sqrt(ee) in metres, divided by the WGS-84 meridian radius and by (prime vertical radius * cos latitude); the east-north correlation has no place in APW and is dropped. The tracker's 95% radius r95 is 2.4477 * sqrt(larger eigenvalue), so for a circular error each component is r95 / 2.4477. ERAM publishes no accuracy: the item is omitted for ERAM.";
