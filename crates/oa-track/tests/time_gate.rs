//! The tracker clock: how far ahead one measurement may move it, how a run of far-ahead
//! measurements steps it forward, and how a run of late refusals steps it back.
//!
//! Lines marked `NEW-STAT` read counters that exist only with the time gate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use common::*;
use oa_track::{
    output_new, tracker_advance, tracker_flush, tracker_live, tracker_new, tracker_stats,
    tracker_update, Measurement, TrackOutput, Tracker, TrackerConfig, DEFAULT_CONFIG, O_COAST,
    O_REACQUIRED, O_TICK,
};

const S: i64 = 1_000_000_000;

/// Aircraft `a` at `t_s` seconds on its own straight line, reported by `sensor`.
fn meas(a: u32, sensor: u16, t_s: f64, rng: &mut Rng) -> Measurement {
    let man = Manoeuvre::straight(200.0, 45.0);
    let mut m = measure(
        &frame0(),
        0xA00000 + a,
        t_s,
        &man.at(t_s, f64::from(a) * 5000.0, 0.0),
        SIGMA_POS,
        Some(SIGMA_VEL),
        false,
        rng,
    );
    m.sensor = sensor;
    m
}

/// Six aircraft, each reported once a second by sensor 1 or 2 alternately, for `t0..=t1` seconds.
fn feed(t: &mut Tracker, out: &mut TrackOutput, rng: &mut Rng, t0: i64, t1: i64) {
    for s in t0..=t1 {
        for a in 0..6 {
            let m = meas(a, 1 + ((s as u32 + a) % 2) as u16, s as f64, rng);
            tracker_update(t, &m, out);
        }
    }
}

fn fleet_until(t1: i64) -> (Tracker, TrackOutput, Rng) {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 20);
    let mut rng = Rng(7);
    feed(&mut t, &mut out, &mut rng, 0, t1);
    assert_eq!(tracker_live(&t).len(), 6);
    (t, out, rng)
}

/// A far-ahead report: `toa` of the aircraft `a` from `sensor`, position as at its real time.
fn far(a: u32, sensor: u16, real_s: f64, toa_s: i64, rng: &mut Rng) -> Measurement {
    let mut m = meas(a, sensor, real_s, rng);
    m.toa_ns = toa_s * S;
    m
}

// REQ: OA-TRK-190
#[test]
fn the_step_limit_is_a_configured_time_between_the_window_and_the_drop_age() {
    assert_eq!(
        DEFAULT_CONFIG.clock_step_limit_ns,
        DEFAULT_CONFIG.ooo_window_ns
    ); // NEW-STAT
    let with = |limit| TrackerConfig {
        clock_step_limit_ns: limit, // NEW-STAT
        ..big_config()
    };
    let c = big_config();
    assert!(tracker_new(&with(c.ooo_window_ns)).is_ok());
    assert!(tracker_new(&with(c.drop_after_ns - 1)).is_ok());
    assert!(tracker_new(&with(c.ooo_window_ns - 1)).is_err());
    assert!(tracker_new(&with(c.drop_after_ns)).is_err());
    assert!(tracker_new(&with(300 * S)).is_err());
}

// REQ: OA-TRK-191
#[test]
fn one_measurement_299_seconds_ahead_is_held_and_sweeps_nothing() {
    let (mut t, mut out, mut rng) = fleet_until(10);
    let ticks = tracker_stats(&t).ticks;
    tracker_update(&mut t, &far(0, 3, 11.0, 10 + 299, &mut rng), &mut out);
    assert_eq!(tracker_stats(&t).time_held, 1);
    assert_eq!(tracker_stats(&t).time_steps, 0);
    assert_eq!(tracker_stats(&t).ticks, ticks);
    assert_eq!(tracker_live(&t).len(), 6);
    // The clock did not move: the next real measurements are applied, none refused as late.
    feed(&mut t, &mut out, &mut rng, 11, 12);
    assert_eq!(tracker_stats(&t).late_window, 0);
    assert_eq!(tracker_live(&t).len(), 6);
}

// REQ: OA-TRK-191
#[test]
fn walking_the_clock_ahead_from_one_address_and_sensor_never_moves_it() {
    let (mut t, mut out, mut rng) = fleet_until(10);
    for k in 1..=20 {
        tracker_update(&mut t, &far(0, 3, 11.0, 10 + 299 * k, &mut rng), &mut out);
    }
    assert_eq!(tracker_stats(&t).time_held, 20);
    assert_eq!(tracker_stats(&t).time_steps, 0);
    assert_eq!(tracker_live(&t).len(), 6);
    feed(&mut t, &mut out, &mut rng, 11, 15);
    assert_eq!(tracker_stats(&t).late_window, 0);
    assert_eq!(tracker_live(&t).len(), 6);
}

// REQ: OA-TRK-192
#[test]
fn a_far_ahead_run_from_one_sensor_waits_for_a_second_sensor_while_two_are_live() {
    let (mut t, mut out, mut rng) = fleet_until(10);
    // Four far-ahead reports in a row, three addresses, all from sensor 3.
    for k in 0..4u32 {
        tracker_update(
            &mut t,
            &far(k % 3, 3, 11.0, 410 + i64::from(k), &mut rng),
            &mut out,
        );
    }
    assert_eq!(tracker_stats(&t).time_held, 4);
    assert_eq!(tracker_stats(&t).time_steps, 0);
    // Another sensor joins the run: it steps.
    tracker_update(&mut t, &far(3, 4, 11.0, 414, &mut rng), &mut out);
    assert_eq!(tracker_stats(&t).time_steps, 1);
    assert_eq!(tracker_stats(&t).time_held, 4);
    assert_eq!(tracker_stats(&t).time_held_applied, 4);
}

// REQ: OA-TRK-192
#[test]
fn a_whole_feed_pause_of_30_seconds_steps_after_three_measurements_and_tracks_continue() {
    let (mut t, mut out, mut rng) = fleet_until(10);
    feed(&mut t, &mut out, &mut rng, 40, 50);
    let st = tracker_stats(&t);
    assert_eq!(st.time_held, 2);
    assert_eq!(st.time_steps, 1);
    // Confirmation delays the two held measurements, it does not lose them.
    assert_eq!(st.time_held_applied, 2);
    assert_eq!(st.applied, st.offered);
    assert_eq!(st.started, 6, "a track was restarted");
    assert_eq!(tracker_live(&t).len(), 6);
    assert!(tracker_live(&t).iter().all(|l| l.1 >= 49 * S));
    assert_eq!(st.late_window, 0);
}

// REQ: OA-TRK-192
#[test]
fn one_aircraft_one_sensor_and_a_15_second_gap_steps_at_once() {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 20);
    let mut rng = Rng(9);
    for s in (0..=10).chain(25..=30) {
        tracker_update(&mut t, &meas(0, 1, s as f64, &mut rng), &mut out);
    }
    let st = tracker_stats(&t);
    // Only its own track and sensor are live: nothing to protect, no run to wait for.
    assert_eq!(st.time_held, 0);
    assert_eq!(st.time_steps, 1);
    assert_eq!(st.started, 1);
    assert_eq!(st.applied, st.offered);
    assert_eq!(tracker_live(&t).len(), 1);
}

// REQ: OA-TRK-199
#[test]
fn one_aircraft_one_sensor_at_17_second_intervals_has_every_report_applied_and_none_held() {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 20);
    let mut rng = Rng(11);
    for k in 0..=10 {
        tracker_update(&mut t, &meas(0, 1, f64::from(k * 17), &mut rng), &mut out);
    }
    let st = tracker_stats(&t);
    assert_eq!(st.time_held, 0);
    assert_eq!(st.late_window, 0);
    assert_eq!(st.applied, 11);
    assert_eq!(st.time_steps, 10);
}

// REQ: OA-TRK-343, OA-TRK-344
#[test]
fn simultaneous_sensor_blackout_coasts_tracks_and_staggered_returns_recover_without_restarts() {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1024);
    let mut rng = Rng(23);

    // Four independent identities are established across four sensors. The feeds stop together,
    // so every track must age and coast even though no report arrives to drive the clock.
    for s in 0..=10 {
        for a in 0..4 {
            tracker_update(&mut t, &meas(a, 1 + a as u16, s as f64, &mut rng), &mut out);
        }
    }
    assert_eq!(tracker_live(&t).len(), 4);
    let started = tracker_stats(&t).started;
    tracker_advance(&mut t, 40 * S, &mut out);
    let saw_coast = (0..out.t_ns.len()).any(|i| {
        out.addr[i] == 0xA00000
            && out.t_ns[i] >= 20 * S
            && out.flags[i] & O_TICK != 0
            && out.flags[i] & O_COAST != 0
    });
    assert!(saw_coast, "the blackout did not produce coast rows");
    assert_eq!(
        tracker_live(&t).len(),
        4,
        "tracks expired before the feeds returned"
    );

    // Three sensors resume in separate lanes. The remaining identity is deliberately silent and
    // must remain the same coasting track while the others reacquire.
    for (a, sensor, at_s) in [(0, 1, 41), (1, 2, 42), (2, 3, 43)] {
        tracker_update(&mut t, &meas(a, sensor, at_s as f64, &mut rng), &mut out);
        assert_eq!(
            tracker_live(&t).len(),
            4,
            "a returning report changed track count"
        );
        assert_eq!(
            tracker_stats(&t).started,
            started,
            "a returning report restarted a track"
        );
        assert!(
            out.addr.iter().enumerate().any(|(i, addr)| {
                *addr == 0xA00000 + a && out.t_ns[i] == at_s * S && out.flags[i] & O_REACQUIRED != 0
            }),
            "returning identity {a} did not publish reacquisition"
        );
    }
    assert!(tracker_live(&t)
        .iter()
        .all(|(addr, _)| (0xA00000..0xA00004).contains(addr)));
    assert_eq!(tracker_stats(&t).applied, tracker_stats(&t).offered);
    assert_eq!(
        tracker_stats(&t).ended[usize::from(oa_track::END_DROPPED)],
        0
    );

    tracker_advance(&mut t, 60 * S, &mut out);
    let silent = tracker_live(&t)
        .into_iter()
        .find(|(addr, _)| *addr == 0xA00003)
        .expect("the unrelated silent identity was lost");
    assert_eq!(
        silent.1,
        10 * S,
        "coasting must preserve the last update time"
    );
    assert!(out.addr.iter().enumerate().any(|(i, addr)| {
        *addr == 0xA00003 && out.t_ns[i] >= 43 * S && out.flags[i] & O_COAST != 0
    }));
}

// REQ: OA-TRK-199
#[test]
fn two_aircraft_reporting_together_17_seconds_apart_lose_no_report_to_a_confirmation() {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 20);
    let mut rng = Rng(13);
    // Both aircraft, one sensor, at 0, 17 and 34 s: the two at 17 s are held until the first at
    // 34 s confirms the step, and are then applied.
    for k in 0..=2 {
        for a in 0..2 {
            tracker_update(&mut t, &meas(a, 1, f64::from(k * 17), &mut rng), &mut out);
        }
    }
    let st = tracker_stats(&t);
    assert_eq!(st.offered, 6);
    assert_eq!(st.time_held, 2);
    assert_eq!(st.time_held_applied, 2);
    assert_eq!(st.time_steps, 1);
    assert_eq!(st.late_window, 0);
    assert_eq!(st.applied, 6);
}

// REQ: OA-TRK-196
#[test]
fn a_clock_forged_ahead_by_two_sensors_steps_back_on_real_data_and_the_tracks_are_rebuilt() {
    let (mut t, mut out, mut rng) = fleet_until(20);
    // Two colluding sensors, three addresses: a confirmed step far ahead.
    for (k, (a, sensor)) in [(0xF1u32, 7u16), (0xF2, 8), (0xF3, 7)].iter().enumerate() {
        let mut m = far(0, *sensor, 21.0, 400 + k as i64, &mut rng);
        m.addr = *a;
        tracker_update(&mut t, &m, &mut out);
    }
    assert_eq!(tracker_stats(&t).time_steps, 1);
    assert!(tracker_stats(&t).ticks_skipped > 0 || tracker_stats(&t).ticks > 300);
    // Real data only from here.
    let max_before = out.t_ns.iter().copied().max().unwrap();
    let rows_before = out.t_ns.len();
    feed(&mut t, &mut out, &mut rng, 21, 45);
    let st = tracker_stats(&t);
    assert_eq!(st.time_steps_back, 1); // NEW-STAT
                                       // The two held forged measurements were applied with the one that confirmed the step: three
                                       // forged tracks to drop.
    assert_eq!(st.time_future_dropped, 3); // NEW-STAT
    let live = tracker_live(&t);
    assert_eq!(live.len(), 6, "{live:?}");
    assert!(live.iter().all(|l| (0xA00000..0xA00006).contains(&l.0)));
    assert!(live.iter().all(|l| l.1 >= 44 * S));
    // Output continued, with earlier times than before the step back.
    assert!(out.t_ns.len() > rows_before + 100);
    assert!(out.t_ns[rows_before..].iter().any(|&x| x < max_before));
    tracker_flush(&mut t, 50 * S, &mut out);
}

// REQ: OA-TRK-200
#[test]
fn forged_sensors_do_not_lock_out_one_real_sensor_from_stepping_the_clock_back() {
    let (mut t, mut out, mut rng) = fleet_until(20);
    for (k, (a, sensor)) in [(0xF1u32, 7u16), (0xF2, 8), (0xF3, 7)].iter().enumerate() {
        let mut m = far(0, *sensor, 21.0, 400 + k as i64, &mut rng);
        m.addr = *a;
        tracker_update(&mut t, &m, &mut out);
    }
    assert_eq!(tracker_stats(&t).time_steps, 1);
    // The second forging sensor delivers once more at the forged time: both are live there.
    let mut m = far(0, 8, 21.0, 403, &mut rng);
    m.addr = 0xF4;
    tracker_update(&mut t, &m, &mut out);
    // From here one real sensor only, many aircraft.
    for s in 21..=45 {
        for a in 0..6 {
            tracker_update(&mut t, &meas(a, 1, s as f64, &mut rng), &mut out);
        }
    }
    assert_eq!(tracker_stats(&t).time_steps_back, 1); // NEW-STAT
    let live = tracker_live(&t);
    assert_eq!(live.len(), 6, "{live:?}");
    assert!(live.iter().all(|l| l.1 >= 44 * S));
}

// REQ: OA-TRK-197
#[test]
fn a_stale_batch_from_one_slow_sensor_does_not_step_the_clock_back() {
    let (mut t, mut out, mut rng) = fleet_until(30);
    for s in 31..=50 {
        for a in 0..6 {
            tracker_update(&mut t, &meas(a, 1, s as f64, &mut rng), &mut out);
            // Sensor 2 delivers the same aircraft 15 s late.
            tracker_update(&mut t, &meas(a, 2, (s - 15) as f64, &mut rng), &mut out);
        }
    }
    let st = tracker_stats(&t);
    assert!(st.late_window >= 100, "{}", st.late_window);
    assert_eq!(st.time_steps_back, 0); // NEW-STAT
    assert_eq!(tracker_live(&t).len(), 6);
}

// REQ: OA-TRK-197
#[test]
fn the_sensor_table_reuses_the_least_recent_place_and_counts_it() {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 16);
    let mut rng = Rng(3);
    for k in 0..70u16 {
        tracker_update(&mut t, &meas(0, k, f64::from(k), &mut rng), &mut out);
    }
    assert_eq!(tracker_stats(&t).time_sensor_evicted, 6); // NEW-STAT
    assert_eq!(tracker_stats(&t).late_window, 0);
}

/// One live track of aircraft 0 reported by sensor 1 until `t1` seconds.
fn lone_track(t1: i64) -> (Tracker, TrackOutput, Rng) {
    let mut t = tracker_new(&big_config()).unwrap();
    let mut out = output_new(1 << 20);
    let mut rng = Rng(11);
    for s in 0..=t1 {
        tracker_update(&mut t, &meas(0, 1, s as f64, &mut rng), &mut out);
    }
    (t, out, rng)
}

// REQ: OA-TRK-234
#[test]
fn exact_repeats_of_one_far_ahead_measurement_are_not_a_run() {
    // Six live tracks from two sensors.
    let (mut t, mut out, mut rng) = fleet_until(10);
    let m = far(0, 3, 11.0, 10 + 299, &mut rng);
    for _ in 0..3 {
        tracker_update(&mut t, &m, &mut out);
    }
    assert_eq!(
        (tracker_stats(&t).time_held, tracker_stats(&t).time_steps),
        (3, 0)
    );
    assert_eq!(tracker_live(&t).len(), 6);
    // One live track (its own address) delivered by another sensor: the spread a run needs is
    // one sensor and one address, which three repeats of one report would meet.
    let (mut t, mut out, mut rng) = lone_track(10);
    let m = far(0, 3, 11.0, 10 + 299, &mut rng);
    for _ in 0..3 {
        tracker_update(&mut t, &m, &mut out);
    }
    let st = tracker_stats(&t);
    assert_eq!((st.time_held, st.time_steps), (3, 0), "held, steps");
    assert_eq!(tracker_live(&t), vec![(0xA00000, 10 * S)]);
    // Two different reports are still a run only at three: the repeats added nothing.
    tracker_update(&mut t, &far(0, 3, 12.0, 10 + 300, &mut rng), &mut out);
    assert_eq!(tracker_stats(&t).time_steps, 0);
    tracker_update(&mut t, &far(0, 3, 13.0, 10 + 301, &mut rng), &mut out);
    assert_eq!(tracker_stats(&t).time_steps, 1);
}

// REQ: OA-TRK-235
#[test]
fn repeats_of_a_far_ahead_measurement_of_the_only_track_and_sensor_add_nothing() {
    let (mut t, mut out, mut rng) = lone_track(10);
    let m = far(0, 1, 11.0, 10 + 299, &mut rng);
    tracker_update(&mut t, &m, &mut out);
    let after_first = *tracker_stats(&t);
    // Nothing else is at stake: the first steps the clock at once.
    assert_eq!((after_first.time_steps, after_first.time_held), (1, 0));
    for _ in 0..2 {
        tracker_update(&mut t, &m, &mut out);
    }
    let st = tracker_stats(&t);
    assert_eq!((st.time_steps, st.time_held, st.late_window), (1, 0, 0));
    assert_eq!(st.duplicates, 2);
    assert_eq!(st.applied, after_first.applied);
    assert_eq!(oa_track::tracker_newest_update_ns(&t), (10 + 299) * S);
}
