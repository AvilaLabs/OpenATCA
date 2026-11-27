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
use oa_encounter_run::qualification::{
    score, AlertEpisode, Case, CaseResult, Evaluation, Outcome, Subject,
};
use oa_encounter_run::run::GroundTable;
use oa_safety::net::{NetReport, NetState, Reason};
use oa_track::{Measurement, TrackerConfig, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT};

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
    assert!(!ok.speed_diagnostics);
    assert!(
        a(&[
            "--adsb",
            "a",
            "--config",
            "k",
            "--out",
            "c",
            "--speed-diagnostics"
        ])
        .unwrap()
        .speed_diagnostics
    );
    assert!(a(&[
        "--adsb",
        "a",
        "--config",
        "k",
        "--out",
        "c",
        "--speed-diagnostics",
        "--speed-diagnostics"
    ])
    .is_err());
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
        ("/home/connoravila/Documents/Avila-Labs/openatca-data/raw/adsblol/2026.11.20/a.oaf", "k.cfg", out.join("o")),
        (
            "fine.oaf",
            "/home/connoravila/Documents/Avila-Labs/openatca-data/scds/stdds-20261119-holdout/k.cfg",
            out.join("o"),
        ),
        (
            "fine.oaf",
            "k.cfg",
            PathBuf::from("/home/connoravila/Documents/Avila-Labs/openatca-data/scds/stdds-smoke-20261119T1/o"),
        ),
    ] {
        // seal-allow: runs the tool's own test binary; the run itself goes through the guard
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
    // seal-allow: runs the tool's own test binary; the run itself goes through the guard
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

// REQ: OA-CA-013
#[test]
fn a_log_capacity_beyond_the_bound_is_refused_by_name_and_the_bound_itself_is_accepted() {
    let bound = 1usize << 24;
    for key in ["alert_log_capacity", "event_log_capacity"] {
        let with = |n: usize| {
            let line = DEV_CFG
                .lines()
                .find(|l| l.starts_with(key))
                .unwrap()
                .to_string();
            DEV_CFG.replace(&line, &format!("{key} = {n}"))
        };
        assert!(parse_config(&with(bound)).is_ok(), "{key}");
        let e = parse_config(&with(bound + 1)).unwrap_err();
        assert!(e.contains(key), "{e}");
        let e = parse_config(&with(usize::MAX)).unwrap_err();
        assert!(e.contains(key), "{e}");
    }
    // More polygon points than the core holds, and a text no configuration is as long as.
    let many: String = (0..40).map(|k| format!(" 41.{k:02},-88.0")).collect();
    let bad = format!("{DEV_CFG}\nvolume = polygon 0 5000 2 500{many}\n");
    assert!(parse_config(&bad).unwrap_err().contains("points"));
    let long = format!("{DEV_CFG}\n#{}\n", "x".repeat(1 << 20));
    assert!(parse_config(&long).unwrap_err().contains("longer than"));
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

    // A rank-one covariance is valid, and one f32 ULP above its determinant boundary is within
    // the documented outward-rounding allowance.
    let rank_one = r95_of([4.0, 2.0, 1.0]);
    assert_eq!(rank_one, (5.991_464_547_107_98f64 * 5.0).sqrt() as f32);
    let one_ulp_up = f32::from_bits(1.0f32.to_bits() + 1);
    let near_edge = r95_of([1.0, one_ulp_up, 1.0]);
    let expected = (5.991_464_547_107_98f64 * (1.0 + f64::from(one_ulp_up))).sqrt() as f32;
    assert_eq!(near_edge, expected);
    assert!(r95_of([1.0, 2.0, 1.0]).is_nan());
    let materially_non_psd = 1.0 + 8.0 * f32::EPSILON;
    assert!(r95_of([1.0, materially_non_psd, 1.0]).is_nan());

    // The existing core boundary treats NaN radius as an input shortfall, and does not call the
    // affected cycle fully running.
    let set = parse_config(DEV_CFG).unwrap();
    let mut state = oa_ca::ca_new(&set.ca).unwrap();
    let mut alerts = oa_ca::alert_log_new(4).unwrap();
    let mut events = oa_ca::event_log_new(4).unwrap();
    let mut net = oa_safety::net::state_table_new(10_000_000_000, 2_000_000_000).unwrap();
    let row = oa_ca::CaTrack {
        lat_deg: 41.5,
        lon_deg: -88.0,
        addr: 0x100001,
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        r95_m: r95_of([1.0, 2.0, 1.0]),
        alt_ft: 10_000.0,
        vrate_fpm: 0.0,
        flags: 0,
        ground: AIRBORNE,
    };
    let out = oa_ca::ca_cycle(&mut state, 0, &[row], &mut alerts, &mut events, &mut net);
    assert_eq!(out.stats.r95_clamped, 1);
    assert_eq!(out.input_short, 1);
    assert!(matches!(
        out.report,
        NetReport::Degraded {
            reason: Reason::InputMissing,
            count: 1,
            ..
        }
    ));
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

// REQ: OA-CA-011, OA-CA-015, OA-CA-025
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
    let input = |l: &[Alert], t: CycleStats, log_overflow: u64, capacity_refused: u64| {
        summary_text(&SummaryInput {
            cfg: &s.ca,
            alerts: l,
            events: &ev,
            totals: t,
            net_cycles: &cycles,
            log_overflow,
            event_overflow: 0,
            tracker_overflow: 0,
            tracker_capacity_refused: capacity_refused,
            measurements: 10,
            tick_rows: 20,
            rows_skipped: 0,
            cycles_both: 0,
            final_net: "RUNNING",
            clip_name: "clip.oaf",
            clip_sha256: &"0".repeat(64),
            sealed_window_dropped: 0,
            config_name: "dev.cfg",
            config_sha256: &"2".repeat(64),
            scope: "test",
            alerts_sha256: &"1".repeat(64),
            adaptation_manifest_sha256: None,
            adaptation_version: None,
        })
    };
    let ok = CycleStats {
        cycles: 3600,
        ..CycleStats::default()
    };
    let t = input(&list, ok, 0, 0);
    assert_eq!(t, input(&rev, ok, 0, 0));
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
    let overflow = input(
        &[],
        CycleStats {
            count_overflow: true,
            ..ok
        },
        0,
        0,
    );
    assert!(overflow.contains("net state: DEGRADED"));
    assert!(overflow.contains("lifetime counter overflow (totals incomplete): 1"));
    assert!(t.contains(
        "bad-row reason counts below overlap; they are subsets of bad rows and must not be added"
    ));
    assert!(t.contains("look-ahead 120 s, hysteresis 10 s;"));
    assert!(t.contains("net state after the end of the run (alerts ended last included): RUNNING"));
    assert!(t.contains("pairs tested horizontally only (vertical rate unknown): 0"));
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
            CycleStats {
                rows_bad: 1,
                rows_bad_speed: 1,
                ..ok
            },
        ),
        (
            "rows above 85 degrees of latitude: 1",
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
            "rows lost before a cycle (told to conflict alert): 6",
            CycleStats {
                rows_lost_upstream: 6,
                ..ok
            },
        ),
        (
            "of them, lost at the end of the run (after the last cycle's report): 2",
            CycleStats {
                finish_dropped: 2,
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
        let t = input(&[], tot, 0, 0);
        assert!(t.contains("net state: DEGRADED"), "{what}");
        assert!(t.contains(what), "{what}");
    }
    let bad_rows = input(
        &[],
        CycleStats {
            rows_bad: 1,
            rows_bad_lat: 1,
            rows_bad_speed: 1,
            ..ok
        },
        0,
        0,
    );
    assert!(bad_rows.contains("rows with a bad position, speed, altitude or rate: 1"));
    assert!(bad_rows.contains("bad latitude (overlapping reason subset): 1"));
    assert!(bad_rows.contains("speed over configured maximum (overlapping reason subset): 1"));
    let t = input(&[], ok, 2, 0);
    assert!(t.contains("net state: DEGRADED") && t.contains("alerts lost to a full log: 2"));
    let capacity_loss = input(&[], ok, 0, 2);
    assert!(capacity_loss.contains("net state: DEGRADED"));
    assert!(
        capacity_loss.contains("valid tracker measurements refused for lack of a track slot: 2")
    );
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

fn synthetic_crossing_and_vertical_control() -> Vec<Measurement> {
    let mut v = Vec::new();
    for t in 0..190 {
        let d = 200.0 * t as f64;
        v.push(meas(t, 0x100001, d, 0.0, 200.0, 20_000));
        v.push(meas(t, 0x100002, 60_000.0 - d, 0.0, -200.0, 20_000));
        v.push(meas(t, 0x100003, d, 150_000.0, 200.0, 20_000));
        v.push(meas(t, 0x100004, 60_000.0 - d, 150_000.0, -200.0, 25_000));
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
    // A tracker output table too small for its rows loses some: conflict alert is told, counts
    // them, and no cycle that follows a loss reads as running.
    let small = TrackerConfig {
        out_capacity: 6,
        ..DEFAULT_CONFIG
    };
    let r = track_and_alert(&head_on(20_000), &small, &none, &set).unwrap();
    assert!(r.tracker_overflow > 0, "no overflow to test");
    assert_eq!(
        r.totals.rows_lost_upstream,
        r.tracker_overflow + r.rows_skipped
    );
    assert!(r
        .net_cycles
        .keys()
        .any(|k| k.starts_with("DEGRADED: INPUT MISSING")));
}

#[test]
fn synthetic_tracker_crossing_and_vertical_control_are_scored_against_declared_windows() {
    // Truth is calculated from the generated measurement trajectories and configured synthetic
    // volume before running tracker or CA. Relative distance is 60,000 - 400t m.
    let set = parse_config(DEV_CFG).unwrap();
    let truth_h_m = set.ca.default_volume.h_m;
    let entry_s = (60_000.0 - truth_h_m) / 400.0;
    let exit_s = (60_000.0 + truth_h_m) / 400.0;
    const LAST_TICK_S: i64 = 189;
    const S: i64 = 1_000_000_000;
    assert!(5000.0 >= set.ca.default_volume.v_ft);

    let ground = GroundTable::new();
    let record = track_and_alert(
        &synthetic_crossing_and_vertical_control(),
        &DEFAULT_CONFIG,
        &ground,
        &set,
    )
    .unwrap();
    assert_eq!(record.alerts.len(), 1);
    assert_eq!(
        record.log_overflow + record.event_overflow + record.tracker_overflow,
        0
    );
    assert_eq!(
        record.totals.not_evaluated + record.totals.tracks_unscanned,
        0
    );
    assert_eq!(record.net_cycles.len(), 1);
    assert_eq!(
        record.net_cycles.get("RUNNING"),
        Some(&record.totals.cycles)
    );

    let alerts: Vec<AlertEpisode> = record
        .alerts
        .iter()
        .map(|alert| AlertEpisode {
            subject: Subject {
                a: alert.addr_a,
                b: alert.addr_b,
            },
            start_ns: alert.start_ns,
            end_ns: alert.end_ns,
            warning_ns: Some(alert.start_ns),
        })
        .collect();
    let cases = [
        Case {
            subject: Subject {
                a: 0x100001,
                b: 0x100002,
            },
            window_start_ns: 0,
            window_end_ns: LAST_TICK_S * S,
            violation: Some((
                (entry_s * S as f64).round() as i64,
                (exit_s * S as f64).round() as i64,
            )),
            alert_desired: true,
            warning_from_ns: 0,
            warning_deadline_ns: ((entry_s - 20.0) * S as f64).floor() as i64,
            evaluation: Evaluation::Available,
        },
        Case {
            subject: Subject {
                a: 0x100003,
                b: 0x100004,
            },
            window_start_ns: 0,
            window_end_ns: LAST_TICK_S * S,
            violation: None,
            alert_desired: false,
            warning_from_ns: 0,
            warning_deadline_ns: LAST_TICK_S * S,
            evaluation: Evaluation::Available,
        },
    ];
    let mut results = [CaseResult {
        outcome: Outcome::Miss,
        warning_index: None,
        lead_ns: None,
    }; 2];
    let totals = score(&cases, &alerts, &mut results).unwrap();
    assert_eq!(
        (totals.desired_cases, totals.hits, totals.misses),
        (1, 1, 0)
    );
    assert_eq!(
        (
            totals.no_alert_cases,
            totals.nuisance_controls,
            totals.quiet_controls
        ),
        (1, 0, 1)
    );
    let lead_ns = results[0].lead_ns.expect("scored warning lead time");
    assert!(lead_ns >= 20 * S, "lead {lead_ns} ns");
    assert_eq!(results[1].outcome, Outcome::Quiet);
}

#[test]
fn tracker_capacity_refusal_degrades_from_the_observation_cycle_and_is_counted() {
    let set = parse_config(DEV_CFG).unwrap();
    let none = GroundTable::new();
    let mut traffic = Vec::new();
    for t in 0..=20 {
        traffic.push(meas(t, 0x100001, 70.0 * t as f64, 0.0, 70.0, 20_000));
        if t >= 10 {
            traffic.push(meas(t, 0x100002, 60_000.0, 150_000.0, 0.0, 20_000));
        }
    }
    let one_slot = TrackerConfig {
        max_tracks: 1,
        ..DEFAULT_CONFIG
    };
    let refused = track_and_alert(&traffic, &one_slot, &none, &set).unwrap();
    assert_eq!(refused.tracker_capacity_refused, 11);
    // The tracker emits ticks strictly before a measurement. The last refusal is at
    // the end of the recording, so it appears in the final state and run count only.
    assert_eq!(refused.totals.rows_lost_upstream, 10);
    assert!(refused.net_cycles.contains_key("RUNNING"));
    assert!(refused
        .net_cycles
        .keys()
        .any(|state| state == "DEGRADED: INPUT MISSING"));
    assert_eq!(refused.final_net, "DEGRADED: INPUT MISSING");

    // With room for both identities, B is admitted and contributes rows without a refusal.
    let two_slots = TrackerConfig {
        max_tracks: 2,
        ..DEFAULT_CONFIG
    };
    let recovered = track_and_alert(&traffic, &two_slots, &none, &set).unwrap();
    assert_eq!(recovered.tracker_capacity_refused, 0);
    assert_eq!(recovered.totals.rows_lost_upstream, 0);
    assert!(recovered.totals.tracks > refused.totals.tracks);
    assert_eq!(recovered.final_net, "RUNNING");

    // Dense distinct identities arrive in one tick interval. The runner must retain their
    // count without retaining one timestamp entry per refusal.
    let mut dense: Vec<Measurement> = (0..=12)
        .map(|t| meas(t, 0x100001, 70.0 * t as f64, 0.0, 70.0, 20_000))
        .collect();
    for i in 0..256u32 {
        let mut candidate = meas(10, 0x200000 + i, 60_000.0, 150_000.0, 0.0, 20_000);
        candidate.toa_ns += i64::from(i) * 1_000_000 + 1;
        dense.push(candidate);
    }
    dense.sort_by_key(|m| m.toa_ns);
    let high_density = track_and_alert(&dense, &one_slot, &none, &set).unwrap();
    assert_eq!(high_density.tracker_capacity_refused, 256);
    assert_eq!(high_density.totals.rows_lost_upstream, 256);
    assert!(high_density
        .net_cycles
        .keys()
        .any(|state| state == "DEGRADED: INPUT MISSING"));
}

// REQ: OA-CA-010
#[test]
fn evaluation_budget_saturation_prioritizes_and_surfaces_deferred_pairs_deterministically() {
    let mut set = parse_config(DEV_CFG).unwrap();
    set.ca.eval_budget = 1;
    let none = GroundTable::new();
    let mut traffic = Vec::new();
    // Pair 1/2 starts closer but closes at 140 m/s. Pair 3/4 starts farther
    // apart and closes at 400 m/s, reaching the 5 NM threshold first. Both
    // crossing times are inside the 120 s look-ahead once the tracks confirm.
    for t in 0..140 {
        let s = t as f64;
        for (a, b, start_m, y_m, speed) in [
            (0x100001, 0x100002, 25_000.0, 0.0, 70.0),
            (0x100003, 0x100004, 40_000.0, 150_000.0, 200.0),
        ] {
            let d = f64::from(speed) * s;
            traffic.push(meas(t, a, d, y_m, speed, 20_000));
            traffic.push(meas(t, b, start_m - d, y_m, -speed, 20_000));
        }
    }
    let run = |rows: &[Measurement]| track_and_alert(rows, &DEFAULT_CONFIG, &none, &set).unwrap();
    let first = run(&traffic);
    assert!(
        first.totals.not_evaluated > 0,
        "budget saturation was not counted"
    );
    assert!(first
        .net_cycles
        .keys()
        .any(|state| state == "DEGRADED: BUDGET EXCEEDED"));
    let first_alert = first.alerts.iter().min_by_key(|a| a.start_ns).unwrap();
    assert_eq!(
        (first_alert.addr_a, first_alert.addr_b),
        (0x100003, 0x100004),
        "the first evaluation slot goes to the pair with the earlier predicted loss"
    );

    // Reverse aircraft rows within each timestamp. The evaluation order, surfaced deferrals,
    // and resulting alerts must not depend on input row order.
    let mut permuted = traffic.clone();
    for rows in permuted.chunks_exact_mut(4) {
        rows.reverse();
    }
    let again = run(&permuted);
    assert_eq!(first.totals, again.totals);
    assert_eq!(first.net_cycles, again.net_cycles);
    assert_eq!(alerts_csv(&first.alerts), alerts_csv(&again.alerts));
}
