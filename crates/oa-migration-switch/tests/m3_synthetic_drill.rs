// The included M3 fixture keeps its in-memory 100-cycle and trace-capacity tests in this
// ordinary integration-test target. The process drill below is test-only synthetic transport.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "../examples/m3_synthetic_drill.rs"]
mod drill;

use oa_audit_core::{AuditReceipt, Event};
use oa_migration_switch::{
    ingest, new_selector, observe_delivery, prepare_candidate, rollback, select_candidate,
    selected, trace, EvidenceKind, Output, Refusal, Side, Snapshot, SwapUnitKey, UnitPolicy,
};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

const PREFIX: &str = "M3_INTERNAL_SYNTHETIC_TEST/1\t";
const PREFIX_BYTES: &[u8] = b"M3_INTERNAL_SYNTHETIC_TEST/1\t";
const MAX_FRAME: usize = 256;
const MAX_LINES: usize = 16;
const MAX_HARNESS_NOISE: usize = 4096;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const TARGET_DECISION_TO_ACK: Duration = Duration::from_secs(1);

fn test_audit_receipt(
    selector: &oa_migration_switch::Selector,
    event: Event,
) -> Result<AuditReceipt, Refusal> {
    let expected = oa_migration_switch::audit_checkpoint(selector);
    let sequence = expected
        .count
        .checked_add(1)
        .ok_or(Refusal::AuditRequired)?;
    Ok(oa_audit_core::receipt_claim(
        sequence,
        [0; 32],
        expected.tail,
        event,
    ))
}

fn audited_select_candidate(
    selector: &mut oa_migration_switch::Selector,
    readiness: &oa_migration_switch::CandidateReadiness,
    authorization: &oa_migration_switch::WindowAuthorization,
    intent: &oa_migration_switch::PromotionIntent,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    let event = oa_migration_switch::candidate_audit_event(selector, intent, now_ns)?;
    let receipt = test_audit_receipt(selector, event)?;
    select_candidate(
        selector,
        readiness,
        authorization,
        intent,
        now_ns,
        max_age_ns,
        receipt,
    )
}

fn audited_rollback(
    selector: &mut oa_migration_switch::Selector,
    cause: oa_migration_switch::RollbackReceipt,
    now_ns: i64,
    max_age_ns: i64,
) -> Result<(), Refusal> {
    let event = oa_migration_switch::rollback_audit_event(selector, cause, now_ns)?;
    let receipt = test_audit_receipt(selector, event)?;
    rollback(selector, cause, now_ns, max_age_ns, receipt)
}

#[derive(Clone, Debug)]
struct Payload {
    side: Side,
    source_epoch: u64,
    source_sequence: u64,
    state_version: u64,
    variation: u32,
    delivery_sequence: u64,
    bytes: Vec<u8>,
}

#[derive(Debug, Eq, PartialEq)]
enum ConsumerReply {
    Ack { sequence: u64, echoed: Vec<u8> },
    Duplicate { expected: u64, received: u64 },
    Gap { expected: u64, received: u64 },
}

struct LocalWorker {
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    replies: Receiver<Result<Vec<u8>, String>>,
}

impl LocalWorker {
    fn request(&mut self, line: &str) -> Result<Vec<u8>, String> {
        if line.len() > MAX_FRAME || !line.ends_with('\n') {
            return Err("request exceeded the frame bound".to_owned());
        }
        let input = self.stdin.as_mut().ok_or("worker stdin is closed")?;
        input
            .write_all(line.as_bytes())
            .map_err(|error| error.to_string())?;
        input.flush().map_err(|error| error.to_string())?;
        self.replies
            .recv_timeout(RESPONSE_TIMEOUT)
            .map_err(|error| error.to_string())?
    }

    fn finish(&mut self, kill: bool) -> Result<ExitStatus, String> {
        self.stdin.take();
        if kill {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let kill_deadline = Instant::now() + CLEANUP_TIMEOUT;
                while Instant::now() < kill_deadline {
                    if let Some(status) =
                        self.child.try_wait().map_err(|error| error.to_string())?
                    {
                        return Ok(status);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                return Err("worker did not exit within bounded cleanup time".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for LocalWorker {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn read_bounded_line(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::with_capacity(64);
    loop {
        let mut byte = [0u8; 1];
        match reader.read(&mut byte)? {
            0 if frame.is_empty() => return Ok(None),
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial frame",
                ));
            }
            _ => {
                let value = byte.first().copied().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "empty read buffer")
                })?;
                if value == b'\n' {
                    return Ok(Some(frame));
                }
                if frame.len() >= MAX_FRAME {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "frame exceeds bound",
                    ));
                }
                frame.push(value);
            }
        }
    }
}

fn read_worker_stdout(stdout: impl Read + Send + 'static, tx: SyncSender<Result<Vec<u8>, String>>) {
    let mut reader = BufReader::new(stdout);
    let mut lines = 0usize;
    let mut noise = 0usize;
    loop {
        match read_bounded_line(&mut reader) {
            Ok(Some(frame)) => {
                lines = lines.saturating_add(1);
                if lines > MAX_LINES {
                    let _ = tx.send(Err("worker line cap exceeded".to_owned()));
                    return;
                }
                if frame.starts_with(PREFIX_BYTES) {
                    if tx.send(Ok(frame)).is_err() {
                        return;
                    }
                } else {
                    noise = noise.saturating_add(frame.len());
                    if noise > MAX_HARNESS_NOISE {
                        let _ = tx.send(Err("test-harness output cap exceeded".to_owned()));
                        return;
                    }
                }
            }
            Ok(None) => {
                let _ = tx.send(Err("worker response pipe closed".to_owned()));
                return;
            }
            Err(error) => {
                let _ = tx.send(Err(error.to_string()));
                return;
            }
        }
    }
}

fn spawn_worker(role: &str) -> Result<LocalWorker, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = Command::new(executable)
        .arg("process_worker_entry")
        .arg("--nocapture")
        .env("M3_INTERNAL_SYNTHETIC_TEST_ROLE", role)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    let stdin = child.stdin.take().ok_or("worker stdin was not piped")?;
    let stdout = child.stdout.take().ok_or("worker stdout was not piped")?;
    let (tx, replies) = mpsc::sync_channel(2);
    let _reader_thread = thread::spawn(move || read_worker_stdout(stdout, tx));
    Ok(LocalWorker {
        child,
        stdin: Some(BufWriter::new(stdin)),
        replies,
    })
}

fn parse_number(value: Option<&str>) -> Result<u64, String> {
    value
        .ok_or_else(|| "missing integer field".to_owned())?
        .parse::<u64>()
        .map_err(|error| error.to_string())
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as FmtWrite;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("invalid lowercase hex".to_owned()),
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let source = text.as_bytes();
    if source.len() > MAX_FRAME || source.len() % 2 != 0 {
        return Err("hex field exceeded its bound".to_owned());
    }
    let mut decoded = Vec::with_capacity(source.len() / 2);
    for pair in source.chunks_exact(2) {
        let high = *pair.first().ok_or("short hex pair")?;
        let low = *pair.get(1).ok_or("short hex pair")?;
        decoded.push((hex_nibble(high)? << 4) | hex_nibble(low)?);
    }
    Ok(decoded)
}

fn make_payload(
    side: Side,
    epoch: u64,
    seq: u64,
    version: u64,
    variation: u32,
    delivery: u64,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(41);
    bytes.extend_from_slice(b"M3P1");
    bytes.push(match side {
        Side::Legacy => b'L',
        Side::Candidate => b'C',
    });
    bytes.extend_from_slice(&epoch.to_be_bytes());
    bytes.extend_from_slice(&seq.to_be_bytes());
    bytes.extend_from_slice(&version.to_be_bytes());
    bytes.extend_from_slice(&variation.to_be_bytes());
    bytes.extend_from_slice(&delivery.to_be_bytes());
    bytes
}

fn state_epoch(side: Side) -> u64 {
    match side {
        Side::Legacy => 11,
        Side::Candidate => 12,
    }
}

fn payload_u64(bytes: &[u8], start: usize) -> Result<u64, String> {
    let end = start.checked_add(8).ok_or("payload offset overflow")?;
    let raw: [u8; 8] = bytes
        .get(start..end)
        .ok_or("payload is truncated")?
        .try_into()
        .map_err(|_| "payload integer width mismatch")?;
    Ok(u64::from_be_bytes(raw))
}

fn decode_payload(bytes: Vec<u8>) -> Result<Payload, String> {
    if bytes.len() != 41 || bytes.get(..4) != Some(b"M3P1".as_slice()) {
        return Err("invalid synthetic payload shape".to_owned());
    }
    let side = match bytes.get(4).copied() {
        Some(b'L') => Side::Legacy,
        Some(b'C') => Side::Candidate,
        _ => return Err("invalid producer side".to_owned()),
    };
    let source_epoch = payload_u64(&bytes, 5)?;
    if source_epoch
        != match side {
            Side::Legacy => 11,
            Side::Candidate => 12,
        }
    {
        return Err("payload epoch does not match its synthetic producer side".to_owned());
    }
    let variation: [u8; 4] = bytes
        .get(29..33)
        .ok_or("payload is truncated")?
        .try_into()
        .map_err(|_| "payload variation width mismatch")?;
    Ok(Payload {
        side,
        source_epoch,
        source_sequence: payload_u64(&bytes, 13)?,
        state_version: payload_u64(&bytes, 21)?,
        variation: u32::from_be_bytes(variation),
        delivery_sequence: payload_u64(&bytes, 33)?,
        bytes,
    })
}

fn write_worker_frame(stdout: &mut impl Write, fields: &str) -> Result<(), String> {
    let line = format!("{PREFIX}{fields}\n");
    if line.len() > MAX_FRAME {
        return Err("worker response exceeded frame bound".to_owned());
    }
    stdout
        .write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    stdout.flush().map_err(|error| error.to_string())
}

fn worker_producer(side: Side) -> Result<(), String> {
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    while let Some(frame) = read_bounded_line(&mut input).map_err(|error| error.to_string())? {
        let line = std::str::from_utf8(&frame).map_err(|error| error.to_string())?;
        let mut fields = line
            .strip_prefix(PREFIX)
            .ok_or("bad protocol prefix")?
            .split('\t');
        let command = fields.next().ok_or("missing command")?;
        let (delivery, seq, version, variation) = match command {
            "STATE" => (
                0,
                parse_number(fields.next())?,
                parse_number(fields.next())?,
                parse_number(fields.next())?,
            ),
            "OUTPUT" => (
                parse_number(fields.next())?,
                parse_number(fields.next())?,
                parse_number(fields.next())?,
                parse_number(fields.next())?,
            ),
            _ => return Err("unknown producer command".to_owned()),
        };
        if fields.next().is_some() || seq == 0 || version == 0 || variation > u64::from(u32::MAX) {
            return Err("producer request outside bounds".to_owned());
        }
        let epoch = state_epoch(side);
        let payload = make_payload(
            side,
            epoch,
            seq,
            version,
            u32::try_from(variation).map_err(|error| error.to_string())?,
            delivery,
        );
        write_worker_frame(&mut output, &format!("PRODUCER\t{}", encode_hex(&payload)))?;
    }
    Ok(())
}

fn worker_consumer() -> Result<(), String> {
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let mut expected = 1u64;
    while let Some(frame) = read_bounded_line(&mut input).map_err(|error| error.to_string())? {
        let line = std::str::from_utf8(&frame).map_err(|error| error.to_string())?;
        let mut fields = line
            .strip_prefix(PREFIX)
            .ok_or("bad protocol prefix")?
            .split('\t');
        if fields.next() != Some("DELIVER") {
            return Err("unknown consumer command".to_owned());
        }
        let seq = parse_number(fields.next())?;
        let side = match fields.next() {
            Some("Legacy") => Side::Legacy,
            Some("Candidate") => Side::Candidate,
            _ => return Err("invalid side".to_owned()),
        };
        let payload = decode_payload(decode_hex(fields.next().ok_or("missing payload")?)?)?;
        if fields.next().is_some() || payload.side != side || payload.delivery_sequence != seq {
            return Err("delivery frame does not bind to payload".to_owned());
        }
        if seq < expected {
            write_worker_frame(
                &mut output,
                &format!("CONSUMER\tNACK\tDUPLICATE\t{expected}\t{seq}"),
            )?;
        } else if seq > expected {
            write_worker_frame(
                &mut output,
                &format!("CONSUMER\tNACK\tGAP\t{expected}\t{seq}"),
            )?;
        } else {
            write_worker_frame(
                &mut output,
                &format!("CONSUMER\tACK\t{seq}\t{}", encode_hex(&payload.bytes)),
            )?;
            expected = expected
                .checked_add(1)
                .ok_or("consumer sequence overflow")?;
        }
    }
    Ok(())
}

fn parse_producer_frame(frame: Vec<u8>) -> Result<Payload, String> {
    let line = String::from_utf8(frame).map_err(|error| error.to_string())?;
    let mut fields = line
        .strip_prefix(PREFIX)
        .ok_or("bad response prefix")?
        .split('\t');
    if fields.next() != Some("PRODUCER") {
        return Err("unexpected producer response".to_owned());
    }
    decode_payload(decode_hex(
        fields.next().ok_or("missing producer payload")?,
    )?)
}

fn request_state(
    worker: &mut LocalWorker,
    seq: u64,
    version: u64,
    variation: u32,
) -> Result<Payload, String> {
    let line = format!("{PREFIX}STATE\t{seq}\t{version}\t{variation}\n");
    parse_producer_frame(worker.request(&line)?)
}

fn request_output(worker: &mut LocalWorker, seq: u64, state: &Payload) -> Result<Payload, String> {
    if state.delivery_sequence != 0 {
        return Err("state payload has a delivery sequence".to_owned());
    }
    let line = format!(
        "{PREFIX}OUTPUT\t{seq}\t{}\t{}\t{}\n",
        state.source_sequence, state.state_version, state.variation
    );
    let payload = parse_producer_frame(worker.request(&line)?)?;
    if payload.side != state.side
        || payload.source_epoch != state.source_epoch
        || payload.source_sequence != state.source_sequence
        || payload.state_version != state.state_version
        || payload.variation != state.variation
        || payload.delivery_sequence != seq
    {
        return Err("output payload differs from the mirrored producer state".to_owned());
    }
    Ok(payload)
}

fn send_to_consumer(worker: &mut LocalWorker, payload: &Payload) -> Result<ConsumerReply, String> {
    let side = match payload.side {
        Side::Legacy => "Legacy",
        Side::Candidate => "Candidate",
    };
    let line = format!(
        "{PREFIX}DELIVER\t{}\t{side}\t{}\n",
        payload.delivery_sequence,
        encode_hex(&payload.bytes)
    );
    let response = String::from_utf8(worker.request(&line)?).map_err(|error| error.to_string())?;
    let mut fields = response
        .strip_prefix(PREFIX)
        .ok_or("bad consumer response prefix")?
        .split('\t');
    if fields.next() != Some("CONSUMER") {
        return Err("unexpected consumer response".to_owned());
    }
    match fields.next() {
        Some("ACK") => {
            let seq = parse_number(fields.next())?;
            let echoed = decode_hex(fields.next().ok_or("missing ACK echo")?)?;
            if fields.next().is_some() {
                return Err("trailing ACK fields".to_owned());
            }
            Ok(ConsumerReply::Ack {
                sequence: seq,
                echoed,
            })
        }
        Some("NACK") => {
            let kind = fields.next().ok_or("missing NACK kind")?;
            let expected = parse_number(fields.next())?;
            let received = parse_number(fields.next())?;
            if fields.next().is_some() {
                return Err("trailing NACK fields".to_owned());
            }
            match kind {
                "DUPLICATE" => Ok(ConsumerReply::Duplicate { expected, received }),
                "GAP" => Ok(ConsumerReply::Gap { expected, received }),
                _ => Err("unknown NACK kind".to_owned()),
            }
        }
        _ => Err("missing consumer ACK/NACK".to_owned()),
    }
}

fn snapshot_from_payload(
    unit: SwapUnitKey,
    policy: UnitPolicy,
    side: Side,
    payload: &Payload,
    at_ns: i64,
) -> Result<Snapshot, Refusal> {
    if payload.side != side
        || payload.source_epoch != state_epoch(side)
        || payload.delivery_sequence != 0
        || payload.source_sequence == 0
        || payload.state_version == 0
    {
        return Err(Refusal::BadSnapshot);
    }
    let mut snapshot = drill::snapshot(unit, policy, side, at_ns, payload.variation)?;
    snapshot.source_epoch = payload.source_epoch;
    snapshot.sequence = payload.source_sequence;
    snapshot.state_version = payload.state_version;
    let expected_lat = i32::try_from(payload.variation).map_err(|_| Refusal::BadSnapshot)?;
    if snapshot
        .tracks
        .first()
        .copied()
        .flatten()
        .map(|row| row.lat_e7)
        != Some(expected_lat)
    {
        return Err(Refusal::StateMismatch);
    }
    Ok(snapshot)
}

fn wall_event_ns(origin: Instant, epoch_ns: i64) -> Result<i64, Refusal> {
    let elapsed = i64::try_from(origin.elapsed().as_nanos()).map_err(|_| Refusal::TimeOverflow)?;
    epoch_ns.checked_add(elapsed).ok_or(Refusal::TimeOverflow)
}

fn finish_worker(worker: &mut LocalWorker, kill: bool) -> Result<ExitStatus, String> {
    worker.finish(kill)
}

#[test]
fn process_worker_entry() {
    let role = std::env::var("M3_INTERNAL_SYNTHETIC_TEST_ROLE");
    let outcome = match role.as_deref() {
        Ok("producer-legacy") => worker_producer(Side::Legacy),
        Ok("producer-candidate") => worker_producer(Side::Candidate),
        Ok("consumer") => worker_consumer(),
        _ => return,
    };
    assert!(outcome.is_ok(), "synthetic worker failed: {outcome:?}");
}

// REQ: OA-MIG-305
#[test]
fn local_process_failover_delivers_selected_payload_and_preserves_consumer_sequence() {
    let mut legacy = spawn_worker("producer-legacy").expect("legacy producer process");
    let mut candidate = spawn_worker("producer-candidate").expect("candidate producer process");
    let mut consumer = spawn_worker("consumer").expect("independent consumer process");
    let policy = drill::drill_policy();
    let start_ns = 5_000;
    let unit = SwapUnitKey {
        function_id: 1,
        scope_id: 1,
        window_start_ns: start_ns,
        window_end_ns: start_ns + 30_000_000_000,
    };
    let mut selector = new_selector(
        unit,
        policy,
        EvidenceKind::Synthetic,
        start_ns,
        &[1001, 1002],
    )
    .expect("empty M3 selector fixture");
    let epoch_ns = start_ns + 10;
    let wall_origin = Instant::now();

    let legacy_1 = request_state(&mut legacy, 1, 1, 0).expect("legacy producer state frame");
    let candidate_1 =
        request_state(&mut candidate, 1, 1, 0).expect("candidate producer state frame");
    let mirror_1_at = wall_event_ns(wall_origin, epoch_ns).expect("first mirror time");
    ingest(
        &mut selector,
        snapshot_from_payload(unit, policy, Side::Legacy, &legacy_1, mirror_1_at)
            .expect("legacy mirror from payload"),
        mirror_1_at,
        1_000_000_000,
    )
    .expect("ingest legacy mirror");
    ingest(
        &mut selector,
        snapshot_from_payload(unit, policy, Side::Candidate, &candidate_1, mirror_1_at)
            .expect("candidate mirror from payload"),
        mirror_1_at,
        1_000_000_000,
    )
    .expect("ingest candidate mirror");
    let grant = drill::window_authorization(&selector, unit, policy, mirror_1_at)
        .expect("accepted reusable M1 S2 window grant");
    let (report_1, ready_1) =
        prepare_candidate(&selector, mirror_1_at, 1_000_000_000).expect("first reconcile");
    assert!(report_1.ready && report_1.differences == 0);
    let intent_1_at = wall_event_ns(wall_origin, epoch_ns).expect("first intent time");
    let intent_1 = drill::promotion_intent(&selector, &ready_1, &grant, 1, intent_1_at)
        .expect("first fresh M2 intent");
    let decision_1_started = Instant::now();
    audited_select_candidate(
        &mut selector,
        &ready_1,
        &grant,
        &intent_1,
        intent_1_at,
        1_000_000_000,
    )
    .expect("select candidate");
    assert_eq!(selected(&selector), Output::Candidate);
    let selected_1 =
        request_output(&mut candidate, 1, &candidate_1).expect("candidate output payload");
    let ack_1 = send_to_consumer(&mut consumer, &selected_1)
        .expect("consumer response for candidate payload");
    assert_eq!(
        ack_1,
        ConsumerReply::Ack {
            sequence: 1,
            echoed: selected_1.bytes.clone()
        }
    );
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Candidate,
        1,
        wall_event_ns(wall_origin, epoch_ns).expect("candidate ACK time"),
    )
    .expect("M2 observes actual candidate ACK");
    let decision_1_to_ack = decision_1_started.elapsed();
    assert!(
        decision_1_to_ack <= TARGET_DECISION_TO_ACK,
        "first local process delivery missed 1s target: {decision_1_to_ack:?}"
    );

    let rollback_decision_started = Instant::now();
    let rollback_1_at = wall_event_ns(wall_origin, epoch_ns).expect("first rollback time");
    audited_rollback(
        &mut selector,
        drill::rollback_receipt(unit, policy, 2, 1, 1, rollback_1_at, rollback_1_at),
        rollback_1_at,
        1_000_000_000,
    )
    .expect("first rollback");
    let selected_2 = request_output(&mut legacy, 2, &legacy_1).expect("legacy rollback payload");
    let ack_2 = send_to_consumer(&mut consumer, &selected_2)
        .expect("consumer response for rollback payload");
    assert_eq!(
        ack_2,
        ConsumerReply::Ack {
            sequence: 2,
            echoed: selected_2.bytes.clone()
        }
    );
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Legacy,
        2,
        wall_event_ns(wall_origin, epoch_ns).expect("legacy ACK time"),
    )
    .expect("M2 observes actual legacy ACK");
    let rollback_decision_to_ack = rollback_decision_started.elapsed();
    assert!(
        rollback_decision_to_ack <= TARGET_DECISION_TO_ACK,
        "first rollback delivery missed 1s target: {rollback_decision_to_ack:?}"
    );

    let legacy_2 = request_state(&mut legacy, 2, 2, 1).expect("refreshed legacy state frame");
    let candidate_2 =
        request_state(&mut candidate, 2, 2, 1).expect("refreshed candidate state frame");
    let mirror_2_at = wall_event_ns(wall_origin, epoch_ns).expect("refreshed mirror time");
    ingest(
        &mut selector,
        snapshot_from_payload(unit, policy, Side::Legacy, &legacy_2, mirror_2_at)
            .expect("refreshed legacy mirror from payload"),
        mirror_2_at,
        1_000_000_000,
    )
    .expect("ingest refreshed legacy mirror");
    ingest(
        &mut selector,
        snapshot_from_payload(unit, policy, Side::Candidate, &candidate_2, mirror_2_at)
            .expect("refreshed candidate mirror from payload"),
        mirror_2_at,
        1_000_000_000,
    )
    .expect("ingest refreshed candidate mirror");
    let (report_2, ready_2) =
        prepare_candidate(&selector, mirror_2_at, 1_000_000_000).expect("second reconcile");
    assert!(report_2.ready && report_2.differences == 0);
    let intent_2_at = wall_event_ns(wall_origin, epoch_ns).expect("second intent time");
    let intent_2 = drill::promotion_intent(&selector, &ready_2, &grant, 3, intent_2_at)
        .expect("fresh M2 intent under reused M1 grant");
    let fallback_started = Instant::now();
    audited_select_candidate(
        &mut selector,
        &ready_2,
        &grant,
        &intent_2,
        intent_2_at,
        1_000_000_000,
    )
    .expect("second candidate selection");
    let candidate_exit =
        finish_worker(&mut candidate, true).expect("bounded candidate process loss");
    assert!(
        !candidate_exit.success(),
        "candidate loss injection did not terminate producer"
    );
    let locally_refused_candidate_request = request_output(&mut candidate, 3, &candidate_2);
    assert!(
        locally_refused_candidate_request.is_err(),
        "closed producer wrapper accepted a request after process loss"
    );
    let rollback_2_at = wall_event_ns(wall_origin, epoch_ns).expect("fallback rollback time");
    audited_rollback(
        &mut selector,
        drill::rollback_receipt(unit, policy, 4, 2, 2, rollback_2_at, rollback_2_at),
        rollback_2_at,
        1_000_000_000,
    )
    .expect("rollback with fresh legacy mirror");
    let fallback_payload =
        request_output(&mut legacy, 3, &legacy_2).expect("legacy fallback payload");
    let fallback_ack =
        send_to_consumer(&mut consumer, &fallback_payload).expect("consumer fallback response");
    assert_eq!(
        fallback_ack,
        ConsumerReply::Ack {
            sequence: 3,
            echoed: fallback_payload.bytes.clone()
        }
    );
    observe_delivery(
        &mut selector,
        policy.consumer_id,
        Output::Legacy,
        3,
        wall_event_ns(wall_origin, epoch_ns).expect("fallback ACK time"),
    )
    .expect("M2 observes actual fallback ACK");
    let fallback_decision_to_ack = fallback_started.elapsed();
    assert!(
        fallback_decision_to_ack <= TARGET_DECISION_TO_ACK,
        "rollback fallback missed 1s target: {fallback_decision_to_ack:?}"
    );

    let duplicate =
        send_to_consumer(&mut consumer, &selected_2).expect("duplicate negative-control response");
    assert_eq!(
        duplicate,
        ConsumerReply::Duplicate {
            expected: 4,
            received: 2
        }
    );
    assert_eq!(
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Legacy,
            2,
            wall_event_ns(wall_origin, epoch_ns).expect("duplicate time")
        ),
        Err(Refusal::OutputDuplicate)
    );
    let gap_payload =
        request_output(&mut legacy, 5, &legacy_2).expect("gap negative-control payload");
    let gap = send_to_consumer(&mut consumer, &gap_payload).expect("gap negative-control response");
    assert_eq!(
        gap,
        ConsumerReply::Gap {
            expected: 4,
            received: 5
        }
    );
    assert_eq!(
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Legacy,
            5,
            wall_event_ns(wall_origin, epoch_ns).expect("gap time")
        ),
        Err(Refusal::OutputGap)
    );

    assert!(finish_worker(&mut legacy, false)
        .expect("legacy producer exit")
        .success());
    assert!(finish_worker(&mut consumer, false)
        .expect("consumer exit")
        .success());
    let (records, record_count) = trace(&selector);
    assert_eq!(record_count, 4);
    assert_eq!(selected(&selector), Output::Legacy);
    assert_eq!(
        records
            .get(3)
            .copied()
            .flatten()
            .map(|record| (record.to, record.reason)),
        Some((Output::Legacy, 2))
    );
    assert_eq!(
        records
            .first()
            .copied()
            .flatten()
            .map(|record| record.m1_window_sequence),
        records
            .get(2)
            .copied()
            .flatten()
            .map(|record| record.m1_window_sequence)
    );

    let consumer_ack_count = 3u32;
    let producer_failure_count = 1u32;
    let consumer_loss_count = 0u32;
    let gap_refusal_count = 1u32;
    let duplicate_refusal_count = 1u32;
    assert_eq!(
        (
            consumer_ack_count,
            producer_failure_count,
            consumer_loss_count,
            gap_refusal_count,
            duplicate_refusal_count
        ),
        (3, 1, 0, 1, 1)
    );
    assert!(
        record_count < u16::try_from(oa_migration_switch::MAX_TRACE).expect("bounded trace size")
    );
    println!(
        "M3_INTERNAL_SYNTHETIC_TEST_REPORT\tversion=1\tselected_output_opportunities=4\tproducer_frames_for_primary_delivery=3\tproducer_failures=1\tproducer_requests_refused_locally_after_loss=1\tconsumer_offered=3\tconsumer_acked=3\tconsumer_lost=0\tconsumer_sequences=1,2,3\tnegative_gap_refusals=1\tnegative_duplicate_refusals=1\tfirst_decision_to_ack_ns={}\trollback_decision_to_ack_ns={}\tfallback_decision_to_ack_ns={}\ttarget_ms={}\tresponse_timeout_ms={}\tcleanup_timeout_ms={}\tprotocol=internal_synthetic_only\trecorded=0\tqualification=INCOMPLETE",
        decision_1_to_ack.as_nanos(),
        rollback_decision_to_ack.as_nanos(),
        fallback_decision_to_ack.as_nanos(),
        TARGET_DECISION_TO_ACK.as_millis(),
        RESPONSE_TIMEOUT.as_millis(),
        CLEANUP_TIMEOUT.as_millis()
    );
}
