//! The committed fuzz seeds (`fuzz/seeds/tbfm`) through the entry points of the `tbfm` fuzz
//! target. `ok-*` seeds must decode with no rejected flight, `bad-*` seeds must be refused
//! (a typed error, or a rejected flight), and nothing may panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_xml::Parser;

#[test]
fn every_committed_seed_decodes_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/tbfm");
    let mut parser = Parser::new(oa_tbfm::default_limits()).unwrap();
    let mut rows = oa_tbfm::new_rows(
        oa_tbfm::DEFAULT_ROW_CAPACITY,
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    );
    let (mut accepted, mut refused) = (0, 0);
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "xml" || e == "txt"))
        .collect();
    names.sort();
    for path in &names {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        assert!(data.len() < 64 * 1024, "{name} is over 64 KiB");
        // The same calls as the fuzz target.
        oa_tbfm::clear_rows(&mut rows);
        let r = oa_tbfm::decode_bytes(&mut parser, &data, &mut rows);
        let clean = matches!(&r, Ok(o) if o.rejected == 0);
        oa_tbfm::clear_rows(&mut rows);
        if let Ok(doc) = parser.parse(&data) {
            let _ = oa_tbfm::decode_doc_prefix(&doc, &mut rows);
        }
        for i in 0..oa_tbfm::rows_len(&rows) {
            let _ = oa_tbfm::row_at(&rows, i);
        }
        if name.starts_with("ok-") {
            assert!(clean, "{name} should decode cleanly: {r:?}");
            accepted += 1;
        } else if name.starts_with("bad-") {
            assert!(!clean, "{name} should be refused: {r:?}");
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok- or bad-");
        }
    }
    eprintln!("tbfm seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (32, 17));
}
