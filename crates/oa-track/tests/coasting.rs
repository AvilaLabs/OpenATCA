//! Coasting output: a live track is never absent, rows say when they are uncertain, and the
//! known limit of the uncertainty (a turn that begins while coasting).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_track::{
    output_new, r95_m_of, tracker_advance, tracker_new, tracker_stats, tracker_update,
    TrackerConfig, O_COAST, O_TICK, O_UNCERTAIN,
};

const ADDR: u32 = 0xA0_0002;
const SPEED: f64 = 200.0;
/// Standard-rate turn, rad/s (3 degrees per second), clockwise.
const OMEGA: f64 = 3.0 * std::f64::consts::PI / 180.0;

/// Truth: eastbound at 200 m/s until `t_turn`, then a standard-rate right turn.
fn truth(t_s: f64, t_turn: f64) -> Truth {
    if t_s <= t_turn {
        return Truth {
            e: SPEED * t_s,
            n: 0.0,
            ve: SPEED,
            vn: 0.0,
            alt_ft: 35_000.0,
            vrate_fpm: 0.0,
        };
    }
    let tau = t_s - t_turn;
    let (s, c) = (OMEGA * tau).sin_cos();
    Truth {
        e: SPEED * t_turn + SPEED / OMEGA * s,
        n: SPEED / OMEGA * (c - 1.0),
        // Heading 90 degrees plus the turned angle.
        ve: SPEED * c,
        vn: -SPEED * s,
        alt_ft: 35_000.0,
        vrate_fpm: 0.0,
    }
}

// REQ: OA-TRK-343, OA-TRK-345
#[test]
fn a_turn_that_begins_while_coasting_makes_the_radius_too_small_a_known_limit() {
    // Reports at 1 Hz for 30 s, then silence; the aircraft turns at standard rate from 30 s on.
    // The covariance is that of a straight-flight model with a turn-rate allowance (no turn is
    // known), so the true error runs past the 95% radius: recorded here, and a limit of the
    // coasting uncertainty to be fixed, not a property to rely on. The row says nothing false
    // about being live: it is flagged coasting from the third missed report.
    let f = frame0();
    let cfg = TrackerConfig {
        out_capacity: 1 << 16,
        tick_ns: 1_000_000_000,
        drop_after_ns: 200 * 1_000_000_000,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    let mut rng = Rng(11);
    for k in 0..=30 {
        let m = measure(
            &f,
            ADDR,
            f64::from(k),
            &truth(f64::from(k), 30.0),
            SIGMA_POS,
            Some(SIGMA_VEL),
            true,
            &mut rng,
        );
        tracker_update(&mut t, &m, &mut out);
    }
    tracker_advance(&mut t, 120 * 1_000_000_000, &mut out);
    let mut worst = 0.0f64;
    let mut over = 0usize;
    let mut coasting_rows = 0usize;
    println!("t_s  true_error_m  r95_m  ratio  flags");
    for i in 0..out.t_ns.len() {
        if out.flags[i] & O_TICK == 0 || out.flags[i] & O_COAST == 0 {
            continue;
        }
        coasting_rows += 1;
        let t_s = out.t_ns[i] as f64 / 1e9;
        let err = pos_err(&f, &out, i, &truth(t_s, 30.0));
        let r95 = r95_m_of(&out.pos_cov_m2[i]);
        worst = worst.max(err / r95);
        over += usize::from(err > r95);
        if (t_s as i64) % 10 == 0 {
            println!(
                "{t_s:>4.0} {err:>13.1} {r95:>8.1} {:>6.2}  uncertain={}",
                err / r95,
                out.flags[i] & O_UNCERTAIN != 0
            );
        }
    }
    println!(
        "coasting rows {coasting_rows}, true error over r95 on {over}, worst ratio {worst:.2}"
    );
    // Every coasting tick has a row, and the radius is too small on some of them.
    assert!(coasting_rows >= 85, "{coasting_rows}");
    assert!(
        over > 0,
        "the radius covered the true error on every row; the known limit no longer shows: {worst}"
    );
    assert_eq!(tracker_stats(&t).nan_covariance, 0);
}

// REQ: OA-TRK-343
#[test]
fn a_radar_only_track_goes_from_live_to_coasting_to_uncertain_and_is_never_absent() {
    // Sparse reports (every 12 s, a position variance of an en-route radar: 300 m sigma), then
    // silence for 100 s. Every tick from confirmation to the end has a row; the row sequence is
    // live, then coasting, then coasting and uncertain.
    let f = frame0();
    let cfg = TrackerConfig {
        out_capacity: 1 << 16,
        tick_ns: 1_000_000_000,
        expected_interval_ns: 12_000_000_000,
        init_interval_ns: 12_000_000_000,
        drop_after_ns: 300 * 1_000_000_000,
        ..big_config()
    };
    let mut t = tracker_new(&cfg).unwrap();
    let mut out = output_new(cfg.out_capacity);
    let mut rng = Rng(5);
    for k in 0..6 {
        let ts = f64::from(k) * 12.0;
        let m = measure(
            &f,
            ADDR,
            ts,
            &truth(ts, 1.0e9),
            300.0,
            None,
            false,
            &mut rng,
        );
        tracker_update(&mut t, &m, &mut out);
    }
    tracker_advance(&mut t, 160 * 1_000_000_000, &mut out);
    let ticks: Vec<usize> = (0..out.t_ns.len())
        .filter(|&i| out.flags[i] & O_TICK != 0)
        .collect();
    let times: Vec<i64> = ticks.iter().map(|&i| out.t_ns[i]).collect();
    assert!(times.windows(2).all(|w| w[1] - w[0] == 1_000_000_000));
    assert_eq!(*times.last().unwrap(), 160 * 1_000_000_000);
    let phase = |i: usize| (out.flags[i] & O_COAST != 0, out.flags[i] & O_UNCERTAIN != 0);
    let seq: Vec<(bool, bool)> = ticks.iter().map(|&i| phase(i)).collect();
    // A sparse radar-only track is already uncertain between its reports (the row says so), then
    // coasts once its interval is over, and stays coasting and uncertain to the end.
    let live = seq.iter().position(|x| !x.0).unwrap();
    let coast = seq.iter().position(|x| x.0).unwrap();
    assert!(live < coast, "{live} {coast}");
    assert!(seq[coast..].iter().all(|x| x.0 && x.1));
    assert!(seq.iter().any(|x| !x.0 && x.1));
    assert_eq!(
        tracker_stats(&t).rows_uncertain,
        seq.iter().filter(|x| x.1).count() as u64
    );
}
