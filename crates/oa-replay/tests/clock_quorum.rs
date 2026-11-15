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
fn a_backward_run_is_a_step_only_when_the_feeds_at_the_clock_do_not_outnumber_it() {
    // Observed directly. Three feeds talk at the clock; a fourth runs three frames ten minutes
    // behind: two feeds' worth of votes are not needed, three back the old clock, so the run is
    // refused and counted, and asks again on its next candidate.
    let mut c = Clock::default();
    let now = T0;
    assert_eq!(clock_observe(&mut c, 0, i64::MIN, T0), Tick::Advance);
    for f in 1..4 {
        assert_eq!(clock_observe(&mut c, f, now, now), Tick::Advance);
    }
    for k in 1..=4 {
        let t = T0 - 10 * MIN + k * NS;
        let tick = clock_observe(&mut c, 3, now, t);
        assert_eq!(tick, Tick::Held { back: true });
    }
    assert_eq!(c.backward_steps, 0);
    assert_eq!(c.steps_outvoted, 2, "the third and the fourth frame asked");
    // With only one feed at the clock the tie goes to the earlier time: the run is a step.
    let mut c = Clock::default();
    assert_eq!(clock_observe(&mut c, 0, i64::MIN, T0), Tick::Advance);
    assert_eq!(clock_observe(&mut c, 1, T0, T0), Tick::Advance);
    let mut last = Tick::Keep;
    for k in 1..=3 {
        last = clock_observe(&mut c, 1, T0, T0 - 10 * MIN + k * NS);
    }
    assert!(matches!(last, Tick::Step { delta } if delta < 0));
    assert_eq!((c.forward_steps, c.backward_steps), (0, 1));
    // A forward run is a step whoever else spoke at the clock: the others are silent over the gap.
    let mut c = Clock::default();
    assert_eq!(clock_observe(&mut c, 0, i64::MIN, T0), Tick::Advance);
    for f in 1..4 {
        assert_eq!(clock_observe(&mut c, f, T0, T0), Tick::Advance);
    }
    for k in 1..=3 {
        last = clock_observe(&mut c, 4, T0, T0 + 10 * MIN + k * NS);
    }
    assert!(matches!(last, Tick::Step { delta } if delta > 0));
    assert_eq!((c.forward_steps, c.backward_steps), (1, 0));
    assert_eq!(c.steps_outvoted, 0);
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
fn two_sparse_feeds_interleaved_do_not_freeze_the_clock_it_moves_by_steps() {
    // Each feed is 5 minutes between frames, offset by 2.5 minutes: no frame is within the
    // tolerance of its own feed's last or of the clock. The first feed to speak is alone while
    // the other has had no accepted frame, so the clock follows it; once the second feed's runs
    // of three confirm as forward steps (no vote), both feeds are known and neither is accepted
    // by proximity again, so from then on the clock moves in steps, each feed's run of three in
    // turn, and trails the newest frame by up to the time of a run. Observed: 11 forward steps
    // over 30 frames of each feed, the clock 450 s behind the first feed's last frame.
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
    let last = *a.last().unwrap().max(b.last().unwrap());
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (11, 0));
    assert!(p.replay_ns > T0 + 100 * MIN, "the clock froze");
    assert!(last - p.replay_ns <= 15 * MIN);
    assert_eq!(p.clock.steps_outvoted, 0);
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

// REQ: OA-RPL-190
#[test]
fn a_feed_resuming_alone_after_another_ended_is_a_forward_step_after_a_pause() {
    // Feed X's last frame is at T0 + 9 s; feed Y, which has spoken once before, alone resumes an
    // hour later. In merged order X has nothing in between, so Y's three frames are one forward
    // step: the flights of both feeds keep their ages (shifted) and are marked stale.
    let first: Vec<i64> = (0..10).map(|k| T0 + k * NS).collect();
    let second: Vec<i64> = (0..10).map(|k| T0 + 60 * MIN + k * NS).collect();
    let mut p = small_pipeline(64);
    let mut xs = flight_stream(1, &first[..1]);
    for (seq, &t) in first[1..].iter().enumerate() {
        xs.extend(frame(3, 1 + seq as u64, t, b""));
    }
    let mut ys = status_stream(&first[..1]);
    for (seq, &t) in second.iter().enumerate() {
        ys.extend(frame(3, 1 + seq as u64, t, b""));
    }
    run(&mut p, vec![(FeedKind::Sfdps, xs), (FeedKind::Adsb, ys)]);
    assert_eq!((p.clock.forward_steps, p.clock.backward_steps), (1, 0));
    assert_eq!(p.clock.steps_outvoted, 0);
    assert_eq!(p.replay_ns, T0 + 60 * MIN + 9 * NS);
    assert!(
        slot_of_gufi(&p.table, gufi(1)).is_some(),
        "the pause evicted the flight"
    );
    assert!(p.stale_marked > 0);
}
