#![forbid(unsafe_code)]
//! `oa-feed`: runs the SCDS bridge (or reads frames from stdin), validates and measures the
//! frame stream, optionally records it, and prints a health line. `oa-feed replay` re-reads
//! recordings. Shell: owns the process, the threads, the clock and stdout (B3).
//!
//! The bridge's stdout is read by a thread into a bounded channel; when this process is slow the
//! channel fills, the reader stops reading, the pipe fills, the bridge blocks before it
//! acknowledges, and the broker holds the backlog (nothing is buffered without bound). The env
//! file is passed to the bridge as a path and never opened here, so no credential enters this
//! process.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command as Proc, ExitCode, Stdio};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oa_feed::cli::{self, Command, Live, Source};
use oa_feed::health::{observe_error, observe_frame, observe_skipped, FeedHealth};
use oa_feed::record::{format_recording, utc_iso, Format, Outcome, Recorder};
use oa_feed::replay::{replay_files, GzNoteKind};
use oa_feed::{
    encode_frame_mono, format_summary, frame_work_refusal_retryable, FrameReader, FrameWorkBudget,
    FrameWorkRefusal, MAX_FRAME_LEN,
};
use oa_zip::TruncStage;

const DEFAULT_JAR: &str = "adapters/scds-bridge/target/scds-bridge.jar";
const CHUNK: usize = 64 * 1024;
/// Chunks in flight between the reader thread and the main loop: 8 * 64 KiB = 512 KiB.
const QUEUE_CHUNKS: usize = 8;

enum Msg {
    /// A chunk read, with the recorder's monotonic time (ns since start) when it arrived.
    Data(Vec<u8>, u64),
    End(Option<String>),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Replay(files)) => replay(&files),
        Ok(Command::Live(cfg)) => match live(&cfg) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("oa-feed: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("oa-feed: {e}\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn replay(files: &[PathBuf]) -> ExitCode {
    let rep = match replay_files(files) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("oa-feed: {e}");
            return ExitCode::FAILURE;
        }
    };
    for fe in &rep.errors {
        println!("file={} {}", fe.file, fe.error);
    }
    for note in &rep.gzip {
        match &note.kind {
            GzNoteKind::Truncated(t) => {
                let at = match t.stage {
                    TruncStage::Header => "in the header",
                    TruncStage::Data => "in the data",
                    TruncStage::Trailer => "in the trailer (data complete, CRC-32 unchecked)",
                };
                println!(
                    "file={} gzip truncated {at}: recovered {} bytes of the last member, {} of them before its last flush",
                    note.file, t.tail_bytes, t.flushed_bytes
                );
            }
            GzNoteKind::Damaged(e) => println!("file={} {e}", note.file),
        }
    }
    // Replay never reads the clock: the report time is the last frame's own time.
    println!(
        "replay files={} bytes={} {}",
        rep.files,
        rep.bytes,
        format_summary(&rep.health, rep.health.last_recv_ns)
    );
    if rep.errors.is_empty() && rep.gzip.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn spawn_bridge(env: &PathBuf, jar: &Option<PathBuf>, java: &Option<PathBuf>) -> io::Result<Child> {
    let java = java
        .clone()
        .unwrap_or_else(|| match std::env::var_os("JAVA_HOME") {
            Some(h) => PathBuf::from(h).join("bin").join("java"),
            None => PathBuf::from("java"),
        });
    let jar = jar.clone().unwrap_or_else(|| {
        std::env::var_os("OPENATCA_BRIDGE_JAR")
            .map_or_else(|| PathBuf::from(DEFAULT_JAR), PathBuf::from)
    });
    Proc::new(java)
        // Bounded heap: the bridge holds at most a few messages, not a backlog.
        .arg("-Xmx512m")
        .arg("-jar")
        .arg(jar)
        .arg(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
}

fn live(cfg: &Live) -> Result<ExitCode, String> {
    let mut child: Option<Child> = None;
    let mut src: Box<dyn Read + Send> = match &cfg.source {
        Source::Stdin => Box::new(io::stdin()),
        Source::Bridge { env, jar, java } => {
            let mut c =
                spawn_bridge(env, jar, java).map_err(|e| format!("cannot start bridge: {e}"))?;
            let out = c.stdout.take().ok_or("bridge has no stdout")?;
            child = Some(c);
            Box::new(out)
        }
    };
    let (tx, rx) = sync_channel::<Msg>(QUEUE_CHUNKS);
    let start = Instant::now();
    thread::spawn(move || loop {
        let mut buf = vec![0u8; CHUNK];
        match src.read(&mut buf) {
            Ok(0) => {
                let _ = tx.send(Msg::End(None));
                return;
            }
            Ok(n) => {
                // The stamp is taken here, as the bytes arrive and before they queue, so a
                // recorder that is slow to drain does not move it.
                let mono = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                buf.truncate(n);
                if tx.send(Msg::Data(buf, mono)).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                let _ = tx.send(Msg::End(Some(e.to_string())));
                return;
            }
        }
    });

    let mut recorder = match &cfg.record {
        Some(dir) => Some(
            Recorder::open(
                dir,
                &cfg.feed,
                cfg.rotate_bytes,
                cfg.max_bytes,
                if cfg.record_raw {
                    Format::Raw
                } else {
                    Format::Gzip
                },
            )
            .map_err(|e| format!("cannot open recording directory: {e}"))?,
        ),
        None => None,
    };
    let mut storage = vec![0u8; 2 * MAX_FRAME_LEN];
    let mut reader = FrameReader::new(&mut storage).map_err(|e| format!("{e:?}"))?;
    let mut health = FeedHealth::new();
    // The recorded form of a frame (an OAF2 frame with the monotonic time), reused per frame.
    let mut stamped: Vec<u8> = Vec::new();

    // The command line bounds both (`cli`), so these sums cannot overflow; they are checked anyway.
    let deadline = match cfg.duration_secs {
        Some(s) => Some(
            start
                .checked_add(Duration::from_secs(s))
                .ok_or("--duration is too long")?,
        ),
        None => None,
    };
    let every = Duration::from_secs(cfg.summary_secs);
    let mut next_summary = Some(
        start
            .checked_add(every)
            .ok_or("--summary-every is too long")?,
    );
    let mut stopping = false;
    let mut stop_at = start;
    let mut end_reason: Option<String> = None;
    let mut exit = ExitCode::SUCCESS;
    let mut terminal_error: Option<String> = None;
    let mut pending_data: Option<(Vec<u8>, usize, u64)> = None;
    let mut deferred_drain = false;
    let mut deferred_mono = 0u64;
    let mut work_deferrals = 0u64;
    let mut pending_peak_bytes = 0usize;
    let fresh_work_profile = FrameWorkBudget::for_capacity(MAX_FRAME_LEN * 2);

    'service: loop {
        let now = Instant::now();
        if next_summary.is_some_and(|n| now >= n) {
            print_summary(&cfg.feed, &health);
            next_summary = next_summary.and_then(|n| n.checked_add(every));
        }
        if !stopping && deadline.is_some_and(|d| now >= d) {
            stopping = true;
            stop_at = now;
            if let Some(c) = child.as_mut() {
                terminate(c);
            }
        }
        // After asking the bridge to stop, give it a few seconds to flush, then stop reading.
        let plan = turn_plan(
            stopping,
            child.is_none() || now.duration_since(stop_at) > Duration::from_secs(6),
            pending_data.is_some(),
            deferred_drain,
        );
        if plan == TurnPlan::Stop {
            break;
        }
        let wake = if stopping {
            stop_at.checked_add(Duration::from_millis(100))
        } else {
            next_summary
        };
        let wait = wake.map_or(Duration::from_millis(200), |w| {
            w.min(deadline.unwrap_or(w))
                .saturating_duration_since(now)
                .clamp(Duration::from_millis(1), Duration::from_millis(200))
        });
        if plan == TurnPlan::Service {
            update_pending_peak(&reader, &pending_data, &mut pending_peak_bytes);
            if let Some(recorder) = recorder.as_mut() {
                if let Err(e) = recorder.idle_flush(now_ns().div_euclid(1_000_000_000)) {
                    eprintln!(
                        "oa-feed: ERROR feed={} recording failed: {e}; frames are no longer \
                         saved, the feed is still consumed, the exit status will be failure",
                        cfg.feed
                    );
                }
            }
            let mut work = fresh_work_profile;
            if deferred_drain {
                match drain(
                    &mut reader,
                    &mut health,
                    &mut recorder,
                    &cfg.feed,
                    deferred_mono,
                    &mut stamped,
                    &mut work,
                ) {
                    Ok(()) => deferred_drain = false,
                    Err(refusal) => {
                        match judge(
                            Work::DrainRefused(refusal),
                            fresh_work_profile,
                            reader.buffered_len(),
                            pending_input_bytes(&pending_data),
                            work_deferrals,
                        ) {
                            Verdict::Cancel(error) => {
                                terminal_error = Some(error);
                                exit = ExitCode::FAILURE;
                                break 'service;
                            }
                            Verdict::Defer { drain_pending } => {
                                deferred_drain |= drain_pending;
                                note_deferral(&cfg.feed, &mut work_deferrals);
                            }
                        }
                        update_pending_peak(&reader, &pending_data, &mut pending_peak_bytes);
                        continue;
                    }
                }
            }
            if let Some((chunk, offset, mono)) = pending_data.as_mut() {
                let mut completed = false;
                while *offset < chunk.len() {
                    let rest = chunk.get(*offset..).unwrap_or(&[]);
                    match reader.push_with_remaining_budget(rest, &mut work) {
                        Ok(took) => {
                            if took == 0 {
                                let input_bytes = chunk.len().saturating_sub(*offset);
                                // A push that took nothing can never be retried: a Defer here
                                // would loop forever, so it cancels as well.
                                let error = match judge(
                                    Work::PushStalled,
                                    fresh_work_profile,
                                    reader.buffered_len(),
                                    input_bytes,
                                    work_deferrals,
                                ) {
                                    Verdict::Cancel(error) => error,
                                    Verdict::Defer { .. } => stall_message(
                                        reader.buffered_len(),
                                        input_bytes,
                                        work_deferrals,
                                    ),
                                };
                                terminal_error = Some(error);
                                exit = ExitCode::FAILURE;
                                break 'service;
                            }
                            *offset += took;
                            let input_bytes = chunk.len().saturating_sub(*offset);
                            pending_peak_bytes = pending_peak_bytes
                                .max(reader.buffered_len().saturating_add(input_bytes));
                        }
                        Err(refusal) => {
                            match judge(
                                Work::PushRefused(refusal),
                                fresh_work_profile,
                                reader.buffered_len(),
                                chunk.len().saturating_sub(*offset),
                                work_deferrals,
                            ) {
                                Verdict::Cancel(error) => {
                                    terminal_error = Some(error);
                                    exit = ExitCode::FAILURE;
                                    break 'service;
                                }
                                Verdict::Defer { drain_pending } => {
                                    deferred_drain |= drain_pending;
                                    note_deferral(&cfg.feed, &mut work_deferrals);
                                }
                            }
                            break;
                        }
                    }
                    match drain(
                        &mut reader,
                        &mut health,
                        &mut recorder,
                        &cfg.feed,
                        *mono,
                        &mut stamped,
                        &mut work,
                    ) {
                        Ok(()) => {}
                        Err(refusal) => {
                            match judge(
                                Work::DrainRefused(refusal),
                                fresh_work_profile,
                                reader.buffered_len(),
                                chunk.len().saturating_sub(*offset),
                                work_deferrals,
                            ) {
                                Verdict::Cancel(error) => {
                                    terminal_error = Some(error);
                                    exit = ExitCode::FAILURE;
                                    break 'service;
                                }
                                Verdict::Defer { drain_pending } => {
                                    deferred_drain |= drain_pending;
                                    note_deferral(&cfg.feed, &mut work_deferrals);
                                }
                            }
                            deferred_mono = *mono;
                            break;
                        }
                    }
                }
                if *offset == chunk.len() {
                    completed = true;
                }
                if completed {
                    pending_data = None;
                }
            }
            update_pending_peak(&reader, &pending_data, &mut pending_peak_bytes);
            continue;
        }
        match rx.recv_timeout(wait) {
            Ok(Msg::Data(chunk, mono)) => {
                pending_data = Some((chunk, 0, mono));
            }
            Ok(Msg::End(reason)) => {
                end_reason = reason;
                if !stopping && child.is_some() {
                    exit = ExitCode::FAILURE;
                    eprintln!("oa-feed: frame source ended before the requested duration");
                }
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                // A quiet feed: frames already written must not sit in the compressor.
                if let Some(r) = recorder.as_mut() {
                    if let Err(e) = r.idle_flush(now_ns().div_euclid(1_000_000_000)) {
                        eprintln!(
                            "oa-feed: ERROR feed={} recording failed: {e}; frames are no longer \
                             saved, the feed is still consumed, the exit status will be failure",
                            cfg.feed
                        );
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(e) = finalize_reader(&mut reader, &mut health, terminal_error.is_some()) {
        eprintln!("oa-feed: {e}");
    }
    if let Some(r) = end_reason {
        eprintln!("oa-feed: source read error: {r}");
        exit = ExitCode::FAILURE;
    }
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    if let Some(r) = recorder.as_mut() {
        if let Err(e) = r.finish() {
            eprintln!("oa-feed: ERROR feed={} recording failed: {e}", cfg.feed);
        }
        // A recording that failed at any point (write, flush or finish), or stopped at the size
        // cap, fails the run.
        if let Some(reason) = r.exit_reason() {
            eprintln!(
                "oa-feed: ERROR feed={} {reason}; exit status is failure",
                cfg.feed
            );
            exit = ExitCode::FAILURE;
        }
    }
    if let Some(r) = &recorder {
        if r.implausible_stamps() > 0 {
            eprintln!(
                "oa-feed: WARNING feed={} {} frames had receive stamps over an hour from local time",
                cfg.feed,
                r.implausible_stamps()
            );
        }
    }
    if let Some(error) = terminal_error {
        eprintln!("oa-feed: {error}");
    }
    print!("final ");
    print_summary(&cfg.feed, &health);
    eprintln!(
        "oa-feed: feed={} work_deferrals={} pending_peak_bytes={}",
        cfg.feed, work_deferrals, pending_peak_bytes
    );
    if let Some(r) = &recorder {
        println!("{}", format_recording(&cfg.feed, r));
    }
    Ok(exit)
}

/// What the top of a live-loop turn does (the pure part of the decision, B2-style: inputs in,
/// plan out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnPlan {
    /// The loop ends: stopping was requested, the bridge is gone or its grace period is over,
    /// and no input or drain is owed.
    Stop,
    /// A held input chunk or a deferred drain is resumed with a fresh allowance.
    Service,
    /// Nothing is owed: wait for the next chunk from the source thread.
    Receive,
}

fn turn_plan(
    stopping: bool,
    stop_ready: bool,
    pending_data: bool,
    deferred_drain: bool,
) -> TurnPlan {
    let owed = pending_data || deferred_drain;
    if stopping && stop_ready && !owed {
        TurnPlan::Stop
    } else if owed {
        TurnPlan::Service
    } else {
        TurnPlan::Receive
    }
}

/// How one unit of service work ended without finishing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Work {
    /// `push` took no bytes although the chunk has some left.
    PushStalled,
    /// The allowance refused a push.
    PushRefused(FrameWorkRefusal),
    /// The allowance refused a drain (`next`).
    DrainRefused(FrameWorkRefusal),
}

/// The decision for an unfinished unit of work.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// Keep every byte and resume next turn with a fresh allowance; `drain_pending` is whether
    /// the reader still has frames to drain before more input is pushed.
    Defer { drain_pending: bool },
    /// Cancel the stream with this error; the exit status is failure.
    Cancel(String),
}

/// A refusal is deferred only when a fresh allowance can satisfy it (`required <= fresh`);
/// otherwise, and for a push that makes no progress, the stream is canceled and the error says
/// what was retained. An invariant break (a refusal that a fresh allowance cannot satisfy)
/// therefore fails closed instead of retrying forever.
fn judge(
    work: Work,
    fresh: FrameWorkBudget,
    reader_bytes: usize,
    input_bytes: usize,
    deferrals: u64,
) -> Verdict {
    let (refusal, drain_pending) = match work {
        Work::PushStalled => {
            return Verdict::Cancel(stall_message(reader_bytes, input_bytes, deferrals));
        }
        Work::PushRefused(r) => (r, false),
        Work::DrainRefused(r) => (r, true),
    };
    if frame_work_refusal_retryable(refusal, fresh) {
        Verdict::Defer { drain_pending }
    } else {
        Verdict::Cancel(work_refusal_cancel(
            refusal,
            reader_bytes,
            input_bytes,
            deferrals,
        ))
    }
}

fn stall_message(reader_bytes: usize, input_bytes: usize, deferrals: u64) -> String {
    format!(
        "frame reader made no progress; live stream canceled with {reader_bytes} reader-buffer bytes and {input_bytes} unpushed input bytes after {deferrals} deferrals"
    )
}

/// Counts a deferral and, each time the count reaches a power of two of 1024 or more, warns
/// once on stderr (not recorded output).
fn note_deferral(feed: &str, deferrals: &mut u64) {
    let before = *deferrals;
    *deferrals = before.saturating_add(1);
    if deferral_warning(before, *deferrals) {
        eprintln!(
            "oa-feed: WARNING feed={feed} work_deferrals reached {}; the frame reader keeps \
             hitting its work allowance, so the input is mostly damaged or the consumer is slow",
            *deferrals
        );
    }
}

/// True when the count went from below to at or above a power of two that is 1024 or more.
fn deferral_warning(before: u64, after: u64) -> bool {
    after >= 1024 && before.leading_zeros() > after.leading_zeros()
}

fn update_pending_peak(
    reader: &FrameReader<'_>,
    pending: &Option<(Vec<u8>, usize, u64)>,
    peak: &mut usize,
) {
    let input_bytes = pending_input_bytes(pending);
    let owned_bytes = reader.buffered_len().saturating_add(input_bytes);
    *peak = (*peak).max(owned_bytes);
}

fn pending_input_bytes(pending: &Option<(Vec<u8>, usize, u64)>) -> usize {
    pending
        .as_ref()
        .map_or(0, |(chunk, offset, _)| chunk.len().saturating_sub(*offset))
}

fn work_refusal_cancel(
    refusal: FrameWorkRefusal,
    reader_bytes: usize,
    input_bytes: usize,
    prior_deferrals: u64,
) -> String {
    // This is the exact retained domain: bytes in FrameReader plus the active chunk suffix.
    // It does not claim a count for unread bytes still upstream or queued in the source channel.
    format!(
        "nonrenewable frame work refusal {refusal:?}; live stream canceled with {reader_bytes} reader-buffer bytes and {input_bytes} unpushed input bytes after {prior_deferrals} deferrals"
    )
}

fn finalize_reader(
    reader: &mut FrameReader<'_>,
    health: &mut FeedHealth,
    work_canceled: bool,
) -> Option<oa_feed::FrameError> {
    let error = if work_canceled { None } else { reader.finish() };
    if let Some(ref e) = error {
        observe_error(health, e);
    }
    observe_skipped(health, reader.skipped_total());
    error
}

fn drain(
    reader: &mut FrameReader<'_>,
    health: &mut FeedHealth,
    recorder: &mut Option<Recorder>,
    feed: &str,
    mono: u64,
    stamped: &mut Vec<u8>,
    work: &mut FrameWorkBudget,
) -> Result<(), oa_feed::FrameWorkRefusal> {
    loop {
        let event = match reader.next_with_remaining_budget(work) {
            Ok(event) => event,
            Err(refusal) => {
                observe_skipped(health, reader.skipped_total());
                return Err(refusal);
            }
        };
        let Some(ev) = event else {
            observe_skipped(health, reader.skipped_total());
            return Ok(());
        };
        match ev {
            Ok(f) => {
                observe_frame(health, &f, f.recv_unix_ns);
                if let Some(r) = recorder.as_mut() {
                    // A recorded frame carries the recorder's monotonic time since start (taken
                    // when the chunk that completed it arrived), so a replay can tell a step of
                    // the wall clock from time that passed. A frame that already has one keeps
                    // it.
                    let raw = if f.mono_ns().is_some() {
                        f.raw
                    } else {
                        stamped.clear();
                        stamped.resize(f.raw.len() + 8, 0);
                        match encode_frame_mono(
                            stamped,
                            f.kind as u8,
                            f.flags,
                            f.seq,
                            f.recv_unix_ns,
                            f.jms_timestamp_ms,
                            mono,
                            f.payload,
                        ) {
                            Some(n) => stamped.get(..n).unwrap_or(f.raw),
                            None => f.raw,
                        }
                    };
                    match r.write_frame(
                        raw,
                        f.recv_unix_ns.div_euclid(1_000_000_000),
                        now_ns().div_euclid(1_000_000_000),
                    ) {
                        Ok(Outcome::Written | Outcome::Stopped | Outcome::Failed) => {}
                        Ok(Outcome::StoppedNow) => eprintln!(
                            "oa-feed: WARNING feed={feed} recording stopped: --max-bytes cap reached \
                             ({} bytes); nothing deleted, frames are no longer saved",
                            r.total_bytes()
                        ),
                        // The recorder keeps the failure and counts the frames lost from here
                        // on; the final summary and the exit status report it.
                        Err(e) => eprintln!(
                            "oa-feed: ERROR feed={feed} recording failed: {e}; frames are no \
                             longer saved, the feed is still consumed, the exit status will be failure"
                        ),
                    }
                }
            }

            Err(e) => {
                observe_error(health, &e);
                eprintln!("oa-feed: feed={feed} {e}");
            }
        }
    }
}

fn print_summary(feed: &str, h: &FeedHealth) {
    let now = now_ns();
    println!(
        "t={} feed={feed} {}",
        utc_iso(now.div_euclid(1_000_000_000)).unwrap_or_else(|| "unrepresentable".to_string()),
        format_summary(h, now)
    );
}

/// Asks the bridge JVM to exit (SIGTERM via `kill`), so it closes the broker session cleanly.
fn terminate(c: &mut Child) {
    let ok = Proc::new("kill")
        .arg("-TERM")
        .arg(c.id().to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = c.kill();
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::{deferral_warning, finalize_reader, judge, turn_plan, TurnPlan, Verdict, Work};
    use oa_feed::{
        encode_frame, ErrorKind, FeedHealth, FrameReader, FrameWorkBudget, FrameWorkKind,
        FrameWorkRefusal, MAX_FRAME_LEN, TRAILER_LEN,
    };

    fn fresh() -> FrameWorkBudget {
        FrameWorkBudget::for_capacity(MAX_FRAME_LEN * 2)
    }

    fn refusal(kind: FrameWorkKind, required: u64) -> FrameWorkRefusal {
        FrameWorkRefusal {
            kind,
            required,
            available: 0,
        }
    }

    // REQ: OA-FEED-186
    #[test]
    fn turn_plan_covers_stop_service_and_receive() {
        // (stopping, stop_ready, pending_data, deferred_drain) -> plan, all 16 inputs.
        for bits in 0u8..16 {
            let (stopping, ready, pending, deferred) =
                (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let want = if pending || deferred {
                TurnPlan::Service
            } else if stopping && ready {
                TurnPlan::Stop
            } else {
                TurnPlan::Receive
            };
            assert_eq!(
                turn_plan(stopping, ready, pending, deferred),
                want,
                "{bits}"
            );
        }
    }

    // REQ: OA-FEED-186
    #[test]
    fn retryable_refusals_defer_and_say_whether_a_drain_is_pending() {
        let f = fresh();
        for kind in [
            FrameWorkKind::ScanPositions,
            FrameWorkKind::CrcBytes,
            FrameWorkKind::CopiedBytes,
            FrameWorkKind::CompactedBytes,
        ] {
            let allowance = match kind {
                FrameWorkKind::ScanPositions => f.scan_positions,
                FrameWorkKind::CrcBytes => f.crc_bytes,
                FrameWorkKind::CopiedBytes => f.copied_bytes,
                _ => f.compacted_bytes,
            };
            // required == fresh is retryable; the allowance is renewed next turn.
            assert_eq!(
                judge(Work::PushRefused(refusal(kind, allowance)), f, 10, 20, 3),
                Verdict::Defer {
                    drain_pending: false
                }
            );
            assert_eq!(
                judge(Work::DrainRefused(refusal(kind, allowance)), f, 10, 20, 3),
                Verdict::Defer {
                    drain_pending: true
                }
            );
        }
    }

    // REQ: OA-FEED-186
    #[test]
    fn the_fresh_allowance_is_the_exact_boundary_between_defer_and_cancel() {
        let f = fresh();
        for kind in [
            FrameWorkKind::ScanPositions,
            FrameWorkKind::CrcBytes,
            FrameWorkKind::CopiedBytes,
            FrameWorkKind::CompactedBytes,
        ] {
            let allowance = match kind {
                FrameWorkKind::ScanPositions => f.scan_positions,
                FrameWorkKind::CrcBytes => f.crc_bytes,
                FrameWorkKind::CopiedBytes => f.copied_bytes,
                _ => f.compacted_bytes,
            };
            for (required, defers) in [(allowance, true), (allowance + 1, false)] {
                let r = refusal(kind, required);
                for work in [Work::PushRefused(r), Work::DrainRefused(r)] {
                    match judge(work, f, 11, 22, 5) {
                        Verdict::Defer { .. } => assert!(defers, "{kind:?} {required}"),
                        Verdict::Cancel(text) => {
                            assert!(!defers, "{kind:?} {required}");
                            assert!(text.contains("nonrenewable frame work refusal"), "{text}");
                            assert!(text.contains(&format!("{r:?}")), "{text}");
                            assert!(text.contains("11 reader-buffer bytes"), "{text}");
                            assert!(text.contains("22 unpushed input bytes"), "{text}");
                            assert!(text.contains("after 5 deferrals"), "{text}");
                        }
                    }
                }
            }
        }
        // Counter overflow is permanent whatever it requires.
        for required in [0, 1, u64::MAX] {
            for work in [
                Work::PushRefused(refusal(FrameWorkKind::CounterOverflow, required)),
                Work::DrainRefused(refusal(FrameWorkKind::CounterOverflow, required)),
            ] {
                assert!(matches!(judge(work, f, 0, 0, 0), Verdict::Cancel(_)));
            }
        }
    }

    // REQ: OA-FEED-186
    #[test]
    fn a_push_that_takes_nothing_cancels_with_the_retained_byte_counts() {
        let Verdict::Cancel(text) = judge(Work::PushStalled, fresh(), 7, 9, 2) else {
            panic!("stall was deferred");
        };
        assert!(text.contains("frame reader made no progress"), "{text}");
        assert!(text.contains("7 reader-buffer bytes"), "{text}");
        assert!(text.contains("9 unpushed input bytes"), "{text}");
        assert!(text.contains("after 2 deferrals"), "{text}");
    }

    // REQ: OA-FEED-191
    #[test]
    fn deferral_warning_fires_once_per_power_of_two_from_1024() {
        let fired: Vec<u64> = (1u64..=5000)
            .filter(|&n| deferral_warning(n - 1, n))
            .collect();
        assert_eq!(fired, vec![1024, 2048, 4096]);
        assert!(!deferral_warning(0, 1023));
        assert!(deferral_warning(1000, 1024));
        assert!(!deferral_warning(u64::MAX, u64::MAX));
    }

    // REQ: OA-FEED-187
    #[test]
    fn work_cancellation_does_not_classify_a_buffered_complete_frame_as_truncated() {
        let payload = b"complete frame retained on cancellation";
        let mut wire = vec![0u8; oa_feed::HEADER_LEN + payload.len() + TRAILER_LEN];
        let wire_len = encode_frame(&mut wire, 1, 0, 42, 7, 0, payload)
            .expect("frame fits its exact-size buffer");
        assert_eq!(wire_len, wire.len());
        let mut storage = vec![0u8; MAX_FRAME_LEN];
        let mut reader = FrameReader::new(&mut storage).unwrap();
        assert_eq!(reader.push(&wire), wire.len());
        let mut health = FeedHealth::new();

        assert_eq!(finalize_reader(&mut reader, &mut health, true), None);
        assert_eq!(health.frame_errors, 0);
        assert_eq!(reader.buffered_len(), wire.len());
        assert_eq!(reader.next().unwrap().unwrap().seq, 42);

        let mut partial_storage = vec![0u8; MAX_FRAME_LEN];
        let mut partial_reader = FrameReader::new(&mut partial_storage).unwrap();
        assert_eq!(partial_reader.push(&wire[..wire.len() - 1]), wire.len() - 1);
        let mut partial_health = FeedHealth::new();
        let error = finalize_reader(&mut partial_reader, &mut partial_health, false).unwrap();
        assert!(matches!(error.kind, ErrorKind::Truncated { .. }));
        assert_eq!(partial_health.frame_errors, 1);
    }
}
