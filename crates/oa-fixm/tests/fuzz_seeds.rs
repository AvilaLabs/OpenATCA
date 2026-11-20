//! The committed fuzz seeds (`fuzz/seeds/fixm`) through the entry points of the `fixm` fuzz
//! target. `ok-*` seeds must parse and decode with no rejected message and no field error,
//! `bad-*` seeds must be refused (an XML error, a refused document, a rejected message or a
//! field error), and nothing may panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[test]
fn every_committed_seed_decodes_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/fixm");
    let mut parser = oa_xml::Parser::new(oa_xml::Limits::default()).unwrap();
    let mut decoder = oa_fixm::Decoder::new(oa_fixm::Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut events = oa_fixm::new_events(256, 256 * 1024);
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
        oa_fixm::clear_events(&mut events);
        let mut clean = false;
        if let Ok(doc) = parser.parse(&data) {
            let r = decoder.decode(&doc, &data, &mut events);
            clean = matches!(&r, Ok(s) if s.rejected == 0 && s.field_errors == 0);
            oa_fixm::clear_events(&mut events);
            let _ = decoder.decode_prefix(&doc, &data, &mut events);
        }
        if name.starts_with("ok-") {
            assert!(clean, "{name} should decode cleanly");
            accepted += 1;
        } else if name.starts_with("bad-") {
            assert!(!clean, "{name} should be refused");
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok- or bad-");
        }
    }
    eprintln!("fixm seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (28, 15));
}
