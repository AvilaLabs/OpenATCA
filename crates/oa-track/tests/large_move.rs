//! The large-move hold (RT-239): a report that passes the position gate but lies more than
//! `confirm_move_m` from the track's predicted position is held until a second report agrees.
//! The gate widens with the gap, so after a gap it is no defence against one forged report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_track::{tracker_stats, Measurement, TrackOutput, TrackerConfig, TrackerStats};

const ADDR: u32 = 0xA0_0001;
const FORGED_OFFSET_M: f64 = 2000.0;

/// A steady eastbound track at 200 m/s, `off_n` metres north of it.
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

fn report_of(rng: &mut Rng, t_s: f64, tr: &Truth) -> Measurement {
    measure(
        &frame0(),
        ADDR,
        t_s,
        tr,
        SIGMA_POS,
        Some(SIGMA_VEL),
        true,
        rng,
    )
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

/// Genuine reports at `genuine` seconds, then forged ones `off_n` north of the track at `forged`.
fn world(genuine: &[f64], forged: &[f64], off_n: f64) -> Vec<Measurement> {
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = genuine
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    ms.extend(
        forged
            .iter()
            .map(|&t| report_of(&mut rng, t, &truth(t, off_n))),
    );
    ms.sort_by_key(|m| m.toa_ns);
    ms
}

fn stats_of(cfg: &TrackerConfig, ms: &[Measurement], end_s: f64) -> (TrackerStats, TrackOutput) {
    let (t, out) = run(cfg, ms, end_s);
    (*tracker_stats(&t), out)
}

/// The rows' times and positions: equal means the track's output is the same.
fn rows(o: &TrackOutput) -> (Vec<i64>, Vec<i32>, Vec<i32>) {
    (o.t_ns.clone(), o.lat_e7.clone(), o.lon_e7.clone())
}

fn off() -> TrackerConfig {
    TrackerConfig {
        confirm_moves: false,
        ..big_config()
    }
}

// REQ: OA-TRK-347, OA-TRK-348
#[test]
fn a_single_forged_report_after_a_gap_is_held_and_the_track_is_unchanged() {
    for gap in [5.0, 10.0, 12.0, 20.0, 40.0] {
        let genuine = world(&seconds(0.0, 20.0), &[], 0.0);
        let mut forged = genuine.clone();
        forged.extend(world(&[], &[20.0 + gap], FORGED_OFFSET_M));
        let end = 20.0 + gap + 30.0;
        let (control, control_out) = stats_of(&big_config(), &genuine, end);
        let (s, out) = stats_of(&big_config(), &forged, end);
        println!(
            "gap {gap}: held {} confirmed {} expired {} jump_rejected {} applied {} (control {})",
            s.moves_held,
            s.moves_confirmed,
            s.moves_expired,
            s.jump_rejected,
            s.applied,
            control.applied
        );
        // Never applied: the count of applied reports and every row are the control's.
        assert_eq!(s.applied, control.applied, "gap {gap}");
        assert_eq!(rows(&out), rows(&control_out), "gap {gap}");
        assert_eq!((s.moves_confirmed, s.resyncs), (0, 0), "gap {gap}");
        // The gate refuses it at short gaps; from 10 s (the allowance) it passes the gate and
        // the hold keeps it.
        let held = u64::from(gap >= 10.0);
        assert_eq!(s.moves_held, held, "gap {gap}");
        assert_eq!(s.jump_rejected, 1 - held, "gap {gap}");
        // The same world with the hold off applies it from 10 s (the finding).
        let (open, _) = stats_of(&off(), &forged, end);
        assert_eq!(open.applied, control.applied + held, "gap {gap}");
    }
}

// REQ: OA-TRK-348
#[test]
fn a_genuine_move_after_a_gap_is_confirmed_by_the_second_report() {
    // 20 s of steady flight east, 20 s of silence in which the aircraft turned north, then
    // reports along the new heading.
    let mut rng = Rng(11);
    let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    let after = |t: f64| Truth {
        e: 6_500.0,
        n: 1_800.0 + 200.0 * (t - 40.0),
        ve: 0.0,
        vn: 200.0,
        alt_ft: 35_000.0,
        vrate_fpm: 0.0,
    };
    for t in seconds(40.0, 50.0) {
        ms.push(report_of(&mut rng, t, &after(t)));
    }
    let (s, out) = stats_of(&big_config(), &ms, 55.0);
    println!(
        "genuine turn: held {} confirmed {} expired {} jump_rejected {} resyncs {}",
        s.moves_held, s.moves_confirmed, s.moves_expired, s.jump_rejected, s.resyncs
    );
    assert_eq!(
        (s.moves_held, s.moves_confirmed, s.moves_expired),
        (1, 1, 0)
    );
    assert_eq!((s.jump_rejected, s.resyncs), (0, 0));
    // Both reports were applied: 21 before the gap, 11 after.
    assert_eq!(s.applied, 32);
    // The track is on the new path after the confirmation.
    let i = (0..out.t_ns.len())
        .rev()
        .find(|&i| out.t_ns[i] <= 50_000_000_000 && out.flags[i] & oa_track::O_TICK == 0)
        .unwrap();
    assert!(pos_err(&frame0(), &out, i, &after(50.0)) < 100.0);
}

// REQ: OA-TRK-348
#[test]
fn a_forger_sending_two_agreeing_reports_moves_the_track() {
    // The residual risk, written down: two forged reports a second apart that agree with each
    // other confirm each other. Only a second source (radar) can refute them.
    let genuine = world(&seconds(0.0, 20.0), &[], 0.0);
    let mut forged = genuine.clone();
    forged.extend(world(&[], &[40.0, 41.0], FORGED_OFFSET_M));
    let (s, out) = stats_of(&big_config(), &forged, 60.0);
    assert_eq!((s.moves_held, s.moves_confirmed), (1, 1));
    // The pending report was applied (the filter may then refuse the second, as it does without
    // the hold): the track's row at 40 s is near the forged path (2 km off the true one).
    assert!(s.applied >= 22);
    let i = (0..out.t_ns.len())
        .find(|&i| out.t_ns[i] == 40_000_000_000 && out.flags[i] & oa_track::O_TICK == 0)
        .unwrap();
    let e = pos_err(&frame0(), &out, i, &truth(40.0, FORGED_OFFSET_M));
    assert!(e < 500.0, "{e}");
}

// REQ: OA-TRK-348
#[test]
fn a_report_that_disagrees_replaces_the_pending_one_and_a_stale_one_expires() {
    // Two forged reports 3 km apart: the second replaces the first (newest wins), neither
    // confirms.
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 41.0, &truth(41.0, -2_000.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (s.moves_held, s.moves_confirmed, s.moves_expired),
        (2, 0, 0)
    );
    assert_eq!(s.applied, 21);

    // A forged pair more than the window apart: the first is discarded and counted when the
    // second arrives, and the second is held alone.
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 46.0, &truth(46.0, 2_000.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (s.moves_held, s.moves_confirmed, s.moves_expired),
        (2, 0, 1)
    );
    assert_eq!(s.applied, 21);

    // A report a quarter second after the pending one is not a second report.
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 40.25, &truth(40.25, 2_000.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!((s.moves_held, s.moves_confirmed), (2, 0));
}

// REQ: OA-TRK-348
#[test]
fn a_pending_report_the_track_has_moved_past_is_discarded() {
    // A forged report at 40 s is held; the genuine track reports again at 41 s (a small move):
    // the pending report is older than the track's newest update and can no longer confirm.
    let mut rng = Rng(7);
    let mut ms: Vec<Measurement> = seconds(0.0, 20.0)
        .iter()
        .map(|&t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 41.0, &truth(41.0, 0.0)));
    ms.push(report_of(&mut rng, 42.0, &truth(42.0, 0.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    // Held once, never confirmed; discarded and counted at the first report after the genuine
    // one that superseded it.
    assert_eq!(
        (s.moves_held, s.moves_confirmed, s.moves_expired),
        (1, 0, 1)
    );
    assert_eq!(s.applied, 23);
}

// REQ: OA-TRK-349
#[test]
fn a_move_under_the_threshold_is_applied_exactly_as_without_the_hold() {
    // 500 m off after a gap of 12 s: passes the gate, under 926 m: the hold changes nothing.
    for (gap, off_n) in [(2.0, 0.0), (12.0, 500.0), (12.0, 800.0), (20.0, -600.0)] {
        let mut ms = world(&seconds(0.0, 20.0), &[], 0.0);
        ms.extend(world(&seconds(20.0 + gap, 40.0 + gap), &[], off_n));
        let (a, oa) = stats_of(&big_config(), &ms, 80.0);
        let (b, ob) = stats_of(&off(), &ms, 80.0);
        assert_eq!(rows(&oa), rows(&ob), "gap {gap} off {off_n}");
        assert_eq!(oa.flags, ob.flags);
        assert_eq!(
            (a.moves_held, a.moves_confirmed, a.moves_expired),
            (0, 0, 0)
        );
        assert_eq!(a.applied, b.applied);
    }
}

// REQ: OA-TRK-350
#[test]
fn the_hold_has_defaults_that_are_on_and_checked() {
    let d = TrackerConfig::default();
    assert_eq!(d.confirm_move_m, 926.0);
    assert_eq!(d.confirm_agree_m, 300.0);
    assert_eq!(d.confirm_window_ns, 5_000_000_000);
    assert!(d.confirm_moves);
    let bad = |f: &dyn Fn(&mut TrackerConfig)| {
        let mut c = d;
        f(&mut c);
        oa_track::check_config(&c).is_err()
    };
    // Zero, negative or not a number is refused, never "off".
    assert!(bad(&|c| c.confirm_move_m = 0.0));
    assert!(bad(&|c| c.confirm_move_m = -1.0));
    assert!(bad(&|c| c.confirm_move_m = f64::NAN));
    assert!(bad(&|c| c.confirm_move_m = f64::INFINITY));
    assert!(bad(&|c| c.confirm_agree_m = 0.0));
    assert!(bad(&|c| c.confirm_agree_m = c.confirm_move_m));
    assert!(bad(&|c| c.confirm_window_ns = 500_000_000));
    assert!(bad(&|c| c.confirm_window_ns = 0));
    assert!(!bad(&|c| c.confirm_move_m = 2_000.0));
    assert!(!bad(&|c| c.confirm_moves = false));
}
