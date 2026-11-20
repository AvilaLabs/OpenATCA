//! The two optional ways to take a track back after a gap: a manoeuvre allowance in the
//! position gate and a re-anchor after a run of rejected reports that agree with each other.
//! Both are off by default; these tests show what each does with forged reports.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_track::{tracker_stats, Measurement, TrackerConfig, TrackerStats};

const ADDR: u32 = 0xA0_0001;
const FORGED_OFFSET_M: f64 = 2000.0;

/// A steady eastbound track at 200 m/s, one report per second, with good NACp and NACv.
fn truth(t_s: f64, off_n: f64) -> Truth {
    Truth {
        e: 200.0 * t_s,
        n: off_n,
        ve: 200.0,
        vn: 0.0,
        alt_ft: 35_000.0,
        vrate_fpm: 0.0,
    }
}

fn report(rng: &mut Rng, t_s: f64, off_n: f64) -> Measurement {
    measure(
        &frame0(),
        ADDR,
        t_s,
        &truth(t_s, off_n),
        SIGMA_POS,
        Some(SIGMA_VEL),
        true,
        rng,
    )
}

/// Genuine reports at `genuine` seconds and forged ones (2 km north of the track) at `forged`.
fn world(genuine: &[f64], forged: &[f64]) -> Vec<Measurement> {
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = genuine.iter().map(|&t| report(&mut rng, t, 0.0)).collect();
    ms.extend(forged.iter().map(|&t| report(&mut rng, t, FORGED_OFFSET_M)));
    ms.sort_by_key(|m| m.toa_ns);
    ms
}

fn seconds(a: f64, b: f64) -> Vec<f64> {
    let mut v = Vec::new();
    let mut t = a;
    while t <= b {
        v.push(t);
        t += 1.0;
    }
    v
}

fn variants() -> Vec<(&'static str, TrackerConfig)> {
    let with = |a: f64, k: u8| TrackerConfig {
        gate_manoeuvre_mps2: a,
        resync_consistent_k: k,
        ..big_config()
    };
    vec![
        ("default", with(0.0, 0)),
        ("A 1.5", with(1.5, 0)),
        ("A 3", with(3.0, 0)),
        ("A 5", with(5.0, 0)),
        ("B 2", with(0.0, 2)),
        ("B 3", with(0.0, 3)),
        ("C A3+B2", with(3.0, 2)),
    ]
}

fn stats_of_run(cfg: &TrackerConfig, ms: &[Measurement]) -> TrackerStats {
    let (t, _) = run(cfg, ms, 120.0);
    *tracker_stats(&t)
}

// REQ: OA-TRK-341, OA-TRK-342
#[test]
fn a_single_forged_report_two_kilometres_off_is_refused_by_every_variant() {
    let ms = world(&seconds(0.0, 40.0), &[20.5]);
    for (name, cfg) in variants() {
        let s = stats_of_run(&cfg, &ms);
        println!(
            "single forged: {name}: jump_rejected {} resyncs {} applied {}",
            s.jump_rejected, s.resyncs, s.applied
        );
        assert_eq!((s.jump_rejected, s.resyncs), (1, 0), "{name}");
    }
}

// REQ: OA-TRK-342
#[test]
fn consistent_forged_reports_straight_after_a_report_cannot_re_anchor_the_track() {
    // Four forged reports a second apart, agreeing with each other and 2 km off: more than the
    // aircraft can fly in the time since its last report, so the hard limit refuses the
    // re-anchor whatever the variant (the five-in-a-row rule is not reached).
    let ms = world(&seconds(0.0, 40.0), &seconds(20.2, 23.2));
    for (name, cfg) in variants() {
        let s = stats_of_run(&cfg, &ms);
        println!(
            "forged run of 4: {name}: jump_rejected {} resyncs {} applied {}",
            s.jump_rejected, s.resyncs, s.applied
        );
        assert_eq!((s.jump_rejected, s.resyncs), (4, 0), "{name}");
    }
}

// REQ: OA-TRK-342
#[test]
fn consistent_forged_reports_after_a_short_gap_are_still_refused_by_the_hard_limit() {
    // 8 s of silence, then two forged reports a second apart, 2 km north: both fail the gate,
    // and the aircraft could not have flown to them (2.7 km in 9 s at a ground speed of 388 kt),
    // so no variant re-anchors.
    let ms = world(&seconds(0.0, 20.0), &[28.0, 29.0]);
    for (name, cfg) in variants() {
        let s = stats_of_run(&cfg, &ms);
        println!(
            "gap 8 s then 2 forged: {name}: jump_rejected {} resyncs {}",
            s.jump_rejected, s.resyncs
        );
        assert_eq!((s.jump_rejected, s.resyncs), (2, 0), "{name}");
    }
}

// REQ: OA-TRK-341
#[test]
fn the_allowance_lets_a_forged_report_through_only_after_a_long_gap() {
    // One forged report 2 km north of the track `gap` seconds after the last report: the number
    // of rejections (1 refused, 0 accepted) per variant. The default gate already accepts it
    // from a 12 s gap (the turn model's covariance has grown to cover 2 km); the allowance moves
    // that to 10 s; at 8 s and less every variant refuses it.
    for gap in [2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 16.0] {
        let mut rng = Rng(7);
        let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
            .iter()
            .map(|&t| report(&mut rng, t, 0.0))
            .collect();
        ms.push(report(&mut rng, 20.0 + gap, FORGED_OFFSET_M));
        let mut row = Vec::new();
        for (name, cfg) in variants() {
            let rejected = stats_of_run(&cfg, &ms).jump_rejected;
            row.push(format!("{name}:{rejected}"));
            if gap <= 8.0 {
                assert_eq!(rejected, 1, "gap {gap} {name}");
            }
            if name == "default" {
                assert_eq!(rejected, u64::from(gap < 12.0), "gap {gap}");
            }
            if name.starts_with("A ") && gap == 10.0 {
                assert_eq!(rejected, 0, "gap {gap} {name}");
            }
        }
        println!("single forged after a gap of {gap} s: {}", row.join(" "));
    }
}

// REQ: OA-TRK-401
#[test]
fn the_first_gap_that_lets_a_forged_report_through_the_shipped_gate_moves_with_the_lag() {
    // The shipped default gate (the manoeuvre allowance of 5 m/s^2 on) and the one forged report
    // 2 km north, after 8 to 12 s of silence, with reports whose velocity is current (lag 0: the
    // behaviour before OA-TRK-401, bit for bit) and with those of a transponder (lag 1 s, ADS-B's).
    // The first gap that lets the report through goes from 10 s to 9 s. The mechanism: the turn
    // model's extra freedom, its turn rate, widens its covariance after a velocity update that
    // also updates the turn rate (the velocity measurement now carries the rate), and the gate is
    // that covariance's.
    let mut first = Vec::new();
    for lag in [0.0, 1.0] {
        let mut row = Vec::new();
        for gap in [8.0, 9.0, 10.0, 11.0, 12.0] {
            let mut rng = Rng(7);
            let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
                .iter()
                .map(|&t| report(&mut rng, t, 0.0))
                .collect();
            ms.push(report(&mut rng, 20.0 + gap, FORGED_OFFSET_M));
            for m in ms.iter_mut() {
                m.vel_lag_s = lag;
            }
            let rejected = stats_of_run(&TrackerConfig::default(), &ms).jump_rejected;
            row.push((gap, rejected));
        }
        println!("lag {lag} s: (gap s, refused): {row:?}");
        first.push(row.iter().find(|r| r.1 == 0).map(|r| r.0));
    }
    assert_eq!(first, [Some(10.0), Some(9.0)]);
}

// REQ: OA-TRK-341, OA-TRK-342, OA-TRK-346
#[test]
fn the_options_have_their_defaults_and_are_checked() {
    let d = TrackerConfig::default();
    // The manoeuvre allowance is on by default (A5, OA-TRK-346), the consistent-run resync off.
    assert_eq!((d.gate_manoeuvre_mps2, d.resync_consistent_k), (5.0, 0));
    // The other defaults of OA-TRK-346: gate 400, the steady-flight velocity noise.
    assert_eq!(d.vel_gate_chi2, 400.0);
    assert_eq!(
        oa_track::params::NACV_VEL_SIGMA_MPS,
        [f64::INFINITY, 0.6142, 0.5938, 0.5209, 0.707]
    );
    for (a, k, ok) in [
        (0.0, 0u8, true),
        (3.0, 2, true),
        (3.0, 5, true),
        (-1.0, 0, false),
        (f64::NAN, 0, false),
        (f64::INFINITY, 0, false),
        (0.0, 1, false),
        (0.0, 6, false),
    ] {
        let c = TrackerConfig {
            gate_manoeuvre_mps2: a,
            resync_consistent_k: k,
            ..d
        };
        assert_eq!(oa_track::check_config(&c).is_ok(), ok, "{a} {k}");
    }
}
