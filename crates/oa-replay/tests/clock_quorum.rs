//! Tests of the replay clock's vote and single-feed rules, mostly through the merge the callers
//! use (frames reach the clock in merged receive order). Every identifier and time is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_feed::Kind;
use oa_replay::{
    clock_observe, process_frame, run_streams, slot_of_gufi, spec_from_bytes, Clock, FeedKind,
    Pipeline, Tick, CLOCK_TOLERANCE_NS, FEED_GAP_MAX_NS,
};

const T0: i64 = DAY * 1_000_000 + 3_500_000_000;
const MIN: i64 = 60 * NS;

/// How many runs of three candidates the vote refused.
fn outvoted(c: &Clock) -> u64 {
    c.steps_outvoted
}

/// A recording of STATUS frames (they move the clock and carry no data) at the given times, in
/// file order.
fn status_stream(times: &[i64]) -> Vec<u8> {
    let mut b = Vec::new();
    for (seq, &t) in times.iter().enumerate() {
        b.extend(frame(3, seq as u64, t, b""));
    }
    b
}

/// An SFDPS recording of one flight reported at each of the times.
fn flight_stream(n: u32, times: &[i64]) -> Vec<u8> {
    let mut b = Vec::new();
    for (seq, &t) in times.iter().enumerate() {
        let m = sf(n, (t - DAY * 1_000_000) / 1_000_000).call("TST1");
        b.extend(frame(1, seq as u64, t, sf_doc(&[m]).as_bytes()));
    }
    b
}

fn run(p: &mut Pipeline, streams: Vec<(FeedKind, Vec<u8>)>) {
    let specs = streams
        .into_iter()
        .map(|(f, b)| spec_from_bytes(f, vec![b]))
        .collect();
    run_streams(p, specs, 64, &mut |_| {}).unwrap();
}

// REQ: OA-RPL-180
#[test]
fn a_stream_that_runs_ahead_cannot_move_the_clock_past_the_others_through_the_merge() {
    // Stream A's times walk 119 s per frame; stream B talks every second at real time for 30
    // minutes. A's frames wait in the merge until B has passed them, so the clock is never ahead
    // of B's latest frame while B has frames, and B's flight is never evicted.
    let b_times: Vec<i64> = (0..1800).map(|k| T0 + k * NS).collect();
    let a_times: Vec<i64> = (1..=100).map(|k| T0 + k * 119 * NS).collect();
    let specs = vec![
        spec_from_bytes(FeedKind::Sfdps, vec![flight_stream(1, &b_times)]),
        spec_from_bytes(FeedKind::Tbfm, vec![status_stream(&a_times)]),
    ];
    let mut p = small_pipeline(64);
    let b_last = *b_times.last().unwrap();
    let mut worst = i64::MIN;
    let mut seen_flight = false;
    let mut after = |p: &Pipeline| {
        let b_done = p.frames[FeedKind::Sfdps as usize] as i64;
        if (1..1800).contains(&b_done) {
            let latest = T0 + (b_done - 1) * NS;
            // Checked at every frame while B still has frames.
            worst = worst.max(p.replay_ns - latest);
            seen_flight |= slot_of_gufi(&p.table, gufi(1)).is_some();
            assert!(
                slot_of_gufi(&p.table, gufi(1)).is_some(),
                "B's flight evicted"
            );
        }
    };
    run_streams(&mut p, specs, 64, &mut after).unwrap();
    assert!(seen_flight);
    assert!(
        worst <= CLOCK_TOLERANCE_NS,
        "clock ahead of B by {worst} ns"
    );
    assert!(p.replay_ns >= b_last);
}

// REQ: OA-RPL-181
#[test]
fn two_feeds_in_disagreement_do_not_step_the_clock_back_and_forth() {
    // A talks at real time; B is ten minutes ahead, in runs of three frames. B's run is a
    // forward step only if more feeds back it than back the old clock; A alone does, so the
    // clock never steps (a tie would let each feed in turn move it).
    let mut c = Clock::default();
    let mut now = i64::MIN;
    let mut t = T0;
    let tick = |c: &mut Clock, now: &mut i64, f: usize, t: i64| match clock_observe(c, f, *now, t) {
        Tick::Advance => *now = t,
        Tick::Step { .. } => *now = t,
        _ => {}
    };
    tick(&mut c, &mut now, 0, t);
    tick(&mut c, &mut now, 1, t);
    for _ in 0..10 {
        for _ in 0..3 {
            t += NS;
            tick(&mut c, &mut now, 1, t + 10 * MIN);
        }
        t += NS;
        tick(&mut c, &mut now, 0, t);
    }
    assert_eq!((c.forward_steps, c.backward_steps), (0, 0));
    assert!(outvoted(&c) > 0);
    assert_eq!(now, t, "the clock follows A");
}

// REQ: OA-RPL-182
#[test]
fn a_feed_that_jumps_back_alone_is_outvoted_by_the_others_through_the_merge() {
    // Three feeds agree; a fourth's recorder clock jumps ten minutes back mid-recording. Its
    // frames come out of the merge first (they are the earliest) and stay a run of candidates the
    // other three outvote; the clock ends where they are.
    let ok: Vec<i64> = (0..60).map(|k| T0 + k * NS).collect();
    let skewed: Vec<i64> = (0..10)
        .map(|k| T0 + k * NS)
        .chain((10..60).map(|k| T0 - 10 * MIN + k * NS))
        .collect();
    let mut p = small_pipeline(64);
    run(
        &mut p,
        vec![
            (FeedKind::Sfdps, status_stream(&ok)),
            (FeedKind::Tbfm, status_stream(&ok)),
            (FeedKind::Tfms, status_stream(&ok)),
            (FeedKind::Fns, status_stream(&skewed)),
        ],
    );
    assert_eq!(p.clock.backward_steps, 0);
    assert!(outvoted(&p.clock) > 0);
    assert_eq!(p.replay_ns, T0 + 59 * NS);
}

// REQ: OA-RPL-183
#[test]
fn a_feed_first_in_merge_order_ten_minutes_behind_is_followed_by_the_majority() {
    // The skewed feed speaks first and the clock starts there. Three feeds that agree come ten
    // minutes later; the first to complete a run finds the others' runs backing it, and the
    // clock takes their time.
    let skew: Vec<i64> = (0..20).map(|k| T0 - 10 * MIN + k * NS).collect();
    let ok: Vec<i64> = (0..30).map(|k| T0 + k * NS).collect();
    let mut p = small_pipeline(64);
    run(
        &mut p,
        vec![
            (FeedKind::Fns, status_stream(&skew)),
            (FeedKind::Sfdps, status_stream(&ok)),
            (FeedKind::Tbfm, status_stream(&ok)),
            (FeedKind::Tfms, status_stream(&ok)),
        ],
    );
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (1, 0));
    assert_eq!(p.replay_ns, T0 + 29 * NS);
}

// REQ: OA-RPL-184
#[test]
fn a_recorder_clock_step_back_of_an_hour_on_every_feed_is_one_step_with_ages_shifted() {
    let times: Vec<i64> = (0..30)
        .map(|k| T0 + k * NS)
        .chain((0..30).map(|k| T0 - 60 * MIN + 30 * NS + k * NS))
        .collect();
    let mut p = small_pipeline(64);
    // The first feed carries a flight at the start, then status frames; the others status only.
    let mut s0 = flight_stream(1, &times[..1]);
    for (seq, &t) in times[1..].iter().enumerate() {
        s0.extend(frame(3, 1 + seq as u64, t, b""));
    }
    run(
        &mut p,
        vec![
            (FeedKind::Sfdps, s0),
            (FeedKind::Tbfm, status_stream(&times)),
            (FeedKind::Tfms, status_stream(&times)),
            (FeedKind::Fns, status_stream(&times)),
            (FeedKind::Adsb, status_stream(&times)),
        ],
    );
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 1));
    assert!(
        slot_of_gufi(&p.table, gufi(1)).is_some(),
        "the step evicted the flight"
    );
    assert_eq!(p.replay_ns, T0 - 60 * MIN + 59 * NS);
    assert!(p.stale_marked > 0);
}

// REQ: OA-RPL-185
#[test]
fn a_lone_feed_with_five_minute_gaps_is_followed_frame_by_frame_and_rows_age_out() {
    let times: Vec<i64> = (0..10).map(|k| T0 + k * 5 * MIN).collect();
    let mut p = small_pipeline(64);
    let mut stream = flight_stream(1, &times[..1]);
    for (seq, &t) in times[1..].iter().enumerate() {
        stream.extend(frame(3, 1 + seq as u64, t, b""));
    }
    let specs = vec![spec_from_bytes(FeedKind::Sfdps, vec![stream])];
    let mut clocks = Vec::new();
    run_streams(&mut p, specs, 64, &mut |p| clocks.push(p.replay_ns)).unwrap();
    assert_eq!(clocks, times, "the clock follows every frame");
    assert_eq!(
        (p.clock.forward_steps, p.clock.backward_steps, p.clock.held),
        (0, 0, 0)
    );
    assert_eq!(p.stale_marked, 0, "no step: ages ran");
    // The flight was last heard 45 minutes before the last frame: aged out.
    assert!(slot_of_gufi(&p.table, gufi(1)).is_none());
}

// REQ: OA-RPL-186
#[test]
fn a_lone_feed_with_a_gap_over_the_limit_is_a_confirmed_step() {
    let first: Vec<i64> = (0..10).map(|k| T0 + k * NS).collect();
    let second: Vec<i64> = (0..10).map(|k| T0 + 45 * MIN + k * NS).collect();
    let mut p = small_pipeline(64);
    let mut s = flight_stream(1, &first[..1]);
    for (seq, &t) in first[1..].iter().chain(second.iter()).enumerate() {
        s.extend(frame(3, 1 + seq as u64, t, b""));
    }
    run(&mut p, vec![(FeedKind::Sfdps, s)]);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (1, 0));
    assert_eq!(p.replay_ns, T0 + 45 * MIN + 9 * NS);
    assert!(
        slot_of_gufi(&p.table, gufi(1)).is_some(),
        "ages carried over the pause"
    );
}

// REQ: OA-RPL-187
#[test]
fn two_sparse_feeds_interleaved_do_not_freeze_the_clock_it_follows_the_first_one() {
    // Each feed is 5 minutes between frames, offset by 2.5 minutes: no frame is within the
    // tolerance of its own feed's last or of the clock. The first feed to speak is alone while the
    // other has had no accepted frame, so its frames are lone-forward and the clock follows them;
    // the other feed's runs of three are refused by the vote (the first feed backs the old
    // clock), so its frames are held and the clock never steps.
    let a: Vec<i64> = (0..30).map(|k| T0 + k * 5 * MIN).collect();
    let b: Vec<i64> = (0..30).map(|k| T0 + 150 * NS + k * 5 * MIN).collect();
    let mut p = small_pipeline(64);
    run(
        &mut p,
        vec![
            (FeedKind::Sfdps, status_stream(&a)),
            (FeedKind::Tbfm, status_stream(&b)),
        ],
    );
    assert_eq!(p.replay_ns, *a.last().unwrap());
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (0, 0));
    assert!(outvoted(&p.clock) > 20);
    assert_eq!(p.clock.held, 30);
}

// REQ: OA-RPL-189
#[test]
fn a_single_wild_forward_frame_in_a_single_feed_recording_is_accepted_within_the_gap_limit() {
    // The stated weakness: with one feed a gap and a glitch cannot be told apart, so a frame
    // 9.9 minutes ahead is taken as time that passed; one more than the limit ahead is not.
    let mut p = small_pipeline(64);
    for k in 0..10 {
        process_frame(&mut p, FeedKind::Sfdps, Kind::Status, T0 + k * NS, &[]);
    }
    let wild = T0 + 9 * NS + 594 * NS;
    process_frame(&mut p, FeedKind::Sfdps, Kind::Status, wild, &[]);
    assert_eq!(p.replay_ns, wild);
    let far = wild + FEED_GAP_MAX_NS + NS;
    process_frame(&mut p, FeedKind::Sfdps, Kind::Status, far, &[]);
    assert_eq!(p.replay_ns, wild);
    assert_eq!(p.clock.held, 1);
}
