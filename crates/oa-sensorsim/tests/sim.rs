//! The radar simulation on synthetic truth with known geometry: noise, bias, timing, detection,
//! quantisation, horizon, ASTERIX round trip, determinism, refusals.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{reports_push, Reports, ROW_EMPTY};
use oa_asterix::{decode_stream, Block};
use oa_sensorsim::geom::{az_diff, frame_at, offset_deg, polar_of, FT_M, NM_M};
use oa_sensorsim::plots::{
    plots_len, to_cat048, Plots, KIND_FALSE, KIND_SPLIT, KIND_TRUE, NONE_ADDR, NONE_ALT, NONE_CODE,
};
use oa_sensorsim::profile::*;
use oa_sensorsim::radar::{beam_az_deg, period_ns, scan_start_ns, simulate_radar, RadarSite};
use oa_sensorsim::SimError;
use oa_truth::{build_truth, truth_at, Truth, TruthQuery, DEFAULT_CONFIG};

const T0: i64 = 1_700_000_000_000_000_000;
const RLAT: f64 = 41.0;
const RLON: f64 = -88.0;

/// A straight track: position at `t` seconds is the start plus `(ve, vn) t` metres.
fn track(
    r: &mut Reports,
    addr: u32,
    (lat0, lon0): (f64, f64),
    (ve, vn): (f64, f64),
    alt_ft: i32,
    dur_s: f64,
    dt_s: f64,
) {
    let mut t = 0.0;
    while t <= dur_s + 1e-9 {
        let (lat, lon) = offset_deg(lat0, lon0, ve * t, vn * t);
        let mut row = ROW_EMPTY;
        row.icao = addr;
        row.time_ns = T0 + (t * 1e9).round() as i64;
        row.lat_e7 = (lat * 1e7).round() as i32;
        row.lon_e7 = (lon * 1e7).round() as i32;
        row.baro_alt_ft = alt_ft;
        row.nic = 8;
        row.nac_p = 10;
        row.sil = 3;
        row.version = 2;
        row.source = 0;
        reports_push(r, &row);
        t += dt_s;
    }
}

fn truth_of(r: &Reports) -> Truth {
    build_truth(r, &DEFAULT_CONFIG).unwrap().0
}

/// A profile with certain detection and no false or split plots, the given sigmas.
fn clean(p: RadarProfile) -> RadarProfile {
    RadarProfile {
        pd: 1.0,
        pd_mode_a: 1.0,
        pd_alt: 1.0,
        false_rate: 0.0,
        split_rate: 0.0,
        ..p
    }
}

fn site(profile: RadarProfile) -> RadarSite {
    RadarSite {
        lat_deg: RLAT,
        lon_deg: RLON,
        height_m: 30.0,
        rotation_period_s: SCAN_TERMINAL_S,
        initial_az_deg: 10.0,
        epoch_ns: T0,
        coverage_range_m: 150_000.0,
        earth_k: EARTH_K_STANDARD,
        profile,
        bias_range_m: 0.0,
        bias_az_deg: 0.0,
        split_dr_m: 400.0,
        split_daz_deg: 0.3,
        sac: 7,
        sic: 9,
    }
}

fn run(t: &Truth, s: &RadarSite, seed: u64) -> (Plots, oa_sensorsim::radar::SimStats) {
    simulate_radar(t, s, seed, i64::MIN / 2, i64::MAX / 2).unwrap()
}

/// Eight stationary aircraft at different bearings and ranges (40 to 80 km).
fn stationary_field(dur_s: f64) -> Reports {
    let mut r = Reports::default();
    for k in 0..8u32 {
        let brg = f64::from(k) * 45.0 + 20.0;
        let d = 40_000.0 + f64::from(k) * 5_000.0;
        let (e, n) = (d * brg.to_radians().sin(), d * brg.to_radians().cos());
        let (lat, lon) = offset_deg(RLAT, RLON, e, n);
        track(
            &mut r,
            0xa00000 + k,
            (lat, lon),
            (0.0, 0.0),
            35_037,
            dur_s,
            5.0,
        );
    }
    r
}

fn mean_sd(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1.0);
    (m, var.sqrt())
}

// REQ: OA-SIM-002
#[test]
fn noise_matches_each_profile() {
    let truth = truth_of(&stationary_field(20_000.0));
    for p in PROFILES {
        let (plots, _) = run(&truth, &site(clean(p)), 11);
        let n = plots_len(&plots);
        assert!(n > 10_000, "{n}");
        let dr: Vec<f64> = (0..n)
            .map(|i| plots.range_m[i] - plots.truth_range_m[i])
            .collect();
        let da: Vec<f64> = (0..n)
            .map(|i| az_diff(plots.az_deg[i], plots.truth_az_deg[i]))
            .collect();
        let (ma, sa) = mean_sd(&da);
        assert!(
            (sa / p.az_sigma_deg - 1.0).abs() < 0.04,
            "{} azimuth sigma {sa} vs {}",
            p.name,
            p.az_sigma_deg
        );
        assert!(ma.abs() < 5.0 * p.az_sigma_deg / (n as f64).sqrt(), "{ma}");
        if p.range_quantum_m == 0.0 {
            let (mr, sr) = mean_sd(&dr);
            assert!(
                (sr / p.range_sigma_m - 1.0).abs() < 0.04,
                "{} range sigma {sr} vs {}",
                p.name,
                p.range_sigma_m
            );
            assert!(mr.abs() < 5.0 * p.range_sigma_m / (n as f64).sqrt(), "{mr}");
        }
    }
}

// REQ: OA-SIM-002
#[test]
fn profiles_carry_the_tabulated_values() {
    assert_eq!(MSSR_EUROCONTROL_1997.range_sigma_m, 70.0);
    assert_eq!(MSSR_EUROCONTROL_1997.az_sigma_deg, 0.08);
    assert_eq!(MODE_S_ATC302.range_sigma_m, 7.6);
    assert!((MODE_S_ATC302.az_sigma_deg - 0.057).abs() < 1e-3);
    assert_eq!(SSR_CD2_ATC302.range_sigma_m, 30.5);
    assert_eq!(SSR_CD2_ATC302.range_quantum_m, NM_M / 8.0);
    assert!((SSR_CD2_ATC302.az_sigma_deg - 3.0e-3_f64.to_degrees()).abs() < 1e-12);
    assert_eq!(SCAN_TERMINAL_S, 4.8);
    assert_eq!(SCAN_EN_ROUTE_S, 12.0);
    assert_eq!(MSSR_EUROCONTROL_1997.alt_quantum_ft, 100);
    assert_eq!(MODE_S_ATC302.alt_quantum_ft, 25);
    assert_eq!(MSSR_EUROCONTROL_1997.pd, 0.97);
    assert_eq!(MSSR_EUROCONTROL_1997.false_rate, 0.001);
    assert_eq!(MSSR_EUROCONTROL_1997.split_rate, 0.003);
    assert_eq!(MSSR_EUROCONTROL_1997.pd_mode_a, 0.98);
    assert_eq!(MSSR_EUROCONTROL_1997.pd_alt, 0.96);
    assert_eq!(profile_by_name("MODE_S_ATC302"), Some(MODE_S_ATC302));
    assert_eq!(profile_by_name("nope"), None);
}

// REQ: OA-SIM-003
#[test]
fn bias_is_applied_exactly() {
    let truth = truth_of(&stationary_field(300.0));
    let zero = RadarProfile {
        range_sigma_m: 0.0,
        az_sigma_deg: 0.0,
        ..clean(MSSR_EUROCONTROL_1997)
    };
    let mut s = site(zero);
    s.bias_range_m = 123.4;
    s.bias_az_deg = -0.25;
    let (plots, _) = run(&truth, &s, 3);
    assert!(plots_len(&plots) > 100);
    for i in 0..plots_len(&plots) {
        assert!((plots.range_m[i] - plots.truth_range_m[i] - 123.4).abs() < 1e-9);
        assert!((az_diff(plots.az_deg[i], plots.truth_az_deg[i]) + 0.25).abs() < 1e-9);
    }
}

// REQ: OA-SIM-004
#[test]
fn scan_time_is_exact_for_a_stationary_target() {
    let mut r = Reports::default();
    let (lat, lon) = offset_deg(RLAT, RLON, 30_000.0, 20_000.0);
    track(&mut r, 0xa11111, (lat, lon), (0.0, 0.0), 10_000, 200.0, 2.0);
    let truth = truth_of(&r);
    let s = site(clean(MSSR_EUROCONTROL_1997));
    let (plots, _) = run(&truth, &s, 1);
    assert!(plots_len(&plots) > 30);
    for i in 0..plots_len(&plots) {
        let k = plots.scan[i];
        let frac = ((plots.truth_az_deg[i] - s.initial_az_deg).rem_euclid(360.0)) / 360.0;
        let want = scan_start_ns(&s, k) + (frac * period_ns(&s) as f64).round() as i64;
        assert_eq!(plots.time_ns[i], want);
        // And the beam is at the target's azimuth at that instant.
        assert!(az_diff(beam_az_deg(&s, plots.time_ns[i]), plots.truth_az_deg[i]).abs() < 1e-6);
        assert!(plots.time_ns[i] - scan_start_ns(&s, k) < period_ns(&s));
    }
    // Consecutive plots are one period apart.
    for w in plots.time_ns.windows(2) {
        assert_eq!(w[1] - w[0], period_ns(&s));
    }
}

// REQ: OA-SIM-004
#[test]
fn scan_time_accounts_for_target_motion() {
    // 250 m/s crossing 25 km abeam of the radar: about 0.57 degree per second.
    let mut r = Reports::default();
    let (lat, lon) = offset_deg(RLAT, RLON, 25_000.0, -40_000.0);
    track(
        &mut r,
        0xa22222,
        (lat, lon),
        (0.0, 250.0),
        20_000,
        300.0,
        1.0,
    );
    let truth = truth_of(&r);
    let s = site(clean(MSSR_EUROCONTROL_1997));
    let (plots, st) = run(&truth, &s, 1);
    assert!(plots_len(&plots) > 30, "{st:?}");
    let frame = frame_at(RLAT, RLON, 30.0);
    let mut moved_from_naive = 0;
    for i in 0..plots_len(&plots) {
        let t = plots.time_ns[i];
        // Independent check through the public truth API: the beam is where the aircraft is.
        let TruthQuery::Ok(q) = truth_at(&truth, 0xa22222, t / 1_000_000) else {
            panic!("no truth at a plot time")
        };
        let (_, az) = polar_of(&frame, q.lat_deg, q.lon_deg, q.baro_alt_ft.unwrap() * FT_M);
        assert!(
            az_diff(beam_az_deg(&s, t), az).abs() < 2e-3,
            "{}",
            az_diff(beam_az_deg(&s, t), az)
        );
        assert!(t >= scan_start_ns(&s, plots.scan[i]));
        assert!(t < scan_start_ns(&s, plots.scan[i] + 1));
        // The crossing is not where a frozen-target solve would put it.
        let frac = ((plots.truth_az_deg[i] - s.initial_az_deg).rem_euclid(360.0)) / 360.0;
        let naive_az_at_start = {
            let ts = scan_start_ns(&s, plots.scan[i]);
            let TruthQuery::Ok(q0) = truth_at(&truth, 0xa22222, ts / 1_000_000) else {
                continue;
            };
            polar_of(&frame, q0.lat_deg, q0.lon_deg, 6000.0).1
        };
        let naive = scan_start_ns(&s, plots.scan[i])
            + (((naive_az_at_start - s.initial_az_deg).rem_euclid(360.0)) / 360.0
                * period_ns(&s) as f64) as i64;
        let _ = frac;
        if (naive - t).abs() > 1_000_000 {
            moved_from_naive += 1;
        }
    }
    assert!(moved_from_naive > 20);
}

// REQ: OA-SIM-005
#[test]
fn detection_false_and_split_rates_match_the_profile() {
    let truth = truth_of(&stationary_field(15_000.0));
    let (plots, st) = run(&truth, &site(MSSR_EUROCONTROL_1997), 5);
    let n = st.in_coverage as f64;
    assert!(n > 20_000.0);
    let binom = |p: f64, x: u64| {
        let sd = (p * (1.0 - p) / n).sqrt();
        assert!(
            (x as f64 / n - p).abs() < 4.5 * sd,
            "rate {} vs {p} (4.5 sigma {})",
            x as f64 / n,
            4.5 * sd
        );
    };
    binom(0.97, st.detected);
    let det = st.detected as f64;
    // False and split rates are per true plot.
    let sd = |p: f64| (p * (1.0 - p) / det).sqrt();
    assert!((st.false_plots as f64 / det - 0.001).abs() < 4.5 * sd(0.001));
    assert!((st.split_plots as f64 / det - 0.003).abs() < 4.5 * sd(0.003));
    let kinds = |k: u8| plots.kind.iter().filter(|&&x| x == k).count() as u64;
    assert_eq!(kinds(KIND_TRUE), st.detected);
    assert_eq!(kinds(KIND_FALSE), st.false_plots);
    assert_eq!(kinds(KIND_SPLIT), st.split_plots);
    // Mode A and altitude presence.
    let with_a = plots
        .mode_a
        .iter()
        .zip(&plots.kind)
        .filter(|(&c, &k)| k == KIND_TRUE && c != NONE_CODE)
        .count();
    let pa = with_a as f64 / det;
    assert!((pa - 0.98).abs() < 4.5 * sd(0.98), "{pa}");
    let with_c = plots
        .alt_ft
        .iter()
        .zip(&plots.kind)
        .filter(|(&c, &k)| k == KIND_TRUE && c != NONE_ALT)
        .count();
    let pc = with_c as f64 / det;
    assert!((pc - 0.96).abs() < 4.5 * sd(0.96), "{pc}");
}

// REQ: OA-SIM-005
#[test]
fn false_plots_fill_the_coverage_and_split_plots_are_offset() {
    let truth = truth_of(&stationary_field(6000.0));
    let mut p = clean(MSSR_EUROCONTROL_1997);
    p.false_rate = 0.5;
    p.split_rate = 0.5;
    let s = site(p);
    let (plots, _) = run(&truth, &s, 8);
    let mut inner = 0usize;
    let mut nf = 0usize;
    for i in 0..plots_len(&plots) {
        match plots.kind[i] {
            KIND_FALSE => {
                nf += 1;
                assert_eq!(plots.truth_addr[i], NONE_ADDR);
                assert_eq!(plots.addr[i], NONE_ADDR);
                assert!(plots.range_m[i] <= s.coverage_range_m);
                assert!((0.0..360.0).contains(&plots.az_deg[i]));
                // The time is when the beam is at that azimuth.
                assert!(az_diff(beam_az_deg(&s, plots.time_ns[i]), plots.az_deg[i]).abs() < 1e-6);
                // Uniform in area: half the plots inside r = R / sqrt(2).
                if plots.range_m[i] < s.coverage_range_m / 2f64.sqrt() {
                    inner += 1;
                }
            }
            KIND_SPLIT => {
                // Offset from the preceding true plot of the same aircraft and scan.
                let j = (0..plots_len(&plots))
                    .find(|&j| {
                        plots.kind[j] == KIND_TRUE
                            && plots.truth_addr[j] == plots.truth_addr[i]
                            && plots.scan[j] == plots.scan[i]
                    })
                    .unwrap();
                assert!(((plots.range_m[i] - plots.range_m[j]).abs() - 400.0).abs() < 1e-6);
                assert_eq!(plots.time_ns[i], plots.time_ns[j]);
                assert!(plots.range_m[i] != plots.range_m[j]);
            }
            _ => {}
        }
    }
    assert!(nf > 2000);
    let frac = inner as f64 / nf as f64;
    assert!((frac - 0.5).abs() < 0.04, "{frac}");
}

// REQ: OA-SIM-007
#[test]
fn altitude_is_quantised_to_the_profile() {
    let truth = truth_of(&stationary_field(200.0));
    for (p, q) in [
        (clean(MSSR_EUROCONTROL_1997), 100),
        (clean(MODE_S_ATC302), 25),
    ] {
        let (plots, _) = run(&truth, &site(p), 2);
        assert!(plots_len(&plots) > 50);
        for &a in &plots.alt_ft {
            assert_eq!(a % q, 0);
            assert!((a - 35_037).abs() <= q / 2);
        }
        // 35 037 ft: Mode C gives 35 000, Mode S 35 025.
        assert_eq!(plots.alt_ft[0], if q == 100 { 35_000 } else { 35_025 });
    }
}

// REQ: OA-SIM-007
#[test]
fn cd2_range_is_on_the_eighth_nautical_mile_grid() {
    let truth = truth_of(&stationary_field(200.0));
    let (plots, _) = run(&truth, &site(clean(SSR_CD2_ATC302)), 2);
    let q = NM_M / 8.0;
    for &r in &plots.range_m {
        let k = r / q;
        assert!((k - k.round()).abs() < 1e-9);
    }
}

// REQ: OA-SIM-006
#[test]
fn coverage_range_and_horizon_cut_off() {
    // Antenna 30 m, aircraft 10 000 ft: horizon about 4.12 (sqrt 30 + sqrt 3048) = 250 km.
    let mut r = Reports::default();
    let place = |km: f64| offset_deg(RLAT, RLON, km * 1000.0, 0.0);
    track(
        &mut r,
        0xb00001,
        place(225.0),
        (0.0, 0.0),
        10_000,
        100.0,
        2.0,
    );
    track(
        &mut r,
        0xb00002,
        place(275.0),
        (0.0, 0.0),
        10_000,
        100.0,
        2.0,
    );
    track(
        &mut r,
        0xb00003,
        place(100.0),
        (0.0, 0.0),
        10_000,
        100.0,
        2.0,
    );
    let truth = truth_of(&r);
    let mut s = site(clean(MSSR_EUROCONTROL_1997));
    s.coverage_range_m = 400_000.0;
    let (plots, st) = run(&truth, &s, 1);
    let count = |a: u32| plots.truth_addr.iter().filter(|&&x| x == a).count();
    assert!(count(0xb00001) > 15, "inside the horizon");
    assert_eq!(count(0xb00002), 0, "past the horizon");
    assert!(count(0xb00003) > 15);
    assert!(st.below_horizon > 15);
    // A shorter coverage range cuts by range, not horizon.
    s.coverage_range_m = 150_000.0;
    let (plots, st) = run(&truth, &s, 1);
    assert_eq!(
        plots.truth_addr.iter().filter(|&&x| x == 0xb00001).count(),
        0
    );
    assert!(st.out_of_range > 30);
    // With a flatter effective earth (k = 1) the 225 km target is below the horizon.
    s.coverage_range_m = 400_000.0;
    s.earth_k = 1.0;
    let (plots, _) = run(&truth, &s, 1);
    assert_eq!(
        plots.truth_addr.iter().filter(|&&x| x == 0xb00001).count(),
        0
    );
}

// REQ: OA-SIM-008
#[test]
fn cat048_decodes_back_to_the_plots() {
    let truth = truth_of(&stationary_field(300.0));
    let mut p = MODE_S_ATC302;
    p.false_rate = 0.05;
    let s = site(p);
    let (plots, _) = run(&truth, &s, 4);
    let bytes = to_cat048(&plots, oa_asterix::SourceId { sac: 7, sic: 9 }).unwrap();
    let blocks = decode_stream(&bytes).unwrap();
    // One block per scan (each scan has well under the record limit).
    let mut scans: Vec<i64> = plots.scan.clone();
    scans.dedup();
    assert_eq!(blocks.len(), scans.len());
    let mut i = 0;
    for b in blocks {
        let Block::Cat048(recs) = b else {
            panic!("not 048")
        };
        for rec in recs {
            assert_eq!(rec.source.sac, 7);
            assert_eq!(rec.source.sic, 9);
            let rho = f64::from(rec.polar.unwrap().rho) / 256.0 * NM_M;
            let th = f64::from(rec.polar.unwrap().theta) * 360.0 / 65_536.0;
            assert!((rho - plots.range_m[i]).abs() <= NM_M / 512.0 + 1e-6);
            assert!(az_diff(th, plots.az_deg[i]).abs() <= 360.0 / 65_536.0 / 2.0 + 1e-9);
            let tod = f64::from(rec.time_of_day.unwrap()) / 128.0;
            let want = (plots.time_ns[i] as f64 / 1e9).rem_euclid(86_400.0);
            // The time is floored to a millisecond, then rounded to 1/128 s.
            assert!(
                (tod - want).abs() <= 1.0 / 256.0 + 1e-3 + 1e-6,
                "{tod} {want}"
            );
            match rec.mode3a {
                Some(m) => assert_eq!(m.code, plots.mode_a[i]),
                None => assert_eq!(plots.mode_a[i], NONE_CODE),
            }
            match rec.flight_level {
                Some(f) => assert_eq!(i32::from(f.raw) * 25, plots.alt_ft[i]),
                None => assert_eq!(plots.alt_ft[i], NONE_ALT),
            }
            match rec.address {
                Some(a) => assert_eq!(a, plots.addr[i]),
                None => assert_eq!(plots.addr[i], NONE_ADDR),
            }
            let d = rec.descriptor.unwrap();
            assert!(d.simulated);
            assert_eq!(d.typ, if plots.addr[i] == NONE_ADDR { 2 } else { 5 });
            i += 1;
        }
    }
    assert_eq!(i, plots_len(&plots));
}

// REQ: OA-SIM-009
#[test]
fn same_seed_same_bytes_and_streams_are_per_aircraft() {
    let truth = truth_of(&stationary_field(300.0));
    let s = site(MSSR_EUROCONTROL_1997);
    let (a, _) = run(&truth, &s, 77);
    let (b, _) = run(&truth, &s, 77);
    assert_eq!(a, b);
    let src = oa_asterix::SourceId { sac: 1, sic: 2 };
    assert_eq!(to_cat048(&a, src).unwrap(), to_cat048(&b, src).unwrap());
    let (c, _) = run(&truth, &s, 78);
    assert_ne!(a, c);
    // Dropping the other aircraft does not change one aircraft's plots.
    let mut one = Reports::default();
    let brg = 20.0f64;
    let (lat, lon) = offset_deg(
        RLAT,
        RLON,
        40_000.0 * brg.to_radians().sin(),
        40_000.0 * brg.to_radians().cos(),
    );
    track(
        &mut one,
        0xa00000,
        (lat, lon),
        (0.0, 0.0),
        35_037,
        300.0,
        2.0,
    );
    let t1 = truth_of(&one);
    let (solo, _) = run(&t1, &s, 77);
    let pick = |p: &Plots| -> Vec<(i64, u64, u64)> {
        (0..plots_len(p))
            .filter(|&i| p.truth_addr[i] == 0xa00000 && p.kind[i] == KIND_TRUE)
            .map(|i| (p.time_ns[i], p.range_m[i].to_bits(), p.az_deg[i].to_bits()))
            .collect()
    };
    assert!(!pick(&solo).is_empty());
    assert_eq!(pick(&solo), pick(&a));
}

// REQ: OA-SIM-010
#[test]
fn mode_a_and_mode_s_identity() {
    let truth = truth_of(&stationary_field(100.0));
    let (plots, _) = run(&truth, &site(clean(MODE_S_ATC302)), 1);
    let mut codes = std::collections::BTreeMap::new();
    for i in 0..plots_len(&plots) {
        assert_eq!(plots.addr[i], plots.truth_addr[i]);
        let c = codes.entry(plots.truth_addr[i]).or_insert(plots.mode_a[i]);
        assert_eq!(*c, plots.mode_a[i]);
        assert!(!matches!(*c, 0 | 0o7500 | 0o7600 | 0o7700));
    }
    assert_eq!(codes.len(), 8);
    let distinct: std::collections::BTreeSet<_> = codes.values().collect();
    assert_eq!(distinct.len(), 8);
    // MSSR (no Mode S): no address on the plot.
    let (plots, _) = run(&truth, &site(clean(MSSR_EUROCONTROL_1997)), 1);
    assert!(plots.addr.iter().all(|&a| a == NONE_ADDR));
}

// REQ: OA-SIM-011
#[test]
fn invalid_sites_are_refused() {
    let truth = truth_of(&stationary_field(100.0));
    let good = site(MSSR_EUROCONTROL_1997);
    let bad: Vec<(&str, RadarSite)> = vec![
        (
            "lat",
            RadarSite {
                lat_deg: 91.0,
                ..good
            },
        ),
        (
            "period",
            RadarSite {
                rotation_period_s: 0.0,
                ..good
            },
        ),
        (
            "az",
            RadarSite {
                initial_az_deg: 360.0,
                ..good
            },
        ),
        (
            "cov",
            RadarSite {
                coverage_range_m: 1.0e6,
                ..good
            },
        ),
        (
            "k",
            RadarSite {
                earth_k: 0.0,
                ..good
            },
        ),
        (
            "bias",
            RadarSite {
                bias_range_m: f64::NAN,
                ..good
            },
        ),
        (
            "pd",
            RadarSite {
                profile: RadarProfile {
                    pd: 1.5,
                    ..MSSR_EUROCONTROL_1997
                },
                ..good
            },
        ),
    ];
    for (n, s) in bad {
        assert!(
            matches!(
                simulate_radar(&truth, &s, 1, 0, 1),
                Err(SimError::BadSite(_))
            ),
            "{n}"
        );
    }
    // A plot range past the CAT048 element is refused at encoding.
    let mut p = Plots::default();
    oa_sensorsim::plots::plots_push(
        &mut p,
        &oa_sensorsim::plots::Plot {
            time_ns: 0,
            scan: 0,
            kind: KIND_TRUE,
            range_m: 600_000.0,
            az_deg: 0.0,
            alt_ft: NONE_ALT,
            mode_a: NONE_CODE,
            addr: NONE_ADDR,
            truth_addr: NONE_ADDR,
            truth_range_m: 0.0,
            truth_az_deg: 0.0,
        },
    );
    assert!(matches!(
        to_cat048(&p, oa_asterix::SourceId::default()),
        Err(SimError::Encode(_))
    ));
}
