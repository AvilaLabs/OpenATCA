//! The committed fuzz seeds (`fuzz/seeds/j2k`) through the entry point of the `j2k` fuzz target.
//! `ok-*` seeds must decode, `bad-*` seeds must be refused with a typed error, and nothing may
//! panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[test]
fn every_committed_seed_decodes_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/j2k");
    let (mut accepted, mut refused) = (0, 0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "j2k"))
        .collect();
    paths.sort();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        assert!(data.len() < 64 * 1024, "{name} is over 64 KiB");
        let r = oa_j2k::decode(&data);
        if name.starts_with("ok-") {
            let img = r.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                img.samples.len(),
                img.width as usize * img.height as usize,
                "{name}"
            );
            accepted += 1;
        } else if name.starts_with("bad-") {
            assert!(r.is_err(), "{name} should be refused");
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok- or bad-");
        }
    }
    eprintln!("j2k seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (20, 35));
}
