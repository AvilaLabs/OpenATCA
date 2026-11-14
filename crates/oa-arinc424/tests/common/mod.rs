//! Shared helpers for the integration tests.
#![allow(dead_code)]

use oa_arinc424::{parse_bytes, parse_record, Cifp, Position, Record, RECORD_LEN};

pub fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// Decodes every line of a fixture, panicking with the line number on the first failure.
pub fn records(name: &str) -> Vec<Record> {
    fixture(name)
        .lines()
        .enumerate()
        .map(|(i, l)| parse_record(i + 1, l).unwrap_or_else(|e| panic!("{name}:{}: {e}", i + 1)))
        .collect()
}

/// The real FAACIFP18 header with its record count replaced, followed by `body`.
pub fn with_header(body: &str) -> String {
    let mut hdr: Vec<String> = fixture("header.txt").lines().map(str::to_string).collect();
    let count = body.lines().count();
    let first = &mut hdr[0];
    first.replace_range(28..35, &format!("{count:07}"));
    let mut out = hdr.join("\n");
    out.push('\n');
    out.push_str(body);
    out
}

/// Parses a fixture as a complete file (header added).
pub fn file_of(names: &[&str]) -> Cifp {
    let body: String = names.iter().map(|n| fixture(n)).collect();
    parse_bytes(with_header(&body).as_bytes(), "fixture").expect("fixture file parses")
}

/// Two positions agree within the hundredth-of-a-second resolution of the source plus the
/// rounding of the 8-decimal NASR values.
pub fn close(p: Position, lat: f64, lon: f64) -> bool {
    (p.lat - lat).abs() <= 3e-6 && (p.lon - lon).abs() <= 3e-6
}

/// Great-circle distance in nautical miles and initial bearing in degrees, from `a` to `b`
/// (spherical, radius 3440.065 NM). Independent of the crate, used to check arc geometry.
pub fn distance_bearing(a: Position, b: Position) -> (f64, f64) {
    let (la1, lo1, la2, lo2) = (
        a.lat.to_radians(),
        a.lon.to_radians(),
        b.lat.to_radians(),
        b.lon.to_radians(),
    );
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    let d = 2.0 * h.sqrt().asin() * 3440.065;
    let y = (lo2 - lo1).sin() * la2.cos();
    let x = la1.cos() * la2.sin() - la1.sin() * la2.cos() * (lo2 - lo1).cos();
    (d, y.atan2(x).to_degrees().rem_euclid(360.0))
}

/// A 132-character record from a fixture line with `text` written at 1-based column `col`.
pub fn patch(line: &str, col: usize, text: &str) -> String {
    let mut s = line.to_string();
    s.replace_range(col - 1..col - 1 + text.len(), text);
    assert_eq!(s.len(), RECORD_LEN);
    s
}

/// xorshift64 pseudo-random numbers (deterministic).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
