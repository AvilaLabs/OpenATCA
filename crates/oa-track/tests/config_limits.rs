//! Public configuration and simulation helpers refuse settings that would panic, hang or
//! allocate without bound, and a fusion configuration refuses two radars with one sensor id.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_bench::adsb_truth::BuiltTruth;
use oa_sensorsim::profile::MSSR_EUROCONTROL_1997;
use oa_track::assoc::{assoc_scratch_caps, assoc_scratch_new};
use oa_track::fusion::{fusion_tracker_config, FusionError};
use oa_track::fusion_score::{
    built_of, make_radar, run_variant, simulate_adsb, simulate_plots, AdsbSim, Kind, ADSB_STUDY,
    EPOCH_NS, V_OURS_NOREG,
};
use oa_track::fusion_world::base_trajs;
use oa_track::registration::reg_new_with;
use oa_track::score::interval_stats;
use oa_track::synth_truth::{build_synthetic_truth, Seg, Traj};
use oa_track::{check_config, Measurement, DEFAULT_CONFIG, NONE_I32, NO_IDENT};

fn small_world() -> BuiltTruth {
    let d = oa_track::fusion_score::site_def(Kind::Terminal);
    let picked: Vec<Traj> = base_trajs(Kind::Terminal, 1)
        .into_iter()
        .step_by(3)
        .take(2)
        .collect();
    built_of(build_synthetic_truth(d.lat_deg, d.lon_deg, EPOCH_NS, &picked).unwrap())
}

fn first_ns(b: &BuiltTruth) -> i64 {
    oa_truth::leg_start_ns(&b.truth, oa_truth::LegIx(0)).unwrap()
}

fn traj(dur_s: f64) -> Traj {
    Traj {
        addr: 0xA00001,
        t0_s: 0.0,
        x0: 0.0,
        y0: 0.0,
        heading_deg: 0.0,
        speed_mps: 100.0,
        alt_ft: 10_000.0,
        segs: vec![Seg {
            dur_s,
            turn_dps: 0.0,
            vrate_fpm: 0.0,
        }],
    }
}

fn meas(addr: u32, toa_ns: i64) -> Measurement {
    Measurement {
        toa_ns,
        pos_cov_m2: [1.0, 0.0, 1.0],
        vel_cov_m2s2: [0.0; 3],
        vel_east_mps: 0.0,
        vel_north_mps: 0.0,
        lat_e7: 0,
        lon_e7: 0,
        alt_ft: NONE_I32,
        vrate_fpm: NONE_I32,
        addr,
        alt_quant_ft: 0,
        sensor: 0,
        ident: NO_IDENT,
        flags: 0,
        category: 0,
    }
}

// REQ: OA-TRK-250
#[test]
fn duplicate_radar_sensor_ids_are_refused() {
    let ra = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let mut rb = make_radar(Kind::EnRoute, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    rb.trk.sensor = ra.trk.sensor;
    let r = run_variant(&V_OURS_NOREG, &[ra.trk, rb.trk], &[&[], &[]], &[]);
    match r {
        Err(FusionError::BadConfig(w)) => assert!(w.contains("sensor"), "{w}"),
        Err(e) => panic!("wrong error {e}"),
        Ok(_) => panic!("two radars with sensor id {} accepted", ra.trk.sensor),
    }
}

// REQ: OA-TRK-250
#[test]
fn distinct_radar_sensor_ids_apply_both_reports() {
    let built = small_world();
    let lo = first_ns(&built);
    let hi = lo + 120_000_000_000;
    let ra = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let mut rb = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    rb.trk.sensor = ra.trk.sensor + 1;
    let (pa, _, _) = simulate_plots(&built.truth, &ra, 500, lo, hi).unwrap();
    let (pb, _, _) = simulate_plots(&built.truth, &rb, 501, lo, hi).unwrap();
    assert!(!pa.is_empty() && !pb.is_empty());
    let r = run_variant(&V_OURS_NOREG, &[ra.trk, rb.trk], &[&pa, &pb], &[]).unwrap();
    assert_eq!(r.fstats.plots_in, (pa.len() + pb.len()) as u64);
    assert_eq!(r.tstats.duplicates, 0, "{:?}", r.tstats);
}

// REQ: OA-TRK-251
#[test]
fn adsb_period_zero_is_refused() {
    let b = small_world();
    let lo = first_ns(&b);
    let cfg = AdsbSim {
        period_ns: 0,
        ..ADSB_STUDY
    };
    assert!(simulate_adsb(&b.truth, &cfg, 1, lo, lo + 2_000_000_000, &|_| true).is_err());
}

// REQ: OA-TRK-251
#[test]
fn adsb_period_negative_is_refused() {
    let b = small_world();
    let lo = first_ns(&b);
    let cfg = AdsbSim {
        period_ns: -1,
        ..ADSB_STUDY
    };
    assert!(simulate_adsb(&b.truth, &cfg, 1, lo, lo + 2_000_000_000, &|_| true).is_err());
}

// REQ: OA-TRK-251
#[test]
fn adsb_period_one_ns_over_seconds_is_refused() {
    let b = small_world();
    let lo = first_ns(&b);
    let cfg = AdsbSim {
        period_ns: 1,
        ..ADSB_STUDY
    };
    assert!(simulate_adsb(&b.truth, &cfg, 1, lo, lo + 5_000_000_000, &|_| true).is_err());
}

// REQ: OA-TRK-251
#[test]
fn adsb_bad_quality_codes_are_refused() {
    let b = small_world();
    let lo = first_ns(&b);
    for cfg in [
        AdsbSim {
            nacp: 0,
            ..ADSB_STUDY
        },
        AdsbSim {
            nacv: 9,
            ..ADSB_STUDY
        },
    ] {
        assert!(simulate_adsb(&b.truth, &cfg, 1, lo, lo + 2_000_000_000, &|_| true).is_err());
    }
}

// REQ: OA-TRK-251
#[test]
fn adsb_study_settings_still_run() {
    let b = small_world();
    let lo = first_ns(&b);
    let m = simulate_adsb(&b.truth, &ADSB_STUDY, 1, lo, lo + 60_000_000_000, &|_| true).unwrap();
    assert!(m.len() >= 100, "{}", m.len());
}

// REQ: OA-TRK-252
#[test]
fn synthetic_truth_refuses_a_path_too_long_to_count() {
    for dur in [1.0e300, f64::MAX] {
        let r = build_synthetic_truth(41.0, -87.0, EPOCH_NS, &[traj(dur)]);
        assert!(r.is_err(), "duration {dur}");
    }
    let two = Traj {
        segs: vec![
            Seg {
                dur_s: f64::MAX,
                turn_dps: 0.0,
                vrate_fpm: 0.0,
            };
            2
        ],
        ..traj(1.0)
    };
    assert!(build_synthetic_truth(41.0, -87.0, EPOCH_NS, &[two]).is_err());
}

// REQ: OA-TRK-252
#[test]
fn synthetic_truth_refuses_times_that_overflow_and_bad_origin() {
    let late = Traj {
        t0_s: 1.0e300,
        ..traj(100.0)
    };
    assert!(build_synthetic_truth(41.0, -87.0, EPOCH_NS, &[late]).is_err());
    assert!(build_synthetic_truth(41.0, -87.0, i64::MAX, &[traj(100.0)]).is_err());
    for (la, lo) in [(f64::NAN, -87.0), (41.0, f64::INFINITY), (95.0, 0.0)] {
        assert!(build_synthetic_truth(la, lo, EPOCH_NS, &[traj(100.0)]).is_err());
    }
}

// REQ: OA-TRK-253
#[test]
fn interval_stats_survives_extreme_times() {
    let m = [meas(1, i64::MIN), meas(1, i64::MAX)];
    assert!(interval_stats(&m).is_none());
}

// REQ: OA-TRK-253
#[test]
fn fusion_tracker_config_survives_extreme_periods() {
    let mut s = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0).trk;
    s.period_ns = i64::MAX;
    let c = fusion_tracker_config(&DEFAULT_CONFIG, &[s]);
    assert!(c.drop_after_ns > 0 && c.ooo_window_ns > 0);
}

// REQ: OA-TRK-254
#[test]
fn tracker_capacities_are_bounded() {
    for n in [usize::MAX, 1 << 40] {
        let c = oa_track::TrackerConfig {
            out_capacity: n,
            ..DEFAULT_CONFIG
        };
        assert!(check_config(&c).is_err(), "out_capacity {n}");
        let c = oa_track::TrackerConfig {
            log_capacity: n,
            ..DEFAULT_CONFIG
        };
        assert!(check_config(&c).is_err(), "log_capacity {n}");
    }
}

// REQ: OA-TRK-254
#[test]
fn registration_and_association_scratch_sizes_are_bounded() {
    let r = reg_new_with(usize::MAX, 1_000_000_000);
    assert_eq!(oa_track::registration::reg_bias(&r).range_m, 0.0);
    let sc = assoc_scratch_new(usize::MAX, usize::MAX, usize::MAX);
    let (t, p, q) = assoc_scratch_caps(&sc);
    assert!(t < usize::MAX && p < usize::MAX && q < usize::MAX);
}
