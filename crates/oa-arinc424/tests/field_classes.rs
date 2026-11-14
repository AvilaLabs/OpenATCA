//! Field character-class and range checks (OA-424-008): a field outside its alphabet or
//! range is an error naming the field, never a quietly wrong value.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::{fixture, with_header};
use oa_arinc424::{parse_bytes, parse_record};

/// `line` with 1-based columns `a..=b` replaced by `with` (same length).
fn put(line: &str, a: usize, b: usize, with: &str) -> String {
    assert_eq!(with.len(), b - a + 1);
    let mut s = line.to_string();
    s.replace_range(a - 1..b, with);
    s
}

fn first(name: &str) -> String {
    fixture(name).lines().next().unwrap().to_string()
}

/// The unmodified line parses; the mutated one is refused with a message naming `field`.
fn refused(name: &str, a: usize, b: usize, with: &str, field: &str) {
    let line = first(name);
    parse_record(1, &line).unwrap();
    let err = parse_record(1, &put(&line, a, b, with))
        .unwrap_err()
        .to_string();
    assert!(err.contains(field), "{name} cols {a}-{b}: {err}");
}

// REQ: OA-424-008
#[test]
fn a_field_outside_its_alphabet_or_range_is_an_error_naming_the_field() {
    refused("airports.txt", 31, 31, "X", "IFR capability");
    refused("airports.txt", 52, 56, "E1900", "magnetic variation");
    refused("runways.txt", 28, 31, "3601", "runway bearing");
    refused("localizers.txt", 52, 55, "9999", "localizer bearing");
    refused("airways.txt", 71, 74, "3601", "outbound course");
    refused("airways.txt", 37, 37, "5", "fix section");
    refused("airways.txt", 38, 38, "*", "fix section");
    refused("airways.txt", 30, 34, "le-v ", "fix identifier");
    let leg = fixture("approach.txt").lines().nth(1).unwrap().to_string();
    parse_record(2, &leg).unwrap();
    let bad = parse_record(2, &put(&leg, 71, 74, "9999"))
        .unwrap_err()
        .to_string();
    assert!(bad.contains("magnetic course"), "{bad}");
    let bad = parse_record(2, &put(&leg, 63, 66, "7000"))
        .unwrap_err()
        .to_string();
    assert!(bad.contains("theta"), "{bad}");
}

// REQ: OA-424-008
#[test]
fn the_header_production_flag_must_be_p_or_t() {
    let ok = with_header("");
    assert!(parse_bytes(ok.as_bytes(), "t").is_ok());
    let bad = put(&ok, 24, 24, "Z");
    assert!(parse_bytes(bad.as_bytes(), "t").is_err());
}
