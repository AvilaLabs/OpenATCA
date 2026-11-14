//! Whole-corpus tests against real FNS messages. They need `OPENATCA_SCDS` (a directory searched
//! recursively for `*.msg` files inside directories named `fns*`) and skip when it is unset; the
//! cross-check also needs `OPENATCA_SNAPSHOT` (an `oa-adapt build` snapshot of one ARTCC). The
//! data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_notam::{
    clear_notams, cross_check, decode_bytes, default_notams, feat_name, field_name,
    format_cross_check, has_circle, new_decoder, new_notams, new_stats, polygons_of, unknown_text,
    Decoder, Field, Limits, Notams, Stats, ALL_FIELDS, FEAT_COUNT, FIELD_COUNT, REJECT_SLOT_NAMES,
};
use oa_xml::Parser;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, in_fns: bool, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            collect(&p, in_fns || name.starts_with("fns"), out);
        } else if in_fns && p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
}

fn corpus() -> Option<Vec<Vec<u8>>> {
    let Some(dir) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real FNS data test");
        return None;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), false, &mut files);
    if files.is_empty() {
        eprintln!(
            "no fns*/ *.msg files under {}: skipping",
            dir.to_string_lossy()
        );
        return None;
    }
    Some(files.iter().map(|f| std::fs::read(f).unwrap()).collect())
}

fn decode_all(
    data: &[Vec<u8>],
    dec: &mut Decoder,
    out: &mut Notams,
    stats: &mut Stats,
    keep: bool,
) {
    let mut parser = Parser::new(oa_xml::Limits::default()).unwrap();
    for bytes in data {
        let _ = decode_bytes(&mut parser, dec, bytes, out, stats);
        if !keep {
            clear_notams(out);
        }
    }
}

// REQ: OA-NOTAM-017
#[test]
fn every_real_fns_message_decodes_and_is_reported() {
    let Some(data) = corpus() else { return };
    let total: usize = data.iter().map(Vec::len).sum();

    // Pass 1, keeping every row, for the distinct counts.
    let mut rows = default_notams();
    let mut dec = new_decoder(Limits::default());
    let mut stats = new_stats(256);
    decode_all(&data, &mut dec, &mut rows, &mut stats, true);

    let mut with_geometry = 0usize;
    let mut with_circle = 0usize;
    let mut with_poly = 0usize;
    let mut kinds = [0usize; FEAT_COUNT + 1];
    let mut ids = std::collections::BTreeSet::new();
    let mut locations = std::collections::BTreeSet::new();
    for i in 0..rows.len {
        if has_circle(&rows, i) || !polygons_of(&rows, i).is_empty() {
            with_geometry += 1;
        }
        with_circle += usize::from(has_circle(&rows, i));
        with_poly += usize::from(!polygons_of(&rows, i).is_empty());
        kinds[usize::from(rows.feat_kind[i])] += 1;
        ids.insert(rows.event_id[i]);
        locations.insert(rows.location[i]);
    }

    eprintln!("FNS real data: {} messages, {} bytes", stats.docs, total);
    eprintln!("  accepted {}, rows {}", stats.accepted, stats.rows);
    for (k, n) in REJECT_SLOT_NAMES.iter().zip(stats.rejects) {
        eprintln!("  rejects by reason {k}: {n}");
    }
    assert_eq!(stats.field_errors.len(), FIELD_COUNT);
    eprintln!(
        "  field errors: {} in total (none expected)",
        stats.field_errors.iter().sum::<u64>()
    );
    for f in ALL_FIELDS {
        let n = stats.field_errors[f as usize];
        if n > 0 {
            eprintln!("  field error {}: {n}", field_name(f));
        }
    }
    for (k, n) in stats.features.iter().enumerate() {
        eprintln!("  rows with feature {}: {n}", feat_name(k as u8 + 1));
    }
    for (k, n) in kinds.iter().enumerate().skip(1) {
        if *n > 0 {
            eprintln!(
                "  rows whose primary feature is {}: {n}",
                feat_name(k as u8)
            );
        }
    }
    eprintln!(
        "  geometry: {with_geometry} rows ({with_circle} circles, {with_poly} with polygons, {} polygons, {} vertices)",
        stats.polygons, stats.vertices
    );
    eprintln!(
        "  unknown elements {}, attributes {}, overflow {}",
        stats.unknown_elements, stats.unknown_attributes, stats.unknown_overflow
    );
    for u in &stats.unknown {
        eprintln!(
            "    unknown {} x{}{}",
            unknown_text(u),
            u.count,
            if u.is_attr { " (attribute)" } else { "" }
        );
    }
    eprintln!(
        "  distinct: {} event ids, {} locations",
        ids.len(),
        locations.len()
    );

    assert_eq!(stats.docs as usize, data.len());
    assert_eq!(
        stats.rejects.iter().sum::<u64>(),
        0,
        "rejects: {:?}",
        stats.rejects
    );
    assert_eq!(
        stats.field_errors.iter().sum::<u64>(),
        0,
        "field errors: {:?}",
        stats.field_errors
    );
    assert_eq!(
        stats.unknown_elements + stats.unknown_attributes,
        0,
        "the survey says every path is known"
    );
    assert_eq!(ids.len(), data.len(), "event ids are distinct");
    assert!(with_geometry > 0 && with_circle > 0 && with_poly > 0);

    // Pass 2, timing only (the files are already in memory; rows are discarded per message).
    let mut scratch = new_notams(16, 1 << 16, 64, 4096);
    let mut s2 = new_stats(16);
    let t = Instant::now();
    let reps = 20;
    for _ in 0..reps {
        decode_all(&data, &mut dec, &mut scratch, &mut s2, false);
    }
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(s2.rejects.iter().sum::<u64>(), 0);
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

// REQ: OA-NOTAM-018
#[test]
fn real_locations_and_polygons_resolve_against_the_model() {
    let Some(data) = corpus() else { return };
    let Some(snap) = std::env::var_os("OPENATCA_SNAPSHOT") else {
        eprintln!("OPENATCA_SNAPSHOT is not set: skipping the model cross-check");
        return;
    };
    let model = oa_airspace::snapshot::decode(&std::fs::read(snap).unwrap()).unwrap();
    let mut rows = default_notams();
    let mut dec = new_decoder(Limits::default());
    let mut stats = new_stats(256);
    decode_all(&data, &mut dec, &mut rows, &mut stats, true);
    assert_eq!(stats.rejects.iter().sum::<u64>(), 0);
    let t = Instant::now();
    let x = cross_check(&model, &rows);
    eprintln!("model ARTCC {}", model.meta.artcc);
    eprint!("{}", format_cross_check(&x));
    eprintln!(
        "  rows in the model's ARTCC: {}; cross-check took {:.2} s",
        x.own_rows,
        t.elapsed().as_secs_f64()
    );
    eprintln!(
        "  unresolved own-FIR locations {:?}, airports {:?}",
        x.own.unresolved_locations.len(),
        x.own.unresolved_airports.len()
    );
    assert!(x.all.location.rows > 0);
    assert!(x.all.location.resolved > 0, "no location resolves at all");
    assert!(
        x.polygons.boundaries > 0,
        "the model's ARTCC has no usable boundary"
    );
    assert_eq!(x.polygons.rows_with_polygons, stats.with_polygons as u32);
    let _ = Field::Text;
}
