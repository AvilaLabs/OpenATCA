//! OS-process recovery drill over the integrated regional runtime and a synthetic admitted log.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_regional_session::cli::synthetic_model;
use oa_regional_session::{flush_receipt_group, parse_event_line, push_event, session_cursor_new};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE: &str = include_str!("../examples/synthetic-session.tsv");
// Hashes only the synthetic fixture's declared settings subset; no claim of full runtime
// subsystem-configuration identity is made by this test protocol.
const SETTINGS: &str = "OA-REGIONAL-RECOVERY-SETTINGS\t1\ncentre\tZAU1\ncentre_lat_e7\t410000000\ncentre_lon_e7\t-870000000\nrange_nm\t180.0\ntracks\t32\nframes\t512\n";
const WORKER_RESPONSE_DEADLINE: Duration = Duration::from_secs(2);
const WORKER_EXIT_DEADLINE: Duration = Duration::from_secs(2);
const WORKER_CLEANUP_DEADLINE: Duration = Duration::from_secs(2);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_PROTOCOL_LINE: usize = 1024;
const AUTHORITY_GENERATION: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Output {
    generation: u64,
    sequence: u64,
    receipt_ns: i64,
    frame_hash: String,
    pixels_hash: String,
    state_hash: String,
    tracks: usize,
    flights: usize,
    trajectory_rows: usize,
    open_alerts: usize,
    source_lag_ns: Option<i64>,
    command_dispatches: usize,
    handoff_state: u8,
    owner: String,
}

struct Worker {
    child: Child,
    input: ChildStdin,
    lines: Receiver<Result<String, String>>,
}

fn read_bounded_line(reader: &mut BufReader<ChildStdout>) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::with_capacity(MAX_PROTOCOL_LINE);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "unterminated worker output",
                ))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if bytes.len().saturating_add(take) > MAX_PROTOCOL_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "worker output line exceeded bound",
            ));
        }
        let complete = available.get(take - 1) == Some(&b'\n');
        let Some(chunk) = available.get(..take) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "worker output slice out of bounds",
            ));
        };
        bytes.extend_from_slice(chunk);
        reader.consume(take);
        if complete {
            bytes.pop();
            return Ok(Some(bytes));
        }
    }
}

impl Worker {
    fn start(generation: u64) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_regional-recovery-worker"))
            .arg(generation.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start regional recovery process");
        let input = child.stdin.take().expect("worker stdin");
        let output = child.stdout.take().expect("worker stdout");
        let (sender, lines) = mpsc::sync_channel(8);
        thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                match read_bounded_line(&mut reader) {
                    Ok(None) => break,
                    Ok(Some(bytes)) => {
                        let line =
                            String::from_utf8(bytes).map_err(|_| "non-UTF8 worker line".to_owned());
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });
        Self {
            child,
            input,
            lines,
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.input, "{line}").expect("write worker command");
        self.input.flush().expect("flush worker command");
    }

    fn receive(&self) -> String {
        match self.lines.recv_timeout(WORKER_RESPONSE_DEADLINE) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => panic!("invalid worker output: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("worker response exceeded {WORKER_RESPONSE_DEADLINE:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("worker stdout closed before response")
            }
        }
    }

    fn wait_bounded(&mut self) -> io::Result<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if start.elapsed() >= WORKER_EXIT_DEADLINE {
                let _ = self.child.kill();
                let cleanup = Instant::now();
                loop {
                    if let Some(status) = self.child.try_wait()? {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("worker exceeded {WORKER_EXIT_DEADLINE:?}; killed with status {status}"),
                        ));
                    }
                    if cleanup.elapsed() >= WORKER_CLEANUP_DEADLINE {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("worker exceeded exit and {WORKER_CLEANUP_DEADLINE:?} cleanup deadlines"),
                        ));
                    }
                    thread::sleep(WORKER_POLL_INTERVAL);
                }
            }
            thread::sleep(WORKER_POLL_INTERVAL);
        }
    }

    fn terminate(mut self) {
        let _ = self.child.kill();
        let status = self
            .wait_bounded()
            .expect("fault-injected worker exits within bounded cleanup deadline");
        assert!(
            !status.success(),
            "fault-injected worker unexpectedly succeeded"
        );
    }

    fn exit(mut self) {
        self.send("EXIT");
        let status = self
            .wait_bounded()
            .expect("worker exits within named deadline");
        assert!(status.success(), "worker exit failed: {status}");
    }

    fn expect_failure(mut self) {
        let status = self
            .wait_bounded()
            .expect("failed worker exits within named deadline");
        assert!(
            !status.success(),
            "invalid worker protocol unexpectedly succeeded"
        );
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let start = Instant::now();
            while start.elapsed() < WORKER_CLEANUP_DEADLINE {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    break;
                }
                thread::sleep(WORKER_POLL_INTERVAL);
            }
        }
    }
}

struct Authority {
    generation: u64,
    last_sequence: u64,
    watermark_ns: i64,
    accepted: Vec<Output>,
}

impl Authority {
    fn accept(&mut self, output: Output) -> Result<(), &'static str> {
        if output.generation != self.generation {
            return Err("generation");
        }
        if output.sequence <= self.last_sequence {
            return Err("sequence replay");
        }
        if output.sequence != self.last_sequence + 1 {
            return Err("sequence gap");
        }
        if output.receipt_ns < self.watermark_ns {
            return Err("watermark regression");
        }
        self.last_sequence = output.sequence;
        self.watermark_ns = output.receipt_ns;
        self.accepted.push(output);
        Ok(())
    }

    fn promote(&mut self) {
        self.generation = self.generation.checked_add(1).expect("generation capacity");
    }
}

fn fixture_identity() -> (String, String) {
    let snapshot =
        oa_airspace::core::snapshot::encode(&synthetic_model().expect("synthetic model"))
            .expect("encode synthetic adaptation");
    (
        oa_sha256::to_hex(&oa_sha256::sha256(SETTINGS.as_bytes())),
        oa_sha256::to_hex(&oa_sha256::sha256(&snapshot)),
    )
}

fn admitted_prefix_hash(count: usize, batches: &[oa_regional_session::ReceiptBatch]) -> String {
    let cutoff = if count == 0 {
        None
    } else {
        Some(batches[count - 1].receipt_ns)
    };
    let mut bytes = Vec::with_capacity(FIXTURE.len());
    for (index, line) in FIXTURE.lines().enumerate() {
        if index == 0 {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
            continue;
        }
        let receipt = line.split('\t').nth(1).unwrap().parse::<i64>().unwrap();
        if cutoff.is_some_and(|value| receipt <= value) {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
    }
    oa_sha256::to_hex(&oa_sha256::sha256(&bytes))
}

fn batches() -> Vec<oa_regional_session::ReceiptBatch> {
    let mut cursor = session_cursor_new();
    let mut result = Vec::new();
    let mut lines = FIXTURE.lines();
    assert_eq!(lines.next(), Some("OA-RS\t1"));
    for line in lines.filter(|line| !line.is_empty()) {
        let event = parse_event_line(line).expect("synthetic fixture event parses");
        if let Some(batch) = push_event(&mut cursor, event).expect("fixture admission") {
            result.push(batch);
        }
    }
    if let Some(batch) = flush_receipt_group(&mut cursor).expect("fixture final group") {
        result.push(batch);
    }
    result
}

fn replay(
    worker: &mut Worker,
    generation: u64,
    count: usize,
    identity: &(String, String),
    batches: &[oa_regional_session::ReceiptBatch],
    expected: &[Output],
) {
    let prefix_hash = admitted_prefix_hash(count, batches);
    worker.send(&format!(
        "REPLAY\t{generation}\t{count}\t{prefix_hash}\t{}\t{}",
        identity.0, identity.1
    ));
    if count == 0 {
        assert_eq!(
            worker.receive(),
            format!("REPLAYED\t{generation}\t0\t0\t-\t-\t-")
        );
        return;
    }
    for (index, known) in expected.iter().take(count).enumerate() {
        let line = worker.receive();
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 7, "bad replay response: {line}");
        assert_eq!(fields[0], "REPLAYED");
        assert_eq!(fields[1].parse::<u64>().unwrap(), generation);
        assert_eq!(fields[2].parse::<usize>().unwrap(), index + 1);
        assert_eq!(fields[3].parse::<u64>().unwrap(), known.sequence);
        assert_eq!(fields[4], known.frame_hash);
        assert_eq!(fields[5], known.pixels_hash);
        assert_eq!(fields[6], known.state_hash);
    }
}

fn parse_output(line: &str) -> Output {
    let fields: Vec<_> = line.split('\t').collect();
    assert_eq!(fields.len(), 15, "bad output line: {line}");
    assert_eq!(fields[0], "OUTPUT");
    Output {
        generation: fields[1].parse().unwrap(),
        sequence: fields[2].parse().unwrap(),
        receipt_ns: fields[3].parse().unwrap(),
        frame_hash: fields[4].to_owned(),
        pixels_hash: fields[5].to_owned(),
        state_hash: fields[6].to_owned(),
        tracks: fields[7].parse().unwrap(),
        flights: fields[8].parse().unwrap(),
        trajectory_rows: fields[9].parse().unwrap(),
        open_alerts: fields[10].parse().unwrap(),
        source_lag_ns: parse_source_lag(fields.get(11).expect("source-lag protocol field")),
        command_dispatches: fields[12].parse().unwrap(),
        handoff_state: fields[13].parse().unwrap(),
        owner: fields[14].to_owned(),
    }
}

fn parse_source_lag(field: &str) -> Option<i64> {
    if field == "-" {
        None
    } else {
        Some(field.parse().expect("integer source-lag field"))
    }
}

fn next_output(worker: &mut Worker, generation: u64, index: usize) -> Output {
    worker.send(&format!("STEP\t{generation}\t{index}"));
    parse_output(&worker.receive())
}

fn acknowledge(worker: &mut Worker, generation: u64, sequence: u64) {
    worker.send(&format!("ACK\t{generation}\t{sequence}"));
    assert_eq!(worker.receive(), format!("ACKED\t{generation}\t{sequence}"));
}

#[test]
// REQ: OA-REG-010
fn integrated_runtime_recomputes_retained_prefix_across_process_death() {
    let batches = batches();
    let identity = fixture_identity();
    assert!(
        batches.len() >= 6,
        "fixture covers multiple continuation frames"
    );

    // Uninterrupted child is the bit-exact continuation reference; the assertions below also
    // check known fixture-level subsystem state so this is not only a comparison to itself.
    let mut reference = Worker::start(1);
    replay(&mut reference, 1, 0, &identity, &batches, &[]);
    let mut expected = Vec::with_capacity(batches.len());
    for index in 0..batches.len() {
        let out = next_output(&mut reference, 1, index);
        assert_eq!(out.sequence as usize, index + 1);
        assert_eq!(
            out.receipt_ns,
            batches.get(index).expect("reference batch").receipt_ns
        );
        acknowledge(&mut reference, 1, out.sequence);
        expected.push(out);
    }
    reference.exit();
    assert_eq!(
        expected[0].tracks, 0,
        "first surveillance reports cannot confirm targets"
    );
    assert_eq!(
        expected[1].tracks, 2,
        "two synthetic targets confirm after repeat reports"
    );
    assert!(
        expected.iter().any(|out| out.flights == 2),
        "flight table state appears in composed frames"
    );
    assert!(
        expected.iter().any(|out| out.trajectory_rows > 0),
        "route conversion contributes trajectory rows"
    );
    assert!(
        expected.iter().any(|out| out.open_alerts > 0),
        "observed traffic produces a CA alert in the fixture"
    );
    assert!(
        expected
            .iter()
            .filter_map(|out| out.source_lag_ns)
            .any(|age| age > 0),
        "feed outage exposes stale measurement age"
    );
    assert_eq!(
        (expected[2].handoff_state, expected[2].owner.as_str()),
        (1, "SECTOR1"),
        "offer preserves current ownership"
    );
    assert_eq!(
        (expected[3].handoff_state, expected[3].owner.as_str()),
        (2, "SECTOR1"),
        "acceptance does not transfer ownership"
    );
    assert_eq!(
        (expected[4].handoff_state, expected[4].owner.as_str()),
        (0, "SECTOR2"),
        "take transfers ownership once"
    );
    assert_eq!(expected[2].command_dispatches, 1);
    assert_eq!(expected[3].command_dispatches, 1);
    assert_eq!(expected[4].command_dispatches, 1);

    let mut authority = Authority {
        generation: AUTHORITY_GENERATION,
        last_sequence: 0,
        watermark_ns: 0,
        accepted: Vec::new(),
    };
    let prefix = 3;
    let mut serving = Worker::start(authority.generation);
    replay(
        &mut serving,
        authority.generation,
        0,
        &identity,
        &batches,
        &[],
    );
    for (index, expected_output) in expected.iter().enumerate().take(prefix) {
        let out = next_output(&mut serving, authority.generation, index);
        assert_eq!(
            out,
            Output {
                generation: authority.generation,
                ..expected_output.clone()
            }
        );
        authority
            .accept(out.clone())
            .expect("serving generation publishes retained prefix");
        acknowledge(&mut serving, authority.generation, out.sequence);
    }

    // Failure immediately before the next command is applied: the retained admitted prefix
    // remains three records. Restarting a process recomputes it from the original fixture bytes.
    let start = Instant::now();
    serving.terminate();
    authority.promote();
    let mut replacement = Worker::start(authority.generation);
    replay(
        &mut replacement,
        authority.generation,
        prefix,
        &identity,
        &batches,
        &expected,
    );

    // The process applies an actual regional frame and dies before publishing an output record.
    replacement.send(&format!("APPLY_ONLY\t{}\t{prefix}", authority.generation));
    assert_eq!(
        replacement.receive(),
        format!(
            "APPLIED\t{}\t{}\t{}",
            authority.generation,
            prefix + 1,
            prefix + 1
        )
    );
    replacement.terminate();
    authority.promote();

    // The next generation reconstructs only the authority's committed input prefix. The
    // unpublished application is deterministically recomputed at the same sequence.
    let mut replacement = Worker::start(authority.generation);
    replay(
        &mut replacement,
        authority.generation,
        prefix,
        &identity,
        &batches,
        &expected,
    );
    let output = next_output(&mut replacement, authority.generation, prefix);
    let expected_unpublished = expected.get(prefix).expect("expected continuation frame");
    assert_eq!(output.frame_hash, expected_unpublished.frame_hash);
    assert_eq!(output.pixels_hash, expected_unpublished.pixels_hash);
    assert_eq!(
        output,
        Output {
            generation: authority.generation,
            ..expected_unpublished.clone()
        }
    );
    authority
        .accept(output.clone())
        .expect("independent output authority accepts current generation");
    let availability_gap_ms = start.elapsed().as_millis();
    // Kill after output is accepted but before the worker receives its output acknowledgment.
    replacement.terminate();
    authority.promote();

    let stale = Output {
        generation: authority.generation - 1,
        ..output.clone()
    };
    assert_eq!(
        authority.accept(stale),
        Err("generation"),
        "old-generation output is fenced"
    );
    let duplicate = Output {
        generation: authority.generation,
        ..output
    };
    assert_eq!(
        authority.accept(duplicate),
        Err("sequence replay"),
        "duplicate accepted output is refused"
    );

    let mut replacement = Worker::start(authority.generation);
    replay(
        &mut replacement,
        authority.generation,
        prefix + 1,
        &identity,
        &batches,
        &expected,
    );
    for (index, expected_output) in expected
        .iter()
        .enumerate()
        .take(batches.len())
        .skip(prefix + 1)
    {
        let out = next_output(&mut replacement, authority.generation, index);
        assert_eq!(
            out,
            Output {
                generation: authority.generation,
                ..expected_output.clone()
            }
        );
        authority
            .accept(out.clone())
            .expect("current-generation output accepted once");
        acknowledge(&mut replacement, authority.generation, out.sequence);
    }
    replacement.exit();
    assert_eq!(authority.accepted.len(), batches.len());

    let source_lag_ns = expected.iter().filter_map(|out| out.source_lag_ns).max();
    eprintln!("regional-recovery: recomputation=true accepted_suffix={} availability_gap_ms={} max_track_measurement_lag_ns={:?} generation={}", authority.accepted.len(), availability_gap_ms, source_lag_ns, authority.generation);
}

#[test]
// REQ: OA-REG-010
fn recovery_protocol_refuses_changed_prefix_and_bounds_bad_or_stalled_workers() {
    let identity = fixture_identity();
    let mut changed = Worker::start(20);
    changed.send(&format!(
        "REPLAY\t20\t0\t{}\t{}\t{}",
        "00".repeat(32),
        identity.0,
        identity.1
    ));
    assert!(changed
        .receive()
        .starts_with("ERROR replay prefix digest mismatch"));
    changed.expect_failure();

    let mut malformed = Worker::start(21);
    replay(&mut malformed, 21, 0, &identity, &[], &[]);
    malformed.send("NOT_A_PROTOCOL_RECORD");
    assert!(malformed
        .receive()
        .starts_with("ERROR malformed or out-of-order protocol record"));
    malformed.expect_failure();

    let mut stalled = Worker::start(22);
    replay(&mut stalled, 22, 0, &identity, &[], &[]);
    stalled.send("STALL");
    assert!(
        stalled
            .lines
            .recv_timeout(WORKER_RESPONSE_DEADLINE)
            .is_err(),
        "stalled worker must hit response deadline"
    );
    stalled.terminate();

    let mut missing = Worker::start(23);
    missing.send(&format!(
        "REPLAY\t23\t999\t{}\t{}\t{}",
        "00".repeat(32),
        identity.0,
        identity.1
    ));
    assert!(missing
        .receive()
        .starts_with("ERROR replay count exceeds admitted prefix"));
    missing.expect_failure();
}

#[test]
// REQ: OA-REG-010
fn unknown_track_measurement_age_is_not_encoded_as_zero() {
    assert_eq!(parse_source_lag("-"), None);
    assert_eq!(parse_source_lag("0"), Some(0));
}
