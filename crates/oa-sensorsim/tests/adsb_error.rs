//! The per-aircraft ADS-B error transform: fractions, bias, latency, labels, determinism.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use oa_adsb::{reports_len, reports_push, Reports, ROW_EMPTY};
use oa_geo::{wgs84, LatLon};
use oa_sensorsim::adsb_error::*;
use oa_sensorsim::SimError;
use oa_truth::{build_truth, nacp_epu_m, Truth, DEFAULT_CONFIG};

const T0: i64 = 1_700_000_000_000_000_000;
const AIRCRAFT: u32 = 20;
const STEPS: i64 = 120;
const NACP: u8 = 10;

/// Twenty aircraft on straight eastbound tracks at 100 m/s, a report a second.
fn base() -> Reports {
    let mut r = Reports::default();
    for a in 0..AIRCRAFT {
        let lat0 = 40.0 + f64::from(a) * 0.1;
        for k in 0..STEPS {
            let (la, lo) =
                oa_sensorsim::geom::offset_geodesic_deg(lat0, 10.0, 100.0 * k as f64, 0.0).unwrap();
            let mut row = ROW_EMPTY;
            row.icao = 0xd00000 + a;
            row.time_ns = T0 + k * 1_000_000_000;
            row.lat_e7 = (la * 1e7).round() as i32;
            row.lon_e7 = (lo * 1e7).round() as i32;
            row.baro_alt_ft = 30_000;
            row.nic = 8;
            row.nac_p = NACP;
            row.sil = 3;
            row.version = 2;
            reports_push(&mut r, &row);
        }
    }
    r
}

fn truth(r: &Reports) -> Truth {
    build_truth(r, &DEFAULT_CONFIG).unwrap().0
}

const ZERO: ErrorParams = ErrorParams {
    seed: 7,
    bias_fraction: 0.0,
    bias_mult: 3.0,
    bias_fallback_bound_m: 100.0,
    lag_fraction: 0.0,
    lag_min_s: 0.5,
    lag_max_s: 0.5,
};

type Table = BTreeMap<(u32, i64), (f64, f64)>;

fn table(e: &Errored) -> Table {
    (0..reports_len(&e.reports))
        .map(|i| {
            (
                (e.reports.icao[i], e.reports.time_ns[i]),
                (
                    f64::from(e.reports.lat_e7[i]) * 1e-7,
                    f64::from(e.reports.lon_e7[i]) * 1e-7,
                ),
            )
        })
        .collect()
}

fn dist_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    wgs84()
        .inverse(
            LatLon::new(a.0, a.1).unwrap(),
            LatLon::new(b.0, b.1).unwrap(),
        )
        .unwrap()
        .distance_m
}

fn azimuth_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    wgs84()
        .inverse(
            LatLon::new(a.0, a.1).unwrap(),
            LatLon::new(b.0, b.1).unwrap(),
        )
        .unwrap()
        .azimuth1_deg
}

// REQ: OA-SIM-033
#[test]
fn the_fractions_are_honoured_exactly_for_every_seed() {
    let r = base();
    let t = truth(&r);
    let mut sets = Vec::new();
    for seed in 0..16u64 {
        let p = ErrorParams {
            seed,
            bias_fraction: 0.25,
            lag_fraction: 0.45,
            ..ZERO
        };
        let e = adsb_error(&t, &r, &p).unwrap();
        assert_eq!(e.bias_addrs.len(), 5);
        assert_eq!(e.lag_addrs.len(), 9);
        sets.push(e.bias_addrs);
    }
    // The aircraft chosen change with the seed.
    assert!(sets.iter().any(|s| s != &sets[0]));
}

// REQ: OA-SIM-034
#[test]
fn a_bias_is_the_multiple_of_the_nacp_bound_in_a_fixed_direction_per_aircraft() {
    let r = base();
    let t = truth(&r);
    let control = table(&adsb_error(&t, &r, &ZERO).unwrap());
    let p = ErrorParams {
        bias_fraction: 0.5,
        ..ZERO
    };
    let e = adsb_error(&t, &r, &p).unwrap();
    let got = table(&e);
    let mut dirs = Vec::new();
    for &a in &e.bias_addrs {
        let mut az: Option<f64> = None;
        for (&(ad, tn), &pos) in &got {
            if ad != a {
                continue;
            }
            let c = control[&(ad, tn)];
            let d = dist_m(c, pos);
            assert!((d - 3.0 * nacp_epu_m(NACP)).abs() < 0.02, "{d}");
            let z = azimuth_deg(c, pos);
            let first = *az.get_or_insert(z);
            assert!(
                ((z - first + 540.0).rem_euclid(360.0) - 180.0).abs() < 0.5,
                "{z} {first}"
            );
        }
        dirs.push(az.unwrap());
    }
    dirs.sort_by(f64::total_cmp);
    dirs.dedup_by(|a, b| (*a - *b).abs() < 1.0);
    assert!(dirs.len() > 1);
}

// REQ: OA-SIM-035
#[test]
fn a_row_without_nacp_takes_the_stated_fallback_bound() {
    let mut r = base();
    for v in &mut r.nac_p {
        *v = oa_adsb::NONE_U8;
    }
    let t = truth(&base());
    let control = table(&adsb_error(&t, &r, &ZERO).unwrap());
    let p = ErrorParams {
        bias_fraction: 1.0,
        bias_fallback_bound_m: 120.0,
        ..ZERO
    };
    let got = table(&adsb_error(&t, &r, &p).unwrap());
    assert!(!got.is_empty());
    for (k, &pos) in &got {
        assert!((dist_m(control[k], pos) - 360.0).abs() < 0.02);
    }
}

// REQ: OA-SIM-036
#[test]
fn a_lagged_aircraft_reports_the_truth_of_t_minus_l_stamped_at_t() {
    let r = base();
    let t = truth(&r);
    let control = table(&adsb_error(&t, &r, &ZERO).unwrap());
    // 2 s is exactly two reports back; 0.5 s is 50 m back on the 100 m/s track.
    for (lag, back_m) in [(2.0, 200.0), (0.5, 50.0)] {
        let p = ErrorParams {
            lag_fraction: 0.5,
            lag_min_s: lag,
            lag_max_s: lag,
            ..ZERO
        };
        let e = adsb_error(&t, &r, &p).unwrap();
        assert_eq!(e.lag_addrs.len(), 10);
        let got = table(&e);
        let mut checked = 0;
        for &(a, ms) in &e.lag_addrs {
            assert_eq!(ms, (lag * 1000.0) as i64);
            for (&(ad, tn), &pos) in &got {
                if ad != a {
                    continue;
                }
                let c = control[&(ad, tn)];
                assert!((dist_m(c, pos) - back_m).abs() < 0.02, "{lag}");
                // Behind the control: west of it.
                assert!(pos.1 < c.1);
                if lag == 2.0 {
                    let earlier = control[&(ad, tn - 2_000_000_000)];
                    assert!(dist_m(earlier, pos) < 0.02);
                }
                checked += 1;
            }
        }
        assert!(checked > 500);
    }
}

// REQ: OA-SIM-037
#[test]
fn the_latency_is_constant_per_aircraft_and_within_the_range() {
    let r = base();
    let t = truth(&r);
    let p = ErrorParams {
        lag_fraction: 1.0,
        lag_min_s: 0.6,
        lag_max_s: 0.8,
        ..ZERO
    };
    let e = adsb_error(&t, &r, &p).unwrap();
    assert_eq!(e.lag_addrs.len(), AIRCRAFT as usize);
    assert!(e.lag_addrs.iter().all(|&(_, ms)| (600..=800).contains(&ms)));
    let mut ms: Vec<i64> = e.lag_addrs.iter().map(|x| x.1).collect();
    ms.dedup();
    assert!(ms.len() > 1);
}

// REQ: OA-SIM-038
#[test]
fn labels_mark_exactly_the_rows_of_the_aircraft_with_each_error() {
    let r = base();
    let t = truth(&r);
    let p = ErrorParams {
        bias_fraction: 0.4,
        lag_fraction: 0.4,
        lag_max_s: 0.9,
        ..ZERO
    };
    let e = adsb_error(&t, &r, &p).unwrap();
    assert_eq!(e.label.len(), reports_len(&e.reports));
    let control = table(&adsb_error(&t, &r, &ZERO).unwrap());
    let got = table(&e);
    let (mut both, mut none) = (0, 0);
    for (i, &l) in e.label.iter().enumerate() {
        let a = e.reports.icao[i];
        let bias = e.bias_addrs.contains(&a);
        let lag = e.lag_addrs.iter().any(|x| x.0 == a);
        assert_eq!(l, (u8::from(bias) * ERR_BIAS) | (u8::from(lag) * ERR_LAG));
        both += usize::from(l == ERR_BIAS | ERR_LAG);
        if l == 0 {
            none += 1;
            // An aircraft with no error is the truth, row for row.
            let k = (a, e.reports.time_ns[i]);
            assert!(dist_m(control[&k], got[&k]) < 1e-6);
        }
    }
    assert!(none > 0 && both > 0);
}

// REQ: OA-SIM-039
#[test]
fn the_output_depends_only_on_the_arguments() {
    let r = base();
    let t = truth(&r);
    let p = ErrorParams {
        bias_fraction: 0.3,
        lag_fraction: 0.3,
        lag_max_s: 0.9,
        ..ZERO
    };
    let a = adsb_error(&t, &r, &p).unwrap();
    assert_eq!(a, adsb_error(&t, &r, &p).unwrap());
    let q = ErrorParams { seed: 8, ..p };
    assert_ne!(a.bias_addrs, adsb_error(&t, &r, &q).unwrap().bias_addrs);
}

// REQ: OA-SIM-040
#[test]
fn the_control_reproduces_the_truth_and_changes_no_other_field() {
    let r = base();
    let t = truth(&r);
    let e = adsb_error(&t, &r, &ZERO).unwrap();
    assert!(e.label.iter().all(|&l| l == 0));
    assert!(e.bias_addrs.is_empty() && e.lag_addrs.is_empty());
    assert_eq!(
        reports_len(&e.reports) as u64 + e.dropped_rows,
        reports_len(&r) as u64
    );
    // The truth of a noiseless straight track is the track: every generated position is the
    // reported one to the rounding of 1e-7 degree.
    let mut j = 0;
    for i in 0..reports_len(&r) {
        if j < reports_len(&e.reports)
            && e.reports.icao[j] == r.icao[i]
            && e.reports.time_ns[j] == r.time_ns[i]
        {
            assert!(
                dist_m(
                    (f64::from(r.lat_e7[i]) * 1e-7, f64::from(r.lon_e7[i]) * 1e-7),
                    (
                        f64::from(e.reports.lat_e7[j]) * 1e-7,
                        f64::from(e.reports.lon_e7[j]) * 1e-7
                    )
                ) < 0.05
            );
            assert_eq!(e.reports.nac_p[j], r.nac_p[i]);
            assert_eq!(e.reports.callsign[j], r.callsign[i]);
            let kt = f64::from(e.reports.gs_dkt[j]) / 10.0;
            assert!((kt - 100.0 * 3600.0 / 1852.0).abs() < 0.1);
            assert!(e.reports.track_ddeg[j].abs_diff(900) <= 5);
            j += 1;
        }
    }
    assert_eq!(j, reports_len(&e.reports));
}

// REQ: OA-SIM-041
#[test]
fn parameters_out_of_range_are_refused() {
    let r = base();
    let t = truth(&r);
    for p in [
        ErrorParams {
            bias_fraction: 1.1,
            ..ZERO
        },
        ErrorParams {
            lag_fraction: -0.1,
            ..ZERO
        },
        ErrorParams {
            bias_fraction: f64::NAN,
            ..ZERO
        },
        ErrorParams {
            bias_mult: -1.0,
            ..ZERO
        },
        ErrorParams {
            bias_fallback_bound_m: f64::INFINITY,
            ..ZERO
        },
        ErrorParams {
            lag_min_s: 0.6,
            lag_max_s: 0.5,
            ..ZERO
        },
        ErrorParams {
            lag_min_s: 0.0,
            lag_max_s: MAX_LAG_S + 1.0,
            ..ZERO
        },
    ] {
        assert!(matches!(adsb_error(&t, &r, &p), Err(SimError::BadSpoof(_))));
    }
}
