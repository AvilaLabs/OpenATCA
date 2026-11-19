//! The committed fuzz seeds (`fuzz/seeds/adsb-trace`) through the entry point of the `adsb-trace`
//! fuzz target, with the ZAU box and with a keep-everything filter. `ok-*` seeds must decode,
//! `rowbad-*` seeds must decode with at least one row refused and counted, `bad-*` seeds must be
//! refused whole with a typed error, and nothing may panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[test]
fn every_committed_seed_decodes_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/adsb-trace");
    let all = oa_adsb::Filter {
        lat_min_e7: i32::MIN,
        lat_max_e7: i32::MAX,
        lon_min_e7: i32::MIN,
        lon_max_e7: i32::MAX,
        from_ns: i64::MIN,
        to_ns: i64::MAX,
    };
    let (mut accepted, mut rows_refused, mut refused) = (0, 0, 0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        assert!(data.len() < 64 * 1024, "{name} is over 64 KiB");
        let mut results = Vec::new();
        for f in [&oa_adsb::ZAU_BOX, &all] {
            let mut out = oa_adsb::Reports::default();
            let mut st = oa_adsb::Stats::default();
            let r = oa_adsb::decode_trace(&data, f, &mut out, &mut st);
            results.push((r, st));
        }
        let wide = &results[1];
        if name.starts_with("ok-") {
            assert!(
                results.iter().all(|(r, _)| r.is_ok()),
                "{name}: {results:?}"
            );
            assert_eq!(
                wide.1.rejected.iter().sum::<u64>(),
                0,
                "{name}: a row was refused"
            );
            accepted += 1;
        } else if name.starts_with("rowbad-") {
            assert!(results.iter().all(|(r, _)| r.is_ok()), "{name}");
            assert!(
                wide.1.rejected.iter().sum::<u64>() > 0,
                "{name}: no row refused"
            );
            rows_refused += 1;
        } else if name.starts_with("bad-") {
            assert!(
                results.iter().all(|(r, _)| r.is_err()),
                "{name} should be refused"
            );
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok-, rowbad- or bad-");
        }
    }
    eprintln!("adsb-trace seeds: {accepted} accepted, {rows_refused} accepted with refused rows, {refused} refused");
    assert_eq!((accepted, rows_refused, refused), (17, 3, 12));
}
