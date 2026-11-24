#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use oa_msaw::SamplePoint;
use oa_msaw_run::regional::{
    load_regional_data, MetarSource, RegionalDataLimits, RegionalDataPaths, WeatherAdmission,
    DEFAULT_MAX_CONFIG_BYTES,
};

const DEV_CFG: &str = include_str!("../testdata/dev.cfg");

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("oa-msaw-regional-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn base_paths(root: &Path) -> RegionalDataPaths {
    let config = root.join("dev.cfg");
    std::fs::write(&config, DEV_CFG).unwrap();
    RegionalDataPaths {
        config,
        terrain_dir: None,
        dof_files: Vec::new(),
        metar_files: Vec::new(),
    }
}

#[test]
fn config_and_dof_bytes_are_refused_before_parsing_when_over_limit() {
    let root = scratch("oversize");
    let mut paths = base_paths(&root);
    std::fs::write(
        &paths.config,
        vec![b'x'; usize::try_from(DEFAULT_MAX_CONFIG_BYTES).unwrap() + 1],
    )
    .unwrap();
    let result = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms: 0 },
    );
    assert!(result.err().unwrap().contains("exceeds"));

    paths = base_paths(&root);
    let dof = root.join("too-large.DAT");
    let limits = RegionalDataLimits {
        max_dof_compressed_file_bytes: 16,
        ..RegionalDataLimits::default()
    };
    std::fs::write(&dof, vec![b' '; 17]).unwrap();
    paths.dof_files.push(dof);
    let result = load_regional_data(&paths, limits, WeatherAdmission { cutoff_ms: 0 });
    assert!(result.err().unwrap().contains("exceeds"));
}

#[test]
fn terrain_cell_budget_refuses_before_tile_access_and_missing_source_stays_missing() {
    let root = scratch("terrain-budget");
    let mut paths = base_paths(&root);
    let terrain = root.join("tiles");
    std::fs::create_dir_all(&terrain).unwrap();
    paths.terrain_dir = Some(terrain);
    let mut data = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms: 0 },
    )
    .unwrap();
    assert!(data.floors.has_terrain());
    assert!(!data.provenance.obstacle_source_present);
    assert!(!data.provenance.chart_available);
    assert_eq!(data.qnh.stats.stations, 0);
    let point = SamplePoint {
        lat_e7: 0,
        lon_e7: -1_000_000_000,
        radius_m: 10_000_000.0,
        track_deg: -1,
    };
    let floor = data.floors.floor_at(&point).unwrap();
    assert_eq!(floor.terrain_ft, oa_msaw::FLOOR_NONE);
    assert_eq!(data.floors.stats.terrain_budget_refused, 1);
    assert_eq!(data.floors.stats.tiles_loaded, 0);
    assert_eq!(data.floors.stats.tiles_absent, 0);
    let mut local = point;
    local.radius_m = 100.0;
    let floor = data.floors.floor_at(&local).unwrap();
    assert_eq!(floor.terrain_ft, oa_msaw::FLOOR_NONE);
    assert!(data.floors.stats.tiles_absent > 0);

    let tile = root.join("tiles/USGS_1_n01w100.tif");
    std::fs::write(&tile, [b'x'; 17]).unwrap();
    let limits = RegionalDataLimits {
        max_terrain_file_bytes: 16,
        ..RegionalDataLimits::default()
    };
    let mut fresh = load_regional_data(
        &paths,
        limits,
        WeatherAdmission { cutoff_ms: 0 },
    )
    .unwrap();
    assert_eq!(fresh.floors.floor_at(&local).unwrap().terrain_ft, oa_msaw::FLOOR_NONE);
    assert_eq!(fresh.floors.stats.terrain_budget_refused, 1);
    assert_eq!(fresh.floors.stats.tiles_loaded, 0);
}

#[test]
fn weather_receipt_and_observation_cutoff_are_both_enforced_and_hashed() {
    let root = scratch("weather");
    let mut paths = base_paths(&root);
    let metar = root.join("hour.json");
    let record = r#"[{"icaoId":"KMNM","receiptTime":"2033-05-18T03:33:20.000Z","obsTime":2000000001,"reportTime":"2033-05-18T03:33:20.000Z","temp":6.1,"dewp":-0.6,"wdir":330,"wspd":9,"visib":"10+","altim":1023.8,"slp":1024.3,"qcField":14,"metarType":"METAR","rawOb":"METAR KMNM 141757Z AUTO 33009KT 10SM CLR 06/M01 A3023 RMK AO2","lat":45.126,"lon":-87.639,"elev":188,"name":"Menominee","cover":"CLR","clouds":[],"fltCat":"VFR"}]"#;
    std::fs::write(&metar, record).unwrap();
    paths.metar_files.push(MetarSource {
        path: metar.clone(),
        received_at_ms: 2_000_000_000_000,
    });
    let cutoff_ms = 2_000_000_000_000;
    let data = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    )
    .unwrap();
    assert_eq!(data.qnh.stats.rows, 0); // observation time 200 seconds is after cutoff
    assert_eq!(data.provenance.sources[1].available_at_ms, cutoff_ms);
    let digest = data.provenance.manifest_sha256;
    let repeat = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    )
    .unwrap();
    assert_eq!(digest, repeat.provenance.manifest_sha256);

    let admissible = record.replace("2000000001", "1999999999");
    std::fs::write(&metar, admissible).unwrap();
    let accepted = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    )
    .unwrap();
    assert_eq!(accepted.qnh.stats.rows, 1);
    assert_eq!(accepted.qnh.stats.stations, 1);

    let row_received_late = record
        .replace("2033-05-18T03:33:20.000Z", "2033-05-18T03:33:21.000Z")
        .replace("2000000001", "1999999999");
    std::fs::write(&metar, row_received_late).unwrap();
    let receipt_filtered = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    )
    .unwrap();
    assert_eq!(receipt_filtered.qnh.stats.rows, 0);

    paths.metar_files[0].received_at_ms = cutoff_ms + 1;
    let late = load_regional_data(
        &paths,
        RegionalDataLimits::default(),
        WeatherAdmission { cutoff_ms },
    );
    assert!(late.err().unwrap().contains("after issue cutoff"));
}
