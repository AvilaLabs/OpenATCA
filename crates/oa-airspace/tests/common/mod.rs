//! Shared test support: the committed fixture files, a stored-ZIP writer, and builders that turn
//! the fixtures into the two input archives and a model. Not every test file uses every item.
#![allow(dead_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_airspace::build::{build_model, BuildOutput, BuildParams};
use oa_airspace::load::load_bytes;
use oa_airspace::source::Sources;
use std::path::PathBuf;

pub const NASR_NAME: &str = "NASR_CSV_FIXTURE.zip";
pub const CIFP_NAME: &str = "CIFP_FIXTURE.zip";

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// The fixture NASR files as (name, bytes), in a fixed order.
pub fn nasr_files() -> Vec<(String, Vec<u8>)> {
    let dir = fixture_dir().join("nasr");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).expect("fixture file");
            (n, bytes)
        })
        .collect()
}

pub fn cifp_file() -> Vec<u8> {
    std::fs::read(fixture_dir().join("cifp").join("FAACIFP18")).expect("cifp fixture")
}

/// A ZIP archive with every entry stored (method 0).
pub fn stored_zip(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let crc = oa_zip::crc32(data);
        let offset = out.len() as u32;
        let le32 = |v: u32| v.to_le_bytes();
        let le16 = |v: u16| v.to_le_bytes();
        out.extend_from_slice(&le32(0x0403_4b50));
        out.extend_from_slice(&le16(20));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(&le16(0x21));
        out.extend_from_slice(&le32(crc));
        out.extend_from_slice(&le32(data.len() as u32));
        out.extend_from_slice(&le32(data.len() as u32));
        out.extend_from_slice(&le16(name.len() as u16));
        out.extend_from_slice(&le16(0));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&le32(0x0201_4b50));
        central.extend_from_slice(&le16(20));
        central.extend_from_slice(&le16(20));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0x21));
        central.extend_from_slice(&le32(crc));
        central.extend_from_slice(&le32(data.len() as u32));
        central.extend_from_slice(&le32(data.len() as u32));
        central.extend_from_slice(&le16(name.len() as u16));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le16(0));
        central.extend_from_slice(&le32(0));
        central.extend_from_slice(&le32(offset));
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

pub fn nasr_zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    stored_zip(files)
}

pub fn cifp_zip(text: &[u8]) -> Vec<u8> {
    stored_zip(&[("FAACIFP18".to_string(), text.to_vec())])
}

/// Loads the unmodified fixtures.
pub fn fixture_sources() -> Sources {
    sources_from(&nasr_files(), &cifp_file())
}

pub fn sources_from(nasr: &[(String, Vec<u8>)], cifp: &[u8]) -> Sources {
    load_bytes(NASR_NAME, &nasr_zip(nasr), CIFP_NAME, &cifp_zip(cifp), &[]).expect("fixtures load")
}

pub fn params(artcc: &str, buffer_m: u32) -> BuildParams {
    BuildParams {
        artcc: artcc.to_string(),
        buffer_m,
    }
}

/// Builds the fixture ZAU model with the default 50 NM buffer.
pub fn fixture_build() -> BuildOutput {
    build_model(&fixture_sources(), &params("ZAU", 92_600)).expect("fixture builds")
}

/// Replaces `from` with `to` in one NASR fixture file; panics if `from` is not there once.
pub fn edit_nasr(files: &mut [(String, Vec<u8>)], file: &str, from: &str, to: &str) {
    let entry = files
        .iter_mut()
        .find(|(n, _)| n == file)
        .expect("file present");
    let text = String::from_utf8(entry.1.clone()).expect("utf-8");
    assert_eq!(
        text.matches(from).count(),
        1,
        "{from:?} must occur once in {file}"
    );
    entry.1 = text.replacen(from, to, 1).into_bytes();
}

/// Moves the latitude of the NASR row of `file` whose line starts with `prefix` north by
/// `metres` (a meridian approximation: 1 degree of latitude is 111,050 m at 42 N). `col` is the
/// 0-based column of the latitude degrees; minutes, seconds, hemisphere and decimal follow it.
/// The build takes positions from the DMS fields and cross-checks the decimal ones, so both are
/// moved together. Northern latitudes only.
pub fn shift_lat(
    files: &mut [(String, Vec<u8>)],
    file: &str,
    prefix: &str,
    col: usize,
    metres: f64,
) {
    let entry = files
        .iter_mut()
        .find(|(n, _)| n == file)
        .expect("file present");
    let text = String::from_utf8(entry.1.clone()).expect("utf-8");
    let mut out = String::new();
    let mut hit = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with(prefix) {
            let mut cols: Vec<String> = line.trim_end().split(',').map(str::to_string).collect();
            assert_eq!(cols[col + 3], "\"N\"", "northern latitudes only");
            let deg: f64 = cols[col].parse().expect("degrees");
            let min: f64 = cols[col + 1].parse().expect("minutes");
            let sec: f64 = cols[col + 2].parse().expect("seconds");
            let total = deg * 3600.0 + min * 60.0 + sec + metres / 111_050.0 * 3600.0;
            let d = (total / 3600.0).floor();
            let m = ((total - d * 3600.0) / 60.0).floor();
            let s = total - d * 3600.0 - m * 60.0;
            cols[col] = format!("{d}");
            cols[col + 1] = format!("{m}");
            cols[col + 2] = format!("{s:.4}");
            cols[col + 4] = format!("{:.8}", total / 3600.0);
            out.push_str(&cols.join(","));
            out.push('\n');
            hit += 1;
        } else {
            out.push_str(line);
        }
    }
    assert_eq!(hit, 1, "{prefix} must match one row of {file}");
    entry.1 = out.into_bytes();
}

/// Removes the one row of `file` whose line starts with `prefix`.
pub fn drop_row(files: &mut [(String, Vec<u8>)], file: &str, prefix: &str) {
    let entry = files
        .iter_mut()
        .find(|(n, _)| n == file)
        .expect("file present");
    let text = String::from_utf8(entry.1.clone()).expect("utf-8");
    let kept: String = text
        .split_inclusive('\n')
        .filter(|l| !l.starts_with(prefix))
        .collect();
    assert!(
        text.len() > kept.len(),
        "{prefix} must match a row of {file}"
    );
    entry.1 = kept.into_bytes();
}
