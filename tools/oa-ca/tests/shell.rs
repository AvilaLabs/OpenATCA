//! The tool: sealed paths refused, strict arguments and configuration, the files deterministic
//! and free of identities in the summary, and a synthetic converging pair through the tracker and
//! conflict alert. Configuration values in these tests are SYNTHETIC TEST VALUES, not ERAM, STARS
//! or 7110.65 values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use oa_ca::{
    Alert, AltLost, CycleStats, AF_ALT_UNKNOWN, AF_COAST, AF_ONE_GROUND, AF_RADAR_ONLY, AIRBORNE,
};
use oa_ca_run::args::parse;
use oa_ca_run::config::parse_config;
use oa_ca_run::report::{
    alerts_csv, context_of, events_csv, flags_text, state_text, summary_text, SummaryInput,
    CONTEXT_NAMES,
};
use oa_ca_run::run::{r95_of, track_and_alert};
use oa_encounter_run::run::GroundTable;
use oa_safety::net::{NetState, Reason};
use oa_track::{Measurement, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT};

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-ca-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// REQ: OA-CA-022
#[test]
fn arguments_are_strict_and_a_sealed_path_exits_2_before_anything_is_opened() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let ok = a(&[
        "--adsb",
        "a",
        "--config",
        "k",
        "--out",
        "c",
        "--scorer-half",
    ])
    .unwrap();
    assert!(ok.scorer_half);
    assert!(
        !a(&["--adsb", "a", "--config", "k", "--out", "c"])
            .unwrap()
            .scorer_half
    );
    assert!(a(&["--adsb", "a", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--config", "k"]).is_err());
    assert!(a(&["--adsb", "a", "--adsb", "b", "--config", "k", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "--config", "k", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--config", "k", "--out", "c", "--x"]).is_err());
    assert!(a(&[
        "--adsb",
        "a",
        "--config",
        "k",
        "--out",
        "c",
        "--scorer-half",
        "--scorer-half"
    ])
    .is_err());
    let exe = env!("CARGO_BIN_EXE_oa-ca");
    let out = scratch("exit");
    for (adsb, cfg, o) in [
        ("/x/scds/rec-20261119/a.oaf", "k.cfg", out.join("o")),
        (
            "fine.oaf",
            "/x/scds/stdds-20261119-holdout/k.cfg",
            out.join("o"),
        ),
        (
            "fine.oaf",
            "k.cfg",
            PathBuf::from("/x/scds/stdds-smoke-20261119T1/o"),
        ),
    ] {
        let r = Command::new(exe)
            .args([
                "--adsb",
                adsb,
                "--config",
                cfg,
                "--out",
                o.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert_eq!(r.status.code(), Some(2), "{adsb} {cfg}");
        assert!(String::from_utf8_lossy(&r.stderr).contains("sealed"));
    }
    assert!(!out.join("o").exists());
    let r = Command::new(exe).args(["--adsb", "a"]).output().unwrap();
    assert_eq!(r.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out);
}

// REQ: OA-CA-023
#[test]
fn the_configuration_has_no_defaults_and_names_what_is_wrong() {
    let s = parse_config(DEV_CFG).unwrap();
    let c = s.ca;
    assert_eq!(c.look_ahead_ns, 120_000_000_000);
    assert_eq!(c.hysteresis_ns, 10_000_000_000);
    assert_eq!(c.default_volume.h_m, 5.0 * 1852.0);
    assert_eq!(c.default_volume.v_ft, 1000.0);
    assert_eq!(c.n_volumes, 1);
    assert_eq!(c.volumes[0].h_m, 3.0 * 1852.0);
    assert_eq!(c.volumes[0].alt_hi_ft, 17_999);
    assert!(DEV_CFG.contains("DEVELOPMENT VALUES, NOT ERAM/STARS VALUES"));
    // Every key is required: take each line away in turn.
    for (n, line) in DEV_CFG.lines().enumerate() {
        let key = line.split('=').next().unwrap().trim();
        if line.starts_with('#') || line.trim().is_empty() || key == "volume" {
            continue;
        }
        let without: String = DEV_CFG
            .lines()
            .enumerate()
            .filter(|(k, _)| *k != n)
            .map(|(_, l)| format!("{l}\n"))
            .collect();
        let e = parse_config(&without).unwrap_err();
        assert!(e.contains(&format!("`{key}`")), "{key}: {e}");
    }
    let with = |extra: &str| format!("{DEV_CFG}\n{extra}\n");
    assert!(parse_config(&with("look_ahead_s = 5"))
        .unwrap_err()
        .contains("given twice"));
    assert!(parse_config(&with("nonsense = 5"))
        .unwrap_err()
        .contains("unknown key"));
    assert!(parse_config(&with("this is not a setting"))
        .unwrap_err()
        .contains("key = value"));
    let bad = DEV_CFG.replace("look_ahead_s = 120", "look_ahead_s = nan");
    assert!(parse_config(&bad).unwrap_err().contains("not finite"));
    let bad = DEV_CFG.replace("look_ahead_s = 120", "look_ahead_s = soon");
    assert!(parse_config(&bad).unwrap_err().contains("not a number"));
    let bad = DEV_CFG.replace("max_tracks = 8192", "max_tracks = -3");
    assert!(parse_config(&bad).unwrap_err().contains("max_tracks"));
    // The core's own check refuses out-of-range values.
    let bad = DEV_CFG.replace("hysteresis_s = 10", "hysteresis_s = 0");
    assert!(parse_config(&bad).unwrap_err().contains("hysteresis_ns"));
    // A polygon volume, and a bad one.
    let poly = with("volume = polygon 0 5000 2 500 41.0,-88.0 41.0,-87.0 42.0,-87.0 42.0,-88.0");
    let s = parse_config(&poly).unwrap();
    assert_eq!(s.ca.n_volumes, 2);
    assert!(matches!(s.ca.volumes[1].area, oa_ca::Area::Polygon(_)));
    let bad = with("volume = polygon 0 5000 2 500 41.0,-88.0 41.0,-87.0");
    assert!(parse_config(&bad).unwrap_err().contains("volume"));
    let bad = with("volume = polygon 0 5000 2 500 41.0,-88.0 41.0,-87.0 41.0,-86.0");
    assert!(parse_config(&bad).unwrap_err().contains("polygon"));
    let bad = with("volume = box 41 42 -88 -87 0 5000 2");
    assert!(parse_config(&bad).unwrap_err().contains("volume"));
    let bad = with("volume = box 91 92 -88 -87 0 5000 2 500");
    assert!(parse_config(&bad).unwrap_err().contains("off the globe"));
}

// REQ: OA-CA-024
#[test]
fn the_95_percent_radius_is_the_larger_eigenvalue_with_the_chi_square_quantile() {
    // Circular, variance 100 m^2: sqrt(5.9915 * 100).
    let r = r95_of([100.0, 0.0, 100.0]);
    assert!((r - 24.478).abs() < 0.01, "{r}");
    // Elongated: the larger variance rules, in either axis and along a diagonal.
    let a = r95_of([400.0, 0.0, 25.0]);
    assert!((a - (5.9915f64 * 400.0).sqrt() as f32).abs() < 0.01);
    assert_eq!(a, r95_of([25.0, 0.0, 400.0]));
    let d = r95_of([212.5, 187.5, 212.5]);
    assert!((d - (5.9915f64 * 400.0).sqrt() as f32).abs() < 0.01);
    // An unusable covariance is NaN, so the core holds it to the cap and counts it.
    assert!(r95_of([f32::NAN, 0.0, 1.0]).is_nan());
    assert!(r95_of([-1.0, 0.0, 1.0]).is_nan());
    assert!(r95_of([f32::INFINITY, 0.0, 1.0]).is_nan());
}

fn alert(a: u32, b: u32, start_s: i64, flags: u16, ground: u8, current: u32) -> Alert {
    Alert {
        addr_a: a,
        addr_b: b,
        start_ns: start_s * 1_000_000_000,
        end_ns: (start_s + 5) * 1_000_000_000,
        peak_ns: (start_s + 2) * 1_000_000_000,
        ttl_first_s: 80.0,
        ttl_peak_s: 12.5,
        h_now_peak_m: 9000.0,
        h_miss_peak_m: 300.0,
        min_h_miss_m: 250.0,
        min_v_miss_ft: if flags & AF_ALT_UNKNOWN != 0 {
            f32::NAN
        } else {
            100.0
        },
        h_eff_m: 9500.0,
        v_eff_ft: if flags & AF_ALT_UNKNOWN != 0 {
            f32::NAN
        } else {
            1000.0
        },
        alt_ft: 12_000.0,
        lat_e7: 411_234_567,
        lon_e7: -881_234_567,
        ticks_conflict: 6,
        ticks_current: current,
        ticks_unevaluated: 0,
        flags,
        ground_either: ground,
    }
}

// REQ: OA-CA-025
#[test]
fn the_files_are_deterministic_and_the_summary_counts_flags_hours_states_and_shortfalls() {
    let s = parse_config(DEV_CFG).unwrap();
    let list = vec![
        alert(0xABCDEF, 0x123456, 3700, AF_COAST, 0, 0),
        alert(0x00AAAA, 0x00BBBB, 100, 0, 0, 3),
        alert(0x00CCCC, 0x00DDDD, 100, AF_ALT_UNKNOWN, 0, 0),
        alert(
            0x00EEEE,
            0x00FFFF,
            40_000,
            AF_ONE_GROUND | AF_ALT_UNKNOWN,
            1,
            0,
        ),
    ];
    let mut rev = list.clone();
    rev.reverse();
    let csv = alerts_csv(&list);
    assert_eq!(csv, alerts_csv(&rev));
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 5);
    assert!(lines[1].starts_with("0000AAAA,0000BBBB,100.000"));
    assert!(csv.contains("00ABCDEF,00123456"));
    assert!(lines[2].contains(",ALT_UNKNOWN,alt_unknown,"));
    assert!(lines[3].contains(",DEGRADED_TRACK,coast,"));
    assert!(lines[4].contains(",GROUND,alt_unknown|one_ground,"));
    let ev = vec![
        AltLost {
            t_ns: 5_000_000_000,
            addr: 0xABCDEF,
            last_alt_ft: 9000.0,
            lat_e7: 411_234_567,
            lon_e7: -881_234_567,
            flags: 0,
            ground: 1,
        },
        AltLost {
            t_ns: 2_000_000_000,
            addr: 0x1,
            last_alt_ft: 100.0,
            lat_e7: 0,
            lon_e7: 0,
            flags: 0,
            ground: 2,
        },
    ];
    let e = events_csv(&ev);
    assert!(e.lines().nth(1).unwrap().starts_with("2.000,00000001,100,"));
    assert!(e.lines().nth(1).unwrap().ends_with(",1"));
    let mut cycles = BTreeMap::new();
    cycles.insert("RUNNING".to_string(), 3598u64);
    cycles.insert("DEGRADED: BUDGET EXCEEDED".to_string(), 2u64);
    let input = |l: &[Alert], t: CycleStats, log_overflow: u64| {
        summary_text(&SummaryInput {
            cfg: &s.ca,
            alerts: l,
            events: &ev,
            totals: t,
            net_cycles: &cycles,
            log_overflow,
            event_overflow: 0,
            tracker_overflow: 0,
            measurements: 10,
            tick_rows: 20,
            clip_name: "clip.oaf",
            clip_sha256: &"0".repeat(64),
            config_name: "dev.cfg",
            config_sha256: &"2".repeat(64),
            scope: "test",
            alerts_sha256: &"1".repeat(64),
        })
    };
    let ok = CycleStats {
        cycles: 3600,
        ..CycleStats::default()
    };
    let t = input(&list, ok, 0);
    assert_eq!(t, input(&rev, ok, 0));
    for hidden in [
        "ABCDEF", "123456", "AAAA", "BBBB", "CCCC", "DDDD", "EEEE", "FFFF",
    ] {
        assert!(!t.to_uppercase().contains(hidden), "{hidden}");
    }
    assert!(t.contains("alerts: 4"));
    assert!(t.contains("  AIRBORNE: 1  [") && t.contains("  ALT_UNKNOWN: 1  ["));
    assert!(t.contains("  GROUND: 1  [") && t.contains("  DEGRADED_TRACK: 1  ["));
    assert!(t.contains("  alt_unknown: 2\n  coast: 1\n"));
    assert!(t.contains("  one_ground: 1\n"));
    assert!(t.contains("were current (inside both minima) at some cycle: 1"));
    assert!(t.contains("  1970-01-01 00:00Z: 2\n  1970-01-01 01:00Z: 1\n  1970-01-01 11:00Z: 1\n"));
    assert!(t.contains("  RUNNING: 3598\n") && t.contains("  DEGRADED: BUDGET EXCEEDED: 2\n"));
    assert!(t.contains("altitude lost (a track that had an altitude and lost it): 2 (on the ground when lost: 1, not: 1)"));
    assert!(t.contains("net state: complete"));
    assert!(t.contains("default volume: horizontal 5.00 NM, vertical 1000 ft"));
    assert!(t.contains("volume 1: box lat 41.4000 to 42.4000, lon -88.4000 to -87.4000, -2000 to 17999 ft, horizontal 3.00 NM"));
    assert!(t.contains(&format!("alerts.csv sha256 {}", "1".repeat(64))));
    // Every shortfall makes the net DEGRADED and is printed with its count.
    for (what, tot) in [
        (
            "pairs not evaluated: 3",
            CycleStats {
                not_evaluated: 3,
                ..ok
            },
        ),
        (
            "tracks the grid scan did not reach: 2",
            CycleStats {
                tracks_unscanned: 2,
                ..ok
            },
        ),
        (
            "rows sharing an address (all left out): 2",
            CycleStats {
                rows_duplicate: 2,
                ..ok
            },
        ),
        (
            "rows with a bad position, speed, altitude or rate: 1",
            CycleStats { rows_bad: 1, ..ok },
        ),
        (
            "rows above the grid's latitude limit: 1",
            CycleStats {
                rows_high_lat: 1,
                ..ok
            },
        ),
        (
            "rows whose 95% radius was held to the cap: 4",
            CycleStats {
                r95_clamped: 4,
                ..ok
            },
        ),
        (
            "cycles refused (time not after the last): 1",
            CycleStats {
                out_of_order: 1,
                ..ok
            },
        ),
        (
            "alerts not opened (table full): 7",
            CycleStats {
                open_dropped: 7,
                ..ok
            },
        ),
    ] {
        let t = input(&[], tot, 0);
        assert!(t.contains("net state: DEGRADED"), "{what}");
        assert!(t.contains(what), "{what}");
    }
    let t = input(&[], ok, 2);
    assert!(t.contains("net state: DEGRADED") && t.contains("alerts lost to a full log: 2"));
    // Names.
    assert_eq!(flags_text(AF_COAST | AF_RADAR_ONLY), "coast|radar_only");
    assert_eq!(flags_text(0), "");
    assert_eq!(
        CONTEXT_NAMES[context_of(&alert(1, 2, 0, 0, 0, 0))],
        "AIRBORNE"
    );
    assert_eq!(state_text(&NetState::Running), "RUNNING");
    assert_eq!(
        state_text(&NetState::Unavailable {
            reason: Reason::SurveillanceLost
        }),
        "UNAVAILABLE: SURVEILLANCE LOST"
    );
}

fn meas(t_s: i64, addr: u32, x_m: f64, y_m: f64, ve: f32, alt: i32) -> Measurement {
    let lat = 40.0 + y_m / 111_000.0;
    let lon = -90.0 + x_m / (111_320.0 * (40.0f64).to_radians().cos());
    Measurement {
        toa_ns: t_s * 1_000_000_000,
        pos_cov_m2: [25.0, 0.0, 25.0],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: ve,
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

/// Two aircraft head on at 200 m/s each from 60 km apart, and a third far away.
fn head_on(alt_b: i32) -> Vec<Measurement> {
    let mut v = Vec::new();
    for t in 0..150 {
        let d = 200.0 * t as f64;
        v.push(meas(t, 0x100001, d, 0.0, 200.0, 20_000));
        v.push(meas(t, 0x100002, 60_000.0 - d, 0.0, -200.0, alt_b));
        v.push(meas(t, 0x100003, d, 150_000.0, 200.0, 20_000));
    }
    v
}

// REQ: OA-CA-026
#[test]
fn a_converging_pair_through_the_tracker_is_one_alert_that_repeats_exactly() {
    let set = parse_config(DEV_CFG).unwrap();
    let none = GroundTable::new();
    let go = |m: &[Measurement]| track_and_alert(m, &DEFAULT_CONFIG, &none, &set).unwrap();
    let r = go(&head_on(20_000));
    assert_eq!(r.alerts.len(), 1, "{:?}", r.totals);
    let a = r.alerts[0];
    assert_eq!((a.addr_a, a.addr_b), (0x100001, 0x100002));
    assert!(a.ttl_first_s > 0.0 && a.ttl_peak_s < a.ttl_first_s);
    assert!(a.min_h_miss_m < 2000.0, "{}", a.min_h_miss_m);
    assert_eq!(a.flags & AF_ALT_UNKNOWN, 0);
    assert_eq!(context_of(&a), 0);
    assert!(a.ticks_conflict > 10);
    assert_eq!(r.log_overflow + r.tracker_overflow + r.event_overflow, 0);
    assert_eq!(r.totals.not_evaluated + r.totals.tracks_unscanned, 0);
    assert_eq!(r.net_cycles.keys().collect::<Vec<_>>(), vec!["RUNNING"]);
    assert_eq!(r.net_cycles["RUNNING"], r.totals.cycles);
    let again = go(&head_on(20_000));
    assert_eq!(alerts_csv(&r.alerts), alerts_csv(&again.alerts));
    assert_eq!(r.totals, again.totals);
    // The same crossing 5,000 ft apart vertically is no alert.
    assert!(go(&head_on(25_000)).alerts.is_empty());
    let _ = AIRBORNE;
}
