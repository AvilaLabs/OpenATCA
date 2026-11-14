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
use std::path::PathBuf;

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
    for i in 0..3 {
        assert_eq!(
            s.hazard_stats[i].records, s.hazard_stats[i].accepted,
            "hazard source {i}"
        );
    }
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
