//! The ADS-B adapter: noise from NACp and NACv, and gating that is oa-truth's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{
    reports_push, Reports, Row, F_ALT_GEOM, F_STALE, NONE_I32, NONE_U16, NONE_U8, ROW_EMPTY,
};
use oa_track::adsb::{
    adsb_measurements, adsb_measurements_with_cache, check_adsb_config, AdsbConfig,
    AdsbQualityCache,
};
use oa_track::params::{NACV_BOUND_VEL_SIGMA_MPS, NACV_VEL_SIGMA_MPS};
use oa_track::{M_ALT_GEOMETRIC, M_HAS_VEL};
use oa_truth::{build_truth, DEFAULT_CONFIG, REMOVE_NAMES};

const S95: f64 = 2.447_746_830_680_816;

fn row(t_s: i64, icao: u32, lat: f64, lon: f64) -> Row {
    Row {
        time_ns: t_s * 1_000_000_000,
        icao,
        lat_e7: (lat * 1.0e7) as i32,
        lon_e7: (lon * 1.0e7) as i32,
        baro_alt_ft: 35_000,
        gs_dkt: 4500,
        track_ddeg: 900,
        vrate_fpm: 0,
        nic: 8,
        nac_p: 9,
        nac_v: 2,
        sil: 3,
        version: 2,
        source: 0,
        ..ROW_EMPTY
    }
}

/// The adapter with the NACv-bound velocity noise (the behaviour before the calibrated table):
/// the tests of the bound arithmetic below pin it, and prove the table is the only change.
fn bound_cfg() -> AdsbConfig {
    AdsbConfig {
        vel_sigma_mps: NACV_BOUND_VEL_SIGMA_MPS,
        ..AdsbConfig::default()
    }
}

fn reports(rows: &[Row]) -> Reports {
    let mut r = Reports::default();
    for x in rows {
        reports_push(&mut r, x);
    }
    r
}

// REQ: OA-TRK-018
#[test]
fn measurement_noise_comes_from_nacp_and_nacv() {
    let rows = vec![row(0, 0xABCDEF, 41.9, -88.3)];
    let (m, s) = adsb_measurements(&reports(&rows), &bound_cfg());
    assert_eq!((s.rows_in, s.passed), (1, 1));
    let m = m[0];
    // NACp 9: 95 % bound 30 m, per-axis sigma 30 / 2.448.
    let sigma = 30.0 / S95;
    assert!((f64::from(m.pos_cov_m2[0]) - sigma * sigma).abs() < 0.01);
    assert_eq!((m.pos_cov_m2[0], m.pos_cov_m2[1]), (m.pos_cov_m2[2], 0.0));
    // NACv 2: 3 m/s bound.
    assert!(m.flags & M_HAS_VEL != 0);
    assert!((f64::from(m.vel_cov_m2s2[0]) - (3.0 / S95).powi(2)).abs() < 1e-4);
    // 450 kt on track 90 degrees: east 231.5 m/s, north about 0.
    assert!(
        (f64::from(m.vel_east_mps) - 231.5).abs() < 0.1 && f64::from(m.vel_north_mps).abs() < 0.1
    );
    assert_eq!(
        (m.alt_ft, m.alt_quant_ft, m.addr, m.toa_ns),
        (35_000, 25, 0xABCDEF, 0)
    );
    // The values of every NACp (DO-260B bounds).
    for (nacp, bound) in [(8u8, 92.6), (9, 30.0), (10, 10.0), (11, 3.0)] {
        let mut r = row(0, 1, 41.9, -88.3);
        r.nac_p = nacp;
        let (m, _) = adsb_measurements(&reports(&[r]), &bound_cfg());
        assert!(
            (f64::from(m[0].pos_cov_m2[0]) - (bound / S95).powi(2)).abs()
                < 0.01 * (bound / S95).powi(2) + 1e-3
        );
    }
}

// REQ: OA-TRK-019
#[test]
fn quality_carry_forward_defaults_and_geometric_altitude() {
    let mut a = row(0, 5, 41.9, -88.3);
    let mut b = row(10, 5, 41.9, -88.2);
    let mut c = row(100, 5, 41.9, -88.1);
    // The second and third rows carry no quality fields: the second inherits within 60 s, the
    // third is too late and fails like truth's no_quality_in_age.
    for r in [&mut b, &mut c] {
        r.nac_p = NONE_U8;
        r.nic = NONE_U8;
        r.sil = NONE_U8;
        r.version = NONE_U8;
        r.nac_v = NONE_U8;
    }
    // Geometric altitude only on the first.
    a.baro_alt_ft = NONE_I32;
    a.geom_alt_ft = 35_100;
    let (m, s) = adsb_measurements(&reports(&[a, b, c]), &bound_cfg());
    assert_eq!((s.passed, s.removed[3]), (2, 1), "{s:?}");
    assert!(m[0].flags & M_ALT_GEOMETRIC != 0 && m[0].alt_ft == 35_100);
    // The second row's NACv was carried (2): sigma 3 / 2.448.
    assert!((f64::from(m[1].vel_cov_m2s2[0]) - (3.0 / S95).powi(2)).abs() < 1e-4);
    // A velocity with no NACv in age is used as NACv 1 (10 m/s bound), and counted.
    let mut d = row(0, 6, 41.9, -88.3);
    d.nac_v = NONE_U8;
    let (m, s) = adsb_measurements(&reports(&[d]), &bound_cfg());
    assert!((f64::from(m[0].vel_cov_m2s2[0]) - (10.0 / S95).powi(2)).abs() < 1e-3);
    assert_eq!(s.velocity_default_nacv, 1);
    // No ground speed: no velocity.
    let mut e = row(0, 7, 41.9, -88.3);
    e.gs_dkt = NONE_U16;
    let (m, _) = adsb_measurements(&reports(&[e]), &bound_cfg());
    assert_eq!(m[0].flags & M_HAS_VEL, 0);
}

// REQ: OA-TRK-286
#[test]
fn bounded_quality_cache_carries_across_frames_and_expires_for_refill() {
    let mut cache = AdsbQualityCache::new(1);
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
    let cfg = AdsbConfig {
        gates: oa_truth::TruthConfig {
            quality_carry_ms: 10_000,
            ..DEFAULT_CONFIG
        },
        ..AdsbConfig::default()
    };
    let first = row(1, 0x100001, 41.9, -88.3);
    let (measurements, stats) = adsb_measurements_with_cache(&reports(&[first]), &cfg, &mut cache);
    assert_eq!((measurements.len(), stats.passed), (1, 1));
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());

    let mut out_of_order = row(0, 0x100001, 41.9, -88.25);
    out_of_order.nac_p = NONE_U8;
    out_of_order.nic = NONE_U8;
    out_of_order.sil = NONE_U8;
    out_of_order.version = NONE_U8;
    out_of_order.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[out_of_order]), &cfg, &mut cache);
    assert!(measurements.is_empty());
    assert_eq!(stats.quality_state_order_refused, 1);
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());

    let mut rejected_without_state = row(9, 0x100002, 41.9, -88.1);
    rejected_without_state.nac_p = NONE_U8;
    rejected_without_state.nic = NONE_U8;
    rejected_without_state.sil = NONE_U8;
    rejected_without_state.version = NONE_U8;
    rejected_without_state.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[rejected_without_state]), &cfg, &mut cache);
    assert!(measurements.is_empty());
    assert_eq!(stats.removed[oa_truth::REM_NO_QUALITY], 1);
    assert_eq!(stats.quality_state_capacity_refused, 0);
    assert_eq!(cache.len(), 1);

    // oa-truth carries a held value inclusively through the configured age.
    let mut carried = row(11, 0x100001, 41.9, -88.2);
    carried.nac_p = NONE_U8;
    carried.nic = NONE_U8;
    carried.sil = NONE_U8;
    carried.version = NONE_U8;
    carried.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[carried]), &cfg, &mut cache);
    assert_eq!((measurements.len(), stats.passed), (1, 1));
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());

    let mut other_without_quality = row(12, 0x100002, 41.9, -88.0);
    other_without_quality.nac_p = NONE_U8;
    other_without_quality.nic = NONE_U8;
    other_without_quality.sil = NONE_U8;
    other_without_quality.version = NONE_U8;
    other_without_quality.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[other_without_quality]), &cfg, &mut cache);
    assert!(measurements.is_empty());
    assert_eq!(stats.removed[oa_truth::REM_NO_QUALITY], 1);
    assert_eq!(stats.quality_state_capacity_refused, 0);
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());

    let refill = row(13, 0x100002, 41.9, -88.0);
    let (measurements, stats) = adsb_measurements_with_cache(&reports(&[refill]), &cfg, &mut cache);
    assert_eq!((measurements.len(), stats.passed), (1, 1));
    assert_eq!(stats.quality_state_capacity_refused, 0);
    assert_eq!(cache.len(), 1);
    assert!(!cache.is_empty());

    let mut carried_after_refill = row(23, 0x100002, 41.9, -87.9);
    carried_after_refill.nac_p = NONE_U8;
    carried_after_refill.nic = NONE_U8;
    carried_after_refill.sil = NONE_U8;
    carried_after_refill.version = NONE_U8;
    carried_after_refill.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[carried_after_refill]), &cfg, &mut cache);
    assert_eq!((measurements.len(), stats.passed), (1, 1));
}

// REQ: OA-TRK-286
#[test]
fn quality_cache_order_and_inclusive_expiry_use_newest_held_field() {
    let cfg = AdsbConfig {
        gates: oa_truth::TruthConfig {
            quality_carry_ms: 10_000,
            ..DEFAULT_CONFIG
        },
        ..AdsbConfig::default()
    };
    let mut cache = AdsbQualityCache::new(1);
    let first = row(1, 0x100001, 41.9, -88.3);
    let (measurements, _) = adsb_measurements_with_cache(&reports(&[first]), &cfg, &mut cache);
    assert_eq!(measurements.len(), 1);

    let mut newest_nacp = row(6, 0x100001, 41.9, -88.2);
    newest_nacp.nac_p = 10;
    newest_nacp.nic = NONE_U8;
    newest_nacp.sil = NONE_U8;
    newest_nacp.version = NONE_U8;
    newest_nacp.nac_v = NONE_U8;
    let (measurements, _) =
        adsb_measurements_with_cache(&reports(&[newest_nacp]), &cfg, &mut cache);
    assert_eq!(measurements.len(), 1);

    let mut regressing_without_quality = row(5, 0x100001, 41.9, -88.1);
    regressing_without_quality.nac_p = NONE_U8;
    regressing_without_quality.nic = NONE_U8;
    regressing_without_quality.sil = NONE_U8;
    regressing_without_quality.version = NONE_U8;
    regressing_without_quality.nac_v = NONE_U8;
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[regressing_without_quality]), &cfg, &mut cache);
    assert!(measurements.is_empty());
    assert_eq!(stats.quality_state_order_refused, 1);
    assert_eq!(cache.len(), 1);

    let refill_at_newest_deadline = row(16, 0x100002, 41.9, -88.0);
    let (measurements, stats) =
        adsb_measurements_with_cache(&reports(&[refill_at_newest_deadline]), &cfg, &mut cache);
    assert_eq!(measurements.len(), 1);
    assert_eq!(stats.quality_state_capacity_refused, 1);
    assert_eq!(cache.len(), 1);
}

// REQ: OA-TRK-286
#[test]
fn a_geometric_flagged_altitude_passes_flagged_like_a_geometric_column() {
    // The readsb form: the altitude column is geometric (flag set), no barometric altitude.
    let mut flagged = row(0, 5, 41.9, -88.3);
    flagged.baro_alt_ft = NONE_I32;
    flagged.geom_alt_ft = 35_000;
    flagged.flags |= F_ALT_GEOM;
    // The same report with the flag clear and the geometric column set.
    let mut plain = flagged;
    plain.flags &= !F_ALT_GEOM;
    plain.icao = 6;
    let (m, s) = adsb_measurements(&reports(&[flagged, plain]), &AdsbConfig::default());
    assert_eq!(s.passed, 2, "{s:?}");
    for x in &m {
        assert!(
            x.flags & M_ALT_GEOMETRIC != 0 && x.alt_ft == 35_000,
            "{x:?}"
        );
    }
}

// REQ: OA-TRK-020
#[test]
fn gating_is_the_same_code_as_the_truth_builder() {
    // A mixed bag of rows over two aircraft, long enough that truth's own leg rules (jumps,
    // short legs) do not interfere; the adapter's removals by reason must equal truth's for the
    // reasons the two share.
    let mut rows = Vec::new();
    for k in 0..60i64 {
        for (icao, dlon) in [(0x100001u32, 0.0), (0x100002, 0.5)] {
            let mut r = row(k * 5, icao, 41.9 + k as f64 * 0.001, -88.3 + dlon);
            match (k + i64::from(icao & 7)) % 11 {
                1 => r.nac_p = 7,
                2 => r.nic = 6,
                3 => r.sil = 2,
                4 => r.version = 1,
                5 => r.flags |= F_STALE,
                6 => r.source = 3,
                7 => {
                    r.nac_p = NONE_U8;
                    r.nic = NONE_U8;
                }
                _ => {}
            }
            rows.push(r);
        }
    }
    let rep = reports(&rows);
    let (_, truth_stats) = build_truth(
        &rep,
        &oa_truth::TruthConfig {
            loo: false,
            ..DEFAULT_CONFIG
        },
    )
    .unwrap();
    let (m, s) = adsb_measurements(&rep, &AdsbConfig::default());
    for (k, name) in REMOVE_NAMES.iter().enumerate().take(8) {
        assert_eq!(s.removed[k], truth_stats.removed[k], "reason {name}");
    }
    let removed: u64 = s.removed.iter().sum();
    assert_eq!(s.passed + removed, s.rows_in);
    assert_eq!(m.len() as u64, s.passed);
    assert!(
        s.removed[..8].iter().filter(|&&c| c > 0).count() >= 6,
        "{s:?}"
    );
    // Output is in time order.
    assert!(m.windows(2).all(|w| w[0].toa_ns <= w[1].toa_ns));
}

// REQ: OA-TRK-339
#[test]
fn the_velocity_noise_comes_from_the_configured_table_by_nacv() {
    // The default is the calibrated table: a NACv-2 report gets its entry squared.
    let rows = vec![row(0, 0xABCDEF, 41.9, -88.3)];
    let (m, _) = adsb_measurements(&reports(&rows), &AdsbConfig::default());
    let want = (NACV_VEL_SIGMA_MPS[2] * NACV_VEL_SIGMA_MPS[2]) as f32;
    assert_eq!(m[0].vel_cov_m2s2, [want, 0.0, want]);
    // A configured table is used as given, whatever the NACv bound says.
    let cfg = AdsbConfig {
        vel_sigma_mps: [f64::INFINITY, 7.0, 0.5, 0.25, 0.125],
        ..AdsbConfig::default()
    };
    for (nacv, sigma) in [(1u8, 7.0f64), (2, 0.5), (3, 0.25), (4, 0.125)] {
        let mut r = row(0, 1, 41.9, -88.3);
        r.nac_v = nacv;
        let (m, _) = adsb_measurements(&reports(&[r]), &cfg);
        let v = (sigma * sigma) as f32;
        assert_eq!(m[0].vel_cov_m2s2, [v, 0.0, v], "NACv {nacv}");
    }
    // A velocity with no NACv takes the default NACv's entry.
    let mut d = row(0, 6, 41.9, -88.3);
    d.nac_v = NONE_U8;
    let (m, s) = adsb_measurements(&reports(&[d]), &cfg);
    assert_eq!(m[0].vel_cov_m2s2[0], 49.0);
    assert_eq!(s.velocity_default_nacv, 1);
    // The bound table is the old arithmetic exactly.
    for (k, b) in [(1usize, 10.0f64), (2, 3.0), (3, 1.0), (4, 0.3)] {
        assert_eq!(NACV_BOUND_VEL_SIGMA_MPS[k], b / S95);
    }
    // Every entry is positive and finite, and the table is accepted.
    assert!(check_adsb_config(&AdsbConfig::default()).is_ok());
    assert!(check_adsb_config(&bound_cfg()).is_ok());
    for sigma in NACV_VEL_SIGMA_MPS.iter().skip(1) {
        assert!(*sigma > 0.0 && sigma.is_finite());
    }
    // A bad entry is refused by the check, and the adapter does not use that velocity.
    for bad in [0.0, -1.0, f64::NAN] {
        let mut t = NACV_VEL_SIGMA_MPS;
        t[2] = bad;
        let cfg = AdsbConfig {
            vel_sigma_mps: t,
            ..AdsbConfig::default()
        };
        assert_eq!(check_adsb_config(&cfg), Err("vel_sigma_mps"));
        let (m, s) = adsb_measurements(&reports(&rows), &cfg);
        assert_eq!(
            (m[0].flags & M_HAS_VEL, s.velocity_refused, s.with_velocity),
            (0, 1, 0)
        );
    }
}
