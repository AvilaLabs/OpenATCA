#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[allow(dead_code)]
#[path = "../../../crates/oa-migration-switch/examples/m3_synthetic_drill.rs"]
mod synthetic;

use oa_migration_switch::{
    observe_delivery, prepare_candidate, rollback, select_candidate, Output,
};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

struct Consumer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Consumer {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_regional-m3-recorded-drill"))
            .arg("--h5-consumer-worker")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("independent consumer process");
        let input = child.stdin.take().expect("consumer stdin");
        let output = BufReader::new(child.stdout.take().expect("consumer stdout"));
        Self {
            child,
            input,
            output,
        }
    }

    fn send(&mut self, sequence: u64, bytes: &[u8]) {
        let digest = oa_sha256::to_hex(&oa_sha256::sha256(bytes));
        let hex = bytes
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        writeln!(
            self.input,
            "H5CONSUMER\t1\tDATA\t{sequence}\t{digest}\t{hex}"
        )
        .expect("send selected output");
        self.input.flush().expect("flush selected output");
        let mut ack = String::new();
        self.output.read_line(&mut ack).expect("read consumer ACK");
        assert_eq!(
            ack.trim_end(),
            format!("H5CONSUMER\t1\tACK\t{sequence}\t{digest}")
        );
    }

    fn end_cycle(&mut self, cycle: usize, expected_messages: u64) {
        writeln!(self.input, "H5CONSUMER\t1\tEND_CYCLE\t{cycle}").expect("send cycle boundary");
        self.input.flush().expect("flush cycle boundary");
        let mut ack = String::new();
        self.output.read_line(&mut ack).expect("read cycle ACK");
        assert_eq!(
            ack.trim_end(),
            format!("H5CONSUMER\t1\tCYCLE_ACK\t{cycle}\t{expected_messages}")
        );
    }

    fn state_begin(&mut self, cycle: u64, generation: u64, tracks: u64) {
        writeln!(
            self.input,
            "H5CONSUMER\t1\tSTATE_BEGIN\t{cycle}\t{generation}\tselected=Legacy\ttracks={tracks}\tflights=0\thandoffs=0\talerts=0"
        )
        .expect("begin selected state transaction");
        self.input.flush().expect("flush state transaction begin");
    }

    fn state_row(&mut self, sequence: u64, bytes: &[u8]) {
        let digest = oa_sha256::to_hex(&oa_sha256::sha256(bytes));
        let hex = bytes
            .iter()
            .map(|value| format!("{value:02x}"))
            .collect::<String>();
        writeln!(
            self.input,
            "H5CONSUMER\t1\tSTATE_ROW\t{sequence}\t{digest}\t{hex}"
        )
        .expect("send selected state row");
        self.input.flush().expect("flush selected state row");
    }

    fn state_commit(&mut self, cycle: u64, generation: u64, tracks: u64, digest: &str) -> String {
        writeln!(
            self.input,
            "H5CONSUMER\t1\tSTATE_COMMIT\t{cycle}\t{generation}\ttracks={tracks}\tflights=0\thandoffs=0\talerts=0\t{digest}"
        )
        .expect("commit selected state");
        self.input.flush().expect("flush selected state commit");
        let mut ack = String::new();
        self.output
            .read_line(&mut ack)
            .expect("read applied-state ACK");
        ack
    }

    fn expect_failed_without_ack(mut self) {
        drop(self.input);
        let mut ack = String::new();
        let read = self
            .output
            .read_line(&mut ack)
            .expect("read failed worker output");
        assert_eq!(read, 0, "refused state must not be ACKed: {ack}");
        assert!(!self.child.wait().expect("refused worker exit").success());
    }

    fn finish(mut self) {
        drop(self.input);
        assert!(self.child.wait().expect("consumer exit").success());
    }
}

#[test]
fn one_hundred_synthetic_m2_flip_rollbacks_reach_independent_consumer() {
    let mut consumer = Consumer::new();
    let mut max_flip_decision_to_ack = 0u128;
    let mut max_rollback_decision_to_ack = 0u128;
    let mut acknowledged = 0usize;
    for index in 0..100u32 {
        let (mut selector, unit, policy, now_ns) =
            synthetic::drill_state(index).expect("selector fixture");
        let (report, readiness) =
            prepare_candidate(&selector, now_ns, policy.max_snapshot_age_ns).expect("reconcile");
        assert!(report.ready && report.differences == 0);
        let grant = synthetic::window_authorization(&selector, unit, policy, now_ns)
            .expect("M1 grant fixture");
        let intent = synthetic::promotion_intent(&selector, &readiness, &grant, 1, now_ns)
            .expect("M2 intent fixture");
        let flip_started = Instant::now();
        select_candidate(
            &mut selector,
            &readiness,
            &grant,
            &intent,
            now_ns,
            policy.max_snapshot_age_ns,
        )
        .expect("M2 candidate selection");
        assert_eq!(oa_migration_switch::selected(&selector), Output::Candidate);
        consumer.send(
            1,
            &normalized_position_row(H5_MEASUREMENT_START_NS, 1),
        );
        max_flip_decision_to_ack = max_flip_decision_to_ack.max(flip_started.elapsed().as_nanos());
        acknowledged += 1;
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Candidate,
            1,
            now_ns + 100_000_000,
        )
        .expect("candidate consumer receipt");
        let rollback_at = now_ns + 200_000_000;
        let rollback_cause =
            synthetic::rollback_receipt(unit, policy, 2, 1, 1, rollback_at, rollback_at);
        let rollback_started = Instant::now();
        rollback(
            &mut selector,
            rollback_cause,
            rollback_at,
            policy.max_snapshot_age_ns,
        )
        .expect("M2 rollback");
        assert_eq!(oa_migration_switch::selected(&selector), Output::Legacy);
        consumer.send(
            2,
            &normalized_position_row(H5_MEASUREMENT_START_NS + 1, 2),
        );
        max_rollback_decision_to_ack =
            max_rollback_decision_to_ack.max(rollback_started.elapsed().as_nanos());
        acknowledged += 1;
        observe_delivery(
            &mut selector,
            policy.consumer_id,
            Output::Legacy,
            2,
            now_ns + 300_000_000,
        )
        .expect("legacy consumer receipt");
        consumer.end_cycle(index as usize, 2);
    }
    consumer.finish();
    assert_eq!(acknowledged, 200);
    assert!(max_flip_decision_to_ack <= 1_000_000_000);
    assert!(max_rollback_decision_to_ack <= 1_000_000_000);
}

#[test]
fn independent_consumer_refuses_duplicate_and_gap_sequences() {
    for invalid_sequence in [1u64, 3u64] {
        let mut consumer = Consumer::new();
        let first_bytes = normalized_position_row(H5_MEASUREMENT_START_NS, 1);
        consumer.send(1, &first_bytes);
        let second_bytes = normalized_position_row(H5_MEASUREMENT_START_NS + 1, 2);
        consumer
            .input
            .write_all(&worker_data_frame(invalid_sequence, &second_bytes))
            .unwrap();
        consumer.input.flush().unwrap();
        let mut response = String::new();
        let _ = consumer.output.read_line(&mut response);
        assert!(response.is_empty(), "invalid sequence must not be ACKed");
        assert!(!consumer.child.wait().unwrap().success());
    }
}

fn normalized_position_row(receipt_ns: i64, sequence: u64) -> Vec<u8> {
    format!(
        "v1\t{receipt_ns}\t{receipt_ns}\t{sequence}\tSURVEILLANCE\t00000000000000000000000000000000\tABCDEF\tZAU1\tPOS\t385000000\t-935000000\t1000\t0\t0\t0"
    )
    .into_bytes()
}

fn worker_data_frame(sequence: u64, payload: &[u8]) -> Vec<u8> {
    let digest = oa_sha256::to_hex(&oa_sha256::sha256(payload));
    let hex = payload
        .iter()
        .map(|value| format!("{value:02x}"))
        .collect::<String>();
    format!("H5CONSUMER\t1\tDATA\t{sequence}\t{digest}\t{hex}\n").into_bytes()
}

#[test]
fn worker_refuses_a_hash_valid_normalized_row_with_a_sealed_timestamp() {
    let mut consumer = Consumer::new();
    let sealed_row = normalized_position_row(1_795_132_800_000_000_000, 1);
    consumer
        .input
        .write_all(&worker_data_frame(1, &sealed_row))
        .unwrap();
    consumer.input.flush().unwrap();
    consumer.expect_failed_without_ack();
}

fn valid_track_state() -> Vec<u8> {
    format!(
        "H5STATE\t1\tselected=Legacy\tTRACK\tkey={}\tordinal=0\tstatus=2\tlat_e7=385000000\tlon_e7=-935000000\taltitude_ft=1000\tdigest={}",
        "11".repeat(32),
        "22".repeat(32)
    )
    .into_bytes()
}

fn state_digest(rows: &[Vec<u8>]) -> String {
    let mut hash = oa_sha256::Sha256::new();
    for row in rows {
        hash.update(
            &u32::try_from(row.len())
                .expect("selected row length fits u32")
                .to_le_bytes(),
        )
        .expect("hash state row length");
        hash.update(row).expect("hash selected state row");
    }
    oa_sha256::to_hex(&hash.finalize())
}

// REQ: OA-MIG-414
#[test]
fn worker_applies_exact_selected_state_rows_before_acknowledging_generation() {
    let rows = [valid_track_state()];
    let digest = state_digest(&rows);
    let mut consumer = Consumer::new();
    consumer.state_begin(0, 1, 1);
    consumer.state_row(1, rows.first().expect("one selected state row"));
    let ack = consumer.state_commit(0, 1, 1, &digest);
    assert_eq!(
        ack.trim_end(),
        format!("H5CONSUMER\t1\tSTATE_APPLIED\t0\t1\tselected=Legacy\t1\t0\t0\t0\t{digest}")
    );
    consumer.finish();
}

// REQ: OA-MIG-414
#[test]
fn worker_does_not_ack_a_hash_valid_but_semantically_invalid_state_row() {
    let invalid = String::from_utf8(valid_track_state())
        .expect("valid UTF-8 fixture")
        .replace("lat_e7=385000000", "lat_e7=not-an-int")
        .into_bytes();
    let mut consumer = Consumer::new();
    consumer.state_begin(0, 1, 1);
    consumer.state_row(1, &invalid);
    consumer.expect_failed_without_ack();
}

// REQ: OA-MIG-414
#[test]
fn worker_does_not_ack_a_partial_state_generation() {
    let row = valid_track_state();
    let digest = state_digest(std::slice::from_ref(&row));
    let mut consumer = Consumer::new();
    consumer.state_begin(0, 1, 2);
    consumer.state_row(1, &row);
    let ack = consumer.state_commit(0, 1, 2, &digest);
    assert!(ack.is_empty(), "partial generation must not be applied ACK");
    consumer.expect_failed_without_ack();
}

// REQ: OA-MIG-414
#[test]
fn worker_does_not_apply_duplicate_family_business_keys() {
    let first = valid_track_state();
    let second = String::from_utf8(valid_track_state())
        .expect("valid UTF-8 fixture")
        .replace("lat_e7=385000000", "lat_e7=385000001")
        .into_bytes();
    let rows = [first, second];
    let digest = state_digest(&rows);
    let mut consumer = Consumer::new();
    consumer.state_begin(0, 1, 2);
    for (index, row) in rows.iter().enumerate() {
        consumer.state_row(u64::try_from(index).expect("two rows") + 1, row);
    }
    let ack = consumer.state_commit(0, 1, 2, &digest);
    assert!(
        ack.is_empty(),
        "duplicate business keys must not be applied ACK"
    );
    consumer.expect_failed_without_ack();
}
