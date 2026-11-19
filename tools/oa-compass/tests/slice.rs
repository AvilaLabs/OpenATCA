//! `oa-compass slice` (OA-CMP-027): a time window of a finished export, on generated files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;

use oa_asterix::{decode_block_with, Block, Cat021Edition};
use oa_compass::core::adsb::{adsb_record, AdsbRow};
use oa_compass::core::convert::track_record;
use oa_compass::core::convert::TrackRow;
use oa_compass::shell::adsb_export::AdsbExport;
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::{Layout, SystemExport};
use oa_compass::shell::notes::{data_sources_json, ADSB_SOURCE, OPENATCA_SOURCE};
use oa_compass::shell::slice::{check_window, parse_clock, slice_dir, slice_dir_with, Stage};

const DAY: i64 = 20_772;
const DAY_NS: i64 = DAY * 86_400 * 1_000_000_000;

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cmp-slice-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn clock(h: i64, m: i64, s: i64) -> i64 {
    DAY_NS + (h * 3600 + m * 60 + s) * 1_000_000_000
}

/// A source directory: a CAT062 file with one record every 10 s from 00:00:00 to 23:59:50
/// (8640 records) and a CAT021 file with one every 20 s (4320).
fn source(name: &str) -> PathBuf {
    let out = tmp(name);
    let lay = Layout {
        out: out.clone(),
        day: DAY,
        split: false,
    };
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, lay.clone());
    for k in 0..8640i64 {
        let row = TrackRow {
            t_ns: clock(0, 0, 0) + k * 10_000_000_000,
            lat_e7: 400_000_000 + (k as i32) * 100,
            lon_e7: 100_000_000,
            vel_east_mps: 100.0,
            vel_north_mps: 50.0,
            pos_cov_m2: [25.0, 0.0, 25.0],
            alt_ft: 10_000.0,
            has_alt: true,
            vrate_fpm: 0.0,
            coast: false,
            radar_disagrees: false,
            uncertain: false,
            reacquired: false,
            address: Some(0xABCDEF),
            monosensor: true,
        };
        let c = track_record(&row).unwrap();
        ex.push(&c, 1, (row.lat_e7, row.lon_e7)).unwrap();
    }
    ex.finish().unwrap().commit().unwrap();
    let mut ax = AdsbExport::new(ADSB_SOURCE, lay);
    for k in 0..4320i64 {
        let r = AdsbRow {
            time_ns: clock(0, 0, 0) + k * 20_000_000_000,
            address: 0x3C66B0,
            non_icao: false,
            lat_e7: 500_000_000 + (k as i32) * 50,
            lon_e7: -100_000_000,
            baro_alt_ft: Some(30_000),
            geom_alt_ft: None,
            ground: false,
            vrate_fpm: None,
            rate_geometric: false,
            gs_dkt: Some(4000),
            track_ddeg: Some(900),
            callsign: *b"ABC123\0\0",
            category: 0xA3,
            nic: Some(8),
            nac_p: Some(9),
            nac_v: Some(2),
            sil: Some(3),
            sil_per_sample: Some(false),
            version: Some(2),
            gva: Some(2),
            sda: Some(2),
        };
        ax.push(&adsb_record(&r).unwrap(), (r.lat_e7, r.lon_e7))
            .unwrap();
    }
    ax.finish().unwrap().commit().unwrap();
    fs::write(out.join("data_sources.json"), data_sources_json()).unwrap();
    fs::write(
        out.join("README.txt"),
        "UTC date to give at import (UTC Day): 2026.11.15\n",
    )
    .unwrap();
    out
}

fn units(h: u32, m: u32, s: u32) -> u32 {
    (h * 3600 + m * 60 + s) * 128
}

fn read_times(p: &std::path::Path, ed: Cat021Edition) -> Vec<u32> {
    let bytes = fs::read(p).unwrap();
    let mut at = 0;
    let mut t = Vec::new();
    while at < bytes.len() {
        let (b, used) = decode_block_with(&bytes[at..], ed).unwrap();
        match b {
            Block::Cat021(r) => t.extend(r.iter().map(|x| x.tomr_position.unwrap())),
            Block::Cat062(r) => t.extend(r.iter().map(|x| x.time.unwrap())),
            Block::Cat048(_) => panic!(),
        }
        at += used;
    }
    t
}

// REQ: OA-CMP-027
#[test]
fn a_window_keeps_exactly_the_records_in_from_to_and_writes_readme_and_sources() {
    let inp = source("in");
    let out = tmp("out");
    let files = slice_dir(&inp, &out, units(12, 0, 0), units(13, 0, 0)).unwrap();
    // One record per 10 s for an hour: 360 (12:00:00 included, 13:00:00 excluded); per 20 s: 180.
    assert_eq!(files.len(), 2);
    let by = |n: &str| files.iter().find(|f| f.name == n).unwrap();
    assert_eq!(
        (
            by("openatca.ast").records_in,
            by("openatca.ast").records_kept
        ),
        (8640, 360)
    );
    assert_eq!(
        (by("adsb.ast").records_in, by("adsb.ast").records_kept),
        (4320, 180)
    );
    let t = read_times(&out.join("openatca.ast"), Cat021Edition::Ed24);
    assert_eq!(
        (t.first(), t.last()),
        (Some(&units(12, 0, 0)), Some(&(units(13, 0, 0) - 1280)))
    );
    let t = read_times(&out.join("adsb.ast"), Cat021Edition::Ed24);
    assert_eq!(
        (t.len(), t[0], *t.last().unwrap()),
        (180, units(12, 0, 0), units(13, 0, 0) - 2560)
    );
    assert_eq!(
        fs::read(out.join("data_sources.json")).unwrap(),
        fs::read(inp.join("data_sources.json")).unwrap()
    );
    let readme = fs::read_to_string(out.join("README.txt")).unwrap();
    for n in [
        "12:00:00.000 to 13:00:00.000",
        "Source directory:",
        "2026.11.15",
        "openatca.ast: 8640 in, 360 kept",
        "adsb.ast: 4320 in, 180 kept",
        "across midnight is refused",
    ] {
        assert!(readme.contains(n), "{n}\n{readme}");
    }
    assert!(!out.join("openatca.ast.partial").exists());
}

// REQ: OA-CMP-027
#[test]
fn a_sliced_record_equals_the_source_record_and_an_empty_window_writes_no_file() {
    let inp = source("in2");
    let out = tmp("out2");
    slice_dir(&inp, &out, units(0, 0, 0), units(0, 1, 0)).unwrap();
    let a = fs::read(inp.join("adsb.ast")).unwrap();
    let b = fs::read(out.join("adsb.ast")).unwrap();
    // The first three records (0, 20, 40 s) come back as the same record values.
    let (ba, _) = decode_block_with(&a, Cat021Edition::Ed24).unwrap();
    let (bb, _) = decode_block_with(&b, Cat021Edition::Ed24).unwrap();
    let (Block::Cat021(ra), Block::Cat021(rb)) = (ba, bb) else {
        panic!()
    };
    assert_eq!(rb.len(), 3);
    assert_eq!(&ra[..3], &rb[..]);
    // A window with no record in it: no file for that system.
    let out = tmp("out3");
    // (openatca.ast ends at 23:59:50, adsb.ast at 23:59:40.)
    let f = slice_dir(&inp, &out, units(23, 59, 45), units(23, 59, 59)).unwrap();
    assert_eq!(f.iter().map(|x| x.records_kept).sum::<u64>(), 1);
    assert!(out.join("openatca.ast").exists() && !out.join("adsb.ast").exists());
}

// REQ: OA-CMP-027
#[test]
fn a_window_across_midnight_or_an_empty_one_is_refused_and_clock_times_are_checked() {
    assert!(matches!(
        check_window(units(23, 0, 0), units(1, 0, 0)),
        Err(ExportError::Input(_))
    ));
    assert!(check_window(units(12, 0, 0), units(12, 0, 0)).is_err());
    assert!(check_window(units(0, 0, 0), units(24, 0, 0)).is_ok());
    assert_eq!(parse_clock("12:00:00"), Some(units(12, 0, 0)));
    assert_eq!(parse_clock("24:00:00"), Some(86_400 * 128));
    for bad in [
        "24:00:01",
        "12:60:00",
        "12:00",
        "1:00:00",
        "12:00:00:00",
        "ab:cd:ef",
        "",
        "01:-5:00",
        "+1:00:00",
        " 1:00:00",
    ] {
        assert_eq!(parse_clock(bad), None, "{bad}");
    }
    let inp = source("in4");
    let out = tmp("out4");
    assert!(slice_dir(&inp, &out, units(23, 0, 0), units(1, 0, 0)).is_err());
    // A directory without .ast files is refused.
    assert!(slice_dir(&tmp("empty"), &out, units(1, 0, 0), units(2, 0, 0)).is_err());
}

// REQ: OA-CMP-027
#[test]
fn a_file_of_another_category_or_a_damaged_one_is_refused_and_leaves_no_partial_file() {
    let inp = source("in5");
    // CAT048 is not sliceable.
    fs::write(inp.join("zz.ast"), [48u8, 0, 4, 0x80]).unwrap();
    let out = tmp("out5");
    assert!(slice_dir(&inp, &out, units(1, 0, 0), units(2, 0, 0)).is_err());
    assert!(!out.join("zz.ast.partial").exists());
    // A truncated block.
    fs::write(inp.join("zz.ast"), [21u8, 0, 200, 0x80]).unwrap();
    assert!(slice_dir(&inp, &out, units(1, 0, 0), units(2, 0, 0)).is_err());
    assert!(!out.join("zz.ast.partial").exists());
}

fn block_21(times: &[u32]) -> Vec<u8> {
    use oa_asterix::cat021::Cat021;
    use oa_asterix::{encode_block_with, SourceId};
    let recs: Vec<Cat021> = times
        .iter()
        .map(|&t| Cat021 {
            source: SourceId { sac: 255, sic: 13 },
            tomr_position: Some(t),
            ..Cat021::default()
        })
        .collect();
    encode_block_with(&Block::Cat021(recs), Cat021Edition::Ed24).unwrap()
}

// REQ: OA-CMP-027
#[test]
fn the_output_must_be_new_empty_and_not_the_input() {
    let inp = source("in6");
    assert!(slice_dir(&inp, &inp, units(1, 0, 0), units(2, 0, 0)).is_err());
    // The same directory by another spelling.
    let alias = inp.join("..").join(inp.file_name().unwrap());
    assert!(slice_dir(&inp, &alias, units(1, 0, 0), units(2, 0, 0)).is_err());
    let out = tmp("out6");
    fs::write(out.join("old.txt"), "x").unwrap();
    assert!(slice_dir(&inp, &out, units(1, 0, 0), units(2, 0, 0)).is_err());
    assert!(!out.join("adsb.ast").exists());
}

// REQ: OA-CMP-027
#[test]
fn a_failure_in_any_file_leaves_no_file_of_the_others() {
    let inp = source("in7");
    // "zz.ast" sorts after the good files: they are sliced first and must not be left behind.
    fs::write(inp.join("zz.ast"), [48u8, 0, 4, 0x80]).unwrap();
    let out = tmp("out7");
    assert!(slice_dir(&inp, &out, units(1, 0, 0), units(2, 0, 0)).is_err());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

// REQ: OA-CMP-027
#[test]
fn a_time_of_day_of_24_hours_and_a_file_mixing_categories_are_refused() {
    let inp = tmp("in8");
    // I073 holds 24 bits; 86400 * 128 = 11 059 200 is not a time of day.
    fs::write(inp.join("a.ast"), block_21(&[units(1, 0, 0), 11_059_200])).unwrap();
    let out = tmp("out8");
    let e = slice_dir(&inp, &out, units(0, 0, 0), units(2, 0, 0)).unwrap_err();
    assert!(
        e.to_string()
            .contains("1 records with a time of day of 24:00:00"),
        "{e}"
    );
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    // A CAT021 block followed by a CAT062 block in one file.
    let src = source("in9");
    let mut mixed = fs::read(src.join("adsb.ast")).unwrap()[..0].to_vec();
    mixed.extend(block_21(&[units(1, 0, 0)]));
    let c62 = fs::read(src.join("openatca.ast")).unwrap();
    let len = usize::from(u16::from_be_bytes([c62[1], c62[2]]));
    mixed.extend_from_slice(&c62[..len]);
    let inp2 = tmp("in10");
    fs::write(inp2.join("m.ast"), mixed).unwrap();
    let out = tmp("out9");
    let e = slice_dir(&inp2, &out, units(0, 0, 0), units(2, 0, 0)).unwrap_err();
    assert!(e.to_string().contains("category 21 and 62"), "{e}");
}

// REQ: OA-CMP-027
#[test]
fn a_failure_in_the_rename_phase_or_after_it_leaves_the_output_empty() {
    let inp = source("r3in1");
    // The data files, data_sources.json and README.txt: four renames.
    for stage in [
        Stage::Verified,
        Stage::Renamed(1),
        Stage::Renamed(2),
        Stage::Renamed(4),
    ] {
        let out = tmp("r3out1");
        let mut hook = |s: Stage| {
            if s == stage {
                Err(ExportError::Io("injected".into()))
            } else {
                Ok(())
            }
        };
        let r = slice_dir_with(&inp, &out, units(12, 0, 0), units(13, 0, 0), &mut hook);
        assert!(r.is_err(), "{stage:?}");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "{stage:?}");
    }
    // A real failure: a directory at a target name stops the rename; the files of this run go.
    let out = tmp("r3out1b");
    let blocker = out.join("data_sources.json");
    let mut hook = |s: Stage| {
        if s == Stage::Verified {
            fs::create_dir(&blocker).map_err(|e| ExportError::Io(e.to_string()))?;
        }
        Ok(())
    };
    assert!(slice_dir_with(&inp, &out, units(12, 0, 0), units(13, 0, 0), &mut hook).is_err());
    let left: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, vec![std::ffi::OsString::from("data_sources.json")]);
}

// REQ: OA-CMP-027
#[test]
fn a_window_in_which_every_file_keeps_no_record_is_refused_with_the_counts() {
    let inp = source("r3in2");
    let out = tmp("r3out2");
    // Between two records of every file: 12:00:01 to 12:00:05 holds none (10 s and 20 s steps).
    let e = slice_dir(&inp, &out, units(12, 0, 1), units(12, 0, 5)).unwrap_err();
    let ExportError::Input(m) = e else {
        panic!("{e:?}")
    };
    assert!(
        m.contains("openatca.ast: 8640 in, 0 kept") && m.contains("adsb.ast: 4320 in, 0 kept"),
        "{m}"
    );
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

// REQ: OA-CMP-027
#[cfg(unix)]
#[test]
fn a_symlink_to_the_input_directory_is_refused_as_the_output() {
    let inp = source("r3in3");
    let link = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cmp-slice-r3link");
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&inp, &link).unwrap();
    let e = slice_dir(&inp, &link, units(1, 0, 0), units(2, 0, 0)).unwrap_err();
    assert!(matches!(e, ExportError::Input(_)), "{e:?}");
    let _ = fs::remove_file(&link);
}
