//! The large-move rule (RT-239): a report that passes the position gate but lies more than
//! `confirm_move_m` from the track's predicted position is applied at once and marked
//! `O_MOVE_UNCONFIRMED` (with a wider radius) until the track's next report confirms it, refutes
//! it (the track is rolled back) or leaves it standing. The gate widens with the gap, so after a
//! gap it is no defence against one forged report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_track::{
    r95_m_of, tracker_stats, Measurement, TrackOutput, TrackerConfig, TrackerStats,
    O_MOVE_UNCONFIRMED, O_TICK,
};

const ADDR: u32 = 0xA0_0001;
const FORGED_OFFSET_M: f64 = 2000.0;
const S: i64 = 1_000_000_000;

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

/// Genuine reports at the given seconds.
fn genuine(rng: &mut Rng, secs: &[f64]) -> Vec<Measurement> {
    secs.iter()
        .map(|&t| report_of(rng, t, &truth(t, 0.0)))
        .collect()
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

fn stats_of(cfg: &TrackerConfig, ms: &[Measurement], end_s: f64) -> (TrackerStats, TrackOutput) {
    let (t, out) = run(cfg, ms, end_s);
    (*tracker_stats(&t), out)
}

fn off() -> TrackerConfig {
    TrackerConfig {
        confirm_moves: false,
        ..big_config()
    }
}

/// The index of the row at `t_ns` that is not a tick (the update row).
fn update_row(o: &TrackOutput, t_ns: i64) -> usize {
    (0..o.t_ns.len())
        .find(|&i| o.t_ns[i] == t_ns && o.flags[i] & O_TICK == 0)
        .unwrap()
}

type Row = (i64, i32, i32, u8, [u32; 3]);

fn rows_after(o: &TrackOutput, t_ns: i64) -> Vec<Row> {
    (0..o.t_ns.len())
        .filter(|&i| o.t_ns[i] > t_ns)
        .map(|i| {
            (
                o.t_ns[i],
                o.lat_e7[i],
                o.lon_e7[i],
                o.flags[i],
                o.pos_cov_m2[i].map(f32::to_bits),
            )
        })
        .collect()
}

// REQ: OA-TRK-347, OA-TRK-348
#[test]
fn a_forged_far_report_is_marked_and_rolled_back_by_the_genuine_one_after_it() {
    for gap in [5.0, 10.0, 12.0, 20.0, 40.0] {
        let mut rng = Rng(7);
        let mut control = genuine(&mut rng, &seconds(0.0, 20.0));
        let mut forged = control.clone();
        let forged_at = 20.0 + gap;
        let forged_report = report_of(&mut rng, forged_at, &truth(forged_at, FORGED_OFFSET_M));
        forged.push(forged_report);
        // Genuine reports a second later and after, on the old path.
        let later = genuine(&mut rng, &seconds(forged_at + 1.0, forged_at + 8.0));
        control.extend(later.iter().copied());
        forged.extend(later.iter().copied());
        let end = forged_at + 30.0;
        let (c, cout) = stats_of(&big_config(), &control, end);
        let (s, out) = stats_of(&big_config(), &forged, end);
        println!(
            "gap {gap}: unconfirmed {} confirmed {} rolled back {} undecided {} jump_rejected {}",
            s.moves_unconfirmed,
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided,
            s.jump_rejected
        );
        // The gate refuses it at short gaps; from 10 s (the allowance) it passes the gate and
        // is applied, marked.
        let moved = u64::from(gap >= 10.0);
        assert_eq!(s.moves_unconfirmed, moved, "gap {gap}");
        assert_eq!(s.moves_rolled_back, moved, "gap {gap}");
        assert_eq!((s.moves_confirmed, s.moves_undecided), (0, 0), "gap {gap}");
        assert_eq!(s.jump_rejected, 1 - moved, "gap {gap}");
        // Without the rule the forged report moves the track (the finding).
        assert_eq!(s.applied, c.applied + moved, "gap {gap}");
        if moved == 1 {
            // The forged report's own row is marked and its radius covers the old path.
            let i = update_row(&out, (forged_at * 1.0e9) as i64);
            // (the filter takes only part of the way to the report: the radius is over the shift).
            assert_ne!(out.flags[i] & O_MOVE_UNCONFIRMED, 0, "gap {gap}");
            assert!(r95_m_of(&out.pos_cov_m2[i]) > 800.0, "gap {gap}");
            // Every row of the track from the move to the rollback carries the mark.
            let t0 = (forged_at * 1.0e9) as i64;
            let t1 = ((forged_at + 1.0) * 1.0e9) as i64;
            for k in 0..out.t_ns.len() {
                if out.t_ns[k] >= t0 && out.t_ns[k] < t1 {
                    assert_ne!(out.flags[k] & O_MOVE_UNCONFIRMED, 0, "gap {gap} row {k}");
                }
            }
            // After the rollback the track is the control's, bit for bit, rows included.
            assert_eq!(rows_after(&out, t1), rows_after(&cout, t1), "gap {gap}");
        } else {
            assert_eq!(
                out.flags
                    .iter()
                    .filter(|&&f| f & O_MOVE_UNCONFIRMED != 0)
                    .count(),
                0
            );
        }
    }
}

// REQ: OA-TRK-348, OA-TRK-349
#[test]
fn a_genuine_reacquisition_is_confirmed_and_its_rows_are_main_s_but_for_the_mark() {
    for gap in [15.0, 30.0, 50.0] {
        let mut rng = Rng(11);
        let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
        let t_back = 20.0 + gap;
        let after = |t: f64| Truth {
            e: 200.0 * t_back,
            n: 1_500.0 + 200.0 * (t - t_back),
            ve: 0.0,
            vn: 200.0,
            alt_ft: 35_000.0,
            vrate_fpm: 0.0,
        };
        for t in seconds(t_back, t_back + 10.0) {
            ms.push(report_of(&mut rng, t, &after(t)));
        }
        let end = t_back + 20.0;
        let (s, out) = stats_of(&big_config(), &ms, end);
        let (m, main_out) = stats_of(&off(), &ms, end);
        println!(
            "gap {gap}: unconfirmed {} confirmed {} rolled back {} undecided {}",
            s.moves_unconfirmed, s.moves_confirmed, s.moves_rolled_back, s.moves_undecided
        );
        assert_eq!(
            (
                s.moves_unconfirmed,
                s.moves_confirmed,
                s.moves_rolled_back,
                s.moves_undecided
            ),
            (1, 1, 0, 0),
            "gap {gap}"
        );
        // Every report is applied exactly as without the rule.
        assert_eq!(s.applied, m.applied);
        assert_eq!((s.jump_rejected, s.resyncs), (m.jump_rejected, m.resyncs));
        // Rows: the same positions at the same times; the flags and covariances are the same
        // except on the rows marked unconfirmed.
        assert_eq!(out.t_ns, main_out.t_ns);
        assert_eq!(out.lat_e7, main_out.lat_e7);
        assert_eq!(out.lon_e7, main_out.lon_e7);
        let mut marked = 0;
        for i in 0..out.t_ns.len() {
            if out.flags[i] & O_MOVE_UNCONFIRMED != 0 {
                marked += 1;
                assert!(r95_m_of(&out.pos_cov_m2[i]) >= r95_m_of(&main_out.pos_cov_m2[i]));
                continue;
            }
            assert_eq!(out.flags[i], main_out.flags[i], "row {i}");
            assert_eq!(
                out.pos_cov_m2[i].map(f32::to_bits),
                main_out.pos_cov_m2[i].map(f32::to_bits)
            );
        }
        assert!(marked >= 1, "gap {gap}");
        // The mark is gone by the end.
        assert_eq!(out.flags.last().unwrap() & O_MOVE_UNCONFIRMED, 0);
    }
}

// REQ: OA-TRK-348
#[test]
fn a_report_after_the_window_or_agreeing_with_neither_leaves_the_move_standing() {
    // The forged report at 40 s; the genuine one on the old path only at 46 s, past the 5 s window
    // of this 1 s feed: undecided, the move stands, the mark is cleared.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    ms.push(report_of(&mut rng, 46.0, &truth(46.0, 0.0)));
    let (s, out) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (
            s.moves_unconfirmed,
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided
        ),
        (1, 0, 0, 1)
    );
    let last_marked = (0..out.t_ns.len())
        .rev()
        .find(|&i| out.flags[i] & O_MOVE_UNCONFIRMED != 0)
        .unwrap();
    assert!(out.t_ns[last_marked] < 46 * S);
    // A report agreeing with neither: 2 km north, then 2 km south.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 41.0, &truth(41.0, -2_000.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (s.moves_confirmed, s.moves_rolled_back, s.moves_undecided),
        (0, 0, 1)
    );
    // A report a quarter second after the move is not a second report: nothing is decided.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, 2_000.0)));
    ms.push(report_of(&mut rng, 40.25, &truth(40.25, 2_000.0)));
    let (s, _) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (s.moves_confirmed, s.moves_rolled_back, s.moves_undecided),
        (0, 0, 0)
    );
}

// REQ: OA-TRK-348
#[test]
fn a_forger_sending_two_agreeing_reports_is_confirmed() {
    // The residual risk, written down: two forged reports a second apart that agree with each
    // other confirm each other. Only a second source (radar) can refute them.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    ms.push(report_of(&mut rng, 41.0, &truth(41.0, FORGED_OFFSET_M)));
    let (s, out) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(
        (s.moves_unconfirmed, s.moves_confirmed, s.moves_rolled_back),
        (1, 1, 0)
    );
    assert_eq!(out.flags.last().unwrap() & O_MOVE_UNCONFIRMED, 0);
}

// REQ: OA-TRK-349
#[test]
fn a_track_that_ends_or_restarts_forgets_its_unconfirmed_move() {
    // Forged at 40 s, then nothing: the track is dropped at the drop age (the rows are marked
    // while it lives), and a new track of the address starts unmarked.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    let (t, _) = run(&big_config(), &ms, 41.0);
    assert_eq!(tracker_stats(&t).moves_unconfirmed, 1);
    let mut ms2 = ms.clone();
    for k in 0..4 {
        let tt = 200.0 + f64::from(k);
        ms2.push(report_of(&mut rng, tt, &truth(tt, 0.0)));
    }
    let (s, out) = stats_of(&big_config(), &ms2, 220.0);
    assert_eq!(s.started, 2);
    assert_eq!(
        s.moves_undecided + s.moves_confirmed + s.moves_rolled_back,
        0
    );
    let first_new =
        (0..out.t_ns.len()).find(|&i| out.t_ns[i] >= 200 * S && out.flags[i] & O_TICK == 0);
    for i in 0..out.t_ns.len() {
        if out.t_ns[i] >= 200 * S {
            assert_eq!(out.flags[i] & O_MOVE_UNCONFIRMED, 0);
        }
    }
    let _ = first_new;
    // A resynchronisation clears it too: forged, then reports at a third place until the filter
    // restarts.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    for k in 0..6 {
        let tt = 41.0 + f64::from(k);
        ms.push(report_of(&mut rng, tt, &truth(tt, -6_000.0)));
    }
    let (s, out) = stats_of(&big_config(), &ms, 60.0);
    assert_eq!(s.moves_undecided, 1);
    assert!(s.resyncs >= 1, "{s:?}");
    assert_eq!(out.flags.last().unwrap() & O_MOVE_UNCONFIRMED, 0);
}

// REQ: OA-TRK-350
#[test]
fn a_move_under_the_threshold_is_applied_exactly_as_without_the_rule() {
    // 500 m off after a gap of 12 s: passes the gate, under 926 m: the rule changes nothing.
    for (gap, off_n) in [(2.0, 0.0), (12.0, 500.0), (12.0, 800.0), (20.0, -600.0)] {
        let mut rng = Rng(7);
        let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
        for t in seconds(20.0 + gap, 40.0 + gap) {
            ms.push(report_of(&mut rng, t, &truth(t, off_n)));
        }
        let (a, oa) = stats_of(&big_config(), &ms, 80.0);
        let (b, ob) = stats_of(&off(), &ms, 80.0);
        assert_eq!(oa, ob, "gap {gap} off {off_n}");
        assert_eq!(
            (
                a.moves_unconfirmed,
                a.moves_confirmed,
                a.moves_rolled_back,
                a.moves_undecided
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(a.applied, b.applied);
    }
}

// REQ: OA-TRK-347, OA-TRK-348
#[test]
fn the_window_follows_the_report_rate() {
    // A thinned feed (7 s between reports): the genuine second report of a move comes 7 s after
    // the first, past a fixed 5 s window; the window of four intervals (28 s) takes it.
    let mut rng = Rng(11);
    let mut ms: Vec<Measurement> = (0..=4)
        .map(|k| f64::from(k) * 7.0)
        .map(|t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    let after = |t: f64| Truth {
        e: 14_000.0,
        n: 1_800.0 + 200.0 * (t - 70.0),
        ve: 0.0,
        vn: 200.0,
        alt_ft: 35_000.0,
        vrate_fpm: 0.0,
    };
    for t in [70.0, 77.0, 84.0] {
        ms.push(report_of(&mut rng, t, &after(t)));
    }
    let cfg = TrackerConfig {
        expected_interval_ns: 7 * S,
        init_interval_ns: 7 * S,
        ..big_config()
    };
    let (s, _) = stats_of(&cfg, &ms, 100.0);
    assert_eq!(
        (s.moves_unconfirmed, s.moves_confirmed, s.moves_undecided),
        (1, 1, 0)
    );
    // With the expected interval left at 1 s the floor of 5 s applies: the move stands.
    let cfg = TrackerConfig {
        expected_interval_ns: S,
        ..cfg
    };
    let (s, _) = stats_of(&cfg, &ms, 100.0);
    assert_eq!((s.moves_confirmed, s.moves_undecided), (0, 1));
}

// REQ: OA-TRK-351
#[test]
fn the_rule_has_defaults_that_are_on_and_checked() {
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
