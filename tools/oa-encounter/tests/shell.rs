//! The tool: sealed paths refused, the files deterministic and free of identities in the summary,
//! and a synthetic close pass through the tracker and the record.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use oa_encounter::{CycleStats, Encounter, CLASS_ALT_UNKNOWN};
use oa_encounter_run::args::parse;
use oa_encounter_run::guard::refuse_sealed;
use oa_encounter_run::report::{
    at_vertical_minimum, band_of, class_name, context_of, encounters_csv, flags_text, summary_text,
    Airport, SummaryInput, CONTEXT_NAMES,
};
use oa_encounter_run::run::{
    ground_at, load_airports, report_separations, tool_config, track_and_record, GroundTable,
};
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
        "x/scds/rec-20261119/day2",
        "x/scds/rec-20261119/day2/sub/file.oaf",
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
        "rec-20261119/day2",
        "stdds-20261119-holdout",
        "stdds-smoke-20261119T000000Z",
    ] {
        let dir = root.join("scds").join(sealed);
        std::fs::create_dir_all(&dir).unwrap();
        let link = root.join(format!("innocent-{}", sealed.replace('/', "-")));
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
        (
            PathBuf::from("/x/scds/rec-20261119/day2/a.oaf"),
            out.join("o"),
        ),
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
    assert_eq!(
        a(&["--adsb", "a", "--out", "c", "--nasr", "n.zip"])
            .unwrap()
            .nasr,
        Some(PathBuf::from("n.zip"))
    );
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
        interp_ns: (start_s + 2) * 1_000_000_000 + 400_000_000,
        min_h_interp_m: min_h - 3.0,
        min_v_ft: if class == CLASS_ALT_UNKNOWN {
            f32::NAN
        } else {
            100.0
        },
        vrate_a_fpm: 0.0,
        vrate_b_fpm: -64.0,
        max_abs_vrate_fpm: 64.0,
        alt_ft: alt,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        ticks_inside: 6,
        ticks_alt_unknown: 0,
        ticks_unevaluated: 0,
        flags_a: u8::from(a == 0xABCDEF),
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
    let csv = encounters_csv(&list, &[], 1000.0);
    assert_eq!(csv, encounters_csv(&rev, &[], 1000.0));
    // Sorted by start time; addresses appear here.
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines[1].starts_with("0000AAAA,0000BBBB,100.000"));
    assert!(csv.contains("00ABCDEF,00123456"));
    assert!(lines[2].contains("ALT_UNKNOWN") && lines[2].contains(",unknown,"));
    assert!(lines[3].contains(",C1,<3000,2500,") && lines[3].contains(",coast,"));
    let input = |l: &[Encounter]| {
        summary_text(&SummaryInput {
            cfg: &cfg,
            encounters: l,
            airports: &[],
            airports_from: "none",
            report_sep_m: &BTreeMap::new(),
            totals: CycleStats {
                cycles: 4,
                ..CycleStats::default()
            },
            lat_limit_deg: 85.8,
            log_overflow: 0,
            tracker_overflow: 0,
            measurements: 10,
            tick_rows: 20,
            clip_name: "clip.oaf",
            clip_sha256: &"0".repeat(64),
            sealed_window_dropped: 0,
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
    assert!(s.contains("  AIRBORNE: 2  ["));
    assert!(s.contains("  AIRBORNE_ALT_UNKNOWN: 1  ["));
    assert!(s.contains("  SURFACE: 0  ["));
    assert!(s.contains("  DEGRADED_TRACK: 1  ["));
    assert!(s.contains("minimum, by class\n  C2: 1\n  C3: 1\n"));
    assert!(
        s.find("\nAIRBORNE, vertically converging or below the minimum, by class")
            .unwrap()
            < s.find("\nAIRBORNE_ALT_UNKNOWN by class").unwrap()
    );
    assert!(
        s.find("\nAIRBORNE_ALT_UNKNOWN by class").unwrap() < s.find("\nSURFACE by class").unwrap()
    );
    assert!(s.contains("  under 900: 2 of 2\n  900 to 925: 0\n"));
    assert!(s.contains("columns: context, class,"));
    assert!(s.contains("AIRBORNE, C3, 120, 100, 4000.0, 3997.0, 5, 0/-64, 64, "));
    assert!(s.contains("C1: 1") && s.contains("ALT_UNKNOWN: 1"));
    assert!(s.contains("1970-01-01 00:00Z: 2, 1"));
    assert!(s.contains("1970-01-01 11:00Z: 1, 0"));
    assert!(!s.contains("1970-01-01 01:00Z"));
    assert!(s.contains("record state: complete"));
    assert!(s.contains(&format!("digest: encounters.csv sha256 {}", "1".repeat(64))));
    assert!(s.contains("lat 41.00 to 41.25, lon -88.25 to -88.00: 3, 1"));
    // A shortfall is stated.
    let t = CycleStats {
        not_evaluated: 3,
        ..CycleStats::default()
    };
    let s = summary_text(&SummaryInput {
        cfg: &cfg,
        encounters: &[],
        airports: &[],
        airports_from: "",
        report_sep_m: &BTreeMap::new(),
        totals: t,
        lat_limit_deg: 85.8,
        log_overflow: 0,
        tracker_overflow: 0,
        measurements: 0,
        tick_rows: 0,
        clip_name: "c",
        clip_sha256: "",
        sealed_window_dropped: 0,
        scope: "",
        csv_sha256: "",
    });
    assert!(s.contains("DEGRADED") && s.contains("pairs not evaluated (budget or table full): 3"));
    // Duplicate or invalid rows and rows above the latitude limit make the record degraded too.
    for t in [
        CycleStats {
            rows_duplicate: 2,
            ..CycleStats::default()
        },
        CycleStats {
            rows_bad: 1,
            ..CycleStats::default()
        },
        CycleStats {
            rows_high_lat: 1,
            ..CycleStats::default()
        },
    ] {
        let s = summary_text(&SummaryInput {
            cfg: &cfg,
            encounters: &[],
            airports: &[],
            airports_from: "",
            report_sep_m: &BTreeMap::new(),
            totals: t,
            lat_limit_deg: 85.8,
            log_overflow: 0,
            tracker_overflow: 0,
            measurements: 0,
            tick_rows: 0,
            clip_name: "c",
            clip_sha256: "",
            sealed_window_dropped: 0,
            scope: "",
            csv_sha256: "",
        });
        assert!(s.contains("record state: DEGRADED"));
    }
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
        vel_lag_s: 0.0,
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
        encounters_csv(&r.encounters, &[], 1000.0),
        encounters_csv(&again.encounters, &[], 1000.0)
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

fn at(addr_a: u32, alt: f32, v: f32, vr: f32, flags: u8) -> Encounter {
    let mut e = enc(addr_a, addr_a + 1, 10, 2, alt, 3000.0);
    e.v_at_closest_ft = v;
    e.max_abs_vrate_fpm = vr;
    e.flags_a = flags;
    e
}

// REQ: OA-ENC-014
#[test]
fn the_context_is_surface_then_degraded_then_by_altitude_knowledge() {
    let ap = |dlat_nm: f64, elev: f64| Airport {
        lat_deg: 41.1234567 + dlat_nm / 60.0,
        lon_deg: -88.1234567,
        elev_ft: elev,
    };
    let base = enc(1, 2, 10, 1, 500.0, 900.0);
    let ctx = |e: &Encounter, a: &[Airport]| CONTEXT_NAMES[context_of(e, a)];
    assert_eq!(ctx(&base, &[]), "AIRBORNE");
    // Within 5 NM and under the field elevation plus 200 ft: surface; at the limit or over it, not.
    assert_eq!(ctx(&base, &[ap(0.0, 700.0)]), "SURFACE");
    assert_eq!(ctx(&base, &[ap(4.9, 1000.0)]), "SURFACE");
    assert_eq!(ctx(&base, &[ap(0.0, 300.0)]), "AIRBORNE");
    assert_eq!(ctx(&base, &[ap(0.0, 299.0)]), "AIRBORNE");
    assert_eq!(ctx(&base, &[ap(0.0, 301.0)]), "SURFACE");
    assert_eq!(ctx(&base, &[ap(5.2, 1000.0)]), "AIRBORNE");
    // The ground flag alone is surface; an unknown altitude does not meet the airport rule.
    let mut g = base;
    g.ground_either = 1;
    assert_eq!(ctx(&g, &[]), "SURFACE");
    let mut unk = base;
    unk.alt_ft = f32::NAN;
    unk.v_at_closest_ft = f32::NAN;
    assert_eq!(ctx(&unk, &[ap(0.0, 700.0)]), "AIRBORNE_ALT_UNKNOWN");
    // Coasting or uncertain, on either track, is degraded unless it is surface.
    for (a, b) in [(1u8, 0u8), (0, 32), (33, 1)] {
        let mut d = base;
        d.flags_a = a;
        d.flags_b = b;
        assert_eq!(ctx(&d, &[]), "DEGRADED_TRACK");
        assert_eq!(ctx(&d, &[ap(0.0, 700.0)]), "SURFACE");
    }
    // Other flags do not degrade.
    let mut r = base;
    r.flags_a = 16 | 64;
    assert_eq!(ctx(&r, &[]), "AIRBORNE");
}

// REQ: OA-ENC-015
#[test]
fn the_high_altitude_table_and_breakdown_and_the_vertical_histogram() {
    let cfg = tool_config();
    let es = vec![
        at(0x100, 30_000.0, 990.0, 0.0, 0),
        at(0x200, 30_000.0, 975.0, 1500.0, 0),
        at(0x300, 30_000.0, 600.0, 1500.0, 0),
        at(0x400, 30_000.0, 600.0, 100.0, 1),
        at(0x500, 30_000.0, 800.0, 0.0, 0),
        at(0x600, 12_000.0, 930.0, 0.0, 0),
    ];
    let mut sep = BTreeMap::new();
    sep.insert((0x300, 0x301), 12.5f32);
    sep.insert((0x400, 0x401), 400.0f32);
    let s = summary_text(&SummaryInput {
        cfg: &cfg,
        encounters: &es,
        airports: &[],
        airports_from: "none",
        report_sep_m: &sep,
        totals: CycleStats::default(),
        lat_limit_deg: 85.8,
        log_overflow: 0,
        tracker_overflow: 0,
        measurements: 0,
        tick_rows: 0,
        clip_name: "c",
        clip_sha256: "",
        sealed_window_dropped: 0,
        scope: "",
        csv_sha256: "",
    });
    assert!(s.contains("pressure altitude at the closest approach: 5 encounters"));
    assert!(s.contains("at or over 975 ft (1,000 ft levels, 25 ft quantisation): 2"));
    assert!(s.contains("a vertical rate of 300 fpm or more on either (climb or descent): 1"));
    assert!(s.contains("anything else: 2"));
    assert!(s.contains("either track coasting or uncertain: 1;"));
    assert!(s.contains("within 50 m of each other at some time: 1"));
    // The coasting one is degraded, so the other five are airborne; the histogram bins by 25 ft.
    assert!(s.contains("  under 900: 2 of 5\n  900 to 925: 0\n  925 to 950: 1\n  950 to 975: 0\n  975 to 1000: 2\n"));
    assert!(s.contains("12.5\n") && s.contains("n/a\n"));
    for hidden in ["000100", "000300"] {
        assert!(!s.to_uppercase().contains(hidden));
    }
}

// REQ: OA-ENC-016
#[test]
fn report_level_separation_is_found_for_the_high_altitude_pairs_only() {
    let mut m = Vec::new();
    for t in 0..10 {
        m.push(meas(t, 0x100001, 200.0 * t as f64, 0.0, 30_000));
        m.push(meas(t, 0x100002, 200.0 * t as f64, 400.0, 30_000));
        m.push(meas(t, 0x100003, 200.0 * t as f64, 400.0, 5_000));
    }
    let mk = |a: u32, b: u32, alt: f32| {
        let mut e = enc(a, b, 0, 1, alt, 400.0);
        e.end_ns = 9 * 1_000_000_000;
        e
    };
    let es = vec![
        mk(0x100001, 0x100002, 30_000.0),
        mk(0x100001, 0x100003, 5_000.0),
    ];
    let r = report_separations(&m, &es);
    assert_eq!(r.len(), 1);
    let d = r[&(0x100001, 0x100002)];
    assert!((d - 400.0).abs() < 3.0, "{d}");
    // A pair that never reported together within 2 s has none.
    let far = vec![mk(0x100001, 0x999999, 30_000.0)];
    assert!(report_separations(&m, &far).is_empty());
}

// REQ: OA-ENC-017
#[test]
fn aircraft_a_level_apart_are_kept_and_counted_apart_from_the_class_tables() {
    let mk = |v: f32, va: f32, vb: f32| {
        let mut e = enc(1, 2, 10, 2, 5000.0, 3000.0);
        e.v_at_closest_ft = v;
        e.vrate_a_fpm = va;
        e.vrate_b_fpm = vb;
        e
    };
    // At the limit (1,000 less one quantum) and both level: in; one under, or either climbing: out.
    assert!(at_vertical_minimum(&mk(975.0, 0.0, 0.0), 1000.0));
    assert!(at_vertical_minimum(&mk(999.0, 299.0, -299.0), 1000.0));
    assert!(!at_vertical_minimum(&mk(974.0, 0.0, 0.0), 1000.0));
    assert!(!at_vertical_minimum(&mk(990.0, 300.0, 0.0), 1000.0));
    assert!(!at_vertical_minimum(&mk(990.0, 0.0, -300.0), 1000.0));
    assert!(!at_vertical_minimum(&mk(990.0, f32::NAN, 0.0), 1000.0));
    let cfg = tool_config();
    let es = vec![
        mk(990.0, 0.0, 0.0),
        mk(980.0, 10.0, 10.0),
        mk(990.0, 800.0, 0.0),
        mk(500.0, 0.0, 0.0),
    ];
    let s = summary_text(&SummaryInput {
        cfg: &cfg,
        encounters: &es,
        airports: &[],
        airports_from: "none",
        report_sep_m: &BTreeMap::new(),
        totals: CycleStats::default(),
        lat_limit_deg: 85.8,
        log_overflow: 0,
        tracker_overflow: 0,
        measurements: 0,
        tick_rows: 0,
        clip_name: "c",
        clip_sha256: "",
        sealed_window_dropped: 0,
        scope: "",
        csv_sha256: "",
    });
    assert!(s.contains("  AIRBORNE: 4  ["));
    assert!(s.contains("    of which AT_VERTICAL_MINIMUM: 2  [vertical separation at the closest approach at least 975 ft"));
    assert!(s.contains("AIRBORNE, vertically converging or below the minimum, by class\n  C3: 2\n"));
    assert!(s.contains("AIRBORNE/AT_VERTICAL_MINIMUM by class\n  C3: 2\n"));
    assert!(s.contains("  AIRBORNE/AT_VERTICAL_MINIMUM C3: 2"));
    // The record keeps them: the column says which.
    let csv = encounters_csv(&es, &[], 1000.0);
    let col = csv
        .lines()
        .next()
        .unwrap()
        .split(',')
        .position(|c| c == "at_vertical_minimum")
        .unwrap();
    let flags: Vec<&str> = csv
        .lines()
        .skip(1)
        .map(|l| l.split(',').nth(col).unwrap())
        .collect();
    assert_eq!(flags, ["1", "1", "0", "0"]);
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// A zip of one stored file, written by hand.
fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let mut z = Vec::new();
    let u16le = |z: &mut Vec<u8>, v: u16| z.extend_from_slice(&v.to_le_bytes());
    let u32le = |z: &mut Vec<u8>, v: u32| z.extend_from_slice(&v.to_le_bytes());
    z.extend_from_slice(b"PK\x03\x04");
    for v in [20u16, 0, 0, 0, 0x21] {
        u16le(&mut z, v);
    }
    u32le(&mut z, crc);
    u32le(&mut z, data.len() as u32);
    u32le(&mut z, data.len() as u32);
    u16le(&mut z, name.len() as u16);
    u16le(&mut z, 0);
    z.extend_from_slice(name.as_bytes());
    z.extend_from_slice(data);
    let cd = z.len() as u32;
    z.extend_from_slice(b"PK\x01\x02");
    for v in [20u16, 20, 0, 0, 0, 0x21] {
        u16le(&mut z, v);
    }
    u32le(&mut z, crc);
    u32le(&mut z, data.len() as u32);
    u32le(&mut z, data.len() as u32);
    u16le(&mut z, name.len() as u16);
    for v in [0u16, 0, 0, 0] {
        u16le(&mut z, v);
    }
    u32le(&mut z, 0);
    u32le(&mut z, 0);
    z.extend_from_slice(name.as_bytes());
    let cd_len = z.len() as u32 - cd;
    z.extend_from_slice(b"PK\x05\x06");
    for v in [0u16, 0, 1, 1] {
        u16le(&mut z, v);
    }
    u32le(&mut z, cd_len);
    u32le(&mut z, cd);
    u16le(&mut z, 0);
    z
}

// REQ: OA-ENC-018
#[test]
fn airports_are_read_from_apt_base_public_airports_only_with_their_elevation() {
    let head = "EFF_DATE,SITE_NO,SITE_TYPE_CODE,STATE_CODE,ARPT_ID,CITY,COUNTRY_CODE,REGION_CODE,ARPT_NAME,FACILITY_USE_CODE,LAT_DEG,LAT_MIN,LAT_SEC,LAT_HEMIS,LAT_DECIMAL,LONG_DEG,LONG_MIN,LONG_SEC,LONG_HEMIS,LONG_DECIMAL,ELEV,MAG_VARN,MAG_HEMIS,MAG_VARN_YEAR,TPA,RESP_ARTCC_ID,COMPUTER_ID,ARPT_STATUS,TWR_TYPE_CODE,ICAO_ID";
    // lat 41 30 0 N = 41.5, lon 88 15 0 W = -88.25
    let row = |id: &str, site: &str, usage: &str, elev: &str| {
        format!(
            "2026/10/29,0001.,{site},IL,{id},CITY,US,AGL,NAME {id},{usage},41,30,0.000,N,41.50000000,88,15,0.000,W,-88.25000000,{elev},3,W,2020,,ZAU,{id},O,NON-ATCT,"
        )
    };
    let csv = format!(
        "{head}\n{}\n{}\n{}\n{}\n",
        row("PUB", "A", "PU", "679.4"),
        row("PRV", "A", "PR", "500.0"),
        row("HEL", "H", "PU", "100.0"),
        row("LOW", "A", "PU", "-12.5"),
    );
    let dir = scratch("nasr");
    let zip = dir.join("NASR.zip");
    std::fs::write(&zip, stored_zip("APT_BASE.csv", csv.as_bytes())).unwrap();
    let a = load_airports(&zip).unwrap();
    assert_eq!(a.len(), 2, "{a:?}");
    assert_eq!(a[0].elev_ft, 679.4);
    assert_eq!(a[1].elev_ft, -12.5);
    assert!((a[0].lat_deg - 41.5).abs() < 1e-6 && (a[0].lon_deg + 88.25).abs() < 1e-6);
    // An archive without the file, and a file that is not an archive, are errors.
    std::fs::write(&zip, stored_zip("OTHER.csv", csv.as_bytes())).unwrap();
    assert!(load_airports(&zip).unwrap_err().contains("APT_BASE.csv"));
    std::fs::write(&zip, b"not a zip").unwrap();
    assert!(load_airports(&zip).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
