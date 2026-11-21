//! The tool: sealed paths refused, strict arguments and configuration, the floor lookups, the
//! CIFP areas, and synthetic aircraft through the tracker and the net. Every configuration value
//! in these tests is a SYNTHETIC TEST VALUE, not an FAA value. The Guam case is SYNTHETIC: an
//! approach that descends early onto rising terrain, in the manner of Korean Air 801 (1997), not
//! its recorded track.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use oa_encounter_run::run::GroundTable;
use oa_msaw::{
    AIRBORNE, CK_APPROACH, CK_DEPARTURE, CK_RUNWAY, FLOOR_NONE, GROUND_UNKNOWN, ON_GROUND,
};
use oa_msaw_run::args::parse;
use oa_msaw_run::cifp::{approach_runway, corridors_from_cifp};
use oa_msaw_run::config::{parse_config, Settings};
use oa_msaw_run::floors::{ceil_ft, tile_name, Floors, ObstacleIndex};
use oa_msaw_run::qnh::Qnh;
use oa_msaw_run::report::{
    alerts_csv, flags_text, inhibitions_csv, skipped_csv, state_text, summary_text, DataUsed,
    SummaryInput,
};
use oa_msaw_run::run::{r95_of, track_and_alert, Record};
use oa_mva::{new_model, Model, SetKind};
use oa_safety::net::{NetState, Reason};
use oa_tiff::{Grid, TileSet};
use oa_track::{Measurement, DEFAULT_CONFIG, M_HAS_VEL, NONE_I32, NO_IDENT};

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");
const S: i64 = 1_000_000_000;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-msaw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// REQ: OA-MSAW-030
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
        "--terrain",
        "t",
        "--dof",
        "d",
        "--mva",
        "m",
        "--cifp",
        "f",
        "--metar",
        "w",
        "--from-s",
        "10",
        "--to-s",
        "70",
        "--scorer-half",
    ])
    .unwrap();
    assert!(ok.scorer_half);
    assert_eq!(ok.window_s, Some((10.0, 70.0)));
    assert_eq!(ok.terrain, Some(PathBuf::from("t")));
    assert_eq!(ok.cifp, Some(PathBuf::from("f")));
    assert_eq!(ok.metar, Some(PathBuf::from("w")));
    let base = a(&["--adsb", "a", "--config", "k", "--out", "c"]).unwrap();
    assert!(!base.scorer_half && base.terrain.is_none() && base.window_s.is_none());
    assert!(a(&["--adsb", "a", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--adsb", "b", "--config", "k", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "--config", "k", "--out", "c"]).is_err());
    assert!(a(&["--adsb", "a", "--config", "k", "--out", "c", "--x"]).is_err());
    assert!(a(&["--adsb", "a", "--config", "k", "--out", "c", "--from-s", "1"]).is_err());
    assert!(
        a(&["--adsb", "a", "--config", "k", "--out", "c", "--from-s", "5", "--to-s", "5"]).is_err()
    );
    assert!(
        a(&["--adsb", "a", "--config", "k", "--out", "c", "--from-s", "x", "--to-s", "9"]).is_err()
    );
    let exe = env!("CARGO_BIN_EXE_oa-msaw");
    let out = scratch("exit");
    let data = "/home/connoravila/Documents/Avila-Labs/openatca-data";
    let sealed_day = format!("{data}/raw/adsblol/2026.11.20/a.oaf");
    let sealed_cfg = format!("{data}/scds/stdds-20261119-holdout/k.cfg");
    let sealed_out = format!("{data}/scds/stdds-smoke-20261119T1/o");
    let sealed_terrain = format!("{data}/scds/rec-20261119/day2/terrain");
    let o = out.join("o");
    let o = o.to_str().unwrap();
    for argv in [
        vec!["--adsb", &sealed_day, "--config", "k.cfg", "--out", o],
        vec!["--adsb", "fine.oaf", "--config", &sealed_cfg, "--out", o],
        vec![
            "--adsb",
            "fine.oaf",
            "--config",
            "k.cfg",
            "--out",
            &sealed_out,
        ],
        vec![
            "--adsb",
            "fine.oaf",
            "--config",
            "k.cfg",
            "--out",
            o,
            "--terrain",
            &sealed_terrain,
        ],
    ] {
        // seal-allow: runs the tool's own test binary; the run itself goes through the guard
        let r = Command::new(exe).args(&argv).output().unwrap();
        assert_eq!(r.status.code(), Some(2), "{argv:?}");
        assert!(String::from_utf8_lossy(&r.stderr).contains("sealed"));
    }
    assert!(!out.join("o").exists());
    // seal-allow: runs the tool's own test binary; the run itself goes through the guard
    let r = Command::new(exe).args(["--adsb", "a"]).output().unwrap();
    assert_eq!(r.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out);
}

// REQ: OA-MSAW-032
#[test]
fn the_configuration_has_no_defaults_and_names_what_is_wrong() {
    let s = parse_config(DEV_CFG).unwrap();
    let c = s.msaw;
    assert_eq!((c.look_ahead_s, c.warning_s, c.samples), (60.0, 20.0, 13));
    assert_eq!(c.hysteresis_ns, 10 * S);
    assert_eq!(
        (c.terrain_clearance_ft, c.obstacle_clearance_ft),
        (500, 500)
    );
    assert_eq!(c.corridors.len(), 0);
    assert_eq!(
        (
            c.approach_allowance_ft,
            c.approach_min_clearance_ft,
            c.velocity_uncertainty_mps
        ),
        (100, 100, 5.0)
    );
    assert_eq!(
        (s.no_qnh_allowance_ft, s.metar_radius_nm, s.metar_max_age_s),
        (300, 100.0, 7200)
    );
    assert!(s.inhibits.is_empty() && s.cifp_airports.is_empty());
    assert!(DEV_CFG.contains("DEVELOPMENT VALUES, NOT FAA VALUES"));
    // Every single-valued key is required: take each line away in turn.
    for (n, line) in DEV_CFG.lines().enumerate() {
        let key = line.split('=').next().unwrap().trim();
        if line.starts_with('#') || line.trim().is_empty() {
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
    let e = parse_config(&with("look_ahead_s = 30")).unwrap_err();
    assert!(e.contains("given twice"), "{e}");
    let e = parse_config(&with("nonsense = 1")).unwrap_err();
    assert!(e.contains("unknown key"), "{e}");
    let e = parse_config(&with("just text")).unwrap_err();
    assert!(e.contains("expected `key = value`"), "{e}");
    // The core's own refusal names the key.
    let e = parse_config(&DEV_CFG.replace("samples = 13", "samples = 1")).unwrap_err();
    assert!(e.contains("samples"), "{e}");
    let e = parse_config(&DEV_CFG.replace("samples = 13", "samples = x")).unwrap_err();
    assert!(e.contains("samples"), "{e}");
    let e = parse_config(&DEV_CFG.replace("warning_s = 20", "warning_s = nan")).unwrap_err();
    assert!(e.contains("warning_s"), "{e}");
    // A corridor, a CIFP airport and an inhibition.
    let s = parse_config(&with(
        "corridor = 40 -88 180 9260 556 1852 600 2250\ncifp_airport = KORD\n\
         inhibit = 5 65 -1000 20000 SUP_1 NUISANCE_ALERTS 13.2,144.6 13.2,145 13.6,145 13.6,144.6",
    ))
    .unwrap();
    assert_eq!(s.msaw.corridors.len(), 1);
    assert_eq!(s.msaw.corridors[0].faf_alt_ft, 2250);
    assert_eq!(s.msaw.corridors[0].thr_lat_e7, 400_000_000);
    assert_eq!(s.cifp_airports, vec!["KORD".to_string()]);
    assert_eq!(s.inhibits[0].reason, "NUISANCE ALERTS");
    assert_eq!(s.inhibits[0].author, "SUP 1");
    assert_eq!(s.inhibits[0].vertices.len(), 4);
    for bad in [
        "corridor = 40 -88 180 9260 556 1852 600",
        "corridor = 40 -88 x 9260 556 1852 600 2250",
        "corridor = 40 -88 400 9260 556 1852 600 2250",
        "inhibit = 5 4 0 1 A B 1,1 1,2 2,2",
        "inhibit = 5 6 0 1 A B 1,1 1,2",
        "inhibit = 5 6 0 1 A",
    ] {
        assert!(parse_config(&with(bad)).is_err(), "{bad}");
    }
    let e = parse_config(&DEV_CFG.replace("metar_radius_nm = 100", "metar_radius_nm = 5000"))
        .unwrap_err();
    assert!(e.contains("metar_radius_nm"), "{e}");
    let e = parse_config(&DEV_CFG.replace("no_qnh_allowance_ft = 300", "no_qnh_allowance_ft = -1"))
        .unwrap_err();
    assert!(e.contains("no_qnh_allowance_ft"), "{e}");
    let e = parse_config(&DEV_CFG.replace(
        "velocity_uncertainty_mps = 5",
        "velocity_uncertainty_mps = -1",
    ))
    .unwrap_err();
    assert!(e.contains("velocity_uncertainty_mps"), "{e}");
}

/// A grid whose elevation rises north of latitude 13.40, 3,000 ft per 0.1 degree, in metres: the
/// synthetic ridge.
fn ridge_grid() -> Grid {
    let (w, h) = (200u32, 400u32);
    let (lat0, lon0, d) = (13.70, 144.70, 0.001);
    let mut elev = Vec::new();
    for r in 0..h {
        let lat = lat0 - f64::from(r) * d;
        let ft = ((lat - 13.40) / 0.10 * 3000.0).max(0.0);
        for _ in 0..w {
            elev.push((ft * 0.3048) as f32);
        }
    }
    Grid {
        width: w,
        height: h,
        lon0,
        lat0,
        dlon: d,
        dlat: d,
        elev,
        nodata_count: 0,
        point_registered: true,
        crs: 4269,
        vertical_cs: None,
    }
}

/// A chart model of one rectangular FUS3 sector over the synthetic area.
fn chart(min_alt: i32, la0: f64, la1: f64, lo0: f64, lo1: f64) -> Model {
    let e7 = |x: f64| (x * 1e7).round() as i32;
    let mut m = new_model();
    m.facility.push(*b"GUM\0");
    m.set.push(SetKind::Fus3);
    m.sub.push([0; 4]);
    m.sector_id.push(*b"A\0\0\0\0\0\0\0");
    m.min_alt_ft.push(min_alt);
    m.ring_first.push(0);
    m.ring_count.push(1);
    m.bbox_lat_min.push(e7(la0));
    m.bbox_lat_max.push(e7(la1));
    m.bbox_lon_min.push(e7(lo0));
    m.bbox_lon_max.push(e7(lo1));
    m.ring_vert_first.push(0);
    m.ring_vert_count.push(5);
    m.ring_hole.push(0);
    for (la, lo) in [(la0, lo0), (la0, lo1), (la1, lo1), (la1, lo0), (la0, lo0)] {
        m.vert_lat_e7.push(e7(la));
        m.vert_lon_e7.push(e7(lo));
    }
    m.chart_facility.push(*b"GUM\0");
    m.chart_set.push(SetKind::Fus3);
    m.chart_sub.push([0; 4]);
    m.chart_first_sector.push(0);
    m.chart_sector_count.push(1);
    m
}

fn ridge_floors() -> Floors {
    Floors::with_tiles(
        TileSet {
            tiles: vec![ridge_grid()],
        },
        Some(ObstacleIndex::from_points(std::iter::empty())),
        Some(chart(100, 13.2, 13.65, 144.65, 144.95)),
    )
}

fn point(lat: f64, lon: f64, radius_m: f32) -> oa_msaw::SamplePoint {
    oa_msaw::SamplePoint {
        lat_e7: (lat * 1e7).round() as i32,
        lon_e7: (lon * 1e7).round() as i32,
        radius_m,
        track_deg: -1,
    }
}

// REQ: OA-MSAW-033
#[test]
fn the_floor_lookups_are_conservative_and_absent_where_there_is_no_data() {
    assert_eq!(ceil_ft(100.0), 329);
    assert_eq!(ceil_ft(0.3048), 1);
    assert_eq!(ceil_ft(-0.1), 0);
    assert_eq!(tile_name(41, -88).as_deref(), Some("USGS_1_n42w088.tif"));
    assert_eq!(tile_name(41, 5), None);
    // Obstacles: the highest top inside the circle, none outside it.
    let ix = ObstacleIndex::from_points(
        [
            (400_000_000, -880_000_000, 900),
            (400_010_000, -880_000_000, 1500),
            (400_300_000, -880_000_000, 4000),
        ]
        .into_iter(),
    );
    assert_eq!(ix.count, 3);
    assert_eq!(ix.highest_within(40.0, -88.0, 50.0), 900);
    assert_eq!(ix.highest_within(40.0, -88.0, 200.0), 1500);
    assert_eq!(ix.highest_within(40.0, -88.0, 40_000.0), 4000);
    assert_eq!(ix.highest_within(41.0, -88.0, 200.0), FLOOR_NONE);
    // Terrain from the grid: the highest sample under the circle, rounded up, and the chart.
    let mut f = ridge_floors();
    // 13.50 N: the grid's elevation there is 3,000 ft at 13.50; with a 250 m radius bucket (about
    // 0.0023 degrees), the cell's half diagonal and the widening by a cell, the highest is some
    // 0.006 degrees north of it, 30 ft per 0.001 degree.
    let s = f.floor_at(&point(13.50, 144.80, 100.0)).unwrap();
    assert!(
        s.terrain_ft >= 3000 && s.terrain_ft < 3400,
        "{}",
        s.terrain_ft
    );
    assert_eq!(s.mva_ft, 100);
    assert_eq!(s.obstacle_ft, FLOOR_NONE);
    assert!(
        s.obstacle_covered,
        "an obstacle file with none near is data"
    );
    assert!(
        !Floors::new(None, None, None)
            .floor_at(&point(13.5, 144.8, 100.0))
            .unwrap()
            .obstacle_covered,
        "no DOF given: no obstacle data"
    );
    // A circle that leaves the grid has no complete answer: absent, never the lower bound.
    let s = f.floor_at(&point(13.7, 144.80, 100.0)).unwrap();
    assert_eq!(s.terrain_ft, FLOOR_NONE);
    // Outside the chart: no chart coverage; terrain still found.
    let s = f.floor_at(&point(13.3, 144.80, 100.0)).unwrap();
    assert_eq!(s.mva_ft, 100);
    let s = f.floor_at(&point(13.68, 144.80, 100.0)).unwrap();
    assert_eq!(s.mva_ft, FLOOR_NONE);
    assert!(s.terrain_ft >= 0);
    assert!(f.stats.terrain_absent >= 1 && f.stats.chart_absent >= 1);
    // Every lookup is made at the point itself: nothing is answered from a cell.
    f.floor_at(&point(13.30, 144.80, 100.0)).unwrap();
    assert_eq!(f.stats.cache_hits, 0);
    // With nothing at all, every part is absent.
    let mut none = Floors::new(None, None, None);
    let s = none.floor_at(&point(13.3, 144.8, 100.0)).unwrap();
    assert_eq!((s.terrain_ft, s.mva_ft), (FLOOR_NONE, FLOOR_NONE));
    assert!(!none.has_terrain() && !none.has_chart() && !none.has_obstacles());
}

fn fixture_cifp() -> oa_arinc424::Cifp {
    let header = include_str!("../../../crates/oa-arinc424/tests/fixtures/header.txt");
    let body = [
        include_str!("../../../crates/oa-arinc424/tests/fixtures/airports.txt"),
        include_str!("../../../crates/oa-arinc424/tests/fixtures/runways.txt"),
        include_str!("../../../crates/oa-arinc424/tests/fixtures/terminal_waypoints.txt"),
        include_str!("../../../crates/oa-arinc424/tests/fixtures/approach.txt"),
    ]
    .concat();
    let mut hdr: Vec<String> = header.lines().map(str::to_string).collect();
    hdr[0].replace_range(28..35, &format!("{:07}", body.lines().count()));
    let mut text = hdr.join("\n");
    text.push('\n');
    text.push_str(&body);
    oa_arinc424::parse_bytes(text.as_bytes(), "fixture").unwrap()
}

// REQ: OA-MSAW-082
#[test]
fn rows_of_takes_one_address_hash_and_is_given_once() {
    let a = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    let base = ["--adsb", "a", "--config", "k", "--out", "c"];
    let with = |extra: &[&str]| a(&[&base[..], extra].concat());
    assert_eq!(
        with(&["--rows-of", "cd5161b4"]).unwrap().rows_of.as_deref(),
        Some("cd5161b4")
    );
    assert_eq!(with(&[]).unwrap().rows_of, None);
    assert!(with(&["--rows-of"]).is_err());
    assert!(with(&["--rows-of", "a", "--rows-of", "b"]).is_err());
}

// REQ: OA-MSAW-080
#[test]
fn every_airport_with_an_approach_in_the_area_gets_its_box_and_strip_and_a_configured_one_is_left_alone(
) {
    use oa_msaw_run::cifp::corridors_from_cifp_area;
    let c = fixture_cifp();
    let area = (41.0, 43.0, -89.0, -87.0);
    let made = corridors_from_cifp_area(&c, area, &[], 1.0).unwrap();
    assert!(made.airports >= 1);
    assert!(made.with_corridors.contains(&"KORD".to_string()));
    // The box of the runway end with the approach and the strip along it, and no departure corridor.
    let ord: Vec<_> = made
        .corridors
        .iter()
        .filter(|k| k.label.starts_with("KORD "))
        .collect();
    let kinds: Vec<u8> = ord.iter().map(|k| k.kind).collect();
    assert_eq!(kinds, vec![CK_APPROACH, CK_RUNWAY]);
    assert!(made.corridors.iter().all(|k| k.kind != CK_DEPARTURE));
    // The same as the airport's own, but for what the area leaves out.
    let own = corridors_from_cifp(&c, "KORD").unwrap();
    assert!(own.corridors.iter().all(|k| ord.iter().any(|o| **o == *k)));
    // An airport outside the box, or one the configuration names, is not built.
    assert_eq!(
        corridors_from_cifp_area(&c, (0.0, 1.0, 0.0, 1.0), &[], 1.0)
            .unwrap()
            .airports,
        0
    );
    let skipped = corridors_from_cifp_area(&c, area, &["KORD".to_string()], 1.0).unwrap();
    assert!(skipped.airports + 1 == made.airports);
    assert!(skipped
        .corridors
        .iter()
        .all(|k| !k.label.starts_with("KORD ")));
    // The configuration key, and the heading tolerance beside it.
    let base = DEV_CFG;
    let with = parse_config(&format!("{base}\ncifp_area_margin_nm = 10\n")).unwrap();
    assert_eq!(with.cifp_area_margin_nm, Some(10.0));
    assert_eq!(with.msaw.capture_heading_deg, 30.0);
    assert!(parse_config(&format!("{base}\ncifp_area_margin_nm = 0\n")).is_err());
    assert_eq!(parse_config(base).unwrap().cifp_area_margin_nm, None);
}

// REQ: OA-MSAW-034
#[test]
fn corridors_come_from_the_runway_end_and_the_final_approach_fix_of_its_approaches() {
    assert_eq!(approach_runway("I04R").as_deref(), Some("04R"));
    assert_eq!(approach_runway("R22L-Z").as_deref(), Some("22L"));
    assert_eq!(approach_runway("L09").as_deref(), Some("09"));
    assert_eq!(approach_runway("V-A"), None);
    assert_eq!(approach_runway("I"), None);
    let mut c = fixture_cifp();
    // The fixture's terminal waypoints do not hold the fix REKKS: the corridor takes 5 NM.
    let a = corridors_from_cifp(&c, "KORD").unwrap();
    // The approach corridor and the strip along its runway (the fixture has no SID and no
    // reciprocal runway, so no departure corridor).
    assert_eq!(a.corridors.len(), 2, "{a:?}");
    let k = &a.corridors[0];
    assert_eq!(k.label, "KORD RW04R");
    assert_eq!(k.kind, CK_APPROACH);
    let strip = &a.corridors[1];
    assert_eq!(strip.kind, CK_RUNWAY);
    assert_eq!(strip.label, "KORD RW04R strip");
    let rwy = c.runways.iter().find(|r| r.ident == "RW04R").unwrap();
    let run_m = (f64::from(rwy.length_ft) - f64::from(rwy.displaced_threshold_ft)) * 0.3048;
    assert!((strip.corridor.length_m - run_m - 1852.0).abs() < 1e-6);
    assert!((strip.corridor.out_bearing_deg - 42.0).abs() < 0.01);
    let half = f64::from(rwy.width_ft.unwrap_or(200)) * 0.3048 / 2.0 + 0.3 * 1852.0;
    assert!((strip.corridor.near_half_width_m - half).abs() < 1e-6);
    assert_eq!(
        strip.corridor.near_half_width_m,
        strip.corridor.far_half_width_m
    );
    assert_eq!(strip.corridor.thr_elev_ft, 661);
    assert_eq!(k.approaches, 1);
    assert!(!k.faf_from_cifp);
    assert_eq!(k.faf_nm, 5.0);
    assert!((k.corridor.length_m - 5.0 * 1852.0).abs() < 1e-6);
    // The capture box: 1.0 NM either side all along, the threshold elevation, and the fix
    // altitude the ILS publishes at REKKS (the glide slope intercept altitude, 2,200 ft).
    assert!((k.corridor.near_half_width_m - 1852.0).abs() < 1e-6);
    assert!((k.corridor.far_half_width_m - 1852.0).abs() < 1e-6);
    assert_eq!(k.corridor.thr_elev_ft, 661);
    assert_eq!(k.corridor.faf_alt_ft, 2200);
    // The procedure's own path: the angle of its runway-fix leg (-3.00) and the runway record's
    // crossing height, from the CIFP; the strip keeps the defaults.
    assert!(k.angle_from_cifp);
    assert!((k.corridor.path_angle_deg - 3.0).abs() < 1e-9);
    assert_eq!(k.tch_from_cifp, rwy.threshold_crossing_height_ft.is_some());
    assert_eq!(
        k.corridor.tch_ft,
        rwy.threshold_crossing_height_ft.map_or(50, |t| t as i32)
    );
    // The runway course is 045 magnetic with a 3 degree west variation: 042 true, so the
    // corridor runs out toward 222.
    assert!(
        (k.corridor.out_bearing_deg - 222.0).abs() < 0.01,
        "{}",
        k.corridor.out_bearing_deg
    );
    let rw = c.runways.iter().find(|r| r.ident == "RW04R").unwrap();
    assert_eq!(
        k.corridor.thr_lat_e7,
        (rw.threshold.lat * 1e7).round() as i32
    );
    // With the fix placed 4.2 NM out along the corridor, the CIFP gives the length; a fix farther
    // than 5 NM gives 5 NM (A2.08).
    let mut w = c.terminal_waypoints[0].clone();
    w.area = "KORD".to_string();
    w.ident = "REKKS".to_string();
    let out = 222.0f64.to_radians();
    w.position = oa_arinc424::Position {
        lat: rw.threshold.lat + 4.2 / 60.0 * out.cos(),
        lon: rw.threshold.lon + 4.2 / 60.0 * out.sin() / rw.threshold.lat.to_radians().cos(),
    };
    c.terminal_waypoints.push(w);
    let a = corridors_from_cifp(&c, "KORD").unwrap();
    let k = &a.corridors[0];
    assert!(k.faf_from_cifp);
    assert!((k.faf_nm - 4.2).abs() < 0.05, "{}", k.faf_nm);
    // A runway end whose approach publishes no altitude at a fix gets no corridor, and is said.
    let b = corridors_from_cifp(&c, "KMDW").unwrap();
    assert!(b.corridors.is_empty());
    // An airport the file does not hold is an error, never an empty list.
    let e = corridors_from_cifp(&c, "KXYZ").unwrap_err();
    assert!(e.contains("KXYZ"), "{e}");
}

// REQ: OA-MSAW-075
#[test]
fn the_path_angle_is_the_final_approachs_only_and_the_strip_holds_its_pavement_and_far_elevation() {
    let mut c = fixture_cifp();
    let base = corridors_from_cifp(&c, "KORD").unwrap();
    let strip = &base.corridors[1].corridor;
    let rwy = c.runways.iter().find(|r| r.ident == "RW04R").unwrap();
    assert!(
        (strip.pave_half_m - f64::from(rwy.width_ft.unwrap_or(200)) * 0.3048 / 2.0).abs() < 1e-6
    );
    // No reciprocal runway in the fixture: the far end is the higher of the threshold and the
    // airport elevation.
    let apt = c.airports.iter().find(|a| a.ident == "KORD").unwrap();
    assert_eq!(
        strip.far_elev_ft,
        strip
            .thr_elev_ft
            .max(apt.elevation_ft.unwrap_or(strip.thr_elev_ft))
    );
    // A step-down angle before the fix (a steep one) and a climb-sign angle after it are not the
    // final approach's glidepath.
    let p = c
        .procedures
        .iter_mut()
        .find(|p| p.kind == oa_arinc424::ProcedureKind::Approach)
        .unwrap();
    let route = &mut p.routes[0];
    let mut early = route.legs[0].clone();
    early.vertical_angle_deg = Some(-5.0);
    early.description[3] = ' ';
    route.legs.insert(0, early);
    let mut late = route.legs.last().unwrap().clone();
    late.vertical_angle_deg = Some(1.0);
    late.description[3] = ' ';
    route.legs.push(late);
    let a = corridors_from_cifp(&c, "KORD").unwrap();
    assert!((a.corridors[0].corridor.path_angle_deg - 3.0).abs() < 1e-9);
}

// REQ: OA-MSAW-077
#[test]
fn the_surface_stands_for_an_obstacle_only_where_the_allowance_covers_the_rise_over_that_lookups_radius(
) {
    // A 150 ft tower (top 590) beside the final mile, below the surface at its own position: left to
    // the surface with the lookup circle of 400 m (the rise over it is 70 ft against the
    // allowance of 100 ft), and counted whole with a circle of 1.2 km (the rise is about 240 ft).
    let (cfg, mut f, mut p) = tower_rig(590, 300.0, false);
    assert_eq!(f.floor_at(&p).unwrap().obstacle_ft, FLOOR_NONE);
    p.radius_m = 1200.0;
    assert_eq!(f.floor_at(&p).unwrap().obstacle_ft, 590);
    // The same wide circle with an allowance that covers the rise is left to the surface again.
    let mut big = cfg;
    big.approach_allowance_ft = 300;
    let (_, mut g, mut q) = tower_rig(590, 300.0, false);
    g.set_corridors(&big);
    q.radius_m = 1200.0;
    assert_eq!(g.floor_at(&q).unwrap().obstacle_ft, FLOOR_NONE);
}

// REQ: OA-MSAW-055
#[test]
fn a_runway_end_with_a_sid_gets_a_departure_corridor_from_its_far_end() {
    let mut c = fixture_cifp();
    let before = corridors_from_cifp(&c, "KORD").unwrap();
    assert!(before.corridors.iter().all(|k| k.kind != CK_DEPARTURE));
    // A SID with a runway transition for 04R.
    let mut sid = c
        .procedures
        .iter()
        .find(|p| p.kind == oa_arinc424::ProcedureKind::Approach)
        .unwrap()
        .clone();
    sid.kind = oa_arinc424::ProcedureKind::Sid;
    sid.ident = "TEST1".to_string();
    sid.routes.truncate(1);
    sid.routes[0].transition = "RW04R".to_string();
    c.procedures.push(sid);
    let a = corridors_from_cifp(&c, "KORD").unwrap();
    let d = a
        .corridors
        .iter()
        .find(|k| k.kind == CK_DEPARTURE)
        .expect("a departure corridor");
    assert_eq!(d.label, "KORD RW04R departure");
    let rwy = c.runways.iter().find(|r| r.ident == "RW04R").unwrap();
    assert!((d.corridor.length_m - 5.0 * 1852.0).abs() < 1e-6);
    assert!((d.corridor.near_half_width_m - 0.3 * 1852.0).abs() < 1e-6);
    assert!((d.corridor.far_half_width_m - 2.0 * 1852.0).abs() < 1e-6);
    assert!((d.corridor.out_bearing_deg - 42.0).abs() < 0.01);
    // It starts at the far end: the threshold moved along the heading by the runway less the
    // displacement.
    let run_m = (f64::from(rwy.length_ft) - f64::from(rwy.displaced_threshold_ft)) * 0.3048;
    let dy = f64::from(d.corridor.thr_lat_e7 - (rwy.threshold.lat * 1e7).round() as i32)
        * 1e-7
        * 111_194.926_644_558_74;
    assert!(
        (dy - run_m * 42.0f64.to_radians().cos()).abs() < 1.0,
        "{dy} {run_m}"
    );
}

fn meas(t_s: i64, addr: u32, lat: f64, lon: f64, vn: f32, alt: i32, vr: i32) -> Measurement {
    Measurement {
        toa_ns: t_s * S,
        pos_cov_m2: [25.0, 0.0, 25.0],
        vel_cov_m2s2: [1.0, 0.0, 1.0],
        vel_east_mps: 0.0,
        vel_north_mps: vn,
        lat_e7: (lat * 1e7).round() as i32,
        lon_e7: (lon * 1e7).round() as i32,
        alt_ft: alt,
        vrate_fpm: vr,
        addr,
        alt_quant_ft: 25,
        sensor: 0,
        ident: NO_IDENT,
        flags: M_HAS_VEL,
        category: 0,
    }
}

/// The synthetic approach: north at 100 m/s from 13.40 N, descending early at 700 ft/min from 2,000 ft.
fn approach(addr: u32, with_alt: bool) -> Vec<Measurement> {
    (0..90)
        .map(|t| {
            let alt = if with_alt {
                (2000.0 - 700.0 / 60.0 * t as f64).round() as i32
            } else {
                NONE_I32
            };
            meas(
                t,
                addr,
                13.40 + 0.0009 * t as f64,
                144.80,
                100.0,
                alt,
                if with_alt { -700 } else { NONE_I32 },
            )
        })
        .collect()
}

fn settings() -> Settings {
    parse_config(DEV_CFG).unwrap()
}

fn go(m: &[Measurement], set: &Settings, floors: Floors) -> Record {
    let none = GroundTable::new();
    track_and_alert(
        m,
        &DEFAULT_CONFIG,
        &none,
        set,
        floors,
        None,
        m.first().map_or(0, |x| x.toa_ns),
    )
    .unwrap()
}

// REQ: OA-MSAW-037
#[test]
fn guam_synthetic_through_the_tracker_alerts_caution_then_warning() {
    let r = go(&approach(0x801, true), &settings(), ridge_floors());
    assert_eq!(r.alerts.len(), 1, "{:?}", r.totals);
    let a = r.alerts[0];
    assert_eq!(a.addr, 0x801);
    let (c, w) = (a.caution_ns.unwrap(), a.warning_ns.unwrap());
    assert!(c < w, "caution {c} then warning {w}");
    assert_eq!(a.peak, oa_msaw::Grade::Warning);
    assert_eq!(a.worst_kind, oa_msaw::FloorKind::Terrain);
    assert!(a.worst_margin_ft < 0.0);
    // The net was running while it alerted (full coverage, nothing missing).
    assert!(
        r.net_cycles.keys().any(|k| k == "RUNNING"),
        "{:?}",
        r.net_cycles
    );
    let csv = alerts_csv(&r.alerts, &[]);
    assert!(csv
        .lines()
        .next()
        .unwrap()
        .starts_with("address_hex,start_s"));
    assert!(csv.contains("WARNING") && csv.contains("terrain"));
}

// REQ: OA-MSAW-038
#[test]
fn guam_synthetic_with_an_inhibition_has_no_alert_and_the_inhibition_is_in_the_output() {
    let mut set = settings();
    let text = format!(
        "{DEV_CFG}\ninhibit = 0 200 -1000 20000 SUPERVISOR_1 NUISANCE_ALERTS 13.2,144.6 13.2,145 13.6,145 13.6,144.6\n"
    );
    set.inhibits = parse_config(&text).unwrap().inhibits;
    let m = approach(0x801, true);
    let t0 = m[0].toa_ns;
    let r = go(&m, &set, ridge_floors());
    assert!(r.alerts.is_empty(), "hidden: {:?}", r.alerts);
    assert!(r.totals.suppressed > 20 && r.totals.opened == 0);
    assert_eq!(r.made.len(), 1);
    assert_eq!(r.made[0].end_ns, t0 + 200 * S);
    let h = r.hidden[&r.made[0].id];
    assert_eq!(h.count, r.totals.suppressed);
    // The net says it is inhibited for the whole run, and the files show the inhibition with its
    // end time, its reason and what it hid.
    assert_eq!(r.net_cycles.keys().collect::<Vec<_>>(), vec!["INHIBITED"]);
    let csv = inhibitions_csv(&r.made, &r.hidden, &r.expired_ns);
    let row = csv.lines().nth(1).unwrap();
    assert!(row.starts_with("1,MSAW,"), "{row}");
    assert!(
        row.contains(&format!("{:.3}", (t0 + 200 * S) as f64 / 1e9)),
        "{row}"
    );
    assert!(
        row.contains("NUISANCE ALERTS") && row.contains("SUPERVISOR 1"),
        "{row}"
    );
    assert!(row.contains(&format!(",{},", h.count)), "{row}");
    let text = summary_of(&r, &set);
    assert!(
        text.contains("inhibitions 1") && text.contains("INHIBITED"),
        "{text}"
    );
    // The same inhibition cut short (ends at 10 s) stops hiding: the alert comes after it.
    let short = format!(
        "{DEV_CFG}\ninhibit = 0 10 -1000 20000 SUPERVISOR_1 NUISANCE_ALERTS 13.2,144.6 13.2,145 13.6,145 13.6,144.6\n"
    );
    set.inhibits = parse_config(&short).unwrap().inhibits;
    let r = go(&m, &set, ridge_floors());
    assert_eq!(r.alerts.len(), 1);
    assert!(
        r.alerts[0].start_ns >= t0 + 10 * S,
        "{}",
        r.alerts[0].start_ns
    );
    assert!(r.expired_ns.contains_key(&r.made[0].id), "expiry reviewed");
}

// REQ: OA-MSAW-039
#[test]
fn missing_terrain_and_missing_altitude_degrade_the_net_end_to_end_and_are_listed() {
    // No terrain, obstacles or charts: no alert, because nothing is known, and the net says so.
    let r = go(
        &approach(0x801, true),
        &settings(),
        Floors::new(None, None, None),
    );
    assert!(r.alerts.is_empty());
    assert!(r.totals.no_coverage > 50, "{:?}", r.totals);
    assert!(
        r.net_cycles.keys().any(|k| k == "DEGRADED: INPUT MISSING"),
        "{:?}",
        r.net_cycles
    );
    assert!(!r.net_cycles.contains_key("RUNNING"));
    assert!(r.skips.keys().any(|k| k.0 == 0x801));
    let csv = skipped_csv(&r.skips);
    assert!(
        csv.contains("NO_COVERAGE") && csv.contains("00000801"),
        "{csv}"
    );
    // A track with no altitude: counted, listed, degraded; the other track is still judged.
    let mut m = approach(0x801, false);
    m.extend(approach(0x802, true).into_iter().map(|mut x| {
        x.lon_e7 += 100_000;
        x
    }));
    m.sort_by_key(|x| x.toa_ns);
    let r = go(&m, &settings(), ridge_floors());
    assert!(r.totals.alt_missing > 50, "{:?}", r.totals);
    assert!(r
        .skips
        .keys()
        .any(|k| k.0 == 0x801 && k.1 == oa_msaw::SkipWhy::AltitudeMissing as u8));
    assert!(
        r.net_cycles.keys().all(|k| !k.contains("ALTITUDE")),
        "{:?}",
        r.net_cycles
    );
    assert!(r.alerts.iter().any(|a| a.addr == 0x802));
    assert!(r.alerts.iter().all(|a| a.addr != 0x801));
    // A budget of one track, with two in the air: the other is counted and the net degraded.
    let mut set = settings();
    set.msaw.track_budget = 1;
    let mut m = approach(0x801, true);
    m.extend(approach(0x802, true).into_iter().map(|mut x| {
        x.lon_e7 += 100_000;
        x
    }));
    m.sort_by_key(|x| x.toa_ns);
    let r = go(&m, &set, ridge_floors());
    assert!(r.totals.over_budget > 50, "{:?}", r.totals);
    assert!(
        r.net_cycles
            .keys()
            .any(|k| k == "DEGRADED: BUDGET EXCEEDED"),
        "{:?}",
        r.net_cycles
    );
}

fn summary_of(r: &Record, set: &Settings) -> String {
    let used = DataUsed::default();
    summary_text(&SummaryInput {
        cfg: &set.msaw,
        alerts: &r.alerts,
        totals: r.totals,
        net_cycles: &r.net_cycles,
        alert_overflow: r.alert_overflow,
        skip_overflow: r.skip_overflow,
        tracker_overflow: r.tracker_overflow,
        measurements: 0,
        tick_rows: r.tick_rows,
        rows_skipped: r.rows_skipped,
        final_net: &r.final_net,
        made: &r.made,
        hidden: &r.hidden,
        expired_ns: &r.expired_ns,
        suppressed_overflow: r.suppressed_overflow,
        floor_stats: r.floor_stats,
        qnh_stats: r.qnh_stats,
        data: &used,
        skipped_listed: r.skips.len(),
        below_mva_tracks: 0,
        clip_name: "clip.oaf",
        clip_sha256: "00",
        sealed_window_dropped: 0,
        config_name: "dev.cfg",
        config_sha256: "11",
        scope: "test",
        alerts_sha256: "22",
        t0_ns: 0,
    })
}

// REQ: OA-MSAW-040
#[test]
fn the_files_are_deterministic_and_the_summary_names_no_aircraft() {
    let set = settings();
    let a = go(&approach(0xA1B2C3, true), &set, ridge_floors());
    let b = go(&approach(0xA1B2C3, true), &set, ridge_floors());
    assert_eq!(alerts_csv(&a.alerts, &[]), alerts_csv(&b.alerts, &[]));
    assert_eq!(skipped_csv(&a.skips), skipped_csv(&b.skips));
    // The wall-clock lines differ between runs; everything else is the same.
    let steady = |t: String| {
        t.lines()
            .filter(|l| !l.contains("wall seconds in"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (s1, s2) = (summary_of(&a, &set), summary_of(&b, &set));
    assert_eq!(steady(s1.clone()), steady(s2));
    assert!(alerts_csv(&a.alerts, &[]).contains("00A1B2C3"));
    assert!(
        !s1.to_uppercase().contains("A1B2C3"),
        "no address in the summary"
    );
    assert!(s1.contains("DEVELOPMENT VALUES, NOT FAA VALUES"));
    assert!(s1.contains("sealed-window rows dropped: 0"));
    assert!(s1.contains("terrain tiles given: NO"), "{s1}");
    assert!(s1.contains("net state at the end"));
    // Small helpers of the files.
    assert_eq!(flags_text(0), "");
    assert_eq!(
        flags_text(oa_msaw::AF_RATE_UNKNOWN | oa_msaw::AF_COAST),
        "rate_unknown|coast"
    );
    assert_eq!(state_text(&NetState::Inhibited), "INHIBITED");
    assert_eq!(
        state_text(&NetState::Unavailable {
            reason: Reason::InputMissing
        }),
        "UNAVAILABLE: INPUT MISSING"
    );
    let _ = BTreeMap::<u8, u8>::new();
}

// REQ: OA-MSAW-041
#[test]
fn the_radius_of_a_covariance_and_the_ground_codes_are_the_cores() {
    assert!((r95_of([25.0, 0.0, 25.0]) - 12.239).abs() < 0.01);
    assert!(r95_of([f32::NAN, 0.0, 1.0]).is_nan());
    assert!(r95_of([-1.0, 0.0, 1.0]).is_nan());
    // The ground table of the encounter tool uses the codes of the core.
    assert_eq!(
        oa_encounter_run::run::ground_at(&GroundTable::new(), 1, 0),
        GROUND_UNKNOWN
    );
    assert_eq!(oa_encounter::AIRBORNE, AIRBORNE);
    assert_eq!(oa_encounter::ON_GROUND, ON_GROUND);
    assert_eq!(oa_encounter::GROUND_UNKNOWN, GROUND_UNKNOWN);
}

fn go_qnh(m: &[Measurement], set: &Settings, floors: Floors, q: Qnh) -> Record {
    let none = GroundTable::new();
    track_and_alert(
        m,
        &DEFAULT_CONFIG,
        &none,
        set,
        floors,
        Some(q),
        m.first().map_or(0, |x| x.toa_ns),
    )
    .unwrap()
}

// REQ: OA-MSAW-072
#[test]
fn with_no_setting_in_reach_the_lowest_setting_of_the_last_12_h_within_200_nm_is_used() {
    const T0: i64 = 1_800_000_000;
    let stale = vec![(T0 - 3 * 3600, 1003.25)];
    // Only a stale observation (3 h old, 10 hPa below standard): the lowest setting of the last
    // 12 h within 200 NM is used, still flagged as having no setting of its own (OA-MSAW-072).
    let mut old = Qnh::from_stations(vec![(40.0, -88.0, stale)], 100.0, 7200);
    let (a, missing) = old.datum_ft(40.0, -88.0, T0, 2000.0, 300);
    assert!(missing && (a - 2000.0 + 270.0).abs() < 25.0, "{a}");
    assert_eq!(old.stats.fallback, 1);
    // Older than 12 h: the allowance.
    let mut older = Qnh::from_stations(
        vec![(40.0, -88.0, vec![(T0 - 13 * 3600, 1003.25)])],
        100.0,
        7200,
    );
    assert_eq!(older.datum_ft(40.0, -88.0, T0, 2000.0, 300), (1700.0, true));
}

// REQ: OA-MSAW-048
#[test]
fn below_fl180_the_altitude_takes_the_nearest_fresh_station_and_the_rest_is_lowered() {
    // Stations are (lat, lon, observations); times in Unix seconds, one hour before the query
    // time T0 for the fresh ones.
    const T0: i64 = 1_800_000_000;
    let fresh = vec![(T0 - 3600, 1023.25)];
    let stale = vec![(T0 - 3 * 3600, 1003.25)];
    let mut q = Qnh::from_stations(
        vec![
            (40.0, -88.0, fresh.clone()),
            (40.2, -88.0, stale.clone()),
            (45.0, -88.0, fresh.clone()),
        ],
        100.0,
        7200,
    );
    // 10 hPa above standard: about 270 ft more than the pressure altitude.
    let (a, missing) = q.datum_ft(40.01, -88.0, T0, 2000.0, 300);
    assert!(!missing);
    assert!((a - 2000.0 - 270.0).abs() < 25.0, "{a}");
    // The nearest station has only a stale observation: the next nearest within the radius is used.
    let mut q2 = Qnh::from_stations(
        vec![(40.0, -88.0, stale.clone()), (40.5, -88.0, fresh.clone())],
        100.0,
        7200,
    );
    let (a, missing) = q2.datum_ft(40.0, -88.0, T0, 2000.0, 300);
    assert!(!missing && a > 2200.0, "{a}");
    // No station within 200 NM (the one at 45 N is 300 NM away): lowered by the allowance and
    // counted as missing.
    // The age of the observation behind a setting is the time from it to the end of the 300 s
    // bucket, and none where there is no setting.
    let age = q.setting_age_s(40.01, -88.0, T0).unwrap();
    assert!((3600..=3900).contains(&age), "{age}");
    assert_eq!(
        Qnh::from_stations(vec![(45.0, -88.0, fresh.clone())], 100.0, 7200)
            .setting_age_s(40.0, -88.0, T0),
        None
    );
    let mut far = Qnh::from_stations(vec![(45.0, -88.0, fresh)], 100.0, 7200);
    assert_eq!(far.datum_ft(40.0, -88.0, T0, 2000.0, 300), (1700.0, true));
    // At and above FL180 the standard setting is flown: no correction, no flag, whatever the data.
    assert_eq!(
        far.datum_ft(40.0, -88.0, T0, 18_000.0, 300),
        (18_000.0, false)
    );
    assert_eq!(
        far.datum_ft(40.0, -88.0, T0, 35_000.0, 300),
        (35_000.0, false)
    );
    assert_eq!(far.stats.missing, 1);
    assert_eq!(far.stats.standard, 2);
    // An observation after the query time is not used.
    let mut future = Qnh::from_stations(vec![(40.0, -88.0, vec![(T0 + 60, 1023.25)])], 100.0, 7200);
    assert_eq!(
        future.datum_ft(40.0, -88.0, T0, 2000.0, 300),
        (1700.0, true)
    );
}

// REQ: OA-MSAW-049
#[test]
fn a_run_with_no_altimeter_setting_lowers_the_altitudes_counts_them_and_degrades_the_net() {
    let set = settings();
    let m = approach(0x801, true);
    let t0 = m[0].toa_ns / S;
    // With a station in range the altitude is corrected and nothing is missing.
    let q = Qnh::from_stations(vec![(13.4, 144.8, vec![(t0 - 600, 1013.25)])], 100.0, 7200);
    let with = go_qnh(&m, &set, ridge_floors(), q);
    assert_eq!(with.totals.qnh_missing, 0);
    assert!(
        with.net_cycles.keys().any(|k| k == "RUNNING"),
        "{:?}",
        with.net_cycles
    );
    let first_with = with.alerts[0].start_ns;
    // With none, every row below FL180 is counted and listed, the net reads degraded, and the
    // altitude is 300 ft lower, so the alert comes at least as early.
    let none = go_qnh(&m, &set, ridge_floors(), Qnh::empty(100.0, 7200));
    assert!(none.totals.qnh_missing > 50, "{:?}", none.totals);
    assert!(none
        .net_cycles
        .keys()
        .any(|k| k == "DEGRADED: INPUT MISSING"));
    assert!(!none.net_cycles.contains_key("RUNNING"));
    assert!(none
        .skips
        .keys()
        .any(|k| k.1 == oa_msaw::SkipWhy::QnhMissing as u8));
    assert!(skipped_csv(&none.skips).contains("QNH_MISSING"));
    assert!(none.alerts[0].start_ns <= first_with);
    assert_ne!(none.alerts[0].flags & oa_msaw::AF_QNH_MISSING, 0);
}

// REQ: OA-MSAW-050
#[test]
fn the_summary_gives_compute_rates_and_a_first_classification_counts_only() {
    let set = settings();
    let r = go(&approach(0xA1B2C3, true), &set, ridge_floors());
    let text = summary_of(&r, &set);
    for want in [
        "compute:",
        "CPU per simulated second",
        "alerts per hour",
        "worst floor set by terrain",
        "alerts within 10 NM of an airport with an approach: 0 of 1",
        "what the track was doing, all (1 alerts)",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    let a = r.alerts[0];
    let base = |vr: f32, alt: f32, speed: f32| oa_msaw::Alert {
        vrate_fpm: vr,
        alt_ft: alt,
        speed_mps: speed,
        ..a
    };
    use oa_msaw_run::report::{classify, Doing};
    assert_eq!(
        classify(&base(-700.0, 2000.0, 70.0), Some(4.0)),
        Doing::OnApproach
    );
    assert_eq!(
        classify(&base(900.0, 2000.0, 70.0), Some(4.0)),
        Doing::Departing
    );
    assert_eq!(
        classify(&base(0.0, 1500.0, 50.0), Some(30.0)),
        Doing::VfrLow
    );
    assert_eq!(
        classify(&base(-700.0, 9000.0, 200.0), Some(30.0)),
        Doing::Other
    );
    assert_eq!(classify(&base(-700.0, 2000.0, 70.0), None), Doing::VfrLow);
}

// REQ: OA-MSAW-058
#[test]
fn a_gtm_clearance_area_gives_its_clearance_to_the_cells_it_holds_and_a_bad_line_is_refused() {
    use oa_msaw_run::floors::Area;
    let b = Area::Box(40.0, 41.0, -89.0, -88.0);
    assert!(b.holds(40.5, -88.5) && !b.holds(41.5, -88.5));
    let tri = Area::Polygon(vec![(40.0, -89.0), (41.0, -89.0), (40.0, -88.0)]);
    assert!(tri.holds(40.2, -88.8) && !tri.holds(40.9, -88.1));
    let base = DEV_CFG;
    let ok = format!("{base}\ngtm_clearance = box 40 41 -89 -88 200\n");
    let s = parse_config(&ok).unwrap();
    assert_eq!(s.gtm_areas.len(), 1);
    assert_eq!(s.gtm_areas[0].ft, 200);
    for bad in [
        "gtm_clearance = box 41 40 -89 -88 200",
        "gtm_clearance = box 40 41 -89 -88 -5",
        "gtm_clearance = hex 1",
    ] {
        assert!(parse_config(&format!("{base}\n{bad}\n")).is_err(), "{bad}");
    }
}

// REQ: OA-MSAW-071
#[test]
fn an_airport_area_needs_the_whole_query_circle_inside_and_an_explicit_area_any_overlap() {
    use oa_msaw_run::floors::{AirportArea, Area};
    let mut f = Floors::new(None, None, None);
    f.set_airports(vec![AirportArea {
        lat: 13.50,
        lon: 144.80,
        radius_m: 9260.0,
        ceiling_ft: 3000,
        clearance_ft: 200,
    }]);
    let m = 111_194.926_644_558_74;
    let at = |north_m: f64| point(13.50 + north_m / m, 144.80, 100.0);
    // The lookup circle is the point's own, 100 m: inside at 9,100 m from the centre, partly outside
    // at 9,200 m, though the point itself is inside the area.
    assert_eq!(f.floor_at(&at(9100.0)).unwrap().apt_clearance_ft, 200);
    assert_eq!(
        f.floor_at(&at(9200.0)).unwrap().apt_clearance_ft,
        FLOOR_NONE
    );
    // An explicit area raises the floor: any overlap of the circle takes it, though the point is
    // outside the box.
    f.set_areas(vec![(Area::Box(14.0, 14.5, 144.0, 145.0), 700)]);
    let to_box = |south_m: f64| point(14.0 - south_m / m, 144.5, 100.0);
    assert_eq!(f.floor_at(&to_box(50.0)).unwrap().gtm_clearance_ft, 700);
    assert_eq!(
        f.floor_at(&to_box(1000.0)).unwrap().gtm_clearance_ft,
        FLOOR_NONE
    );
    // A polygon is overlapped by its edge too.
    let tri = Area::Polygon(vec![(14.0, 144.0), (14.5, 144.0), (14.0, 145.0)]);
    assert!(tri.overlaps(14.0 - 300.0 / m, 144.5, 408.0));
    assert!(!tri.overlaps(14.0 - 1000.0 / m, 144.5, 408.0));
}

// REQ: OA-MSAW-081
#[test]
fn a_lookup_circle_is_the_points_own_radius_with_no_cell_and_no_bucket() {
    use oa_msaw_run::floors::ObstacleIndex;
    let m = 111_194.926_644_558_74;
    // A 900 ft tower 300 m north of the point.
    let tower = ((40.0_f64 + 300.0 / m) * 1e7).round() as i32;
    let mut f = Floors::new(
        None,
        Some(ObstacleIndex::from_points(std::iter::once((
            tower,
            -880_000_000,
            900,
        )))),
        None,
    );
    let at = |r: f32| point(40.0, -88.0, r);
    assert_eq!(f.floor_at(&at(250.0)).unwrap().obstacle_ft, FLOOR_NONE);
    assert_eq!(f.floor_at(&at(280.0)).unwrap().obstacle_ft, FLOOR_NONE);
    assert_eq!(f.floor_at(&at(320.0)).unwrap().obstacle_ft, 900);
    // The same circle from a neighbouring point of the old 0.002 degree cell: the same answer
    // wherever the point is in it, since the circle is the point's.
    let off = point(40.0 - 0.0019, -88.0, 100.0);
    assert_eq!(f.floor_at(&off).unwrap().obstacle_ft, FLOOR_NONE);
    assert_eq!(Floors::effective_radius_m(250.0), 250.0);
}

// REQ: OA-MSAW-073
#[test]
fn a_point_outside_the_obstacle_file_extent_has_no_obstacle_data_and_inside_it_has() {
    use oa_msaw_run::floors::ObstacleIndex;
    let mut ix = ObstacleIndex::from_points(std::iter::once((400_000_000, -880_000_000, 900)));
    // No extent given: covered everywhere (as a test hands the data over).
    assert!(ix.covers(10.0, 10.0, 100.0));
    ix.set_extent_box(380_000_000, 460_000_000, -935_000_000, -835_000_000);
    assert!(ix.covers(40.0, -88.0, 5000.0));
    // Outside, or a circle that crosses the edge: not covered, never "no obstacle".
    assert!(!ix.covers(47.0, -88.0, 100.0));
    assert!(!ix.covers(46.0, -88.0, 100_000.0));
    let mut f = Floors::new(None, Some(ix), None);
    assert!(
        f.floor_at(&point(40.0, -88.0, 100.0))
            .unwrap()
            .obstacle_covered
    );
    assert!(
        !f.floor_at(&point(47.0, -88.0, 100.0))
            .unwrap()
            .obstacle_covered
    );
}

// REQ: OA-MSAW-059
#[test]
fn an_airport_area_gives_its_lower_clearance_and_ceiling_to_the_cells_it_holds_whole() {
    use oa_msaw_run::floors::AirportArea;
    let mut f = Floors::new(None, None, None);
    let area = |lat: f64, ceiling_ft: i32, clearance_ft: i32| AirportArea {
        lat,
        lon: 144.80,
        radius_m: 5.0 * 1852.0,
        ceiling_ft,
        clearance_ft,
    };
    // Two overlapping areas: the lower clearance and the lower ceiling apply.
    f.set_airports(vec![area(13.50, 3600, 200), area(13.52, 3000, 150)]);
    let s = f.floor_at(&point(13.51, 144.80, 100.0)).unwrap();
    assert_eq!((s.apt_clearance_ft, s.apt_ceiling_ft), (150, 3000));
    // Inside only the first one.
    let s = f.floor_at(&point(13.425, 144.80, 100.0)).unwrap();
    assert_eq!((s.apt_clearance_ft, s.apt_ceiling_ft), (200, 3600));
    // Outside every area, and an explicit gtm_clearance is a separate field.
    let s = f.floor_at(&point(13.8, 144.80, 100.0)).unwrap();
    assert_eq!(
        (s.apt_clearance_ft, s.apt_ceiling_ft),
        (FLOOR_NONE, FLOOR_NONE)
    );
    assert_eq!(s.gtm_clearance_ft, FLOOR_NONE);
    // The new keys are required and range-checked.
    for k in [
        "approach_half_width_nm",
        "airport_area_nm",
        "airport_area_ceiling_ft_agl",
        "airport_area_clearance_ft",
    ] {
        let cut: String = DEV_CFG
            .lines()
            .filter(|l| !l.starts_with(k))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_config(&cut).unwrap_err().contains(k), "{k}");
    }
    let bad = DEV_CFG.replace("approach_half_width_nm = 1.0", "approach_half_width_nm = 0");
    assert!(parse_config(&bad).is_err());
}

// REQ: OA-MSAW-062
#[test]
fn the_summary_gives_counts_per_bucket_for_the_diagnostics_of_the_alerts() {
    let set = settings();
    let a = go(&approach(0xA1B2C3, true), &set, ridge_floors());
    assert_eq!(a.alerts.len(), 1);
    // The violating point and its radius are kept.
    assert!(a.alerts[0].worst_radius_m > 0.0);
    let t = summary_of(&a, &set);
    assert!(t.contains("diagnostics of the 1 alerts:"), "{t}");
    for k in [
        "depth below the floor at the predicted violation:",
        "time to violation:",
        "query radius at the violating point:",
        "distance from the violating point to the cell that set the floor (0 alerts",
        "aligned within 30 degrees of a runway heading at the nearest airport: yes 0, no 0, no corridors 1",
    ] {
        assert!(t.contains(k), "{k}");
    }
}

// REQ: OA-MSAW-063
#[test]
fn the_diagnosis_gives_the_altitude_radius_obstacle_and_terrain_sections_and_names_no_aircraft() {
    use oa_msaw_run::diagnose::{diagnosis_text, DiagIn};
    let argv = |extra: &[&str]| -> Vec<String> {
        ["--adsb", "c.oaf", "--config", "d.cfg", "--out", "o"]
            .iter()
            .chain(extra.iter())
            .map(|s| (*s).to_string())
            .collect()
    };
    assert!(parse(&argv(&["--diagnose"])).unwrap().diagnose);
    assert!(!parse(&argv(&[])).unwrap().diagnose);
    assert_eq!(
        parse(&argv(&["--landing-of", "old/alerts.csv"]))
            .unwrap()
            .landing_of,
        Some("old/alerts.csv".to_string())
    );
    assert!(parse(&argv(&["--diagnose", "--diagnose"])).is_err());
    let set = settings();
    let a = go(&approach(0xA1B2C3, true), &set, ridge_floors());
    let mut f = Floors::new(None, None, None);
    let rows = BTreeMap::new();
    let t = diagnosis_text(&mut DiagIn {
        alerts: &a.alerts,
        rows: &rows,
        floors: &mut f,
        dof: None,
        cfg: &set.msaw,
        terrain_dir: None,
        airports: &[],
        all_airports: &[],
        all_thresholds: &[],
        n_configured: 0,
        sealed: oa_sealed::Sealed::production(),
        corridors: &[],
        qnh: None,
        basis: &BTreeMap::new(),
    });
    for k in [
        "1. the altitude judged",
        "2. the query radius",
        "3. the obstacles",
        "4. the terrain-floor alerts",
        "5. what the predicted depth is made of",
        "6. the landing zone",
        "7. the deep alerts",
        "8. the alignment of every alert",
        "dominant term over all 1 alerts",
    ] {
        assert!(t.contains(k), "{k}\n{t}");
    }
    assert!(!t.to_uppercase().contains("A1B2C3"));
}

// REQ: OA-MSAW-074
#[test]
fn an_altitude_with_only_geometric_measurements_is_no_altitude_and_is_listed_so() {
    let mut m = approach(0xA1B2C3, true);
    for x in &mut m {
        x.flags |= oa_track::M_ALT_GEOMETRIC;
    }
    let r = go(&m, &settings(), ridge_floors());
    // The tracker keeps no altitude from them, and a row that had one with no barometric time
    // behind it would be counted in `alt_age_unknown` and taken as none.
    assert_eq!(r.totals.alt_age_unknown, 0);
    assert!(r.totals.alt_missing > 20, "{:?}", r.totals);
    assert!(r.alerts.is_empty());
    let csv = skipped_csv(&r.skips);
    assert!(
        csv.contains("MSAW not monitoring: altitude missing"),
        "{csv}"
    );
}

// REQ: OA-MSAW-065
#[test]
fn a_track_whose_altitude_stops_arriving_is_not_monitored_once_it_is_stale_and_is_counted() {
    let set = settings();
    assert_eq!(
        (set.msaw.max_altitude_age_s, set.msaw.terminal_lookahead_s),
        (12.0, 15.0)
    );
    let mut m = approach(0xA1B2C3, true);
    for x in m.iter_mut().skip(20) {
        x.alt_ft = NONE_I32;
        x.vrate_fpm = NONE_I32;
    }
    let r = go(&m, &set, ridge_floors());
    // Rows from about 12 s after the last altitude are stale; before that they are judged.
    assert!(r.totals.alt_stale > 30, "{}", r.totals.alt_stale);
    assert!(
        r.totals.alt_extrapolated > 10,
        "{}",
        r.totals.alt_extrapolated
    );
    assert!(r.totals.evaluated >= 20, "{}", r.totals.evaluated);
    assert!(r
        .skips
        .keys()
        .any(|k| k.1 == oa_msaw::SkipWhy::AltitudeStale as u8));
    assert!(
        r.net_cycles.keys().all(|k| !k.contains("ALTITUDE")),
        "{:?}",
        r.net_cycles
    );
    // The track carries the status.
    let csv = skipped_csv(&r.skips);
    assert!(
        csv.contains("ALTITUDE_STALE") && csv.contains("MSAW not monitoring: altitude stale"),
        "{csv}"
    );
}

// REQ: OA-MSAW-066
#[test]
fn alerts_of_one_track_20_s_apart_or_less_are_one_episode() {
    use oa_msaw_run::report::{episodes, EPISODE_GAP_S};
    let r = go(&approach(0x801, true), &settings(), ridge_floors());
    let a = r.alerts[0];
    let at = |addr: u32, s: i64, e: i64| oa_msaw::Alert {
        addr,
        start_ns: s * 1_000_000_000,
        end_ns: e * 1_000_000_000,
        ..a
    };
    // Gaps of 20 s (merged) and 21 s (not), and a second track.
    let v = [at(1, 0, 10), at(1, 30, 40), at(1, 61, 70), at(2, 5, 6)];
    assert_eq!(episodes(&v, EPISODE_GAP_S), (3, 2));
    assert_eq!(episodes(&[], EPISODE_GAP_S), (0, 0));
    assert_eq!(episodes(&v, 0), (4, 2));
}

// REQ: OA-MSAW-076
#[test]
fn the_alerts_of_another_run_are_counted_in_the_landing_zone_by_this_runs_corridors() {
    let c = fixture_cifp();
    let a = corridors_from_cifp(&c, "KORD").unwrap();
    let cfg = settings().msaw;
    let strip = &a.corridors[1].corridor;
    // A point on the strip, 500 m past its start, and one 100 km north.
    let (lat, lon) = (
        f64::from(strip.thr_lat_e7) / 1e7 + 0.0001,
        f64::from(strip.thr_lon_e7) / 1e7 + 0.0001,
    );
    let dir = std::env::temp_dir().join(format!("oa-msaw-landing-of-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("alerts.csv");
    std::fs::write(
        &p,
        format!(
            "address_hex,worst_lat_deg,worst_lon_deg,worst_flags_raw\n0000A1,{lat:.5},{lon:.5},128\n0000A2,{:.5},{lon:.5},0\n",
            lat + 1.0
        ),
    )
    .unwrap();
    let t = oa_msaw_run::run::landing_of(
        oa_sealed::Sealed::production(),
        p.to_str().unwrap(),
        &a.corridors,
        &cfg,
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(t.contains("alerts in alerts.csv: 2;"), "{t}");
    assert!(
        t.contains(
            "last 1 NM of a capture box: 1; no altimeter setting in reach at the worst cycle: 1\n"
        ),
        "{t}"
    );
    assert!(
        t.contains("obstacle-floor alerts with the worst point in no corridor: 0\n"),
        "{t}"
    );
}

/// A 3 degree approach to a 600 ft threshold at 40 N 88 W, out toward the south, and the lookup
/// point 1,400 m out in the last mile, at the centre of its lookup cell.
fn tower_rig(
    tower_top_ft: i32,
    cross_m: f64,
    departure: bool,
) -> (oa_msaw::MsawConfig, Floors, oa_msaw::SamplePoint) {
    use oa_msaw::{Corridor, CK_APPROACH, CK_DEPARTURE};
    let m = 111_194.926_644_558_74;
    let mut cfg = settings().msaw;
    cfg.approach_allowance_ft = 100;
    let approach = Corridor {
        kind: CK_APPROACH,
        thr_lat_e7: 400_000_000,
        thr_lon_e7: -880_000_000,
        out_bearing_deg: 180.0,
        length_m: 9260.0,
        near_half_width_m: 1852.0,
        far_half_width_m: 1852.0,
        thr_elev_ft: 600,
        faf_alt_ft: 2250,
        land_from_m: 0.0,
        pave_half_m: 0.0,
        far_elev_ft: 0,
        path_angle_deg: 3.0,
        tch_ft: 50,
    };
    let mut corridors = vec![approach];
    if departure {
        corridors.push(Corridor {
            kind: CK_DEPARTURE,
            ..approach
        });
    }
    cfg.corridors = corridors.into_boxed_slice();
    let lat = ((40.0f64 - 1400.0 / m) / 0.002).floor() * 0.002 + 0.001;
    let lon = (-88.0f64 / 0.002).floor() * 0.002 + 0.001;
    let tower = (
        (lat * 1e7).round() as i32,
        ((lon + cross_m / (m * 40.0f64.to_radians().cos())) * 1e7).round() as i32,
        tower_top_ft,
    );
    let mut f = Floors::new(
        None,
        Some(ObstacleIndex::from_points(std::iter::once(tower))),
        None,
    );
    f.set_corridors(&cfg);
    let mut p = point(lat, lon, 400.0);
    // On final: heading north, the inbound course of the corridor.
    p.track_deg = 0;
    (cfg, f, p)
}

// REQ: OA-MSAW-077
#[test]
fn an_obstacle_below_the_approach_surface_at_its_own_position_is_left_to_the_surface() {
    use oa_msaw::{corridor_hit_ft, floor_of};
    let judge = |top: i32, cross: f64, departure: bool| {
        let (cfg, mut f, p) = tower_rig(top, cross, departure);
        let mut s = f.floor_at(&p).unwrap();
        s.terrain_ft = 600;
        let hit = corridor_hit_ft(&cfg.corridors[0], p.lat_e7, p.lon_e7, 100.0, &cfg);
        (s.obstacle_ft, floor_of(&s, hit, &cfg).unwrap().0)
    };
    // The surface about 1,450 m out is near 600 + 50 + 250 - 100 = 800 ft. A 150 ft tower on 600 ft ground
    // 300 m beside the final mile (top 750) is below it: it does not raise the floor, so an
    // aircraft at 820 ft is not below it.
    let (o, floor) = judge(750, 300.0, false);
    assert_eq!(o, FLOOR_NONE);
    assert!((780..=810).contains(&floor), "{floor}");
    // A tower that penetrates the surface (top 850) still raises the floor to its top, and the
    // aircraft at 820 ft is below it.
    let (o, floor) = judge(850, 300.0, false);
    assert_eq!(o, 850);
    assert_eq!(floor, 850);
    assert!(820 < floor);
    // The same low tower with a departure corridor over the circle counts in full.
    assert_eq!(judge(750, 300.0, true).0, 750);
    // Outside the corridor nothing changes: the same tower in a net with no corridor counts, and
    // so does one in the corridor's net whose circle is not inside it.
    let (_, mut f, p) = tower_rig(750, 300.0, false);
    f.set_corridors(&oa_msaw::MsawConfig {
        corridors: Box::default(),
        ..settings().msaw
    });
    assert_eq!(f.floor_at(&p).unwrap().obstacle_ft, 750);
    let (cfg, _, mut p) = tower_rig(750, 300.0, false);
    p.lon_e7 += 500_000;
    let mut g = Floors::new(
        None,
        Some(ObstacleIndex::from_points(std::iter::once((
            p.lat_e7, p.lon_e7, 750,
        )))),
        None,
    );
    g.set_corridors(&cfg);
    assert_eq!(g.floor_at(&p).unwrap().obstacle_ft, 750);
}

// REQ: OA-MSAW-073
#[test]
fn the_coverage_of_several_files_is_the_union_of_each_files_own_extent() {
    use oa_msaw_run::floors::ObstacleIndex;
    // California and Colorado loaded: a point in Nevada between them is obstacle data absent, and a
    // point inside either is covered.
    // One obstacle in each 0.5 degree cell of a block, but for an interior hole.
    let block = |lat0: i32, lat1: i32, lon0: i32, lon1: i32, hole: (i32, i32)| {
        let mut v = Vec::new();
        for a in lat0..lat1 {
            for b in lon0..lon1 {
                if (a, b) != hole {
                    v.push((a * 5_000_000 + 2_500_000, b * 5_000_000 + 2_500_000, 900));
                }
            }
        }
        v
    };
    // California about 33 to 40 N, 123 to 117 W; Colorado 37 to 41 N, 108 to 103 W.
    let ca = ObstacleIndex::from_file_points(block(66, 80, -246, -234, (72, -240)));
    let co = ObstacleIndex::from_file_points(block(74, 82, -216, -206, (78, -211)));
    let mut both = ca;
    both.merge(co);
    assert_eq!(both.files, 2);
    assert!(both.covers(36.0, -121.0, 1000.0), "California");
    assert!(both.covers(39.0, -105.5, 1000.0), "Colorado");
    // An interior gap of a file (a cell with no obstacle between obstacle cells) is covered;
    // its edge and the cells beyond are not.
    assert!(
        both.covers(36.25, -119.75, 100.0),
        "the hole of the California block"
    );
    assert!(!both.covers(40.25, -121.0, 100.0), "past the northern edge");
    assert!(
        !both.covers(36.0, -116.0, 100.0),
        "30 NM across the border into Nevada"
    );
    assert!(!both.covers(38.5, -116.5, 1000.0), "Nevada: no file of it");
    // The box around both would have covered it.
    let (s, n, w, e) = both.extent.unwrap();
    assert!(s < 385_000_000 && n > 385_000_000 && w < -1_165_000_000 && e > -1_165_000_000);
    let _ = (s, n, w, e);
    let mut f = Floors::new(None, Some(both), None);
    assert!(
        !f.floor_at(&point(38.5, -116.5, 100.0))
            .unwrap()
            .obstacle_covered
    );
    assert!(
        f.floor_at(&point(36.0, -121.0, 100.0))
            .unwrap()
            .obstacle_covered
    );
}

// REQ: OA-MSAW-073
#[test]
fn a_gap_is_filled_along_its_row_or_its_column_in_each_file_alone() {
    use oa_msaw_run::floors::ObstacleIndex;
    // One obstacle in the middle of each 0.5 degree cell of a block, but for some holes.
    let block = |a0: i32, a1: i32, b0: i32, b1: i32, holes: &dyn Fn(i32, i32) -> bool| {
        let mut v = Vec::new();
        for a in a0..a1 {
            for b in b0..b1 {
                if !holes(a, b) {
                    v.push((a * 5_000_000 + 2_500_000, b * 5_000_000 + 2_500_000, 900));
                }
            }
        }
        v
    };
    let at = |a: i32, b: i32| (f64::from(a) * 0.5 + 0.25, f64::from(b) * 0.5 + 0.25);
    // A lake of 10 cells north to south and 3 east to west in a national file: obstacles on both
    // sides along the rows, none within 4 cells along the columns. Covered by the row.
    let national = ObstacleIndex::from_file_points(block(0, 20, 0, 20, &|a, b| {
        (5..15).contains(&a) && (8..11).contains(&b)
    }));
    let (la, lo) = at(10, 9);
    assert!(national.covers(la, lo, 100.0), "the lake, along its rows");
    // The same lake turned: 3 north to south and 10 east to west, covered by the column.
    let turned = ObstacleIndex::from_file_points(block(0, 20, 0, 20, &|a, b| {
        (8..11).contains(&a) && (5..15).contains(&b)
    }));
    let (la, lo) = at(9, 10);
    assert!(turned.covers(la, lo, 100.0), "the lake, along its columns");
    // A hole 8 cells across either way is not within 4 cells of obstacles on both sides: not
    // covered, in its middle.
    let wide = ObstacleIndex::from_file_points(block(0, 20, 0, 20, &|a, b| {
        (6..14).contains(&a) && (6..14).contains(&b)
    }));
    let (la, lo) = at(10, 10);
    assert!(!wide.covers(la, lo, 100.0), "a hole wider than 4 cells");
    // A coast: the block ends; the cells past its edge and a notch open to the sea (obstacles on
    // one side only) are not covered.
    let coast = ObstacleIndex::from_file_points(block(0, 20, 0, 20, &|a, b| {
        (5..15).contains(&a) && b >= 14
    }));
    let (la, lo) = at(10, 15);
    assert!(!coast.covers(la, lo, 100.0), "a notch open to the sea");
    let (la, lo) = at(10, 21);
    assert!(!coast.covers(la, lo, 100.0), "open ocean past the edge");
    // This inlet has obstacles on both sides of each row. Its opening along the column
    // connects it to the file edge, so bracketing along the row cannot establish coverage.
    let inlet = ObstacleIndex::from_file_points(block(0, 20, 0, 20, &|a, b| {
        a >= 5 && (8..11).contains(&b)
    }));
    let (la, lo) = at(10, 9);
    assert!(
        !inlet.covers(la, lo, 100.0),
        "an inlet open on the other axis"
    );
    // Maryland and New Jersey loaded, Delaware (2 cells wide) between them, no file of it: each
    // file's own gaps are filled, the strip between the two files is not.
    let md = ObstacleIndex::from_file_points(block(0, 12, 0, 10, &|_, _| false));
    let nj = ObstacleIndex::from_file_points(block(4, 16, 12, 22, &|_, _| false));
    let mut both = md;
    both.merge(nj);
    let (la, lo) = at(8, 10);
    assert!(!both.covers(la, lo, 100.0), "Delaware, west cell");
    let (la, lo) = at(8, 11);
    assert!(!both.covers(la, lo, 100.0), "Delaware, east cell");
    let (la, lo) = at(8, 5);
    assert!(both.covers(la, lo, 100.0), "Maryland");
    let (la, lo) = at(8, 15);
    assert!(both.covers(la, lo, 100.0), "New Jersey");
}

// REQ: OA-MSAW-072
#[test]
fn the_metar_argument_names_a_directory_or_files_separated_by_commas() {
    let dir = scratch("metar-list");
    let rec = |obs: i64, report: &str, altim: f64, a: u32| {
        format!(
            "[{{\"icaoId\":\"KMNM\",\"receiptTime\":\"{report}\",\"obsTime\":{obs},\"reportTime\":\"{report}\",\"temp\":6.1,\"dewp\":-0.6,\"wdir\":330,\"wspd\":9,\"visib\":\"10+\",\"altim\":{altim},\"slp\":1024.3,\"qcField\":14,\"metarType\":\"METAR\",\"rawOb\":\"METAR KMNM 141757Z AUTO 33009KT 10SM CLR 06/M01 A{a} RMK AO2\",\"lat\":45.126,\"lon\":-87.639,\"elev\":188,\"name\":\"Menominee\",\"cover\":\"CLR\",\"clouds\":[],\"fltCat\":\"VFR\"}}]"
        )
    };
    let (a, b) = (dir.join("first.json"), dir.join("second-hour.json"));
    std::fs::write(
        &a,
        rec(1_794_679_020, "2026-11-14T18:00:00.000Z", 1023.8, 3023),
    )
    .unwrap();
    std::fs::write(
        &b,
        rec(1_794_682_620, "2026-11-14T19:00:00.000Z", 1020.4, 3013),
    )
    .unwrap();
    let list = PathBuf::from(format!("{},{}", a.display(), b.display()));
    let mut q = Qnh::from_dir(&list, 100.0, 7200).unwrap();
    assert_eq!(q.stats.files, 2);
    // Each file's observation is the current one at its own time.
    let at = |q: &mut Qnh, t: i64| q.setting_hpa(45.1, -87.6, t).unwrap();
    assert!((at(&mut q, 1_794_679_200) - 1023.8).abs() < 0.2);
    assert!((at(&mut q, 1_794_682_800) - 1020.4).abs() < 0.2);
    // A name that is neither a file nor a directory is an error, not an empty set.
    let bad = PathBuf::from(format!(
        "{},{}",
        a.display(),
        dir.join("none.json").display()
    ));
    assert!(Qnh::from_dir(&bad, 100.0, 7200).is_err());
}

// REQ: OA-MSAW-078
#[test]
fn the_cold_temperature_correction_only_lowers_an_altitude_and_says_when_it_could_not_be_made() {
    use oa_msaw_run::qnh::cold_correction_ft;
    // Doc 8168: 1,000 ft above a station at sea level in air at -20 C reads about 160 ft high
    // (ISA deviation 35 degrees: 1000 x 35 / (273 - 20 - 0.5 x 0.00198 x 1000) = 139 ft).
    let dh = cold_correction_ft(1000.0, 0.0, -20.0);
    assert!((dh - 139.0).abs() < 2.0, "{dh}");
    // Warm or standard air, or no height: nothing, never a raise.
    assert_eq!(cold_correction_ft(1000.0, 0.0, 15.0), 0.0);
    assert_eq!(cold_correction_ft(1000.0, 0.0, 30.0), 0.0);
    assert_eq!(cold_correction_ft(-50.0, 0.0, -20.0), 0.0);
    let t = 1_700_000_000i64;
    let obs = vec![(t - 600, 1013.25)];
    let mut q = Qnh::from_stations(vec![(40.0, -88.0, obs.clone())], 100.0, 7200);
    q.set_station_weather(0, 600.0, vec![-10.0]);
    let (a, code) = q.cold_ft(40.0, -88.0, t, 2600.0);
    assert_eq!(code, 1);
    assert!(a < 2600.0 && a > 2400.0, "{a}");
    // Above 5,000 ft over the station the correction is not made, and said so.
    assert_eq!(q.cold_ft(40.0, -88.0, t, 6000.0), (6000.0, 3));
    // At or below the station there is nothing to correct; a warm station gives no correction.
    assert_eq!(q.cold_ft(40.0, -88.0, t, 600.0), (600.0, 0));
    let mut warm = Qnh::from_stations(vec![(40.0, -88.0, obs.clone())], 100.0, 7200);
    warm.set_station_weather(0, 600.0, vec![20.0]);
    assert_eq!(warm.cold_ft(40.0, -88.0, t, 2600.0), (2600.0, 0));
    // The temperature missing: no correction, said so.
    let mut none = Qnh::from_stations(vec![(40.0, -88.0, obs)], 100.0, 7200);
    none.set_station_weather(0, 600.0, vec![f64::NAN]);
    assert_eq!(none.cold_ft(40.0, -88.0, t, 2600.0), (2600.0, 2));
}

// REQ: OA-MSAW-073
#[test]
fn an_empty_obstacle_file_is_an_error_not_full_coverage() {
    let dir = std::env::temp_dir().join(format!("oa-msaw-empty-dof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("DOF.DAT");
    std::fs::write(&p, b"").unwrap();
    let e = oa_msaw_run::run::read_dof(&p)
        .map(|_| ())
        .expect_err("an empty file is refused");
    assert!(!e.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}
