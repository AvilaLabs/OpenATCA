//! The committed fuzz seeds (`fuzz/seeds/awc`) through the entry points of the `awc` fuzz target.
//! The first byte of each seed picks the product (0 METAR, 1 TAF, 2 PIREP, 3 SIGMET, 4 AIRSIGMET,
//! 5 G-AIRMET) and the rest is the file. `*-ok-*` and `*-real-*` seeds must load with every record
//! accepted, `*-bad-*` seeds must be refused (a file error, or a record refused by reason), and
//! nothing may panic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

/// Loads `body` as product `sel` the way the fuzz target does; true if the file loaded and every
/// record in it was accepted.
fn load(sel: u8, body: &[u8]) -> bool {
    match sel % 6 {
        0 => {
            let (mut t, mut s) = (oa_awc::new_metar_table(), oa_awc::MetarStats::default());
            let r = oa_awc::parse_metar(body, &mut t, &mut s);
            let _ = oa_awc::finalize_metar(&mut t);
            r.is_ok() && s.accepted == s.records
        }
        1 => {
            let (mut t, mut s) = (oa_awc::new_taf_table(), oa_awc::TafStats::default());
            let r = oa_awc::parse_taf(body, &mut t, &mut s);
            let _ = oa_awc::finalize_taf(&mut t);
            r.is_ok() && s.accepted == s.records
        }
        2 => {
            let (mut t, mut s) = (oa_awc::new_pirep_table(), oa_awc::PirepStats::default());
            let r = oa_awc::parse_pirep(body, &mut t, &mut s);
            let _ = oa_awc::finalize_pirep(&mut t);
            r.is_ok() && s.accepted == s.records
        }
        n => {
            let (mut t, mut s) = (oa_awc::new_hazard_table(), oa_awc::HazardStats::default());
            let r = match n {
                3 => oa_awc::parse_sigmet(body, &mut t, &mut s),
                4 => oa_awc::parse_airsigmet(body, &mut t, &mut s),
                _ => oa_awc::parse_gairmet(body, &mut t, &mut s),
            };
            let _ = oa_awc::finalize_hazard(&mut t);
            r.is_ok() && s.accepted == s.records
        }
    }
}

#[test]
fn every_committed_seed_loads_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/awc");
    let (mut accepted, mut refused) = (0, 0);
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
        let (&sel, body) = data.split_first().expect("a selector byte");
        let want_sel = ["metar", "taf", "pirep", "sigmet", "airsigmet", "gairmet"]
            .iter()
            .position(|p| name.starts_with(&format!("{p}-")))
            .expect("the name starts with its product");
        assert_eq!(sel as usize, want_sel, "{name}: selector byte");
        let clean = load(sel, body);
        if name.contains("-ok-") || name.contains("-real-") {
            assert!(clean, "{name} should load with every record accepted");
            accepted += 1;
        } else if name.contains("-bad-") {
            assert!(!clean, "{name} should be refused");
            refused += 1;
        } else {
            panic!("{name}: seed names carry -ok-, -real- or -bad-");
        }
    }
    eprintln!("awc seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (19, 10));
}
