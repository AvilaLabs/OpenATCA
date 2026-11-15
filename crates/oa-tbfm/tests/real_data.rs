//! Whole-corpus tests against real TBFM messages. They need `OPENATCA_SCDS` (a directory searched
//! recursively for `*.msg` files inside directories named `tbfm*`) and skip when it is unset;
//! the cross-check also needs `OPENATCA_SNAPSHOT` (an `oa-adapt build` snapshot of one ARTCC).
//! The data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_tbfm::{
    clear_rows, cross_check, decode_bytes, default_limits, error_kind_name, field_name,
    format_cross_check, new_rows, new_stats, observe, rows_len, DocError, MsgKind, Rows,
    FIELD_COUNT, MSG_KIND_NAMES, REJECT_NAMES,
};
use oa_xml::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, in_tbfm: bool, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            collect(&p, in_tbfm || name.starts_with("tbfm"), out);
        } else if in_tbfm && p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
}

fn corpus() -> Option<Vec<Vec<u8>>> {
    let Some(dir) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real TBFM data test");
        return None;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), false, &mut files);
    if files.is_empty() {
        eprintln!(
            "no tbfm*/ *.msg files under {}: skipping",
            dir.to_string_lossy()
        );
        return None;
    }
    Some(files.iter().map(|f| std::fs::read(f).unwrap()).collect())
}

fn decode_all(data: &[Vec<u8>], rows: &mut Rows, keep: bool) -> oa_tbfm::Stats {
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut stats = new_stats();
    for bytes in data {
        let before = rows_len(rows);
        let r = decode_bytes(&mut parser, bytes, rows);
        observe(&mut stats, &r, &rows.section[before..]);
        if !keep {
            clear_rows(rows);
        }
    }
    stats
}

// REQ: OA-TBFM-016
#[test]
fn every_real_tbfm_message_decodes_and_is_reported() {
    let Some(data) = corpus() else { return };
    let total: usize = data.iter().map(Vec::len).sum();

    // Pass 1, keeping every row, for the distinct counts and the field-error detail.
    let mut rows = new_rows(200_000);
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut stats = new_stats();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    for bytes in &data {
        let before = rows_len(&rows);
        let r = decode_bytes(&mut parser, bytes, &mut rows);
        observe(&mut stats, &r, &rows.section[before..]);
        if let Err(DocError::Field(f)) = &r {
            *errors
                .entry(format!(
                    "{} {}",
                    field_name(f.field),
                    error_kind_name(f.kind)
                ))
                .or_default() += 1;
        }
    }
    let mut flights = BTreeSet::new();
    let mut gufis = BTreeSet::new();
    let mut airports = BTreeSet::new();
    let mut fixes = BTreeSet::new();
    let mut centers = BTreeSet::new();
    for i in 0..rows_len(&rows) {
        flights.insert((rows.center[i], rows.tma_id[i]));
        if rows.gufi[i] != [0; 12] {
            gufis.insert(rows.gufi[i]);
        }
        for a in [rows.apt[i], rows.dap[i]] {
            if a != [0; 4] {
                airports.insert(a);
            }
        }
        if rows.mfx[i] != [0; 8] {
            fixes.insert(rows.mfx[i]);
        }
        centers.insert(rows.center[i]);
    }

    eprintln!(
        "TBFM real data: {} messages, {} bytes",
        stats.messages, total
    );
    eprintln!("  accepted {}, rows {}", stats.accepted, stats.rows);
    for (k, n) in REJECT_NAMES.iter().zip(stats.rejects) {
        eprintln!("  rejects by reason {k}: {n}");
    }
    for (k, n) in MSG_KIND_NAMES.iter().zip(stats.kinds) {
        eprintln!("  kind {k}: {n}");
    }
    eprintln!(
        "  rows by section (flt, mrp, eta, sta, sch): {:?}",
        &stats.sections[1..]
    );
    assert_eq!(stats.field_errors.len(), FIELD_COUNT);
    eprintln!(
        "  field errors by field: {} (none expected)",
        stats.field_errors.iter().sum::<u64>()
    );
    for (k, n) in &errors {
        eprintln!("  field error {k}: {n}");
    }
    eprintln!(
        "  unknown paths {}, ignored paths {}",
        stats.unknown_paths, stats.ignored_paths
    );
    eprintln!(
        "  distinct: {} flights (centre + tmaId), {} GUFIs, {} airports, {} meter fixes, {} centres",
        flights.len(),
        gufis.len(),
        airports.len(),
        fixes.len(),
        centers.len()
    );

    assert_eq!(stats.messages as usize, data.len());
    assert_eq!(
        stats.rejects.iter().sum::<u64>(),
        0,
        "rejects: {:?}, field errors: {errors:?}",
        stats.rejects
    );
    assert_eq!(
        stats.unknown_paths, 0,
        "the survey says every path is known"
    );
    eprintln!(
        "  flights refused alone {}, schedule-flag words kept as unknown {}, speeds of unknown unit {}",
        stats.rejected_flights, stats.unknown_words, stats.unknown_units
    );
    assert_eq!(
        (
            stats.rejected_flights,
            stats.unknown_words,
            stats.unknown_units
        ),
        (0, 0, 0),
        "field errors: {:?}",
        stats.field_errors
    );
    // Every stored speed has a unit, and the sample has both knots and Mach.
    let mut units = [0u64; oa_tbfm::spd_unit::COUNT];
    for i in 0..rows_len(&rows) {
        assert_eq!(
            rows.spd_milli[i] == oa_tbfm::NO_NUM,
            rows.spd_unit[i] == oa_tbfm::spd_unit::NONE,
            "row {i}: a speed and its unit go together"
        );
        units[usize::from(rows.spd_unit[i])] += 1;
    }
    eprintln!(
        "  speeds by unit: knots {}, mach {}, unknown {}",
        units[usize::from(oa_tbfm::spd_unit::KNOTS)],
        units[usize::from(oa_tbfm::spd_unit::MACH)],
        units[usize::from(oa_tbfm::spd_unit::UNKNOWN)]
    );
    assert!(units[usize::from(oa_tbfm::spd_unit::KNOTS)] > 0);
    assert!(units[usize::from(oa_tbfm::spd_unit::MACH)] > 0);
    assert!(stats.rows > 0);
    assert!(stats.kinds[MsgKind::Air as usize] > 0);

    // Pass 2, timing only (the files are already in memory; rows are discarded per message).
    let mut scratch = new_rows(oa_tbfm::DEFAULT_ROW_CAPACITY);
    let t = Instant::now();
    let reps = 5;
    for _ in 0..reps {
        let s = decode_all(&data, &mut scratch, false);
        assert_eq!(s.rejects.iter().sum::<u64>(), 0);
    }
    let secs = t.elapsed().as_secs_f64();
    let per_s = (data.len() * reps) as f64 / secs;
    eprintln!(
        "  throughput {:.0} messages/s, {:.1} MB/s (parse + decode, one thread)",
        per_s,
        (total * reps) as f64 / secs / 1e6
    );
    if cfg!(debug_assertions) {
        eprintln!("  (debug build: the 20,000 messages/s requirement is checked in release)");
    } else {
        assert!(
            per_s >= 20_000.0,
            "throughput {per_s:.0} messages/s is below 20,000"
        );
    }
}

// REQ: OA-TBFM-017
#[test]
fn real_meter_fixes_and_airports_resolve_against_the_model() {
    let Some(data) = corpus() else { return };
    let Some(snap) = std::env::var_os("OPENATCA_SNAPSHOT") else {
        eprintln!("OPENATCA_SNAPSHOT is not set: skipping the model cross-check");
        return;
    };
    let model = oa_airspace::snapshot::decode(&std::fs::read(snap).unwrap()).unwrap();
    let mut rows = new_rows(200_000);
    let stats = decode_all(&data, &mut rows, true);
    assert_eq!(stats.rejects.iter().sum::<u64>(), 0);
    let x = cross_check(&model, &rows);
    eprintln!("model ARTCC {}", model.meta.artcc);
    eprint!("{}", format_cross_check(&x));
    // Structure only (no identifiers printed): what the unresolved own-centre meter fixes share.
    let mp = x
        .own
        .unresolved_fixes
        .iter()
        .filter(|f| f.ends_with("MP"))
        .count();
    eprintln!(
        "  own unresolved meter fixes: {}, of which {} end in \"MP\"",
        x.own.unresolved_fixes.len(),
        mp
    );
    // The model's own centre must contribute rows, and at least some of its fixes and
    // airports must resolve, or the link between feed and model is broken.
    assert!(
        x.own.meter_fix.rows > 0,
        "no rows from the model's own centre"
    );
    assert!(x.own.meter_fix.resolved > 0);
    assert!(x.own.arrival_airport.resolved > 0);
}
