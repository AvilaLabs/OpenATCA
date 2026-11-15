//! The argument parser of `oa-adsb-clip`: time parsing and every flag.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::cli::{parse_args, parse_iso, parse_time, usage};
use oa_adsb::*;

fn args(v: &[&str]) -> Result<oa_adsb::cli::Args, String> {
    parse_args(&v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
}

// REQ: OA-ADSB-160
#[test]
fn iso_time_is_exact_and_each_field_is_range_checked() {
    assert_eq!(parse_iso("2026-11-14T16:13:00Z"), Some(1_794_672_780));
    assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso("1969-12-31T23:59:59Z"), Some(-1));
    assert_eq!(parse_iso("2000-02-29T23:59:59Z"), Some(951_868_799));
    assert_eq!(parse_iso("2026-01-01T00:00:00Z"), Some(1_767_225_600));
    assert_eq!(parse_iso("2026-02-28T12:00:00Z"), Some(1_772_280_000));
    assert_eq!(parse_iso("2026-03-01T00:00:00Z"), Some(1_772_323_200));
    assert_eq!(parse_iso("2026-12-31T23:59:59Z"), Some(1_798_761_599));
    assert_eq!(parse_iso("2026-10-15T01:02:03Z"), Some(1_792_026_123));
    // Each field weighs what it should: one unit of each.
    let base = parse_iso("2026-05-10T10:10:10Z").unwrap();
    assert_eq!(parse_iso("2026-05-10T10:10:11Z").unwrap() - base, 1);
    assert_eq!(parse_iso("2026-05-10T10:11:10Z").unwrap() - base, 60);
    assert_eq!(parse_iso("2026-05-10T11:10:10Z").unwrap() - base, 3600);
    assert_eq!(parse_iso("2026-05-11T10:10:10Z").unwrap() - base, 86_400);
    assert_eq!(
        parse_iso("2026-06-10T10:10:10Z").unwrap() - base,
        31 * 86_400
    );
    assert_eq!(
        parse_iso("2027-05-10T10:10:10Z").unwrap() - base,
        365 * 86_400
    );
    // Valid edges.
    assert!(parse_iso("2026-01-01T00:00:00Z").is_some());
    assert!(parse_iso("2026-12-31T23:59:59Z").is_some());
    assert!(parse_iso("2026-01-31T23:59:59Z").is_some());
    // A leap second is read as the next minute's start.
    assert_eq!(parse_iso("2024-12-31T23:59:60Z"), Some(1_735_689_600));
    // Each out-of-range field on its own.
    for bad in [
        "2026-00-10T10:10:10Z",
        "2026-13-10T10:10:10Z",
        "2026-05-00T10:10:10Z",
        "2026-05-32T10:10:10Z",
        "2026-05-10T24:10:10Z",
        "2026-05-10T10:60:10Z",
        "2026-05-10T10:10:61Z",
    ] {
        assert_eq!(parse_iso(bad), None, "{bad}");
    }
    // Shape: length, separators, terminator, digits.
    for bad in [
        "",
        "2026-11-14T16:13:00",
        "2026-11-14T16:13:00z",
        "2026-11-14 16:13:00Z",
        "2026/11/14T16:13:00Z",
        "2026-11-14T16-13:00Z",
        "2026-11-14T16:13-00Z",
        "2026-11-14T16:13:00ZZ",
        "2026-1-14T16:13:00ZZ",
        "20x6-11-14T16:13:00Z",
        "2026-11-14T1a:13:00Z",
        "2026-11-14T16:13:0 Z",
    ] {
        assert_eq!(parse_iso(bad), None, "{bad}");
    }
}

// REQ: OA-ADSB-160
#[test]
fn time_is_unix_seconds_or_iso() {
    assert_eq!(parse_time("1794672780"), Ok(1_794_672_780));
    assert_eq!(parse_time("-5"), Ok(-5));
    assert_eq!(parse_time("2026-11-14T16:13:00Z"), Ok(1_794_672_780));
    let e = parse_time("yesterday").unwrap_err();
    assert!(e.contains("bad time `yesterday`"), "{e}");
    assert!(parse_time("2026-13-14T16:13:00Z").is_err());
}

// REQ: OA-ADSB-161
#[test]
fn every_flag_is_read() {
    let a = args(&["--out", "o.oaf", "p1", "p2"]).unwrap();
    assert_eq!(a.out, "o.oaf");
    assert_eq!(a.parts, ["p1", "p2"]);
    assert_eq!(a.filter, ZAU_BOX);
    assert_eq!(a.frame_ms, DEFAULT_FRAME_MS);
    assert!(!a.allow_truncated);

    let a = args(&["p", "--allow-truncated", "--out", "-"]).unwrap();
    assert!(a.allow_truncated);
    assert_eq!(a.out, "-");
    assert_eq!(a.parts, ["p"]);

    let a = args(&["--out", "o", "--box", "41.5, 42.5,-91,-89.25", "p"]).unwrap();
    assert_eq!(
        (
            a.filter.lat_min_e7,
            a.filter.lat_max_e7,
            a.filter.lon_min_e7,
            a.filter.lon_max_e7
        ),
        (415_000_000, 425_000_000, -910_000_000, -892_500_000)
    );
    assert_eq!(
        (a.filter.from_ns, a.filter.to_ns),
        (ZAU_BOX.from_ns, ZAU_BOX.to_ns)
    );

    let a = args(&["--out", "o", "--from", "1000", "--to", "2000", "p"]).unwrap();
    assert_eq!(a.filter.from_ns, 1_000_000_000_000);
    assert_eq!(a.filter.to_ns, 2_000_000_000_000);
    assert_eq!(a.filter.lat_min_e7, ZAU_BOX.lat_min_e7);

    let a = args(&[
        "--out",
        "o",
        "--from",
        "2026-11-14T16:13:00Z",
        "--to",
        "2026-11-14T16:14:00Z",
        "p",
    ])
    .unwrap();
    assert_eq!(a.filter.from_ns, 1_794_672_780_000_000_000);
    assert_eq!(a.filter.to_ns, 1_794_672_840_000_000_000);

    let a = args(&["--out", "o", "--frame-ms", "250", "p"]).unwrap();
    assert_eq!(a.frame_ms, 250);
    assert_eq!(
        args(&["--out", "o", "--frame-ms", "1", "p"])
            .unwrap()
            .frame_ms,
        1
    );
    assert_eq!(
        args(&["--out", "o", "--frame-ms", "60000", "p"])
            .unwrap()
            .frame_ms,
        60_000
    );
}

// REQ: OA-ADSB-161
#[test]
fn bad_arguments_are_refused_with_a_reason() {
    // Help, in both spellings, is an empty message (no error line, only the usage).
    assert_eq!(args(&["--help"]).unwrap_err(), "");
    assert_eq!(args(&["-h"]).unwrap_err(), "");
    assert_eq!(args(&["--out", "o", "p", "--help"]).unwrap_err(), "");
    assert!(usage().starts_with("usage: oa-adsb-clip"));
    for (v, want) in [
        (&["p"][..], "--out is required"),
        (&["--out", "o"], "no archive parts"),
        (&["--out"], "--out needs a value"),
        (&["--out", "o", "p", "--to"], "--to needs a value"),
        (&["--out", "o", "p", "--from"], "--from needs a value"),
        (&["--out", "o", "p", "--box"], "--box needs a value"),
        (
            &["--out", "o", "p", "--frame-ms"],
            "--frame-ms needs a value",
        ),
        (&["--out", "o", "--bogus", "p"], "unknown option --bogus"),
        (&["--out", "o", "--to", "soon", "p"], "bad time `soon`"),
        (&["--out", "o", "--from", "soon", "p"], "bad time `soon`"),
        (&["--out", "o", "--box", "1,2,3", "p"], "--box needs"),
        (&["--out", "o", "--box", "1,2,3,4,5", "p"], "--box needs"),
        (&["--out", "o", "--box", "1,2,x,4", "p"], "--box needs"),
        (
            &["--out", "o", "--frame-ms", "0", "p"],
            "--frame-ms must be 1 to 60000",
        ),
        (&["--out", "o", "--frame-ms", "60001", "p"], "got `60001`"),
    ] {
        let e = args(v).unwrap_err();
        assert!(e.contains(want), "{v:?}: {e}");
    }
    // The box: each bound on its own, at the edge and just past it, and reversed.
    for ok in [
        "-90,90,-180,180",
        "0,0,0,0",
        "-90,-90,-180,-180",
        "90,90,180,180",
    ] {
        assert!(args(&["--out", "o", "--box", ok, "p"]).is_ok(), "{ok}");
    }
    for bad in [
        "-90.1,0,0,0",
        "0,90.1,0,0",
        "0,0,-180.1,0",
        "0,0,0,180.1",
        "-91,-90,0,0",
        "10,5,0,0",
        "0,0,10,5",
    ] {
        let e = args(&["--out", "o", "--box", bad, "p"]).unwrap_err();
        assert!(e.contains("out of range or reversed"), "{bad}: {e}");
    }
}
