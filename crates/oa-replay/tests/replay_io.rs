//! Tests of the checkpoint ring, the replay state digest and the failure of the output writes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use std::io::{self, Write};

use common::*;
use oa_feed::Kind;
use oa_replay::report::{drain_checkpoint_lines, summary, write_failure_message, write_summary};
use oa_replay::{
    new_pipeline, pipeline_drain_checkpoints, process_frame, replay_hash, replay_state_digest,
    run_streams_with, spec_from_bytes, ErrKind, FeedKind, Pipeline, PipelineConfig, PipelineError,
};

const T0: i64 = DAY * 1_000_000;

fn status(p: &mut Pipeline, t: i64) {
    process_frame(p, FeedKind::Sfdps, Kind::Status, t, b"state=up\n");
}

fn ring_pipeline(ring: usize) -> Pipeline {
    new_pipeline(PipelineConfig {
        checkpoint_ring: ring,
        ..PipelineConfig::default()
    })
    .unwrap()
}

/// One STATUS frame every 10 s: a checkpoint for each frame after the first.
fn run_frames(p: &mut Pipeline, n: i64, mut after: impl FnMut(&mut Pipeline)) {
    for k in 0..n {
        status(p, T0 + k * 10 * NS);
        after(p);
    }
}

// REQ: OA-RPL-230
#[test]
fn a_full_ring_drops_the_oldest_checkpoint_and_counts_it_without_growing() {
    let mut reference = ring_pipeline(1000);
    run_frames(&mut reference, 100, |_| {});
    let all = reference.checkpoints.to_vec();
    assert_eq!(all.len(), 99);
    assert_eq!(reference.checkpoints.dropped(), 0);

    let mut p = ring_pipeline(4);
    let cap = p.checkpoints.capacity();
    run_frames(&mut p, 100, |p| assert_eq!(p.checkpoints.capacity(), cap));
    assert_eq!(p.checkpoints.capacity(), 4);
    assert_eq!(p.checkpoints.len(), 4);
    assert_eq!(p.checkpoints.dropped(), 95);
    assert_eq!(p.checkpoints.recorded(), 99);
    // The ones kept are the newest, oldest first.
    assert_eq!(p.checkpoints.to_vec(), all[95..].to_vec());
    assert!(summary(&p, 0, 0).contains("checkpoints=99 checkpoints_dropped=95"));

    // The default ring is 24 hours at the default spacing, and does not move.
    let mut d = new_pipeline(PipelineConfig::default()).unwrap();
    assert_eq!(d.checkpoints.capacity(), 8_640);
    run_frames(&mut d, 500, |_| {});
    assert_eq!(d.checkpoints.capacity(), 8_640);
}

// REQ: OA-RPL-231
#[test]
fn draining_as_it_goes_drops_none_even_with_a_ring_of_one() {
    let mut reference = ring_pipeline(1000);
    run_frames(&mut reference, 100, |_| {});

    let mut p = ring_pipeline(1);
    let mut seen = Vec::new();
    run_frames(&mut p, 100, |p| {
        pipeline_drain_checkpoints(p, |c| seen.push(*c));
    });
    assert_eq!(p.checkpoints.dropped(), 0);
    assert!(p.checkpoints.is_empty());
    assert_eq!(seen, reference.checkpoints.to_vec());

    // The printing path of the binary: lines out, none dropped, `--quiet` takes them out unprinted.
    let mut q = ring_pipeline(1);
    let mut out: Vec<u8> = Vec::new();
    run_frames(&mut q, 100, |q| {
        drain_checkpoint_lines(q, &mut out, false).unwrap();
    });
    assert_eq!(q.checkpoints.dropped(), 0);
    assert_eq!(String::from_utf8(out).unwrap().lines().count(), 99);
    let mut quiet = ring_pipeline(1);
    let mut out: Vec<u8> = Vec::new();
    run_frames(&mut quiet, 100, |q| {
        drain_checkpoint_lines(q, &mut out, true).unwrap();
    });
    assert!(out.is_empty());
    assert_eq!(quiet.checkpoints.dropped(), 0);
}

// REQ: OA-RPL-232
#[test]
fn checkpoint_settings_must_be_sane() {
    let ok = |cfg: PipelineConfig| new_pipeline(cfg).map(|_| ());
    let base = PipelineConfig::default();
    assert_eq!(
        ok(PipelineConfig {
            checkpoint_ring: 0,
            ..base
        }),
        Err(PipelineError::CheckpointRing(0))
    );
    assert_eq!(
        ok(PipelineConfig {
            checkpoint_ns: 999_999_999,
            ..base
        }),
        Err(PipelineError::Checkpoint(999_999_999))
    );
    assert_eq!(
        ok(PipelineConfig {
            checkpoint_ns: 0,
            ..base
        }),
        Err(PipelineError::Checkpoint(0))
    );
    assert!(ok(PipelineConfig {
        checkpoint_ns: NS,
        checkpoint_ring: 1,
        ..base
    })
    .is_ok());
}

// REQ: OA-RPL-233
#[test]
fn equal_content_hash_but_different_clock_gives_different_state_digests() {
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let mut q = new_pipeline(PipelineConfig::default()).unwrap();
    status(&mut p, T0);
    status(&mut p, T0 + 7_200 * NS);
    status(&mut q, T0);
    status(&mut q, T0);
    // P holds a candidate step (2 hours ahead is past the clock tolerance and the feed gap).
    assert_eq!(p.clock.held, 1);
    assert_eq!(q.clock.held, 0);
    assert_eq!(replay_hash(&p), replay_hash(&q));
    assert_ne!(replay_state_digest(&p), replay_state_digest(&q));
    // The digest is a function of the state: the same input gives the same digest.
    let mut p2 = new_pipeline(PipelineConfig::default()).unwrap();
    status(&mut p2, T0);
    status(&mut p2, T0 + 7_200 * NS);
    assert_eq!(replay_state_digest(&p), replay_state_digest(&p2));

    // Identical further input: P steps, Q does not.
    for k in 1..=2 {
        status(&mut p, T0 + 7_200 * NS + k * NS);
        status(&mut q, T0 + 7_200 * NS + k * NS);
    }
    assert_eq!(p.clock.forward_steps, 1);
    assert_eq!(q.clock.forward_steps, 0);
    assert_ne!(p.replay_ns, q.replay_ns);
}

/// A sink that fails after `ok_writes` successful writes.
struct Failing {
    ok_writes: usize,
    kind: io::ErrorKind,
    got: Vec<u8>,
}

impl Write for Failing {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.ok_writes == 0 {
            return Err(io::Error::new(self.kind, "sink closed"));
        }
        self.ok_writes -= 1;
        self.got.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// REQ: OA-RPL-234
#[test]
fn a_failed_write_is_returned_and_the_checkpoint_stays_held() {
    let mut p = ring_pipeline(16);
    run_frames(&mut p, 50, |_| {});
    let held = p.checkpoints.len();
    assert!(held >= 4);
    // `writeln!` of one line may be one or two writes; allow the first line, then fail.
    let mut sink = Failing {
        ok_writes: 2,
        kind: io::ErrorKind::BrokenPipe,
        got: Vec::new(),
    };
    let err = drain_checkpoint_lines(&mut p, &mut sink, false).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    let printed = String::from_utf8(sink.got.clone()).unwrap().lines().count();
    // Nothing is taken out of the ring for a line that was not written.
    assert_eq!(p.checkpoints.len(), held - printed);
    assert!(!p.checkpoints.is_empty());
    // The next checkpoint to print is the first that failed.
    let mut again = Vec::new();
    drain_checkpoint_lines(&mut p, &mut again, false).unwrap();
    assert_eq!(
        String::from_utf8(again).unwrap().lines().count(),
        held - printed
    );

    // The summary and timing line fail too, and the message names a closed pipe.
    let mut sink = Failing {
        ok_writes: 0,
        kind: io::ErrorKind::BrokenPipe,
        got: Vec::new(),
    };
    let e = write_summary(&p, &mut sink, 1, 1, 1.0, 0).unwrap_err();
    assert!(write_failure_message(&e).contains("broken pipe"));
    let other = io::Error::new(io::ErrorKind::StorageFull, "no space");
    let m = write_failure_message(&other);
    assert!(m.starts_with("oa-replay: cannot write output:") && !m.contains('\n'));
    // A working sink gets the summary and the timing line.
    let mut good: Vec<u8> = Vec::new();
    write_summary(&p, &mut good, 1, 1, 1.0, 7).unwrap();
    let t = String::from_utf8(good).unwrap();
    assert!(
        t.contains("summary files=1")
            && t.contains("timing elapsed_s=")
            && t.contains("state_digest=")
    );
}

// REQ: OA-RPL-235
#[test]
fn a_failing_hook_stops_the_run_at_that_frame() {
    let mut bytes = Vec::new();
    for k in 0..20i64 {
        bytes.extend(frame(3, k as u64, T0 + k * NS, b"state=up\n"));
    }
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let mut calls = 0;
    let r = run_streams_with(
        &mut p,
        vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
        64,
        &mut |_| {
            calls += 1;
            if calls == 5 {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(r.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(p.frames[FeedKind::Sfdps as usize], 5);
}

// REQ: OA-RPL-264
#[test]
fn a_failing_hook_still_leaves_the_framing_error_counts_in_the_pipeline() {
    // Damaged framing before a valid frame, then a hook that fails after that frame.
    let mut bytes = b"JUNKJUNKJUNK".to_vec();
    for k in 0..3i64 {
        bytes.extend(frame(3, k as u64, T0 + k * NS, b"state=up\n"));
    }
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let r = run_streams_with(
        &mut p,
        vec![spec_from_bytes(FeedKind::Sfdps, vec![bytes])],
        64,
        &mut |_| Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")),
    );
    assert_eq!(r.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(p.frames[FeedKind::Sfdps as usize], 1);
    assert!(
        p.errors[ErrKind::FrameBadMagic as usize] >= 1,
        "{:?}",
        p.errors
    );
}

// REQ: OA-RPL-255
#[test]
fn a_file_is_read_in_bounded_pieces_whatever_its_size() {
    use std::cell::Cell;
    use std::io::Read;
    use std::rc::Rc;
    // A 12 MiB file of zeros that records the largest buffer it was asked to fill.
    struct Zeros(usize, Rc<Cell<usize>>);
    impl Read for Zeros {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.1.set(self.1.get().max(buf.len()));
            let n = buf.len().min(self.0);
            buf[..n].fill(0);
            self.0 -= n;
            Ok(n)
        }
    }
    let biggest = Rc::new(Cell::new(0));
    let b = Rc::clone(&biggest);
    let mut once = Some(());
    let spec = oa_replay::StreamSpec {
        feed: FeedKind::Sfdps,
        open: Box::new(move || {
            once.take()?;
            Some(Ok(Box::new(Zeros(12 << 20, Rc::clone(&b))) as Box<dyn Read>))
        }),
    };
    let mut p = new_pipeline(PipelineConfig::default()).unwrap();
    let r = run_streams_with(&mut p, vec![spec], usize::MAX, &mut |_| Ok(())).unwrap();
    assert_eq!(r.bytes, 12 << 20);
    assert!(
        biggest.get() <= oa_replay::source::MAX_CHUNK,
        "{}",
        biggest.get()
    );
}
