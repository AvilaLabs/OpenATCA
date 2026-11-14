//! Whole-corpus tests against real TFMS messages. They need `OPENATCA_SCDS` (a directory
//! searched recursively for `*.msg` files inside directories named `tfms*`; the linkage test
//! also reads `sfdps*`) and skip when it is unset. The data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_fixm::{clear_events, new_events, Decoder, Limits as FixmLimits};
use oa_tfms::rows::{fkind, kind, prog};
use oa_tfms::{
    clear_rows, decode_bytes, default_limits, error_kind_name, field_name, flights_len,
    format_link_report, link_flights, name_str, new_rows, new_stats, observe, DocError, Rows,
    DEFAULT_FLIGHT_CAPACITY, DEFAULT_FLOW_CAPACITY, DEFAULT_STRING_BYTES, DOC_KIND_NAMES,
    FIELD_COUNT, REJECT_NAMES,
};
use oa_xml::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, prefix: &str, in_dir: bool, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            collect(&p, prefix, in_dir || name.starts_with(prefix), out);
        } else if in_dir && p.extension().is_some_and(|e| e == "msg") {
            out.push(p);
        }
    }
}

fn corpus(prefix: &str) -> Option<Vec<Vec<u8>>> {
    let Some(dir) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real {prefix} data test");
        return None;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), prefix, false, &mut files);
    if files.is_empty() {
        eprintln!(
            "no {prefix}*/ *.msg files under {}: skipping",
            dir.to_string_lossy()
        );
        return None;
    }
    Some(files.iter().map(|f| std::fs::read(f).unwrap()).collect())
}

fn new_default_rows() -> Rows {
    new_rows(
        DEFAULT_FLIGHT_CAPACITY,
        DEFAULT_FLOW_CAPACITY,
        DEFAULT_STRING_BYTES,
    )
}

fn decode_all(data: &[Vec<u8>], rows: &mut Rows) -> oa_tfms::Stats {
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut stats = new_stats();
    for bytes in data {
        let f0 = flights_len(&rows.flights);
        let w0 = oa_tfms::flows_len(&rows.flows);
        let r = decode_bytes(&mut parser, bytes, rows);
        observe(
            &mut stats,
            &r,
            &rows.flights.kind[f0..],
            &rows.flows.kind[w0..],
        );
        clear_rows(rows);
    }
    stats
}

// REQ: OA-TFMS-017
#[test]
fn every_real_tfms_message_decodes_and_is_reported() {
    let Some(data) = corpus("tfms") else { return };
    let total: usize = data.iter().map(Vec::len).sum();

    // Pass 1, one document at a time, keeping distinct sets and the field-error detail.
    let mut rows = new_default_rows();
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut stats = new_stats();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut refs = BTreeSet::new();
    let mut gufis = BTreeSet::new();
    let mut acids = BTreeSet::new();
    let mut airports = BTreeSet::new();
    let mut with_pos = 0usize;
    let mut programs = [0u64; prog::COUNT];
    let mut max_text = 0usize;
    for bytes in &data {
        let f0 = flights_len(&rows.flights);
        let w0 = oa_tfms::flows_len(&rows.flows);
        let r = decode_bytes(&mut parser, bytes, &mut rows);
        observe(
            &mut stats,
            &r,
            &rows.flights.kind[f0..],
            &rows.flows.kind[w0..],
        );
        if let Err(DocError::Field(f)) = &r {
            *errors
                .entry(format!(
                    "{} {} at byte {}",
                    field_name(f.field),
                    error_kind_name(f.kind),
                    f.at
                ))
                .or_default() += 1;
        }
        let fl = &rows.flights;
        for i in 0..flights_len(fl) {
            refs.insert(fl.flight_ref[i]);
            if fl.gufi[i] != [0; 16] {
                gufis.insert(fl.gufi[i]);
            }
            acids.insert(fl.acid[i]);
            for a in [fl.dep[i], fl.arr[i]] {
                if a != [0; 8] {
                    airports.insert(a);
                }
            }
            if fl.lat_e7[i] != oa_tfms::NO_COORD {
                with_pos += 1;
            }
        }
        for &p in &rows.flows.program {
            programs[p as usize] += 1;
        }
        max_text = max_text.max(rows.strings_used);
        clear_rows(&mut rows);
    }

    eprintln!(
        "TFMS real data: {} messages, {} bytes",
        stats.documents, total
    );
    eprintln!(
        "  accepted {}, inner messages {}, flight rows {}, flow rows {}",
        stats.accepted, stats.messages, stats.flight_rows, stats.flow_rows
    );
    for (k, n) in DOC_KIND_NAMES.iter().zip(stats.doc_kinds) {
        eprintln!("  documents of kind {k}: {n}");
    }
    for (k, n) in kind::NAMES.iter().zip(stats.flight_kinds) {
        eprintln!("  flight rows {k}: {n}");
    }
    for (k, n) in fkind::NAMES.iter().zip(stats.flow_kinds) {
        eprintln!("  flow rows {k}: {n}");
    }
    for (k, n) in prog::NAMES.iter().zip(programs) {
        eprintln!("  flow program {k}: {n}");
    }
    for (k, n) in REJECT_NAMES.iter().zip(stats.rejects) {
        eprintln!("  rejects by reason {k}: {n}");
    }
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
        "  distinct: {} flights (flight reference), {} gufis, {} aircraft ids, {} airports; \
         {} rows with a position; largest text use in one document {} bytes",
        refs.len(),
        gufis.len(),
        acids.len(),
        airports.len(),
        with_pos,
        max_text
    );
    let _ = name_str(&[]);

    assert_eq!(stats.documents as usize, data.len());
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
    assert!(stats.flight_rows > 0 && stats.flow_rows > 0);

    // Pass 2, timing only (the files are already in memory; rows are discarded per message).
    let mut scratch = new_default_rows();
    let t = Instant::now();
    let reps = 5;
    for _ in 0..reps {
        let s = decode_all(&data, &mut scratch);
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

// REQ: OA-TFMS-018
#[test]
fn real_tfms_flights_link_to_real_sfdps_flights() {
    let (Some(tfms), Some(sfdps)) = (corpus("tfms"), corpus("sfdps")) else {
        return;
    };
    let mut parser = Parser::new(default_limits()).unwrap();
    let mut rows = new_rows(tfms.len() * 24, tfms.len() * 2, 64 << 20);
    for bytes in &tfms {
        decode_bytes(&mut parser, bytes, &mut rows).unwrap();
    }
    let mut dec = Decoder::new(FixmLimits::default());
    let mut ev = new_events(200_000, 16 << 20);
    let mut sparser = Parser::new(oa_xml::Limits::default()).unwrap();
    for bytes in &sfdps {
        let doc = sparser.parse(bytes).unwrap();
        // The decoder is all or nothing per document; rows accumulate across documents.
        match dec.decode(&doc, bytes, &mut ev) {
            Ok(_) | Err(oa_fixm::FixmError::UnsupportedRoot) => {}
            Err(e) => panic!("SFDPS decode failed: {e}"),
        }
    }
    let rep = link_flights(&ev, &rows);
    eprintln!("{}", format_link_report(&rep));
    assert_eq!(rep.tfms_rows as usize, flights_len(&rows.flights));
    assert!(rep.tfms_flights > 0 && rep.sfdps_flights > 0);
    // The two feeds' gufi elements differ in form: no TFMS gufi is a UUID, so none can match an
    // SFDPS gufi; the plan-identifier match is where the overlap is.
    assert_eq!(rep.uuid_matched, 0);
    assert!(
        rep.by_gufi > 0,
        "the sample minutes overlap in some flights"
    );
    assert_eq!(rep.disagree, 0, "plan identifier and acid+route must agree");
    clear_events(&mut ev);
}
