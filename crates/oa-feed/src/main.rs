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
use oa_feed::health::{format_summary, observe_error, observe_frame, observe_skipped, FeedHealth};
use oa_feed::record::{utc_iso, Format, Outcome, Recorder};
use oa_feed::replay::{replay_files, GzNoteKind};
use oa_feed::{FrameReader, MAX_FRAME_LEN};
use oa_zip::TruncStage;

const DEFAULT_JAR: &str = "adapters/scds-bridge/target/scds-bridge.jar";
const CHUNK: usize = 64 * 1024;
/// Chunks in flight between the reader thread and the main loop: 8 * 64 KiB = 512 KiB.
const QUEUE_CHUNKS: usize = 8;

enum Msg {
    Data(Vec<u8>),
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
    thread::spawn(move || loop {
        let mut buf = vec![0u8; CHUNK];
        match src.read(&mut buf) {
            Ok(0) => {
                let _ = tx.send(Msg::End(None));
                return;
            }
            Ok(n) => {
                buf.truncate(n);
                if tx.send(Msg::Data(buf)).is_err() {
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

    let start = Instant::now();
    let deadline = cfg.duration_secs.map(|s| start + Duration::from_secs(s));
    let every = Duration::from_secs(cfg.summary_secs);
    let mut next_summary = start + every;
    let mut stopping = false;
    let mut stop_at = start;
    let mut end_reason: Option<String> = None;
    let mut exit = ExitCode::SUCCESS;

    loop {
        let now = Instant::now();
        if now >= next_summary {
            print_summary(&cfg.feed, &health);
            next_summary += every;
        }
        if !stopping && deadline.is_some_and(|d| now >= d) {
            stopping = true;
            stop_at = now;
            if let Some(c) = child.as_mut() {
                terminate(c);
            }
        }
        // After asking the bridge to stop, give it a few seconds to flush, then stop reading.
        if stopping && (child.is_none() || now.duration_since(stop_at) > Duration::from_secs(6)) {
            break;
        }
        let wake = if stopping {
            stop_at + Duration::from_millis(100)
        } else {
            next_summary
        };
        let wait = wake
            .min(deadline.unwrap_or(wake))
            .saturating_duration_since(now)
            .clamp(Duration::from_millis(1), Duration::from_millis(200));
        match rx.recv_timeout(wait) {
            Ok(Msg::Data(chunk)) => {
                let mut off = 0usize;
                while off < chunk.len() {
                    let took = reader.push(chunk.get(off..).unwrap_or(&[]));
                    off += took;
                    drain(&mut reader, &mut health, &mut recorder, &cfg.feed);
                    if took == 0 {
                        break;
                    }
                }
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
                        eprintln!("oa-feed: WARNING feed={} recording failed: {e}", cfg.feed);
                        recorder = None;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(e) = reader.finish() {
        observe_error(&mut health, &e);
        eprintln!("oa-feed: {e}");
    }
    observe_skipped(&mut health, reader.skipped_total());
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
            eprintln!("oa-feed: WARNING feed={} recording failed: {e}", cfg.feed);
            exit = ExitCode::FAILURE;
        }
    }
    print!("final ");
    print_summary(&cfg.feed, &health);
    if let Some(r) = &recorder {
        println!(
            "recorded feed={} files={} bytes={}",
            cfg.feed,
            r.files_opened(),
            r.total_bytes()
        );
    }
    Ok(exit)
}

fn drain(
    reader: &mut FrameReader<'_>,
    health: &mut FeedHealth,
    recorder: &mut Option<Recorder>,
    feed: &str,
) {
    while let Some(ev) = reader.next() {
        match ev {
            Ok(f) => {
                observe_frame(health, &f, f.recv_unix_ns);
                if let Some(r) = recorder.as_mut() {
                    match r.write_frame(f.raw, f.recv_unix_ns.div_euclid(1_000_000_000)) {
                        Ok(Outcome::Written | Outcome::Stopped) => {}
                        Ok(Outcome::StoppedNow) => eprintln!(
                            "oa-feed: WARNING feed={feed} recording stopped: --max-bytes cap reached \
                             ({} bytes); nothing deleted, frames are no longer saved",
                            r.total_bytes()
                        ),
                        Err(e) => {
                            eprintln!("oa-feed: WARNING feed={feed} recording failed: {e}");
                            *recorder = None;
                        }
                    }
                }
            }
            Err(e) => {
                observe_error(health, &e);
                eprintln!("oa-feed: feed={feed} {e}");
            }
        }
    }
    observe_skipped(health, reader.skipped_total());
}

fn print_summary(feed: &str, h: &FeedHealth) {
    let now = now_ns();
    println!(
        "t={} feed={feed} {}",
        utc_iso(now.div_euclid(1_000_000_000)),
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
