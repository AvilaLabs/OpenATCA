//! Whole-corpus tests against the real AWC files. They need `OPENATCA_DATA` (the openatca-data
//! directory; the files are read from `raw/awc`) and skip with a printed note when it is unset
//! or the directory is missing. The data is never copied into the repository.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_awc::*;
use std::path::{Path, PathBuf};

fn awc_dir() -> Option<PathBuf> {
    let Some(d) = std::env::var_os("OPENATCA_DATA") else {
        eprintln!("OPENATCA_DATA is not set: skipping the real AWC data test");
        return None;
    };
    let dir = PathBuf::from(d).join("raw").join("awc");
    if !dir.is_dir() {
        eprintln!(
            "{} is missing: skipping the real AWC data test",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// Hazard files may legitimately hold records we refuse, so the rule is explicit. Every record is
/// accounted for (records = accepted + refused), and each refused record is printed with its file.
/// Only a refusal of a `reversed-band` (the source's own contradictory altitudes) is tolerated.
/// Every other reason (a bad geometry, an unknown code, a missing field, a bad type or time) means
/// either the format changed or a valid kind of record is not handled yet, which is a defect of
/// the parser, so the test fails once, loudly, until the parser handles it. A point hazard whose
/// `rawSigmet` states another position is also a failure.
fn hazard_records_are_accounted_for(dir: &Path, s: &summary::Summary) {
    // Sealed directories are skipped and counted, never descended into (OA-SEAL-012).
    let walk = oa_sealed::walk_unsealed(dir).unwrap();
    eprintln!("sealed entries skipped: {}", walk.skipped);
    let files = walk.files;
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let src = if name.contains("airsigmet") {
            hazard::SRC_AIRSIGMET
        } else if name.contains("gairmet") {
            hazard::SRC_GAIRMET
        } else if name.contains("sigmet") {
            hazard::SRC_SIGMET
        } else {
            continue;
        };
        for line in describe_hazard_refusals(&oa_sealed::read(p).unwrap(), src).unwrap() {
            eprintln!("REFUSED {name}: {line}");
        }
    }
    for (i, st) in s.hazard_stats.iter().enumerate() {
        let refused: u32 = st.refused.iter().sum();
        assert_eq!(st.records, st.accepted + refused, "hazard source {i}");
        for (r, &n) in st.refused.iter().enumerate() {
            assert!(
                n == 0 || r == Reason::ReversedBand as usize,
                "hazard source {i}: {n} records refused as {} (see the REFUSED lines)",
                REASON_NAMES[r]
            );
        }
        assert_eq!(
            st.point_raw_disagree, 0,
            "hazard source {i}: point vs rawSigmet"
        );
    }
    let p = &s.hazard_stats[0];
    eprintln!(
        "international SIGMET points={} checked against rawSigmet={}",
        p.points, p.point_raw_checked
    );
}

// REQ: OA-AWC-016
#[test]
fn real_awc_files_parse_cleanly() {
    let Some(dir) = awc_dir() else { return };
    let s = summary::scan_dir(&dir).unwrap();
    eprintln!("{}", summary::format_summary(&s));
    assert!(s.file_errors.is_empty(), "{:?}", s.file_errors);
    // Every record the hourly files hold is accepted; nothing is dropped silently.
    for (name, records, accepted) in [
        ("metar", s.metar_stats.records, s.metar_stats.accepted),
        ("pirep", s.pirep_stats.records, s.pirep_stats.accepted),
        ("taf", s.taf_stats.records, s.taf_stats.accepted),
    ] {
        assert!(records > 0, "{name}: no records");
        assert_eq!(records, accepted, "{name}: refused records");
    }
    hazard_records_are_accounted_for(&dir, &s);
    // The two altimeter sources agree, with no withheld altimeter.
    assert_eq!(s.metar_stats.altim_disagree, 0);
    assert!(s.metar_stats.altim_checked > 0);
    // De-duplication is exact: rows in = kept + duplicates, and overlapping hourly windows exist.
    let d = s.metar_dedupe;
    assert_eq!(d.rows_in, d.rows_kept + d.duplicates);
    assert_eq!(metar_len(&s.metar) as u32, d.rows_kept);
    // The ZAU-area stations mostly have an altimeter nearly every hour.
    let a = &s.availability;
    assert!(a.stations > 50);
    assert!(
        a.stations_ok * 10 >= a.stations * 9,
        "{} of {}",
        a.stations_ok,
        a.stations
    );
    // Spot lookup on a real station.
    let st = unpack_ident(s.metar.station[0]);
    let t_s = s.metar.obs_s[0] + 600;
    let fix = altimeter_at(&s.metar, &st, t_s, DEFAULT_MAX_ALTIMETER_AGE_S).unwrap();
    assert!((900.0..1100.0).contains(&fix.hpa));
    assert!(fix.age_s >= 0 && fix.age_s <= 600);
}
