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
use oa_track::fusion_score::{
    built_of, coverage_counts, identify, make_radar, run_variant, score_run, simulate_adsb,
    simulate_plots, Kind, RunOut, Timing, ADSB_STUDY, EPOCH_NS, V_BASE_Q1, V_OURS_NOREG,
    V_OURS_REG,
};
use oa_track::fusion_world::{base_trajs, PAIR_GEOMS};
use oa_track::score::{score, RowSet, TrackRun};
use oa_track::synth_truth::{build_synthetic_truth, Seg, Traj};
use oa_track::{output_hash_into, output_new, OutRow, TrackLog, TrackOutput, TrackerStats};
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
    let d = oa_track::fusion_score::site_def(kind);
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
                vrate_fpm: 1_500.0,
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
    );
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
    );
    assert_eq!(m, again);
    let other_seed = simulate_adsb(
        &built.truth,
        &ADSB_STUDY,
        6,
        f,
        f + 2_000_000_000_000,
        &|a| a == 0xA00001,
    );
    assert_ne!(m, other_seed);
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
    let raw = oa_track::fusion_score::raw_plot_errors(&built.truth, &radar, &plots);
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
    );
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

// REQ: OA-TRK-065
#[test]
fn the_binary_writes_the_report_and_refuses_bad_usage() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fusion_out");
    let exe = env!("CARGO_BIN_EXE_oa-track-fusion");
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

// REQ: OA-TRK-170
#[test]
fn the_real_clip_runs_give_the_pinned_digests() {
    let (Ok(path), Ok(_)) = (
        std::env::var("OPENATCA_ADSB_CLIP"),
        std::env::var("OPENATCA_DATA"),
    ) else {
        eprintln!("skipped: OPENATCA_ADSB_CLIP or OPENATCA_DATA not set");
        return;
    };
    let built = oa_track::fusion_world::clip_world(&path).unwrap();
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
            simulate_adsb(&built.truth, &ADSB_STUDY, 501, lo, hi, &|a| a & 1 == 0)
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
        got.push(d);
    }
    // The two radars together on one run.
    let ra = make_radar(Kind::Terminal, MSSR_EUROCONTROL_1997, 50.0, 0.05);
    let rb = make_radar(Kind::EnRoute, MSSR_EUROCONTROL_1997, 0.0, 0.0);
    let (pa, _, _) = simulate_plots(&built.truth, &ra, 500, lo, hi + 1).unwrap();
    let (pb, _, _) = simulate_plots(&built.truth, &rb, 502, lo, hi + 1).unwrap();
    let adsb = simulate_adsb(&built.truth, &ADSB_STUDY, 501, lo, hi, &|a| a & 1 == 0);
    let r = run_variant(&V_OURS_REG, &[ra.trk, rb.trk], &[&pa, &pb], &adsb).unwrap();
    println!("clip digest two radars: {}", run_digest(&r));
    got.push(run_digest(&r));
    assert_eq!(got, PINNED_CLIP_DIGESTS);
}

// The first two (radar only) are the digests of the code before the grid, the newest-update bound
// and the hold and flag changes. The three with ADS-B and registration were re-pinned when
// registration took 256 reference slots kept while their aircraft make residuals (they used
// 2,487, 723 and 2,469 residuals before and use 3,108, 1,008 and 3,107 now).
const PINNED_CLIP_DIGESTS: [&str; 5] = [
    "1cdbbcf99e55c40595ef48cd8560f3c3ac98ebedb805f12d7ce6070e0d30214e",
    "0949ca7ec907774fd73328bbb567e3d2c5b0c8c2cd53af02348cabfca02addd7",
    "90100f80d69750006b206a5df9770b64b7668a8f73a1a92296b1d841428e77e1",
    "b577f949668860e962255391923e328a80e25377c49b1168d130a9b76215d9d2",
    "da355beedf3d921fc87462703f02375632d57a93dabc69a38767315dbb5f1b21",
];
