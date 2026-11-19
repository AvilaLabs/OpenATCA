//! The tool: sealed paths refused, the files deterministic and free of identities in the summary,
//! and a synthetic close pass through the tracker and the record.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use oa_encounter::{CycleStats, Encounter, CLASS_ALT_UNKNOWN};
use oa_encounter_run::args::parse;
use oa_encounter_run::guard::refuse_sealed;
use oa_encounter_run::report::{
    band_of, class_name, encounters_csv, flags_text, summary_text, SummaryInput,
};
use oa_encounter_run::run::{ground_at, tool_config, track_and_record, GroundTable};
use oa_track::{Measurement, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-encounter-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// REQ: OA-ENC-009
#[test]
fn the_sealed_recordings_are_refused_as_given() {
    for p in [
        "x/scds/rec-20261119",
        "x/scds/rec-20261119/sub/file.oaf",
        "/data/scds/stdds-20261119-holdout",
        "/data/scds/stdds-20261119-holdout/a",
        "/data/scds/stdds-smoke-20261119T192936Z/b.bin",
    ] {
        assert!(refuse_sealed(Path::new(p)).is_err(), "{p}");
    }
    for p in [
        "/data/scds/day-20261114/rec/sfdps",
        "/data/scds/rec-20261118",
        "/data/rec-20261119",
        "/data/scds/x/rec-20261119",
        "/data/scds/stdds-smoke-20261118T1",
    ] {
        assert!(refuse_sealed(Path::new(p)).is_ok(), "{p}");
    }
}

// REQ: OA-ENC-009
#[test]
#[cfg(unix)]
fn a_link_into_a_sealed_recording_is_refused_and_so_is_an_output_below_one() {
    let root = scratch("link");
    for sealed in [
        "rec-20261119",
        "stdds-20261119-holdout",
        "stdds-smoke-20261119T000000Z",
    ] {
        let dir = root.join("scds").join(sealed);
        std::fs::create_dir_all(&dir).unwrap();
        let link = root.join(format!("innocent-{sealed}"));
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(refuse_sealed(&link).is_err(), "{sealed}");
        // An output directory that does not exist yet, below the link: its nearest existing
        // ancestor is resolved.
        assert!(
            refuse_sealed(&link.join("new").join("out")).is_err(),
            "{sealed}"
        );
    }
    assert!(refuse_sealed(&root.join("fine").join("out")).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

// REQ: OA-ENC-009
#[test]
fn the_program_exits_2_for_a_sealed_path_and_opens_and_creates_nothing() {
    let exe = env!("CARGO_BIN_EXE_oa-encounter");
    let out = scratch("exit");
    let fine = out.join("fine.oaf");
    for (adsb, o) in [
        (PathBuf::from("/x/scds/rec-20261119/a.oaf"), out.join("o")),
        (
            fine.clone(),
            PathBuf::from("/x/scds/stdds-smoke-20261119T1/o"),
        ),
    ] {
        let r = Command::new(exe)
            .args([
                "--adsb",
                adsb.to_str().unwrap(),
                "--out",
                o.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert_eq!(r.status.code(), Some(2), "{adsb:?} {o:?}");
        assert!(String::from_utf8_lossy(&r.stderr).contains("sealed"));
    }
    assert!(!out.join("o").exists());
    let r = Command::new(exe).args(["--adsb", "a"]).output().unwrap();
    assert_eq!(r.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out);
}

// REQ: OA-ENC-009
#[test]
fn arguments_are_strict() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let ok = a(&["--adsb", "a", "--out", "c", "--scorer-half"]).unwrap();
    assert!(ok.scorer_half);
    assert!(!a(&["--adsb", "a", "--out", "c"]).unwrap().scorer_half);
    assert!(a(&["--adsb", "a"]).is_err());
    assert!(a(&["--adsb", "a", "--adsb", "b", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--out", "c", "--x"]).is_err());
}

fn enc(a: u32, b: u32, start_s: i64, class: u8, alt: f32, min_h: f32) -> Encounter {
    Encounter {
        addr_a: a,
        addr_b: b,
        start_ns: start_s * 1_000_000_000,
        end_ns: (start_s + 5) * 1_000_000_000,
        closest_ns: (start_s + 2) * 1_000_000_000,
        min_h_m: min_h,
        v_at_closest_ft: if class == CLASS_ALT_UNKNOWN {
            f32::NAN
        } else {
            120.0
        },
        min_scaled_m: 800.5,
        alt_ft: alt,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        ticks_inside: 6,
        ticks_alt_unknown: 0,
        ticks_unevaluated: 0,
        flags_a: 1,
        flags_b: 0,
        ground_either: 0,
        class,
    }
}

// REQ: OA-ENC-011
#[test]
fn the_files_are_deterministic_and_the_summary_names_no_aircraft() {
    let cfg = tool_config();
    let list = vec![
        enc(0xABCDEF, 0x123456, 3700, 0, 2500.0, 300.0),
        enc(0x00AAAA, 0x00BBBB, 100, 1, 12_000.0, 1500.0),
        enc(0x00CCCC, 0x00DDDD, 100, CLASS_ALT_UNKNOWN, f32::NAN, 2000.0),
        enc(0x00EEEE, 0x00FFFF, 40_000, 2, 30_000.0, 4000.0),
    ];
    let mut rev = list.clone();
    rev.reverse();
    let csv = encounters_csv(&list);
    assert_eq!(csv, encounters_csv(&rev));
    // Sorted by start time; addresses appear here.
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines[1].starts_with("00AAAA,00BBBB,100.000"));
    assert!(csv.contains("ABCDEF,123456"));
    assert!(lines[2].contains("ALT_UNKNOWN") && lines[2].contains(",unknown,"));
    assert!(lines[3].contains(",C1,<3000,2500,") && lines[3].contains(",coast,"));
    let input = |l: &[Encounter]| {
        summary_text(&SummaryInput {
            cfg: &cfg,
            encounters: l,
            totals: CycleStats {
                cycles: 4,
                ..CycleStats::default()
            },
            log_overflow: 0,
            tracker_overflow: 0,
            measurements: 10,
            tick_rows: 20,
            clip_name: "clip.oaf",
            clip_sha256: &"0".repeat(64),
            scope: "test",
            csv_sha256: &"1".repeat(64),
        })
    };
    let s = input(&list);
    assert_eq!(s, input(&rev));
    for hidden in [
        "ABCDEF", "123456", "AAAA", "BBBB", "CCCC", "DDDD", "EEEE", "FFFF",
    ] {
        assert!(!s.to_uppercase().contains(hidden), "{hidden}");
    }
    assert!(s.contains("encounters: 4"));
    assert!(s.contains("C1: 1") && s.contains("ALT_UNKNOWN: 1"));
    assert!(s.contains("1970-01-01 00:00Z: 2, 1"));
    assert!(s.contains("1970-01-01 01:00Z: 1, 1"));
    assert!(s.contains("record state: complete"));
    assert!(s.contains(&format!("digest: encounters.csv sha256 {}", "1".repeat(64))));
    assert!(s.contains("lat 41.00 to 41.25, lon -88.25 to -88.00: 4, 2"));
    // A shortfall is stated.
    let t = CycleStats {
        not_evaluated: 3,
        ..CycleStats::default()
    };
    let s = summary_text(&SummaryInput {
        cfg: &cfg,
        encounters: &[],
        totals: t,
        log_overflow: 0,
        tracker_overflow: 0,
        measurements: 0,
        tick_rows: 0,
        clip_name: "c",
        clip_sha256: "",
        scope: "",
        csv_sha256: "",
    });
    assert!(s.contains("DEGRADED") && s.contains("pairs not evaluated (budget): 3"));
    // Names and bands.
    assert_eq!(class_name(0), "C1");
    assert_eq!(class_name(CLASS_ALT_UNKNOWN), "ALT_UNKNOWN");
    assert_eq!(class_name(254), "VOLUME_ONLY");
    assert_eq!(flags_text(1 | 32 | 16), "coast|uncertain|radar_only");
    assert_eq!(flags_text(0), "");
    assert_eq!(
        [2999.9, 3000.0, 9999.0, 10_000.0, 17_999.0, 18_000.0].map(band_of),
        [0, 1, 1, 2, 2, 3]
    );
    assert_eq!(band_of(f32::NAN), 4);
}

fn meas(t_s: i64, addr: u32, x_m: f64, y_m: f64, alt: i32) -> Measurement {
    let lat = 40.0 + y_m / 111_000.0;
    let lon = -90.0 + x_m / (111_320.0 * (40.0f64).to_radians().cos());
    Measurement {
        toa_ns: t_s * 1_000_000_000,
        pos_cov_m2: [25.0, 0.0, 25.0],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: 200.0,
        vel_north_mps: 0.0,
        lat_e7: (lat * 1e7).round() as i32,
        lon_e7: (lon * 1e7).round() as i32,
        alt_ft: alt,
        vrate_fpm: NONE_I32,
        addr,
        alt_quant_ft: 25,
        sensor: 0,
        ident: NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
    }
}

/// Two aircraft side by side at 390 kt for two minutes, 400 m apart, 100 ft apart vertically,
/// and a third far away.
fn pass(alt_b: i32) -> Vec<Measurement> {
    let mut v = Vec::new();
    for t in 0..120 {
        let x = 200.0 * t as f64;
        v.push(meas(t, 0x100001, x, 0.0, 5000));
        v.push(meas(t, 0x100002, x, 400.0, alt_b));
        v.push(meas(t, 0x100003, x, 90_000.0, 20_000));
    }
    v
}

// REQ: OA-ENC-012
#[test]
fn a_close_pass_through_the_tracker_is_one_c1_encounter_and_repeats_exactly() {
    let none = GroundTable::new();
    let go = |m: &[Measurement], g: &GroundTable| {
        track_and_record(m, &DEFAULT_CONFIG, g, &tool_config()).unwrap()
    };
    let r = go(&pass(5100), &none);
    assert_eq!(r.encounters.len(), 1, "{:?}", r.totals);
    let e = r.encounters[0];
    assert_eq!((e.addr_a, e.addr_b), (0x100001, 0x100002));
    assert_eq!(e.class, 0, "{e:?}");
    assert!(e.min_h_m > 350.0 && e.min_h_m < 450.0, "{}", e.min_h_m);
    assert!(e.v_at_closest_ft > 50.0 && e.v_at_closest_ft < 150.0);
    assert!(e.ticks_inside > 60);
    assert_eq!(r.log_overflow + r.tracker_overflow, 0);
    assert_eq!(r.totals.not_evaluated + r.totals.tracks_unscanned, 0);
    let again = go(&pass(5100), &none);
    assert_eq!(
        encounters_csv(&r.encounters),
        encounters_csv(&again.encounters)
    );
    assert_eq!(r.totals, again.totals);
    // The same pass 5,000 ft apart vertically is not an encounter.
    assert!(go(&pass(10_000), &none).encounters.is_empty());
    // Both reporting the ground at every tick: counted, not recorded.
    let mut g = GroundTable::new();
    for a in [0x100001u32, 0x100002] {
        g.insert(
            a,
            (0..12)
                .map(|k| (k * 10_000_000_000, oa_encounter::ON_GROUND))
                .collect(),
        );
    }
    let r = go(&pass(5100), &g);
    assert!(r.encounters.is_empty());
    assert!(r.totals.ground_excluded > 60);
}

// REQ: OA-ENC-012
#[test]
fn the_ground_state_at_a_tick_is_the_latest_report_not_too_old() {
    let mut g = GroundTable::new();
    g.insert(7, vec![(10_000_000_000, 2), (20_000_000_000, 1)]);
    let at = |s: i64| ground_at(&g, 7, s * 1_000_000_000);
    assert_eq!(at(9), 0);
    assert_eq!(at(10), 2);
    assert_eq!(at(19), 2);
    assert_eq!(at(20), 1);
    assert_eq!(at(50), 1);
    assert_eq!(at(51), 0);
    assert_eq!(ground_at(&g, 8, 15_000_000_000), 0);
}
