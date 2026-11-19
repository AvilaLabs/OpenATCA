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
    output_new, r95_m_of, tracker_moves_active, tracker_new, tracker_note_radar_disagreement,
    tracker_predict, tracker_stats, tracker_update, Measurement, TrackOutput, Tracker,
    TrackerConfig, TrackerStats, O_MOVE_UNCONFIRMED, O_RADAR_DISAGREES, O_TICK, O_UNCERTAIN,
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
        // The forged report where the gate refuses it, else the old-path vote (the moved track's
        // gate refuses that).
        assert_eq!(s.jump_rejected, 1, "gap {gap}");
        // Without the rule the forged report moves the track (the finding).
        // (The forged report is applied, the vote after it is not: the same count.)
        assert_eq!(s.applied, c.applied, "gap {gap}");
        if moved == 1 {
            // The forged report's own row is marked and its radius covers the old path.
            let i = update_row(&out, (forged_at * 1.0e9) as i64);
            // (the filter takes only part of the way to the report: the radius is over the shift).
            assert_ne!(out.flags[i] & O_MOVE_UNCONFIRMED, 0, "gap {gap}");
            assert!(r95_m_of(&out.pos_cov_m2[i]) > 800.0, "gap {gap}");
            // Every row of the track from the move to the rollback carries the mark.
            let t0 = (forged_at * 1.0e9) as i64;
            // The first report on the old path (a second after) is a vote, not a rollback
            // (RT-273); the second, a second later, rolls back.
            let t1 = ((forged_at + 2.0) * 1.0e9) as i64;
            for k in 0..out.t_ns.len() {
                if out.t_ns[k] >= t0 && out.t_ns[k] < t1 {
                    assert_ne!(out.flags[k] & O_MOVE_UNCONFIRMED, 0, "gap {gap} row {k}");
                }
            }
            assert_eq!(s.moves_old_path_votes, 1, "gap {gap}");
            // After the rollback the track is the control's but for the vote's report, which it
            // did not take: the same rows, flags and, within 25 m, positions.
            let (a, b) = (rows_after(&out, t1), rows_after(&cout, t1));
            assert_eq!(a.len(), b.len(), "gap {gap}");
            for (x, y) in a.iter().zip(b.iter()) {
                assert_eq!((x.0, x.3), (y.0, y.3), "gap {gap}");
                assert!(
                    (x.1 - y.1).abs() < 2000 && (x.2 - y.2).abs() < 2000,
                    "gap {gap} {} {}",
                    x.1 - y.1,
                    x.2 - y.2
                );
            }
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
        // Every report is applied exactly as without the rule: every counter that is not the
        // rule's own is equal. `rows_uncertain` differs on purpose: a marked row's radius is
        // widened to cover the old path, so it can be uncertain by definition.
        assert!(s.rows_uncertain >= m.rows_uncertain);
        let plain = |x: &TrackerStats| TrackerStats {
            moves_unconfirmed: 0,
            moves_confirmed: 0,
            moves_abandoned_ended: 0,
            moves_decision_rejected: 0,
            moves_peak_active: 0,
            rows_uncertain: 0,
            ..*x
        };
        assert_eq!(plain(&s), plain(&m), "gap {gap}");
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

/// Every unconfirmed move ends in exactly one counted outcome (E6): the moves made equal the
/// confirmed, rolled back, undecided and abandoned ones plus those still unconfirmed.
fn balanced(t: &Tracker) {
    let s = tracker_stats(t);
    let outcomes = s.moves_confirmed
        + s.moves_rolled_back
        + s.moves_undecided
        + s.moves_abandoned_ended
        + s.moves_abandoned_restarted
        + s.moves_abandoned_resync
        + s.moves_abandoned_demoted;
    assert_eq!(
        s.moves_unconfirmed,
        outcomes + tracker_moves_active(t) as u64,
        "{s:?}"
    );
}

/// A live tracker fed one report at a time, so a test can look between them.
struct Live {
    t: Tracker,
    out: TrackOutput,
}

impl Live {
    fn new(cfg: &TrackerConfig) -> Self {
        Live {
            t: tracker_new(cfg).unwrap(),
            out: output_new(cfg.out_capacity),
        }
    }
    fn feed(&mut self, ms: &[Measurement]) {
        for m in ms {
            tracker_update(&mut self.t, m, &mut self.out);
        }
    }
    fn s(&self) -> TrackerStats {
        *tracker_stats(&self.t)
    }
    /// The flags of the newest row of the track.
    fn last_flags(&self) -> u8 {
        *self.out.flags.last().unwrap()
    }
}

/// A report of the moved track's own predicted position at `t_s`, shifted `north_m` north.
fn on_track(l: &Live, rng: &mut Rng, t_s: f64, north_m: f64) -> Measurement {
    let (row, _, _) = tracker_predict(&l.t, ADDR, (t_s * 1.0e9) as i64).unwrap();
    let mut m = report_of(rng, t_s, &truth(t_s, 0.0));
    m.lat_e7 = row.lat_e7 + (north_m / 0.011_119_5) as i32;
    m.lon_e7 = row.lon_e7;
    m
}

// REQ: OA-TRK-348
#[test]
fn a_report_that_is_not_applied_decides_nothing_and_the_mark_survives() {
    // Forged at 40 s (marked); a junk report a second later 5 km away is refused by the gate:
    // the mark and the snapshot stay (the review's finding), and the genuine report after it,
    // on the old path, still rolls the move back.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    assert_ne!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    l.feed(&[report_of(&mut rng, 41.0, &truth(41.0, 5_000.0))]);
    let s = l.s();
    assert_eq!(s.moves_decision_rejected, 1, "{s:?}");
    assert_eq!(
        (s.moves_confirmed, s.moves_rolled_back, s.moves_undecided),
        (0, 0, 0)
    );
    assert_eq!(tracker_moves_active(&l.t), 1);
    let (row, _, _) = tracker_predict(&l.t, ADDR, 41 * S).unwrap();
    assert_ne!(row.flags & O_MOVE_UNCONFIRMED, 0);
    // The junk report is no vote; the genuine ones after it are, two apart (RT-273).
    l.feed(&[report_of(&mut rng, 42.0, &truth(42.0, 0.0))]);
    assert_eq!(l.s().moves_rolled_back, 0);
    l.feed(&[report_of(&mut rng, 43.0, &truth(43.0, 0.0))]);
    let s = l.s();
    assert_eq!(
        (s.moves_rolled_back, s.moves_decision_rejected),
        (1, 1),
        "{s:?}"
    );
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-348
#[test]
fn a_report_after_the_window_or_agreeing_with_neither_leaves_the_move_standing() {
    // Past the window (5 s on a 1 s feed) a report on the moved track is applied and the move
    // stands (undecided): the mark goes only because the report was applied.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    let m = on_track(&l, &mut rng, 46.0, 0.0);
    l.feed(&[m]);
    let s = l.s();
    assert_eq!(
        (
            s.moves_unconfirmed,
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided
        ),
        (1, 0, 0, 1),
        "{s:?}"
    );
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
    // The genuine report on the old path only at 46 s is refused by the gate (the track is on
    // the forged path): it decides nothing, the mark stays.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    l.feed(&[report_of(&mut rng, 46.0, &truth(46.0, 0.0))]);
    let s = l.s();
    assert_eq!(
        (s.moves_undecided, s.moves_decision_rejected),
        (0, 1),
        "{s:?}"
    );
    assert_ne!(tracker_moves_active(&l.t), 0);
    balanced(&l.t);
    // A report that agrees with neither path and is applied (a thinned feed, a 12 s gap that
    // lets the gate pass 600 m off the moved track): the move stands.
    let cfg = TrackerConfig {
        expected_interval_ns: 7 * S,
        init_interval_ns: 7 * S,
        ..big_config()
    };
    let mut rng = Rng(7);
    let mut l = Live::new(&cfg);
    let ms: Vec<Measurement> = (0..=5)
        .map(|k| f64::from(k) * 7.0)
        .map(|t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    l.feed(&ms);
    l.feed(&[report_of(&mut rng, 70.0, &truth(70.0, FORGED_OFFSET_M))]);
    let m = on_track(&l, &mut rng, 82.0, 850.0);
    l.feed(&[m]);
    let s = l.s();
    assert_eq!(
        (
            s.moves_unconfirmed,
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided
        ),
        (1, 0, 0, 1),
        "{s:?}"
    );
    balanced(&l.t);
    // A report a quarter second after the move is not a second report: nothing is decided and
    // the mark stays.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    let m = on_track(&l, &mut rng, 40.25, 0.0);
    l.feed(&[m]);
    let s = l.s();
    assert_eq!(
        (
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided,
            s.moves_decision_rejected
        ),
        (0, 0, 0, 0),
        "{s:?}"
    );
    assert_ne!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-348
#[test]
fn a_later_large_move_keeps_the_original_snapshot_and_a_refuting_report_rolls_back_to_it() {
    // A forged move at 40 s, undecided by a second large move 12 s later (past the window): the
    // track stays marked, with the snapshot from before the first move; the genuine report
    // after rolls back to it.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    let m = on_track(&l, &mut rng, 52.0, 1_400.0);
    l.feed(&[m]);
    let s = l.s();
    println!("{s:?}");
    assert_eq!((s.moves_unconfirmed, s.moves_undecided), (2, 1), "{s:?}");
    assert_eq!(tracker_moves_active(&l.t), 1);
    assert_ne!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
    // Two reports on the old path (RT-273): the first a vote, the second rolls back.
    l.feed(&[report_of(&mut rng, 53.0, &truth(53.0, 0.0))]);
    assert_eq!(l.s().moves_rolled_back, 0);
    l.feed(&[report_of(&mut rng, 54.0, &truth(54.0, 0.0))]);
    let s = l.s();
    assert_eq!(s.moves_rolled_back, 1, "{s:?}");
    assert_eq!(tracker_moves_active(&l.t), 0);
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-348
#[test]
fn a_forger_who_repeats_the_same_false_place_is_confirmed_by_the_second_report() {
    // The residual risk, written down exactly. The second forged report, a second after the
    // first and agreeing with the moved track (the gate does not refuse it once the track is
    // there) and not with the old path, confirms the move: the track is unmarked at
    // the false place and nothing in the tracker refutes it, only a second source (radar) can.
    // The other two ways to an unmarked false track are a report in the gate that agrees with
    // neither path (undecided: `a_report_after_the_window_or_agreeing_with_neither...`) and a
    // resynchronisation onto a consistent run of reports the gate refuses (resync: in
    // `a_track_that_ends_or_restarts...`); each ends the move in its own counted outcome.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    for k in 0..8 {
        let tt = 40.0 + f64::from(k);
        l.feed(&[report_of(&mut rng, tt, &truth(tt, FORGED_OFFSET_M))]);
    }
    let s = l.s();
    assert_eq!(
        (
            s.moves_unconfirmed,
            s.moves_confirmed,
            s.moves_rolled_back,
            s.moves_undecided
        ),
        (1, 1, 0, 0),
        "{s:?}"
    );
    // (Some repeats are refused by the gate, the track having seen one report there and the
    // noise of the feed being what it is: the one at 41 s decides nothing,
    // `moves_decision_rejected`, and the one at 42 s confirms. The counts are those of this
    // seeded feed.)
    assert_eq!((s.resyncs, s.moves_decision_rejected), (0, 1), "{s:?}");
    assert_eq!(tracker_moves_active(&l.t), 0, "{s:?}");
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-349
#[test]
fn a_track_that_ends_or_restarts_ends_its_unconfirmed_move_with_a_count() {
    // Forged at 40 s, then nothing: the track is dropped at the drop age; the move is counted as
    // abandoned (ended), and a new track of the address starts unmarked.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    let (t, _) = run(&big_config(), &ms, 41.0);
    assert_eq!(tracker_stats(&t).moves_unconfirmed, 1);
    balanced(&t);
    let mut ms2 = ms.clone();
    for k in 0..4 {
        let tt = 200.0 + f64::from(k);
        ms2.push(report_of(&mut rng, tt, &truth(tt, 0.0)));
    }
    let (t, out) = run(&big_config(), &ms2, 220.0);
    let s = tracker_stats(&t);
    assert_eq!(s.started, 2);
    assert_eq!(s.moves_abandoned_ended, 1, "{s:?}");
    balanced(&t);
    for i in 0..out.t_ns.len() {
        if out.t_ns[i] >= 200 * S {
            assert_eq!(out.flags[i] & O_MOVE_UNCONFIRMED, 0);
        }
    }
    // A resynchronisation ends it too: forged, then reports at a third place until the filter
    // restarts.
    let mut rng = Rng(7);
    let mut ms = genuine(&mut rng, &seconds(0.0, 20.0));
    ms.push(report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M)));
    for k in 0..6 {
        let tt = 41.0 + f64::from(k);
        ms.push(report_of(&mut rng, tt, &truth(tt, -6_000.0)));
    }
    let (t, out) = run(&big_config(), &ms, 60.0);
    let s = tracker_stats(&t);
    assert!(s.resyncs >= 1, "{s:?}");
    assert_eq!(s.moves_abandoned_resync, 1, "{s:?}");
    assert_eq!(out.flags.last().unwrap() & O_MOVE_UNCONFIRMED, 0);
    balanced(&t);
}

// REQ: OA-TRK-349
#[test]
fn a_rollback_does_not_undo_what_the_fusion_layer_set_in_between() {
    // The radar-disagreement mark is written from outside the update path after the move; the
    // rollback restores measurement state only (the expected interval, the other such column, is
    // outside the snapshot too and set by the fusion layer, which these tests do not run).
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    assert!(tracker_note_radar_disagreement(&mut l.t, ADDR, 100 * S));
    l.feed(&[report_of(&mut rng, 41.0, &truth(41.0, 0.0))]);
    l.feed(&[report_of(&mut rng, 42.0, &truth(42.0, 0.0))]);
    assert_eq!(l.s().moves_rolled_back, 1);
    let i = l.out.flags.len() - 1;
    assert_ne!(l.out.flags[i] & O_RADAR_DISAGREES, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-347
#[test]
fn the_forecast_of_a_marked_track_carries_the_mark_and_the_widened_radius() {
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    let (calm, _, _) = tracker_predict(&l.t, ADDR, 30 * S).unwrap();
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    let (row, confirmed, _) = tracker_predict(&l.t, ADDR, 41 * S).unwrap();
    assert!(confirmed);
    assert_ne!(row.flags & O_MOVE_UNCONFIRMED, 0);
    assert!(
        r95_m_of(&row.pos_cov_m2) > 800.0,
        "{}",
        r95_m_of(&row.pos_cov_m2)
    );
    assert_eq!(calm.flags & O_MOVE_UNCONFIRMED, 0);
    // The same row the tracker emits at that time.
    l.feed(&[report_of(&mut rng, 41.0, &truth(41.0, 0.0))]);
    l.feed(&[report_of(&mut rng, 42.0, &truth(42.0, 0.0))]);
    let (row, _, _) = tracker_predict(&l.t, ADDR, 43 * S).unwrap();
    assert_eq!(row.flags & O_MOVE_UNCONFIRMED, 0);
}

/// A report of aircraft `addr` at `t_s`, `off_n` metres north of the steady track.
fn report_for(rng: &mut Rng, addr: u32, t_s: f64, off_n: f64) -> Measurement {
    let mut m = report_of(rng, t_s, &truth(t_s, off_n));
    m.addr = addr;
    m
}

// REQ: OA-TRK-347
#[test]
fn with_no_snapshot_the_move_is_marked_and_the_radius_fails_closed() {
    // A pool of one entry: the second track's move has no snapshot; it is applied and marked,
    // its rows carry a radius of 500 km (the old path is unknown), a refuting
    // report cannot roll it back, and the count says so. The first track's snapshot returns to
    // the pool when its move is decided, and the next move takes it.
    let cfg = TrackerConfig {
        confirm_pool: 1,
        ..big_config()
    };
    let mut rng = Rng(7);
    let mut l = Live::new(&cfg);
    // A third aircraft reports every second throughout, so the clock keeps up and the two
    // others have the 20 s gap the gate needs to pass the forged report.
    for t in seconds(0.0, 42.0) {
        for addr in [0xA0_0001u32, 0xA0_0002, 0xA0_0003] {
            if t > 20.0 && addr != 0xA0_0003 {
                continue;
            }
            let mut m = report_of(&mut rng, t, &truth(t, 0.0));
            m.addr = addr;
            l.feed(&[m]);
        }
    }
    l.feed(&[report_for(&mut rng, 0xA0_0001, 43.0, FORGED_OFFSET_M)]);
    l.feed(&[report_for(&mut rng, 0xA0_0002, 43.0, FORGED_OFFSET_M)]);
    let s = l.s();
    assert_eq!(
        (s.moves_unconfirmed, s.move_no_snapshot, s.moves_peak_active),
        (2, 1, 2),
        "{s:?}"
    );
    let (row, _, _) = tracker_predict(&l.t, 0xA0_0002, 44 * S).unwrap();
    assert_ne!(row.flags & O_MOVE_UNCONFIRMED, 0);
    // The finite cap (500 km), not the largest the format holds.
    let r = r95_m_of(&row.pos_cov_m2);
    assert!((499_999.0..500_010.0).contains(&r), "{r}");
    assert!(s.move_widen_failed >= 1, "{s:?}");
    // The refuting report: the first track rolls back, the second cannot.
    for t in [44.0, 45.0] {
        l.feed(&[report_for(&mut rng, 0xA0_0001, t, 0.0)]);
        l.feed(&[report_for(&mut rng, 0xA0_0002, t, 0.0)]);
    }
    let s = l.s();
    assert_eq!(s.moves_rolled_back, 1, "{s:?}");
    assert!(tracker_moves_active(&l.t) <= 1);
    balanced(&l.t);
    // The freed snapshot serves the next move.
    for t in seconds(45.0, 69.0) {
        let mut m = report_of(&mut rng, t, &truth(t, 0.0));
        m.addr = 0xA0_0003;
        l.feed(&[m]);
    }
    l.feed(&[report_for(&mut rng, 0xA0_0001, 70.0, FORGED_OFFSET_M)]);
    let s = l.s();
    assert_eq!(s.move_no_snapshot, 1, "{s:?}");
}

// REQ: OA-TRK-349
#[test]
fn a_late_replay_that_refuses_the_forged_report_in_time_order_loses_the_rollback_and_counts_it() {
    // A genuine report from 39.6 s arrives late: in time order the forged report at 40 s is a
    // jump 0.4 s later and is refused, so it is no longer in the ring and the rollback point
    // cannot be derived before it. The mark stays and the loss is counted.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    l.feed(&genuine(&mut rng, &seconds(0.0, 20.0)));
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    l.feed(&[report_of(&mut rng, 39.6, &truth(39.6, 0.0))]);
    let s = l.s();
    assert_eq!(s.refiltered, 1, "{s:?}");
    assert_eq!(s.moves_rollback_lost_late, 1, "{s:?}");
    assert_ne!(tracker_moves_active(&l.t), 0);
    // A refuting report cannot roll back, and decides nothing it can undo.
    l.feed(&[report_of(&mut rng, 41.0, &truth(41.0, 0.0))]);
    assert_eq!(l.s().moves_rolled_back, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-349
#[test]
fn a_late_report_from_a_second_sensor_during_a_move_leaves_the_rollback_in_place() {
    // The routine case with two sensors: a genuine report of a second sensor arrives late (from
    // before the gap) while a forged move is unconfirmed. The replay re-derives the rollback point from the ring
    // (the state before the forged report, the late one included), so the report that refutes
    // the move afterwards still rolls the track back, to where a track that never saw the
    // forgery would be.
    let cfg = TrackerConfig {
        ooo_window_ns: 30 * S,
        clock_step_limit_ns: 30 * S,
        drop_after_ns: 60 * S,
        ..big_config()
    };
    let mut rng = Rng(7);
    let mut l = Live::new(&cfg);
    let mut control = Live::new(&cfg);
    let early = genuine(&mut rng, &seconds(0.0, 20.0));
    l.feed(&early);
    control.feed(&early);
    l.feed(&[report_of(&mut rng, 40.0, &truth(40.0, FORGED_OFFSET_M))]);
    let mut late = report_of(&mut rng, 19.5, &truth(19.5, 0.0));
    late.sensor = 2;
    l.feed(&[late]);
    control.feed(&[late]);
    let s = l.s();
    assert_eq!(s.refiltered, 1, "{s:?}");
    assert_eq!(s.moves_rollback_lost_late, 0, "{s:?}");
    assert_eq!(tracker_moves_active(&l.t), 1);
    for t in [41.0, 42.0] {
        let refuting = report_of(&mut rng, t, &truth(t, 0.0));
        l.feed(&[refuting]);
        control.feed(&[refuting]);
    }
    let s = l.s();
    assert_eq!((s.moves_rolled_back, s.moves_confirmed), (1, 0), "{s:?}");
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    // Where the track is after the rollback: the control's, which never saw the forgery.
    let (a, _, _) = tracker_predict(&l.t, ADDR, 42 * S).unwrap();
    let (b, _, _) = tracker_predict(&control.t, ADDR, 42 * S).unwrap();
    let d = ((f64::from(a.lat_e7 - b.lat_e7) * 0.011_119_5).powi(2)
        + (f64::from(a.lon_e7 - b.lon_e7) * 0.011_119_5).powi(2))
    .sqrt();
    // (The control also took the report at 41 s that the rolled-back track had only as a vote.)
    assert!(d < 30.0, "{d} m from the track without the forgery");
    balanced(&l.t);
}

// REQ: OA-TRK-347
#[test]
fn a_late_replay_that_moves_the_estimate_far_marks_the_track() {
    // Sparse reports (20 s apart): a forged report arrives late, between two stored ones, and the
    // re-filter moves the current estimate by over 926 m.
    let cfg = TrackerConfig {
        expected_interval_ns: 20 * S,
        init_interval_ns: 20 * S,
        ooo_window_ns: 60 * S,
        clock_step_limit_ns: 120 * S,
        drop_after_ns: 300 * S,
        ..big_config()
    };
    let mut rng = Rng(7);
    let mut l = Live::new(&cfg);
    let ms: Vec<Measurement> = (0..=6)
        .map(|k| f64::from(k) * 20.0)
        .map(|t| report_of(&mut rng, t, &truth(t, 0.0)))
        .collect();
    l.feed(&ms);
    let before = l.s();
    l.feed(&[report_of(&mut rng, 119.6, &truth(119.6, 2000.0))]);
    let s = l.s();
    assert_eq!(s.refiltered, before.refiltered + 1, "{s:?}");
    assert_eq!((s.moves_late, s.moves_unconfirmed), (1, 1), "{s:?}");
    assert_ne!(tracker_moves_active(&l.t), 0);
    let (row, _, _) = tracker_predict(&l.t, ADDR, 125 * S).unwrap();
    assert_ne!(row.flags & O_MOVE_UNCONFIRMED, 0);
    // The row the replay itself emitted is marked too, and widened like any marked row (RT-275):
    // its radius covers the old path (the track as it was before the late report, about 2 km
    // away), so it is uncertain.
    let i = (0..l.out.t_ns.len())
        .rev()
        .find(|&i| l.out.t_ns[i] == 119_600_000_000 && l.out.flags[i] & O_TICK == 0)
        .unwrap();
    assert_ne!(l.out.flags[i] & O_MOVE_UNCONFIRMED, 0);
    assert_ne!(l.out.flags[i] & O_UNCERTAIN, 0);
    let r = r95_m_of(&l.out.pos_cov_m2[i]);
    assert!(r > 1_500.0, "{r}");
    balanced(&l.t);
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
    // The window cannot be longer than the drop age.
    assert!(bad(&|c| c.drop_after_ns = c.confirm_window_ns - 1));
    assert!(!bad(&|c| c.confirm_move_m = 2_000.0));
    assert!(!bad(&|c| c.confirm_moves = false));
}

/// A report of a track that stands still `north_m` north of the origin.
fn still(rng: &mut Rng, t_s: f64, north_m: f64) -> Measurement {
    let tr = Truth {
        e: 0.0,
        n: north_m,
        ve: 0.0,
        vn: 0.0,
        alt_ft: 35_000.0,
        vrate_fpm: 0.0,
    };
    report_of(rng, t_s, &tr)
}

// REQ: OA-TRK-352
#[test]
fn one_report_at_the_old_place_does_not_roll_back_a_genuine_relocation() {
    // A stationary track, a 40 s gap, then a genuine 2 km relocation. One stray report at the old
    // place a second later is a vote for the old path, nothing more (RT-273): the move is not
    // rolled back and the genuine continuation confirms it.
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    for t in seconds(0.0, 20.0) {
        l.feed(&[still(&mut rng, t, 0.0)]);
    }
    l.feed(&[still(&mut rng, 60.0, 2_000.0)]);
    let s = l.s();
    assert_eq!(s.moves_unconfirmed, 1, "{s:?}");
    l.feed(&[still(&mut rng, 61.0, 0.0)]);
    let s = l.s();
    assert_eq!(
        (s.moves_rolled_back, s.moves_old_path_votes),
        (0, 1),
        "{s:?}"
    );
    assert_ne!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    l.feed(&[still(&mut rng, 62.0, 2_000.0)]);
    let s = l.s();
    assert_eq!((s.moves_rolled_back, s.moves_confirmed), (0, 1), "{s:?}");
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}

// REQ: OA-TRK-352
#[test]
fn two_reports_at_the_old_place_roll_back_a_forged_move_but_not_two_in_a_burst() {
    let mut rng = Rng(7);
    let mut l = Live::new(&big_config());
    for t in seconds(0.0, 20.0) {
        l.feed(&[still(&mut rng, t, 0.0)]);
    }
    l.feed(&[still(&mut rng, 60.0, 2_000.0)]);
    // Two reports less than 0.5 s apart are one vote.
    l.feed(&[still(&mut rng, 61.0, 0.0)]);
    l.feed(&[still(&mut rng, 61.3, 0.0)]);
    let s = l.s();
    assert_eq!(
        (s.moves_rolled_back, s.moves_old_path_votes),
        (0, 1),
        "{s:?}"
    );
    l.feed(&[still(&mut rng, 62.0, 0.0)]);
    let s = l.s();
    assert_eq!(s.moves_rolled_back, 1, "{s:?}");
    assert_eq!(l.last_flags() & O_MOVE_UNCONFIRMED, 0);
    balanced(&l.t);
}
