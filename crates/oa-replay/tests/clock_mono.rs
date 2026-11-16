//! Tests of the replay clock's use of the recorder's monotonic time: a real gap in a recording
//! against a step of the recorder's wall clock, and recordings made without the time. Every
//! identifier and time is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_feed::{encode_frame, encode_frame_mono, Kind};
use oa_replay::{
    clock_observe_mono, pipeline_stale_count, process_frame_mono, replay_state_digest, run_streams,
    spec_from_bytes, Clock, FeedKind, Pipeline, Tick, CLOCK_TOLERANCE_NS,
};

const T0: i64 = DAY * 1_000_000 + 3_500_000_000;
const MIN: i64 = 60 * NS;
const HOUR: i64 = 60 * MIN;
/// The recorder's monotonic time at the first frame (it started a little before).
const M0: u64 = 5_000_000_000;

fn ms(t: i64) -> i64 {
    (t - DAY * 1_000_000) / 1_000_000
}

/// One frame of a synthetic recording: its wall time and, if the recording has one, the
/// monotonic time.
struct Fr {
    wall: i64,
    mono: Option<u64>,
    payload: Vec<u8>,
}

fn status(wall: i64, mono: Option<u64>) -> Fr {
    Fr {
        wall,
        mono,
        payload: b"state=up\n".to_vec(),
    }
}

/// An ACTIVE flight (silence allowed: 2 hours), reported once.
fn flight(wall: i64, mono: Option<u64>) -> Fr {
    let doc = sf_doc(&[sf(1, ms(wall))
        .call("TST1")
        .pos(41.0, -88.0, ms(wall))
        .status("ACTIVE")]);
    Fr {
        wall,
        mono,
        payload: doc.into_bytes(),
    }
}

fn encode(frames: &[Fr]) -> Vec<u8> {
    let mut out = Vec::new();
    for (seq, f) in frames.iter().enumerate() {
        let mut v = vec![0u8; f.payload.len() + 64];
        let n = match f.mono {
            Some(m) => encode_frame_mono(&mut v, 1, 0, seq as u64, f.wall, 0, m, &f.payload),
            None => encode_frame(&mut v, 1, 0, seq as u64, f.wall, 0, &f.payload),
        }
        .unwrap();
        out.extend_from_slice(&v[..n]);
    }
    out
}

fn replay(frames: &[Fr]) -> Pipeline {
    let mut p = small_pipeline(64);
    let specs = vec![spec_from_bytes(FeedKind::Sfdps, vec![encode(frames)])];
    run_streams(&mut p, specs, 4096, &mut |_| {}).unwrap();
    p
}

/// Ten quiet seconds, the flight at the fifth, then `after` seconds-spaced frames starting at
/// `wall0` with monotonic time `mono0` (`None`: a recording with no monotonic time).
fn with_gap(wall0: i64, mono0: Option<u64>, strip_mono: bool) -> Vec<Fr> {
    let m = |k: i64| (!strip_mono).then_some(M0 + (k as u64) * NS as u64);
    let mut v = Vec::new();
    for k in 0..10 {
        v.push(if k == 4 {
            flight(T0 + k * NS, m(k))
        } else {
            status(T0 + k * NS, m(k))
        });
    }
    for k in 0..5 {
        let mono = mono0.map(|b| b + (k as u64) * NS as u64);
        v.push(status(wall0 + k * NS, if strip_mono { None } else { mono }));
    }
    v
}

// REQ: OA-RPL-266
#[test]
fn a_genuine_three_hour_gap_expires_a_track_that_was_not_heard_again() {
    // The recorder ran through the three hours (wall and monotonic time advance together) and
    // nothing arrived: the flight's 2-hour limit passes in the silence.
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + 3 * HOUR as u64;
    let p = replay(&with_gap(wall0, Some(mono0), false));
    assert_eq!(
        p.table.live, 0,
        "the silent flight was kept across a real gap"
    );
    assert_eq!(p.table.stats.evicted, 1);
    assert_eq!(
        pipeline_stale_count(&p),
        0,
        "no row is marked by a real gap"
    );
}

// REQ: OA-RPL-266
#[test]
fn the_first_frame_after_a_real_gap_is_processed_at_its_own_time() {
    let mut p = small_pipeline(64);
    for k in 0..10 {
        let t = T0 + k * NS;
        process_frame_mono(
            &mut p,
            FeedKind::Sfdps,
            Kind::Status,
            t,
            0,
            Some(M0 + k as u64 * NS as u64),
            &[],
        );
    }
    let t = T0 + 9 * NS + 3 * HOUR;
    let m = M0 + 9 * NS as u64 + 3 * HOUR as u64;
    process_frame_mono(&mut p, FeedKind::Sfdps, Kind::Status, t, 0, Some(m), &[]);
    assert_eq!(p.replay_ns, t, "processed at the old replay time");
    assert_eq!((p.clock.real_gaps, p.clock.held), (1, 0));
}

// REQ: OA-RPL-267
#[test]
fn a_wall_clock_step_of_three_hours_in_one_second_is_translated() {
    // The wall clock jumps +3 h while the monotonic time moves one second: a step, handled as
    // before: ages shift, the flight is kept and marked stale.
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + NS as u64;
    let p = replay(&with_gap(wall0, Some(mono0), false));
    assert_eq!(p.clock.forward_steps, 1);
    assert_eq!((p.clock.real_gaps, p.clock.mono_steps), (0, 1));
    assert_eq!(p.table.live, 1);
    assert_eq!(pipeline_stale_count(&p), 1);
}

// REQ: OA-RPL-267
#[test]
fn a_backward_wall_clock_step_is_translated() {
    let wall0 = T0 + 9 * NS - 3 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + NS as u64;
    let p = replay(&with_gap(wall0, Some(mono0), false));
    assert_eq!(p.clock.backward_steps, 1);
    assert_eq!((p.clock.real_gaps, p.clock.mono_steps), (0, 1));
    assert_eq!(p.table.live, 1);
}

// REQ: OA-RPL-268
#[test]
fn a_recording_with_no_monotonic_time_behaves_as_before_across_a_gap() {
    // The same wall times as the genuine gap, in a recording without the field: translated, the
    // flight kept, and the digest is that of feeding the frames with no monotonic time at all.
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let frames = with_gap(wall0, None, true);
    let p = replay(&frames);
    assert_eq!(p.clock.forward_steps, 1);
    assert_eq!(
        (
            p.clock.real_gaps,
            p.clock.mono_steps,
            p.clock.unclassified_steps
        ),
        (0, 0, 0)
    );
    assert_eq!(p.clock.domain_log, 0);
    assert_eq!(p.table.live, 1);
    let mut q = small_pipeline(64);
    for f in &frames {
        let kind = Kind::Message;
        oa_replay::process_frame(&mut q, FeedKind::Sfdps, kind, f.wall, &f.payload);
    }
    assert_eq!(replay_state_digest(&p), replay_state_digest(&q));
}

// REQ: OA-RPL-269
#[test]
fn two_replays_that_classify_a_gap_differently_do_not_hash_the_same() {
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let real = replay(&with_gap(
        wall0,
        Some(M0 + 9 * NS as u64 + 3 * HOUR as u64),
        false,
    ));
    let step = replay(&with_gap(
        wall0,
        Some(M0 + 9 * NS as u64 + NS as u64),
        false,
    ));
    let none = replay(&with_gap(wall0, None, true));
    let (a, b, c) = (
        replay_state_digest(&real),
        replay_state_digest(&step),
        replay_state_digest(&none),
    );
    assert!(a != b && a != c && b != c, "digests collide");
    // The same recording twice hashes the same.
    let again = replay(&with_gap(
        wall0,
        Some(M0 + 9 * NS as u64 + NS as u64),
        false,
    ));
    assert_eq!(replay_state_digest(&again), b);
    // Two steps that differ only in the monotonic time elapsed log differently.
    let step2 = replay(&with_gap(
        wall0,
        Some(M0 + 9 * NS as u64 + 2 * NS as u64),
        false,
    ));
    assert_ne!(step2.clock.domain_log, step.clock.domain_log);
    assert_ne!(replay_state_digest(&step2), b);
}

// REQ: OA-RPL-270
#[test]
fn the_classification_rule_at_its_tolerance() {
    let mut c = Clock::default();
    let mut now = i64::MIN;
    let first = clock_observe_mono(&mut c, 0, now, T0, 0, Some(M0));
    assert_eq!(first, Tick::Advance);
    now = T0;
    // Elapsed times that differ by exactly the clock tolerance are noise: time that passed; one
    // nanosecond more is a step.
    let wall = T0 + 3 * HOUR;
    let agree = M0 + 3 * HOUR as u64 - CLOCK_TOLERANCE_NS as u64;
    let mut ok = c;
    assert_eq!(
        clock_observe_mono(&mut ok, 0, now, wall, 0, Some(agree)),
        Tick::RealGap
    );
    let mut stepped = c;
    let t = clock_observe_mono(&mut stepped, 0, now, wall, 0, Some(agree - 1));
    assert_eq!(t, Tick::Held { back: false });
    assert_eq!(stepped.real_gaps, 0);
    // A monotonic time that went back (a recorder restart) is not classified.
    let mut back = c;
    let t = clock_observe_mono(&mut back, 0, now, wall, 0, Some(M0 - 1));
    assert_eq!(t, Tick::Held { back: false });
    // A frame with no monotonic time is not classified either.
    let mut none = c;
    assert_eq!(
        clock_observe_mono(&mut none, 0, now, wall, 0, None),
        Tick::Held { back: false }
    );
    // Behind the clock a matching gap is not a real gap: only a forward frame is accepted.
    let mut behind = c;
    let t = clock_observe_mono(&mut behind, 0, now, T0 - 3 * HOUR, 0, Some(M0 + NS as u64));
    assert_eq!(t, Tick::Held { back: true });
}

/// Replays several streams of the same feed together (each its own recorder run).
fn replay_streams(streams: &[&[Fr]]) -> Pipeline {
    let mut p = small_pipeline(64);
    let specs = streams
        .iter()
        .map(|s| spec_from_bytes(FeedKind::Sfdps, vec![encode(s)]))
        .collect();
    run_streams(&mut p, specs, 4096, &mut |_| {}).unwrap();
    p
}

// REQ: OA-RPL-271
#[test]
fn monotonic_times_of_different_recorder_runs_are_never_compared() {
    // Two recordings of the same feed from different runs: the second one's monotonic origin is
    // 3 hours after the first one's, so by coincidence its first frame looks like a genuine
    // 3-hour gap after the first one's last. They are not the same run: nothing says time passed.
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + 3 * HOUR as u64;
    let frames = with_gap(wall0, Some(mono0), false);
    let (a, b) = frames.split_at(10);
    let p = replay_streams(&[a, b]);
    assert_eq!(
        p.clock.real_gaps, 0,
        "a gap across two recorder runs was taken as real"
    );
    assert_eq!(
        p.table.live, 1,
        "the flight expired across two recorder runs"
    );
    assert_eq!(p.clock.forward_steps, 1);
}

// REQ: OA-RPL-272
#[test]
fn a_recorder_stall_before_the_stamp_does_not_turn_an_outage_into_a_step() {
    // A 3-hour outage whose first frame waited 20 seconds in the recorder before it was stamped:
    // the monotonic time is 20 seconds short of the wall time. That is noise, not a step.
    let wall0 = T0 + 9 * NS + 3 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + 3 * HOUR as u64 - 20 * NS as u64;
    let p = replay(&with_gap(wall0, Some(mono0), false));
    assert_eq!(
        p.clock.real_gaps, 1,
        "the outage was not taken as time that passed"
    );
    assert_eq!(
        p.table.live, 0,
        "the silent flight was kept across the outage"
    );
}

// REQ: OA-RPL-272
#[test]
fn a_real_gap_that_also_holds_a_wall_step_is_both() {
    // 1.5 hours of recorder time pass and the wall clock also steps +30 minutes: the clock takes
    // the new wall time, the ages move by the 1.5 hours and not the 30 minutes.
    let wall0 = T0 + 9 * NS + 2 * HOUR;
    let mono0 = M0 + 9 * NS as u64 + 90 * MIN as u64;
    let p = replay(&with_gap(wall0, Some(mono0), false));
    assert_eq!(
        (p.clock.forward_steps, p.clock.mono_steps, p.clock.real_gaps),
        (1, 1, 0)
    );
    assert_eq!(p.table.live, 1, "a flight silent for 1.5 hours expired");
    let age = p.replay_ns - p.table.last_update_ns.iter().copied().max().unwrap();
    assert!(
        (90 * MIN..91 * MIN).contains(&age),
        "the flight's age is {} s, not the 1.5 hours that passed",
        age / NS
    );
}

/// A clock at `T0` whose feeds `0..n` each have an accepted frame at `T0` with monotonic time
/// `M0` of stream `feed`.
fn clock_at_t0(n: usize) -> Clock {
    let mut c = Clock::default();
    let mut now = i64::MIN;
    for f in 0..n {
        let t = clock_observe_mono(&mut c, f, now, T0, f as u32, Some(M0));
        assert!(matches!(t, Tick::Advance | Tick::Keep), "{t:?}");
        now = T0;
    }
    c
}

// REQ: OA-RPL-271
#[test]
fn a_frame_of_another_stream_is_not_classified_even_if_the_times_agree() {
    let c = clock_at_t0(1);
    let (wall, mono) = (T0 + 3 * HOUR, M0 + 3 * HOUR as u64);
    let mut same = c;
    assert_eq!(
        clock_observe_mono(&mut same, 0, T0, wall, 0, Some(mono)),
        Tick::RealGap
    );
    let mut other = c;
    assert_eq!(
        clock_observe_mono(&mut other, 0, T0, wall, 1, Some(mono)),
        Tick::Held { back: false }
    );
    assert_eq!(other.real_gaps, 0);
    // Three such frames are a step the old way, counted as one that could not be compared.
    let mut c3 = c;
    for k in 0..3 {
        let t = wall + k * NS;
        let r = clock_observe_mono(&mut c3, 0, T0, t, 1, Some(mono + k as u64 * NS as u64));
        if k == 2 {
            assert_eq!(
                r,
                Tick::Step {
                    delta: 3 * HOUR + 2 * NS
                }
            );
        }
    }
    assert_eq!((c3.mono_steps, c3.unclassified_steps), (0, 1));
}

// REQ: OA-RPL-272
#[test]
fn a_gap_is_its_monotonic_time_plus_a_step_and_only_the_step_shifts_the_ages() {
    // 90 minutes pass and the wall clock steps +30 minutes: three candidates, then a step whose
    // shift is the 30 minutes, not the 2 hours the clock moves.
    let mut c = clock_at_t0(1);
    let mut last = Tick::Keep;
    for k in 0..3 {
        last = clock_observe_mono(
            &mut c,
            0,
            T0,
            T0 + 2 * HOUR + k * NS,
            0,
            Some(M0 + 90 * MIN as u64 + k as u64 * NS as u64),
        );
    }
    assert_eq!(last, Tick::Step { delta: 30 * MIN });
    assert_eq!((c.forward_steps, c.mono_steps, c.real_gaps), (1, 1, 0));
    // A pure step: the wall time elapsed is 3 hours + 2 seconds and the monotonic time elapsed
    // 3 seconds, so the step is 3 hours less 1 second.
    let mut p = clock_at_t0(1);
    let mut last = Tick::Keep;
    for k in 0..3 {
        last = clock_observe_mono(
            &mut p,
            0,
            T0,
            T0 + 3 * HOUR + k * NS,
            0,
            Some(M0 + NS as u64 + k as u64 * NS as u64),
        );
    }
    assert_eq!(
        last,
        Tick::Step {
            delta: 3 * HOUR - NS
        }
    );
}

// REQ: OA-RPL-272
#[test]
fn the_vote_holds_for_a_gap_that_is_partly_a_step() {
    // Feed 0 steps back an hour (its monotonic time moves a little). Feeds 1 and 2 speak at the
    // old time: the step is outvoted. When they back the new time, it goes through, shifted by
    // the hour alone.
    let back = |c: &mut Clock, f: usize, k: i64| {
        clock_observe_mono(
            c,
            f,
            T0,
            T0 - HOUR + (k + 1) * NS,
            f as u32,
            Some(M0 + (k as u64 + 1) * NS as u64),
        )
    };
    let mut c = clock_at_t0(3);
    let r: Vec<Tick> = (0..3).map(|k| back(&mut c, 0, k)).collect();
    assert_eq!(r[2], Tick::Held { back: true });
    assert_eq!((c.steps_outvoted, c.backward_steps), (1, 0));
    let mut c = clock_at_t0(3);
    for f in [1, 2] {
        assert_eq!(back(&mut c, f, 0), Tick::Held { back: true });
    }
    let r: Vec<Tick> = (0..3).map(|k| back(&mut c, 0, k)).collect();
    assert_eq!(r[2], Tick::Step { delta: -HOUR });
    assert_eq!(
        (c.steps_outvoted, c.backward_steps, c.mono_steps),
        (0, 1, 1)
    );
}
