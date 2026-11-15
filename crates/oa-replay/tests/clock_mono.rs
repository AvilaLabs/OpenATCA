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
    spec_from_bytes, Clock, FeedKind, Pipeline, Tick, CLOCK_TOLERANCE_NS, MONO_AGREE_TOL_NS,
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
            Some(M0 + k as u64 * NS as u64),
            &[],
        );
    }
    let t = T0 + 9 * NS + 3 * HOUR;
    let m = M0 + 9 * NS as u64 + 3 * HOUR as u64;
    process_frame_mono(&mut p, FeedKind::Sfdps, Kind::Status, t, Some(m), &[]);
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
    let first = clock_observe_mono(&mut c, 0, now, T0, Some(M0));
    assert_eq!(first, Tick::Advance);
    now = T0;
    // Elapsed times that differ by exactly the tolerance agree; one nanosecond more is a step.
    let wall = T0 + 3 * HOUR;
    let agree = M0 + 3 * HOUR as u64 - MONO_AGREE_TOL_NS as u64;
    let mut ok = c;
    assert_eq!(
        clock_observe_mono(&mut ok, 0, now, wall, Some(agree)),
        Tick::RealGap
    );
    let mut stepped = c;
    let t = clock_observe_mono(&mut stepped, 0, now, wall, Some(agree - 1));
    assert_eq!(t, Tick::Held { back: false });
    assert_eq!(stepped.real_gaps, 0);
    // A monotonic time that went back (a recorder restart) is not classified.
    let mut back = c;
    let t = clock_observe_mono(&mut back, 0, now, wall, Some(M0 - 1));
    assert_eq!(t, Tick::Held { back: false });
    // A frame with no monotonic time is not classified either.
    let mut none = c;
    assert_eq!(
        clock_observe_mono(&mut none, 0, now, wall, None),
        Tick::Held { back: false }
    );
    // Behind the clock a matching gap is not a real gap: only a forward frame is accepted.
    let mut behind = c;
    let t = clock_observe_mono(&mut behind, 0, now, T0 - 3 * HOUR, Some(M0 + NS as u64));
    assert_eq!(t, Tick::Held { back: true });
    const { assert!(CLOCK_TOLERANCE_NS > 10 * MONO_AGREE_TOL_NS) };
}
