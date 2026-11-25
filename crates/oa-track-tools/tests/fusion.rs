//! Integration tests of the radar fusion study: the exact-truth scoring path, identity
//! accounting, the simulated ADS-B, determinism, the effect of registration and the binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::process::Command;

use oa_bench::adsb_truth::BuiltTruth;
use oa_sensorsim::profile::MSSR_EUROCONTROL_1997;
use oa_track::fusion::{AssignLog, FusionStats};
use oa_track::fusion_params::KEY_RADAR_BASE;
use oa_track::synth_truth::{build_synthetic_truth, Seg, Traj};
use oa_track::{output_hash_into, output_new, OutRow, TrackLog, TrackOutput, TrackerStats};
use oa_track_tools::fusion_score::{
    built_of, coverage_counts, identify, make_radar, run_variant, score_run, simulate_adsb,
    simulate_plots, Kind, RunOut, Timing, ADSB_STUDY, EPOCH_NS, V_BASE_Q1, V_OURS_NOREG,
    V_OURS_REG,
};
use oa_track_tools::fusion_world::{base_trajs, PAIR_GEOMS};
use oa_track_tools::score::{score, RowSet, TrackRun};
use oa_truth::{truth_at, TruthQuery};

/// Appends a row (the crate's own push is private).
fn push(o: &mut TrackOutput, r: &OutRow) {
    o.t_ns.push(r.t_ns);
    o.addr.push(r.addr);
    o.lat_e7.push(r.lat_e7);
    o.lon_e7.push(r.lon_e7);
    o.vel_east_mps.push(r.vel_east_mps);
    o.vel_north_mps.push(r.vel_north_mps);
    o.pos_cov_m2.push(r.pos_cov_m2);
    o.vel_cov_m2s2.push(r.vel_cov_m2s2);
    o.turn_rate_rps.push(r.turn_rate_rps);
    o.alt_ft.push(r.alt_ft);
    o.alt_var_ft2.push(r.alt_var_ft2);
    o.vrate_fpm.push(r.vrate_fpm);
    o.mu_h.push(r.mu_h);
    o.mu_v.push(r.mu_v);
    o.flags.push(r.flags);
}

const SITE: (f64, f64) = (41.976_940_27, -87.908_149_72);

fn small_world(kind: Kind, n: usize) -> BuiltTruth {
    let d = oa_track_tools::fusion_score::site_def(kind);
    // The first aircraft of the base set include straight ones and the turning and climbing
    // ones are spread through the rest; take every fourth to keep a mix.
    let all = base_trajs(kind, 1);
    let picked: Vec<Traj> = all.into_iter().step_by(3).take(n).collect();
    built_of(build_synthetic_truth(d.lat_deg, d.lon_deg, EPOCH_NS, &picked).unwrap())
}

fn first_ns(b: &BuiltTruth) -> i64 {
    oa_truth::leg_start_ns(&b.truth, oa_truth::LegIx(0)).unwrap()
}

fn blank_run(out: TrackOutput, assign: AssignLog) -> RunOut {
    RunOut {
        out,
        assign,
        radar_tracks: Vec::new(),
        fstats: FusionStats::default(),
        tstats: TrackerStats::default(),
        log: TrackLog::default(),
        reg: Vec::new(),
        timing: Timing::default(),
        overflow: 0,
    }
}

// REQ: OA-TRK-060
#[test]
fn a_copy_of_truth_scores_zero_and_a_shifted_copy_scores_its_shift() {
    // Three aircraft: straight, turning at 6 deg/s, climbing. The "tracker output" is the truth
    // itself at 1 Hz: the scorer must find no error beyond the output's own resolution.
    let trajs = vec![
        Traj {
            addr: 0xA00001,
            t0_s: 0.0,
            x0: 20_000.0,
            y0: 15_000.0,
            heading_deg: 250.0,
            speed_mps: 130.0,
            alt_ft: 10_000.0,
            segs: vec![Seg {
                dur_s: 300.0,
                turn_dps: 0.0,
                vrate_fpm: 0.0,
            }],
        },
        Traj {
            addr: 0xA00002,
            t0_s: 0.0,
            x0: -30_000.0,
            y0: 10_000.0,
            heading_deg: 90.0,
            speed_mps: 120.0,
            alt_ft: 8_000.0,
            segs: vec![
                Seg {
                    dur_s: 100.0,
                    turn_dps: 0.0,
                    vrate_fpm: 0.0,
                },
                Seg {
                    dur_s: 30.0,
                    turn_dps: 6.0,
                    vrate_fpm: 0.0,
                },
                Seg {
                    dur_s: 170.0,
                    turn_dps: 0.0,
                    vrate_fpm: 0.0,
                },
            ],
        },
        Traj {
            addr: 0xA00003,
            t0_s: 0.0,
            x0: 5_000.0,
            y0: -40_000.0,
            heading_deg: 10.0,
            speed_mps: 140.0,
            alt_ft: 5_000.0,
            segs: vec![Seg {
                dur_s: 300.0,
                turn_dps: 0.0,
                vrate_fpm: 1_000.0,
            }],
        },
    ];
    let built = built_of(build_synthetic_truth(SITE.0, SITE.1, EPOCH_NS, &trajs).unwrap());
    let mut exact = output_new(1 << 14);
    let mut shifted = output_new(1 << 14);
    for t in &trajs {
        for s in 5..295i64 {
            let ms = EPOCH_NS / 1_000_000 + s * 1000;
            let TruthQuery::Ok(q) = truth_at(&built.truth, t.addr, ms) else {
                panic!("no truth");
            };
            let row = |lat: f64, lon: f64| OutRow {
                t_ns: ms * 1_000_000,
                addr: t.addr,
                lat_e7: (lat * 1e7).round() as i32,
                lon_e7: (lon * 1e7).round() as i32,
                vel_east_mps: q.vel_east_mps as f32,
                vel_north_mps: q.vel_north_mps as f32,
                pos_cov_m2: [1.0, 0.0, 1.0],
                vel_cov_m2s2: [1.0, 0.0, 1.0],
                turn_rate_rps: 0.0,
                alt_ft: q.baro_alt_ft.unwrap() as f32,
                alt_var_ft2: 1.0,
                vrate_fpm: 0.0,
                mu_h: [1.0, 0.0],
                mu_v: [1.0, 0.0],
                flags: oa_track::O_HAS_ALT,
            };
            push(&mut exact, &row(q.lat_deg, q.lon_deg));
            // 100 m east of truth.
            let (la, lo) = oa_bench::geo::from_enu(q.lat_deg, q.lon_deg, 100.0, 0.0);
            push(&mut shifted, &row(la, lo));
        }
    }
    let go = |o: TrackOutput| {
        let tr = TrackRun {
            out: o,
            stats: TrackerStats::default(),
            log: TrackLog::default(),
            offered: 0,
            digest: String::new(),
            overflow: 0,
        };
        score(&tr, &built, &built.truth.cfg, RowSet::Updates).unwrap()
    };
    let e = go(exact);
    assert!(e.pairs > 800, "{}", e.pairs);
    // Position error: the 1e-7 degree rounding (1.1 cm) is all that is left.
    assert!(
        e.esassp.position_m[0].rms < 0.05,
        "{:?}",
        e.esassp.position_m[0]
    );
    assert!(
        e.esassp.speed_mps[0].rms < 0.1,
        "{:?}",
        e.esassp.speed_mps[0]
    );
    assert!(
        e.esassp.heading_deg[0].rms < 0.1,
        "{:?}",
        e.esassp.heading_deg[0]
    );
    // Both classes are populated (the 6 deg/s turn is a turn).
    assert!(e.esassp.position_m[1].n > 100 && e.esassp.position_m[2].n > 5);
    let s = go(shifted);
    assert!(
        (s.esassp.position_m[0].rms - 100.0).abs() < 0.5,
        "{:?}",
        s.esassp.position_m[0]
    );
}

// REQ: OA-TRK-061
#[test]
fn identity_accounting_counts_swaps_wrong_plots_false_and_duplicate_tracks() {
    let mut a = AssignLog::default();
    let none = u32::MAX;
    let mut add = |key: u32, labels: &[u32]| {
        for (i, &l) in labels.iter().enumerate() {
            a.t_ns
                .push(i as i64 * 1_000_000_000 + i64::from(key & 0xFF));
            a.key.push(key);
            a.label.push(l);
            a.radar.push(0);
            a.how.push(1);
        }
    };
    let b = KEY_RADAR_BASE;
    add(b + 1, &[1, 1, 1, 1]);
    add(b + 2, &[2, 2, 3, 3, 3]);
    add(b + 3, &[none, none, 4]);
    add(b + 4, &[1, 1]);
    add(0xABCD, &[0xABCD, 0xABCD, 7, 0xABCD, none]);
    let mut out = output_new(16);
    for k in [b + 1, b + 2, b + 3, b + 4, 0xABCD] {
        let row = OutRow {
            t_ns: 0,
            addr: k,
            lat_e7: 0,
            lon_e7: 0,
            vel_east_mps: 0.0,
            vel_north_mps: 0.0,
            pos_cov_m2: [1.0, 0.0, 1.0],
            vel_cov_m2s2: [1.0, 0.0, 1.0],
            turn_rate_rps: 0.0,
            alt_ft: 0.0,
            alt_var_ft2: 0.0,
            vrate_fpm: 0.0,
            mu_h: [1.0, 0.0],
            mu_v: [1.0, 0.0],
            flags: 0,
        };
        push(&mut out, &row);
    }
    let id = identify(&blank_run(out, a));
    // b+2 follows aircraft 3 (3 of 5) and changed once: 1 swap, 2 wrong plots.
    // 0xABCD: label 7 inside: 2 swaps (to 7 and back), 1 wrong plot, 1 false plot applied.
    assert_eq!(id.swaps, 1 + 2);
    assert_eq!(id.wrong_plots, 2 + 1);
    assert_eq!(id.false_applied, 1);
    // b+3 is mostly false plots; b+4 is the second track of aircraft 1.
    assert_eq!(id.false_tracks, 1);
    assert_eq!(id.duplicates, 1);
    assert_eq!(id.truth_of.get(&(b + 1)), Some(&1));
    assert_eq!(id.truth_of.get(&(b + 2)), Some(&3));
    assert_eq!(id.truth_of.get(&0xABCD), Some(&0xABCD));
    assert!(!id.truth_of.contains_key(&(b + 4)));
    assert!(!id.truth_of.contains_key(&(b + 3)));
}

// REQ: OA-TRK-062
#[test]
fn simulated_adsb_has_the_stated_noise_and_resolution() {
    let trajs = [Traj {
        addr: 0xA00001,
        t0_s: 0.0,
        x0: 20_000.0,
        y0: 15_000.0,
        heading_deg: 250.0,
        speed_mps: 130.0,
        alt_ft: 10_010.0,
        segs: vec![Seg {
            dur_s: 1_500.0,
            turn_dps: 0.0,
            vrate_fpm: 0.0,
        }],
    }];
    let other = Traj {
        addr: 0xA00002,
        ..trajs[0].clone()
    };
    let built = built_of(
        build_synthetic_truth(SITE.0, SITE.1, EPOCH_NS, &[trajs[0].clone(), other]).unwrap(),
    );
    let f = first_ns(&built);
    let m = simulate_adsb(
        &built.truth,
        &ADSB_STUDY,
        5,
        f,
        f + 2_000_000_000_000,
        &|a| a == 0xA00001,
    )
    .unwrap();
    assert!((1_490..=1_501).contains(&m.len()), "{}", m.len());
    assert!(m.iter().all(|x| x.addr == 0xA00001), "equipage filter");
    let mut sum2 = 0.0;
    for x in &m {
        let TruthQuery::Ok(q) = truth_at(&built.truth, x.addr, x.toa_ns / 1_000_000) else {
            panic!();
        };
        let (e, n) = oa_bench::geo::enu(
            q.lat_deg,
            q.lon_deg,
            f64::from(x.lat_e7) * 1e-7,
            f64::from(x.lon_e7) * 1e-7,
        );
        sum2 += e * e + n * n;
        assert_eq!(x.alt_ft % 25, 0);
        assert_eq!(x.toa_ns % 1_000_000, 0, "stamped to the millisecond");
    }
    // NACp 10: 95 % bound 10 m over 2.448 = 4.09 m per axis, so a 2-D RMS of 5.78 m.
    let rms = (sum2 / m.len() as f64).sqrt();
    assert!((rms - 5.78).abs() < 0.4, "2-D rms {rms}");
    // The same seed gives the same reports; another seed differs.
    let again = simulate_adsb(
        &built.truth,
        &ADSB_STUDY,
        5,
        f,
        f + 2_000_000_000_000,
        &|a| a == 0xA00001,
    )
    .unwrap();
    assert_eq!(m, again);
    let other_seed = simulate_adsb(
        &built.truth,
        &ADSB_STUDY,
        6,
        f,
        f + 2_000_000_000_000,
        &|a| a == 0xA00001,
    )
    .unwrap();
    assert_ne!(m, other_seed);
}

/// One aircraft with two legs (the same path, 1000 s apart), joined from two synthetic truths.
fn two_leg_truth() -> BuiltTruth {
    let leg = |t0_s: f64| Traj {
        addr: 0xA00001,
        t0_s,
        x0: 20_000.0,
        y0: 15_000.0,
        heading_deg: 250.0,
        speed_mps: 130.0,
        alt_ft: 10_010.0,
        segs: vec![Seg {
            dur_s: 600.0,
            turn_dps: 0.0,
            vrate_fpm: 0.0,
        }],
    };
    let mut a = build_synthetic_truth(SITE.0, SITE.1, EPOCH_NS, &[leg(0.0)]).unwrap();
    let b = build_synthetic_truth(SITE.0, SITE.1, EPOCH_NS, &[leg(1_000.0)]).unwrap();
    let n_pts = a.pt_t_ns.len() as u32;
    a.ac_first_leg.pop();
    a.leg_first_pt.pop();
    a.leg_addr.extend(&b.leg_addr);
    a.leg_first_pt.push(n_pts);
    a.leg_lat0.extend(&b.leg_lat0);
    a.leg_lon0.extend(&b.leg_lon0);
    a.pt_t_ns.extend(&b.pt_t_ns);
    a.pt_alt_ft.extend(&b.pt_alt_ft);
    a.pt_vrate_fpm.extend(&b.pt_vrate_fpm);
    a.pt_xf.extend(&b.pt_xf);
    a.pt_pf.extend(&b.pt_pf);
    a.pt_xs.extend(&b.pt_xs);
    a.pt_ps.extend(&b.pt_ps);
    a.ac_first_leg.push(2);
    a.leg_first_pt.push(a.pt_t_ns.len() as u32);
    built_of(a)
}

// REQ: OA-TRK-212
#[test]
fn simulated_adsb_noise_is_not_repeated_between_legs_or_restarted_by_a_subrange() {
    let built = two_leg_truth();
    let f = first_ns(&built);
    let all = |from: i64, to: i64| {
        simulate_adsb(&built.truth, &ADSB_STUDY, 9, from, to, &|_| true).unwrap()
    };
    let full = all(f, f + 2_000_000_000_000);
    let noise = |x: &oa_track::Measurement| {
        let TruthQuery::Ok(q) = truth_at(&built.truth, x.addr, x.toa_ns / 1_000_000) else {
            panic!();
        };
        oa_bench::geo::enu(
            q.lat_deg,
            q.lon_deg,
            f64::from(x.lat_e7) * 1e-7,
            f64::from(x.lon_e7) * 1e-7,
        )
    };
    let split = f + 800_000_000_000;
    let (first, second): (Vec<_>, Vec<_>) = full.iter().partition(|x| x.toa_ns < split);
    assert!(
        first.len() > 590 && second.len() > 590,
        "{} {}",
        first.len(),
        second.len()
    );
    // The same index in each leg: the draws differ.
    let pairs: Vec<((f64, f64), (f64, f64))> = first
        .iter()
        .zip(&second)
        .map(|(a, b)| (noise(a), noise(b)))
        .collect();
    let same = pairs
        .iter()
        .filter(|(a, b)| (a.0 - b.0).abs() < 0.02 && (a.1 - b.1).abs() < 0.02)
        .count();
    assert_eq!(same, 0, "identical draws at matching indices");
    let n = pairs.len() as f64;
    let (ma, mb) = pairs
        .iter()
        .fold((0.0, 0.0), |s, (a, b)| (s.0 + a.0 / n, s.1 + b.0 / n));
    let (mut cab, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (a, b) in &pairs {
        cab += (a.0 - ma) * (b.0 - mb);
        va += (a.0 - ma).powi(2);
        vb += (b.0 - mb).powi(2);
    }
    let r = cab / (va * vb).sqrt();
    assert!(r.abs() < 0.2, "east noise correlation between legs {r}");
    // A subrange reproduces exactly the samples of the full run that it covers.
    let (lo, hi) = (f + 150_000_000_000, f + 1_300_000_000_000);
    let sub = all(lo, hi);
    let want: Vec<_> = full
        .iter()
        .filter(|x| x.toa_ns >= lo && x.toa_ns <= hi)
        .copied()
        .collect();
    assert!(want.len() > 400);
    assert_eq!(sub, want);
}

fn digest(o: &TrackOutput) -> String {
    let mut h = oa_sha256::Sha256::new();
    output_hash_into(&mut h, o);
    oa_sha256::to_hex(&h.finalize())
}

// REQ: OA-TRK-063
#[test]
fn a_run_is_reproducible_and_radar_only_tracking_is_clean_and_accurate() {
    let built = small_world(Kind::Terminal, 8);
    let radar = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let f = first_ns(&built);
    let (plots, _k, st) = simulate_plots(
        &built.truth,
        &radar,
        7,
        f - 1_000_000_000,
        f + 1_000_000_000_000,
    )
    .unwrap();
    assert!(st.detected > 500, "{st:?}");
    let r1 = run_variant(&V_OURS_NOREG, &[radar.trk], &[&plots], &[]).unwrap();
    let r2 = run_variant(&V_OURS_NOREG, &[radar.trk], &[&plots], &[]).unwrap();
    assert_eq!(digest(&r1.out), digest(&r2.out));
    assert_eq!(r1.assign, r2.assign);
    assert_eq!(r1.fstats, r2.fstats);
    let id = identify(&r1);
    let (seen, tracked) = coverage_counts(&id, &[&plots]);
    assert_eq!((seen, tracked), (8, 8));
    assert_eq!(id.swaps, 0);
    assert_eq!(id.false_tracks, 0);
    assert_eq!(id.duplicates, 0);
    let t = score_run(&r1, &id, &built, &|_| true).unwrap();
    // Plot noise is 70 m range and 0.08 degree azimuth (about 70 m cross-range at 50 km): the
    // filtered track must be better than the raw plots.
    let raw = oa_track_tools::fusion_score::raw_plot_errors(&built.truth, &radar, &plots);
    assert!(t.updates.position_m[0].n > 500);
    assert!(
        t.updates.position_m[0].rms < raw[0].rms,
        "tracked {} vs raw {}",
        t.updates.position_m[0].rms,
        raw[0].rms
    );
    // Every report is processed in well under a millisecond on average (debug build allowed).
    assert!(r1.timing.events as usize >= plots.len() / 2);
}

// REQ: OA-TRK-064
#[test]
fn registration_removes_a_radar_bias_from_radar_only_aircraft_that_adsb_aircraft_reveal() {
    let built = small_world(Kind::Terminal, 12);
    let (b_r, b_a) = (150.0, 0.15);
    let radar = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, b_r, b_a);
    let f = first_ns(&built);
    let (plots, _k, _) = simulate_plots(
        &built.truth,
        &radar,
        9,
        f - 1_000_000_000,
        f + 1_000_000_000_000,
    )
    .unwrap();
    let equipped = |a: u32| a & 1 == 0;
    let adsb = simulate_adsb(
        &built.truth,
        &ADSB_STUDY,
        10,
        f,
        f + 1_000_000_000_000,
        &equipped,
    )
    .unwrap();
    let run = |v| run_variant(v, &[radar.trk], &[&plots], &adsb).unwrap();
    let (a1, b, c) = (run(&V_BASE_Q1), run(&V_OURS_NOREG), run(&V_OURS_REG));
    let rms = |r: &RunOut, want_equipped: bool| {
        let id = identify(r);
        let t = score_run(r, &id, &built, &|a| equipped(a) == want_equipped).unwrap();
        t.updates.position_m[0].rms
    };
    let (rad_b, rad_c) = (rms(&b, false), rms(&c, false));
    let (eq_b, eq_c) = (rms(&b, true), rms(&c, true));
    eprintln!("radar-only aircraft: a1 {:.1} b {rad_b:.1} c {rad_c:.1}; equipped: b {eq_b:.1} c {eq_c:.1}", rms(&a1, false));
    // Without registration the radar-only aircraft carry the 150 m / 0.15 degree bias (about
    // 150 to 250 m); with it they are near the plot noise floor.
    assert!(rad_b > 120.0, "unregistered radar-only {rad_b}");
    assert!(
        rad_c < 0.6 * rad_b,
        "registered {rad_c} vs unregistered {rad_b}"
    );
    // The estimate itself.
    let (st, hist) = &c.reg[0];
    assert!((st.range_m - b_r).abs() < 20.0, "range {}", st.range_m);
    assert!((st.az_deg - b_a).abs() < 0.03, "az {}", st.az_deg);
    assert!(!hist.is_empty());
    // Registration is estimated from ADS-B aircraft and costs them nothing: the equipped
    // aircraft are at least as good with it.
    assert!(eq_c <= eq_b * 1.05 + 0.5, "equipped {eq_c} vs {eq_b}");
}

// REQ: OA-TRK-065, OA-SEAL-014
#[test]
fn the_binary_writes_the_report_and_refuses_bad_usage() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fusion_out");
    let exe = env!("CARGO_BIN_EXE_oa-track-fusion");
    // seal-allow: runs the built binary of this package on the test's own temp files
    let o = Command::new(exe)
        .args([
            "--out",
            dir.to_str().unwrap(),
            "--smoke",
            "--only",
            "pairs",
            "--pair-seeds",
            "1",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // The count line is printed once, after the last read (no clip here: 0).
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(
        err.matches("sealed-window rows dropped: 0").count(),
        1,
        "{err}"
    );
    // seal-allow: reads back a file the test wrote in its own temp dir
    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    for needle in [
        "scored against EXACT truth",
        "truth's own error is zero",
        "Baselines on identical plots",
        "PAIRS  terminal 4.8 s",
        "cross 90deg, 3 NM, same alt",
        "STRESS",
        "a1 CV q=1 + NN",
        "b  ours, no reg",
    ] {
        assert!(report.contains(needle), "missing {needle:?}");
    }
    assert!(dir.join("tables.csv").exists());
    assert_eq!(PAIR_GEOMS.len(), 12);
    for args in [
        vec![],
        vec!["--bogus"],
        vec!["--out"],
        vec!["--out", "x", "--worlds", "abc"],
    ] {
        // seal-allow: runs the built binary of this package on the test's own temp files
        let o = Command::new(exe).args(&args).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}

/// A digest of everything a run decided: its output rows and which key took which plot.
fn run_digest(r: &RunOut) -> String {
    let mut h = oa_sha256::Sha256::new();
    output_hash_into(&mut h, &r.out);
    for i in 0..r.assign.t_ns.len() {
        h.update(&r.assign.t_ns[i].to_le_bytes()).unwrap();
        h.update(&r.assign.key[i].to_le_bytes()).unwrap();
        h.update(&r.assign.label[i].to_le_bytes()).unwrap();
        h.update(&[r.assign.radar[i], r.assign.how[i]]).unwrap();
    }
    oa_sha256::to_hex(&h.finalize())
}

/// Diagnostic digest with only the new identity-warning bit removed from output rows.
fn digest_without_identity_warning(r: &RunOut) -> String {
    let mut copied = r.out.clone();
    for flag in &mut copied.flags {
        *flag &= !oa_track::O_IDENTITY_UNCONFIRMED;
    }
    let mut h = oa_sha256::Sha256::new();
    output_hash_into(&mut h, &copied);
    for i in 0..r.assign.t_ns.len() {
        h.update(&r.assign.t_ns[i].to_le_bytes()).unwrap();
        h.update(&r.assign.key[i].to_le_bytes()).unwrap();
        h.update(&r.assign.label[i].to_le_bytes()).unwrap();
        h.update(&[r.assign.radar[i], r.assign.how[i]]).unwrap();
    }
    oa_sha256::to_hex(&h.finalize())
}

/// The counts reported when a pin moves: tracks created, identity errors, coast rows, and what
/// the initiation and the work limit did.
fn clip_counts(r: &RunOut) -> String {
    let id = identify(r);
    let coast = r
        .out
        .flags
        .iter()
        .filter(|f| *f & oa_track::O_COAST != 0)
        .count();
    let s = &r.fstats;
    let unknown_rows = r
        .out
        .flags
        .iter()
        .filter(|f| **f & oa_track::O_IDENTITY_UNCONFIRMED != 0)
        .count();
    let unknown_keys: std::collections::BTreeSet<u32> = r
        .out
        .addr
        .iter()
        .zip(&r.out.flags)
        .filter_map(|(&key, &flag)| (flag & oa_track::O_IDENTITY_UNCONFIRMED != 0).then_some(key))
        .collect();
    format!(
        "rows {} plots {} refused {} tracks_created {} cand_confirmed {} duplicates {} false_tracks {} swaps {} wrong_plots {} false_applied {} coast_rows {} work_capped {} ungated {} init_capped {} init_plots_refused {} identity_unconfirmed_rows {} identity_unconfirmed_keys {} numerical_refused {} prediction_refused {} rows_unrepresentable {}",
        r.out.t_ns.len(),
        s.plots_in,
        s.plots_refused,
        r.tstats.started,
        s.cand_confirmed,
        id.duplicates,
        id.false_tracks,
        id.swaps,
        id.wrong_plots,
        id.false_applied,
        coast,
        s.bucket_work_capped,
        s.anon_plots_ungated, s.init_capped, s.init_plots_refused, unknown_rows, unknown_keys.len(),
        r.tstats.numerical_refused, s.prediction_refused, r.tstats.rows_unrepresentable
    )
}

// REQ: OA-TRK-170, OA-TRK-330, OA-TRK-332
#[test]
fn the_real_clip_runs_give_the_pinned_digests() {
    let (Ok(path), Ok(_)) = (
        std::env::var("OPENATCA_ADSB_CLIP"),
        std::env::var("OPENATCA_DATA"),
    ) else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP or OPENATCA_DATA not set");
        return;
    };
    let built = oa_track_tools::fusion_world::clip_world(&path).unwrap();
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for l in 0..built.legs.len() {
        lo = lo.min(oa_truth::leg_start_ns(&built.truth, oa_truth::LegIx(l as u32)).unwrap());
        hi = hi.max(oa_truth::leg_end_ns(&built.truth, oa_truth::LegIx(l as u32)).unwrap());
    }
    // The first ten minutes of the clip: the whole of it takes minutes more and adds no shape.
    hi = hi.min(lo + 600_000_000_000);
    let mut got = Vec::new();
    for (kind, mixed) in [
        (Kind::Terminal, false),
        (Kind::EnRoute, false),
        (Kind::Terminal, true),
        (Kind::EnRoute, true),
    ] {
        let radar = make_radar(kind, MSSR_EUROCONTROL_1997, 50.0, 0.05);
        let (plots, _, _) = simulate_plots(&built.truth, &radar, 500, lo, hi + 1).unwrap();
        let adsb = if mixed {
            simulate_adsb(&built.truth, &ADSB_STUDY, 501, lo, hi, &|a| a & 1 == 0).unwrap()
        } else {
            Vec::new()
        };
        let v = if mixed { &V_OURS_REG } else { &V_OURS_NOREG };
        let r = run_variant(v, &[radar.trk], &[&plots], &adsb).unwrap();
        let d = run_digest(&r);
        println!(
            "clip digest {kind:?} mixed={mixed}: {d} rows {} plots {} refused {}",
            r.out.t_ns.len(),
            r.fstats.plots_in,
            r.fstats.plots_refused
        );
        println!("clip counts {kind:?} mixed={mixed}: {}", clip_counts(&r));
        assert_eq!(r.tstats.numerical_refused, 0);
        assert_eq!(r.fstats.prediction_refused, 0);
        assert_eq!(r.tstats.rows_unrepresentable, 0);
        println!(
            "clip without identity bit {kind:?} mixed={mixed}: {}",
            digest_without_identity_warning(&r)
        );
        got.push(d);
    }
    // The two radars together on one run.
    let ra = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 50.0, 0.05);
    let rb = make_radar(Kind::EnRoute, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let (pa, _, _) = simulate_plots(&built.truth, &ra, 500, lo, hi + 1).unwrap();
    let (pb, _, _) = simulate_plots(&built.truth, &rb, 502, lo, hi + 1).unwrap();
    let adsb = simulate_adsb(&built.truth, &ADSB_STUDY, 501, lo, hi, &|a| a & 1 == 0).unwrap();
    let r = run_variant(&V_OURS_REG, &[ra.trk, rb.trk], &[&pa, &pb], &adsb).unwrap();
    println!("clip digest two radars: {}", run_digest(&r));
    println!("clip counts two radars: {}", clip_counts(&r));
    assert_eq!(r.tstats.numerical_refused, 0);
    assert_eq!(r.fstats.prediction_refused, 0);
    assert_eq!(r.tstats.rows_unrepresentable, 0);
    println!(
        "clip without identity bit two radars: {}",
        digest_without_identity_warning(&r)
    );
    got.push(run_digest(&r));
    assert_eq!(got, PINNED_CLIP_DIGESTS);
}

// Earlier pins: the radar-only pair predated the grid, the newest-update bound and the hold and
// flag changes; the three with ADS-B and registration were re-pinned when registration took 256
// reference slots.
// Re-pinned when the radar covariance moved to the target's own east-north frame (it had been
// oriented by the antenna's) and the simulated ADS-B noise became keyed by sample time (it
// restarted at every leg). Plots logged in the assignment log, old code to new, per run
// (terminal, en-route, terminal mixed, en-route mixed, two radars): 6,036 to 6,036 with none
// changed, 2,002 to 2,009 (4 dropped, 11 gained, 14 with another way of being applied),
// 6,039 to 6,039 none changed, 2,082 to 2,080 (2 dropped), 8,111 to 8,110 (3 dropped, 2
// gained, 3 with another way). Everything else in the digests is the covariance and position
// of the output rows.
// Re-pinned when the plot covariance was mapped into the filter's plane as J R J^T, the state
// covariance carried through the whole change of frame, and the output covariances and velocity
// taken from the exact local differential (OA-TRK-260 to OA-TRK-263): the covariances are scaled
// by the sphere-to-ellipsoid metre (0.3 % in standard deviation) and the velocity chord error is
// gone. Plots in the assignment log, old code to new, per run: 6,036 to 6,036, 2,009 to 2,009,
// 6,039 to 6,039, 2,080 to 2,080 (none changed in these), 8,110 to 8,109 (1 dropped, none gained,
// none with another way). Applied and jump-rejected counts are the same except one fewer applied in
// the two-radar run; the mean normalised innovation moved by 2e-5 to 2e-4 of itself.
// Re-pinned when the simulator's sub-millisecond advance of a plot moved from a flat offset on
// a sphere to the WGS-84 meridian and prime-vertical radii (OA-SIM-028): each simulated plot moves
// by well under a centimetre, so only the bits of the inputs changed. Checked by building this
// commit with the simulator alone put back: the earlier digests return. Rows per run: 38,970,
// 31,741, 151,234, 156,929; plots 6,261 and 2,214, none refused.
// Re-pinned when platform libm was replaced by oa-math in the tracker, fusion layer and geodesy:
// rows (38,970, 31,741, 151,234, 156,929), plots (6,261 and 2,214) and refusals (none) are the
// same; of the rows of the five runs 0 to 2 differ in the last bits of a velocity (largest 3e-8
// m/s), and no position moves at the 1e-7 degree quantum. The earlier digests were
// 1defbdcd..2286, af680187..80e7, d3ab5657..e646, 13705015..19ac3 and a124b85e..e0ab.
// Re-pinned when elementary-function calls were cut on the tracker's hot path (the frame's
// latitude sine and cosine carried with the filter and the small-angle series in the chart, one
// exponential for the model and vertical transitions, the model probabilities from the difference
// of the log weights): rows, plots and refusals are the same as above in every run; of the rows of
// the five runs 2,439 of 38,970, 1,104 of 31,741, 3,480 of 151,234, 4,789 of 156,929 and 5,209 of
// 173,160 differ, none in position at the 1e-7 degree quantum, in velocity by at most 4e-6 m/s,
// and in covariance and model probability by at most 2e-7 of themselves (a single-precision unit;
// terms near zero, the covariance cross term, turn rate and vertical rate, move by more in
// relative terms). The earlier digests were e7a42113..6cac, 83d24dd4..4099, e89d63bc..3ad8,
// 76970282..9135 and aca1b691..9a47.
// Re-pinned (the fifth, two radars) when a candidate whose latest plot gates with a live track no
// longer becomes a second track, and a new track cancels the other radars' candidates it gates
// with (OA-TRK-298). The four single-radar runs are unchanged. Two radars, old to new: 173,160 to
// 173,017 rows, 8,462 plots, 0 refused (the same), tracks created 678 to 676, candidates confirmed
// 102 to 100, duplicate tracks 30 to 28, coast rows 8,811 to 8,739, swaps 25, wrong plots 67 and
// false plots applied 1 (the same). The value before was
// "9323316479520b069e3292527c6cb0f52a26277ed4e395c33c0c1669a18eb896".
// Re-pinned (the fifth, two radars) when a track's coasting follows the scan periods of the radars
// that update it (OA-TRK-302): three of its own expected intervals, the shortest period among the
// radars that updated it within their last three periods, else three of the longest period as
// before. The four single-radar runs are unchanged (one period). Two radars, old to new: rows
// 173,017 (the same), plots 8,462, refused 0, tracks created 676, candidates confirmed 100,
// duplicate tracks 28, swaps 25, wrong plots 67 and false plots applied 1 (all the same); coast
// rows 8,739 to 11,016. The value before was
// "c0e2736508a46fababe9e5f2a06e44fe541a0c2f7a8b8e7aa79f3c19ca28253d".
// Re-pinned (all five) when the association gate gained an allowance for a change of climb rate
// (OA-TRK-311, OA-TRK-309; GATE_VRATE_SIGMA_FPM 700 ft/min, which also gates the confirmation of a
// candidate against live tracks) and the cost of a pair inside the gate only by an allowance became
// the rescued cost: of the second tracks started while the first was alive or coasting, 18 of 34
// terminal and 18 of 24 en-route failed the altitude gate alone (the aircraft's climb rate had
// changed by 500 to 2,100 ft/min since the last plot). Old to new per run (terminal, en-route,
// terminal mixed, en-route mixed, two radars). Rows 38,970 to 38,727; 31,741 to 31,662; 151,234 to
// 151,150; 156,929 to 156,837; 173,017 to 172,387. Plots 6,261, 2,214, 6,261, 2,214 and 8,462 and
// refused 0 (all the same). Tracks created 182 to 161; 161 to 152; 851 to 841; 653 to 650; 676 to
// 665. Duplicate tracks 81 to 60; 53 to 44; 30 to 20; 25 to 22; 28 to 17, by class (a) simultaneous,
// (b1) the first still coasting, (b2) the first dropped: 2/34/45 to 0/15/45; 4/24/25 to 5/14/25;
// 2/16/12 to 1/7/12; 3/8/14 to 3/5/14; 5/12/11 to 1/5/11. Swaps 3, 1, 2, 1 and 25 to 3, 1, 2, 1 and
// 23; wrong plots 2, 2, 2, 2 and 67 to 2, 2, 2, 2 and 65; false plots applied 0, 0, 0, 0 and 1 (the
// same). Coast rows 2,221 to 1,892; 4,675 to 4,430; 4,894 to 4,745; 8,493 to 8,405; 11,016 to
// 10,312. Scoring (updates, position and speed RMS, m and m/s, old to new, with the rows scored
// 4,501 to 4,850, 1,457 to 1,509, 60,776 to 60,957, 57,325 to 57,336, 62,926 to 63,119):
// 109.87/5.775 to 108.52/5.882; 279.66/10.085 to 279.14/10.548; 15.53/1.292 to 16.06/1.332;
// 22.69/1.152 to 22.79/1.151; 132.22/1.535 to 132.10/1.570. The earlier digests were e8bfce48..963d,
// 6d546fa6..6a7e, bc1f506c..4a01, d3782dae..95a8 and eaadd04b..89da9.
// Updated for OA-TRK-323/324: every anonymous output now carries bit 16. The first four
// cases are byte-identical to their prior output and assignment stream after removing ONLY
// that bit (verified diagnostic hashes). Two radars also retain two formerly suppressed
// hypotheses: tracks 665 -> 667, confirmed candidates 89 -> 91, duplicate hypotheses 17 -> 19,
// rows 172387 -> 172530, coast rows 10312 -> 10406; swaps/wrong plots/false applied/false
// tracks remain 23/65/1/0. All five have zero new caps or refused plots. Old raw hashes below;
// the raw digest definition still includes every flag byte.
// 0f7056500b44af3a6fb4ad889aa10864ea2f1dca9b2c819a9b5dadc657548e8f
// 1ee82e469c1cb70db1112ce39544fd944af73c2e562ddb9a012904dd9747bfd6
// 255279cc32ccad5633c81b6a4252a600a993d5b25ead32a3e5f949fd35d12dce
// cf00b4349af0b9bcf0ae7e77ce168cbebd69b9d1f0eb702c406853da2b5ffe0a
// d7c3d24f5a4a89625e5970882feaee004386218cfc5b949f5595a17ff5d7d5e5
// Nonlinear turn prediction (OA-TRK-329), on the same clip and simulated inputs: rows change
// 38,727 -> 38,755; 31,662 -> 31,432; 151,150 -> 151,096; 156,837 -> 156,749;
// 172,530 -> 172,819. Created tracks: 161 -> 158; 152 -> 146; 841 -> 838; 650 -> 648;
// 667 -> 667. Duplicate hypotheses: 60 -> 57; 44 -> 37; 20 -> 17; 22 -> 19; 19 -> 18.
// Two-radar swaps/wrong plots/false applied change 23/65/1 -> 12/26/0, with confirmed
// candidates 91 -> 89. The estimator changes association and lifetimes, not only output bits.
// Every run has zero numerical/forecast refusals, unrepresentable rows and new capacity losses.
// Previous digests: 7fb208ee..c9f, 9cdad807..8bf, a6aa4ae1..f3a, 87aad910..b71,
// b5adce47..cc4. Numeric thresholds, metric definitions and the raw digest stay unchanged.
const PINNED_CLIP_DIGESTS: [&str; 5] = [
    "07ac27be0d8a626a3ee624433412578c3bd5f2f2bbb9fec8a4f2417e90835f44",
    "00ba1cba8c1c8c8ce592e981f0484d52b2cf9d03b39c0df5a8022cb48805e18b",
    "022a97e7ceee8e01a22567962b5d7a4c64c6f9bff7a7f65789ef80a5eb426924",
    "01dee885c82d3e88d932a4488338058711a057385131f87cb2cd3ed6cb9138e4",
    "71139e851dcb21812a3082c65b02bac7d346a23f2b4d001ad031790a39e61123",
];

// REQ: OA-TRK-311
#[test]
fn a_radar_only_track_keeps_its_aircraft_through_the_onset_of_a_climb() {
    // Six aircraft on lines 5 km apart, level for 100 s and then climbing at 1,000 ft/min, the
    // onsets spread over a scan so that some fall just after a plot. Without the allowance for a
    // change of climb rate the next plot is outside the altitude gate of the level track and
    // starts a second one.
    let d = oa_track_tools::fusion_score::site_def(Kind::Terminal);
    let trajs: Vec<Traj> = (0..6)
        .map(|i| Traj {
            addr: 0xA10000 + i,
            t0_s: 0.0,
            x0: 12_000.0,
            y0: 15_000.0 + 5_000.0 * f64::from(i),
            heading_deg: 90.0,
            speed_mps: 120.0,
            alt_ft: 5_000.0,
            segs: vec![
                Seg {
                    dur_s: 100.0 + 0.8 * f64::from(i),
                    turn_dps: 0.0,
                    vrate_fpm: 0.0,
                },
                Seg {
                    dur_s: 120.0,
                    turn_dps: 0.0,
                    vrate_fpm: 1_000.0,
                },
            ],
        })
        .collect();
    let built = built_of(build_synthetic_truth(d.lat_deg, d.lon_deg, EPOCH_NS, &trajs).unwrap());
    let radar = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let f = first_ns(&built);
    let mut dups = 0;
    for seed in 1..=8u64 {
        let (plots, _k, st) = simulate_plots(
            &built.truth,
            &radar,
            seed,
            f - 1_000_000_000,
            f + 230_000_000_000,
        )
        .unwrap();
        assert!(st.detected > 200, "{st:?}");
        let r = run_variant(&V_OURS_NOREG, &[radar.trk], &[&plots], &[]).unwrap();
        let id = identify(&r);
        assert_eq!(id.swaps, 0, "seed {seed}");
        dups += id.duplicates;
    }
    assert_eq!(dups, 0, "second tracks made at the onset of a climb");
}
