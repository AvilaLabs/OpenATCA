//! Whole-corpus test against real FAA SWIM SFDPS messages. Needs `OPENATCA_SCDS` (a directory
//! searched recursively for `sfdps-*` subdirectories of `*.msg` files) and skips when it is
//! unset. The data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_fixm::{
    clear_events, code_str, has_field, new_events, unknown_text, Decoder, Field, FixmError, Limits,
    ALL_FIELDS, FIELD_COUNT, FIELD_NAMES, REJECT_NAMES,
};
use oa_xml::Parser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    // Sealed directories are skipped and counted, never descended into (OA-SEAL-012).
    let walk = oa_sealed::walk_unsealed(dir).unwrap();
    eprintln!("sealed entries skipped: {}", walk.skipped);
    out.extend(walk.files.into_iter().filter(|p| {
        p.extension().is_some_and(|e| e == "msg")
            && p.parent()
                .and_then(|d| d.file_name())
                .is_some_and(|n| n.to_string_lossy().starts_with("sfdps-"))
    }));
}

// REQ: OA-FIXM-014
#[test]
fn every_real_sfdps_message_decodes() {
    let Some(dir) = std::env::var_os("OPENATCA_SCDS") else {
        eprintln!("OPENATCA_SCDS is not set: skipping the real SFDPS data test");
        return;
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    assert!(!files.is_empty(), "no sfdps-*/*.msg files found");
    let data: Vec<Vec<u8>> = files.iter().map(|f| oa_sealed::read(f).unwrap()).collect();
    let total: usize = data.iter().map(Vec::len).sum();

    let mut parser = Parser::new(oa_xml::Limits::default()).unwrap();
    // A message timed in the sealed window is dropped inside the decoder, before any count.
    let mut dec = Decoder::new(Limits {
        ..Limits::new(oa_sealed::DropWindow::sealed())
    });
    let mut ev = new_events(1024, 1 << 20);
    let mut parse_failed = 0usize;
    let mut unsupported = 0usize;
    let mut rows_with_pos = 0usize;
    let mut with_error = 0usize;
    let mut present = [0usize; FIELD_COUNT];
    let mut gufis: HashSet<u128> = HashSet::new();
    let mut callsigns: HashSet<[u8; 8]> = HashSet::new();
    let mut rows = 0usize;
    let mut elapsed = std::time::Duration::ZERO;
    for bytes in &data {
        let t = Instant::now();
        let r = match parser.parse(bytes) {
            Ok(doc) => dec.decode(&doc, bytes, &mut ev),
            Err(_) => {
                parse_failed += 1;
                elapsed += t.elapsed();
                continue;
            }
        };
        elapsed += t.elapsed();
        match r {
            Ok(_) => {}
            Err(FixmError::UnsupportedRoot) => unsupported += 1,
            Err(e) => panic!("decode failed: {e}"),
        }
        for row in 0..ev.len {
            rows += 1;
            gufis.insert(ev.gufi[row]);
            if has_field(&ev, row, Field::Callsign) {
                callsigns.insert(ev.callsign[row]);
            }
            if has_field(&ev, row, Field::Position) {
                rows_with_pos += 1;
            }
            if ev.errors[row] != 0 {
                with_error += 1;
            }
            for (k, f) in ALL_FIELDS.iter().enumerate() {
                if has_field(&ev, row, *f) {
                    present[k] += 1;
                }
            }
        }
        clear_events(&mut ev);
    }
    let st = dec.stats();
    let secs = elapsed.as_secs_f64();
    eprintln!("files: {} ({} bytes)", files.len(), total);
    eprintln!(
        "parse failures: {parse_failed}; unsupported roots (non-MessageCollection): {unsupported}"
    );
    eprintln!("messages: {}; rows: {}", st.messages, st.rows);
    let rejected: u64 = st.rejected.iter().sum();
    eprintln!("rejected: {rejected}");
    for (k, c) in st.rejected.iter().enumerate() {
        if *c > 0 {
            eprintln!("  {}: {c}", REJECT_NAMES[k]);
        }
    }
    eprintln!("rows with at least one field error: {with_error}");
    for (k, c) in st.field_errors.iter().enumerate() {
        if *c > 0 {
            eprintln!("  field error {}: {c}", FIELD_NAMES[k]);
        }
    }
    for (k, c) in present.iter().enumerate() {
        eprintln!("  present {}: {c}", FIELD_NAMES[k]);
    }
    eprintln!(
        "unknown elements {} attributes {} overflow {}",
        st.unknown_elements, st.unknown_attributes, st.unknown_overflow
    );
    let mut top: Vec<_> = st.unknown.iter().collect();
    top.sort_by_key(|u| std::cmp::Reverse(u.count));
    for u in top.iter().take(10) {
        eprintln!("  unknown {} x{}", unknown_text(u), u.count);
    }
    eprintln!(
        "distinct GUFIs: {}; distinct callsigns: {}; rows with position: {rows_with_pos}",
        gufis.len(),
        callsigns.len()
    );
    let _ = code_str(b"");
    let msgs_per_s = st.messages as f64 / secs;
    eprintln!(
        "throughput (XML parse + decode): {:.0} messages/s, {:.1} MB/s ({:.3} s)",
        msgs_per_s,
        total as f64 / 1e6 / secs,
        secs
    );
    assert_eq!(parse_failed, 0, "every sample must parse");
    assert_eq!(st.rows as usize, rows);
    assert_eq!(st.messages, st.rows + rejected);
    assert!(
        msgs_per_s >= 20_000.0 || cfg!(debug_assertions),
        "decode throughput {msgs_per_s:.0} messages/s is below 20,000"
    );
}
