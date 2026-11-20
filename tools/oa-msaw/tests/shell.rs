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
use oa_msaw::{AIRBORNE, FLOOR_NONE, GROUND_UNKNOWN, ON_GROUND};
use oa_msaw_run::args::parse;
use oa_msaw_run::cifp::{approach_runway, areas_from_cifp};
use oa_msaw_run::config::{parse_config, Settings};
use oa_msaw_run::floors::{ceil_ft, tile_name, Floors, ObstacleIndex};
use oa_msaw_run::report::{
    alerts_csv, flags_text, inhibitions_csv, skipped_csv, state_text, summary_text, DataUsed,
    SummaryInput,
};
use oa_msaw_run::run::{r95_of, track_and_alert, Record};
use oa_mva::{new_model, Model, SetKind};
use oa_safety::geom::polygon_contains;
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
    assert_eq!(c.n_procs, 0);
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
    // A procedure area, a CIFP airport and an inhibition.
    let s = parse_config(&with(
        "procedure = 1200 40,-88 40,-87.9 40.1,-87.9 40.1,-88\ncifp_airport = KORD\n\
         inhibit = 5 65 -1000 20000 SUP_1 NUISANCE_ALERTS 13.2,144.6 13.2,145 13.6,145 13.6,144.6",
    ))
    .unwrap();
    assert_eq!(s.msaw.n_procs, 1);
    assert_eq!(s.msaw.procs[0].floor_ft, 1200);
    assert_eq!(s.cifp_airports, vec!["KORD".to_string()]);
    assert_eq!(s.inhibits[0].reason, "NUISANCE ALERTS");
    assert_eq!(s.inhibits[0].author, "SUP 1");
    assert_eq!(s.inhibits[0].vertices.len(), 4);
    for bad in [
        "procedure = 1200 40,-88 40,-87.9",
        "procedure = 1200 40,-88 40,-87.9 x",
        "inhibit = 5 4 0 1 A B 1,1 1,2 2,2",
        "inhibit = 5 6 0 1 A B 1,1 1,2",
        "inhibit = 5 6 0 1 A",
    ] {
        assert!(parse_config(&with(bad)).is_err(), "{bad}");
    }
    let e =
        parse_config(&DEV_CFG.replace("cifp_radius_nm = 5", "cifp_radius_nm = 80")).unwrap_err();
    assert!(e.contains("cifp_radius_nm"), "{e}");
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
        None,
        Some(chart(100, 13.2, 13.65, 144.65, 144.95)),
    )
}

fn point(lat: f64, lon: f64, radius_m: f32) -> oa_msaw::SamplePoint {
    oa_msaw::SamplePoint {
        lat_e7: (lat * 1e7).round() as i32,
        lon_e7: (lon * 1e7).round() as i32,
        radius_m,
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
    // 13.50 N: the grid's elevation there is 3,000 ft at 13.50; with a 250 m radius (about 0.0023
    // degrees) the highest is a little north of it.
    let s = f.floor_at(&point(13.50, 144.80, 100.0)).unwrap();
    assert!(
        s.terrain_ft >= 3000 && s.terrain_ft < 3200,
        "{}",
        s.terrain_ft
    );
    assert_eq!(s.mva_ft, 100);
    assert_eq!(s.obstacle_ft, FLOOR_NONE);
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
    // The same cell and bucket is answered from the cache.
    let before = f.stats.cache_hits;
    f.floor_at(&point(13.30, 144.80, 100.0)).unwrap();
    assert_eq!(f.stats.cache_hits, before + 1);
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

// REQ: OA-MSAW-034
#[test]
fn procedure_areas_come_from_the_lowest_published_floor_altitude_of_a_runways_approaches() {
    assert_eq!(approach_runway("I04R").as_deref(), Some("04R"));
    assert_eq!(approach_runway("R22L-Z").as_deref(), Some("22L"));
    assert_eq!(approach_runway("L09").as_deref(), Some("09"));
    assert_eq!(approach_runway("V-A"), None);
    assert_eq!(approach_runway("I"), None);
    let c = fixture_cifp();
    let a = areas_from_cifp(&c, "KORD", 5.0).unwrap();
    assert_eq!(a.len(), 1, "{a:?}");
    assert_eq!(a[0].label, "KORD RW04R");
    assert_eq!(a[0].approaches, 1);
    // The lowest floor altitude of the fixture's ILS 04R legs is the 713 ft at the threshold
    // (the at-or-below and the other-coded altitudes are not floors).
    assert_eq!(a[0].area.floor_ft, 713);
    // The circle holds the threshold and not a point 10 NM away.
    let rw = c.runways.iter().find(|r| r.ident == "RW04R").unwrap();
    let e7 = |x: f64| (x * 1e7).round() as i32;
    assert!(polygon_contains(
        &a[0].area.area,
        e7(rw.threshold.lat),
        e7(rw.threshold.lon)
    ));
    assert!(!polygon_contains(
        &a[0].area.area,
        e7(rw.threshold.lat + 10.0 / 60.0),
        e7(rw.threshold.lon)
    ));
    // An airport the file does not hold is an error, never an empty list.
    let e = areas_from_cifp(&c, "KXYZ", 5.0).unwrap_err();
    assert!(e.contains("KXYZ"), "{e}");
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
    let csv = alerts_csv(&r.alerts);
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
        r.net_cycles
            .keys()
            .any(|k| k == "DEGRADED: ALTITUDE MISSING"),
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
        data: &used,
        skipped_listed: r.skips.len(),
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
    assert_eq!(alerts_csv(&a.alerts), alerts_csv(&b.alerts));
    assert_eq!(skipped_csv(&a.skips), skipped_csv(&b.skips));
    let (s1, s2) = (summary_of(&a, &set), summary_of(&b, &set));
    assert_eq!(s1, s2);
    assert!(alerts_csv(&a.alerts).contains("00A1B2C3"));
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
