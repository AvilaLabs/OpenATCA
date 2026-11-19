//! The committed fuzz seeds (`fuzz/seeds/asterix`) through the entry points of the `asterix`
//! fuzz target. `ok-*` seeds must decode as a stream and every block must encode again, `bad-*`
//! seeds must be refused with a typed error by `decode_block` or `decode_stream`, and nothing may panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[test]
fn every_committed_seed_decodes_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/asterix");
    let (mut accepted, mut refused) = (0, 0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "bin"))
        .collect();
    paths.sort();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        assert!(data.len() < 64 * 1024, "{name} is over 64 KiB");
        // The same calls as the fuzz target.
        let single = oa_asterix::decode_block(&data);
        let stream = oa_asterix::decode_stream(&data);
        if let Ok(blocks) = &stream {
            for b in blocks {
                let _ = oa_asterix::encode_block(b);
            }
        }
        if name.starts_with("ok-") {
            let blocks = stream.as_ref().unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(single.is_ok(), "{name}: the first block should decode");
            for b in blocks {
                oa_asterix::encode_block(b).unwrap_or_else(|e| panic!("{name}: encode: {e}"));
            }
            accepted += 1;
        } else if name.starts_with("bad-") {
            // Refused by the single-block entry or the stream entry (an empty input is an empty
            // stream, but not a block).
            assert!(
                stream.is_err() || single.is_err(),
                "{name} should be refused"
            );
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok- or bad-");
        }
    }
    eprintln!("asterix seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (23, 23));
}
