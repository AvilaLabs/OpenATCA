#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use oa_display::interaction::CommandOutcome;
use oa_regional_session::cli::synthetic_model;
use oa_regional_session::runtime::{regional_new, regional_step, RegionalFrame, RegionalRuntime};
use oa_regional_session::surveillance::SurveillanceConfig;
use oa_regional_session::{parse_event_line, ReceiptBatch};

const SECOND: i64 = 1_000_000_000;
const FLIGHT: u128 = 0x1234;
const ADDRESS: u32 = 0xabc101;
const PLAN: &str = "PLAN00000001";

fn row(
    receipt: i64,
    source: i64,
    sequence: u64,
    body: &str,
) -> oa_regional_session::NormalizedEvent {
    parse_event_line(&format!(
        "v1\t{receipt}\t{source}\t{sequence}\tFLIGHT\t{FLIGHT:032X}\t{ADDRESS:06X}\tZAU1\t{body}"
    ))
    .expect("valid test event")
}

fn position(receipt: i64, sequence: u64) -> oa_regional_session::NormalizedEvent {
    parse_event_line(&format!("v1\t{receipt}\t{receipt}\t{sequence}\tSURVEILLANCE\t{FLIGHT:032X}\t{ADDRESS:06X}\tZAU1\tPOS\t410000000\t-870000000\t10000\t0\t0\t0"))
        .expect("valid position")
}

fn runtime() -> RegionalRuntime {
    let model = synthetic_model().expect("synthetic model");
    let bytes = oa_airspace::core::snapshot::encode(&model).expect("snapshot bytes");
    regional_new(
        model,
        oa_sha256::sha256(&bytes),
        *b"ZAU1",
        SurveillanceConfig {
            centre_lat_e7: 410000000,
            centre_lon_e7: -870000000,
            range_nm: 180.0,
        },
    )
    .expect("runtime")
}

fn ready(with_owner: bool) -> (RegionalRuntime, RegionalFrame) {
    let mut engine = runtime();
    let mut events = vec![position(SECOND, 1)];
    let mut seq = 1;
    for body in [
        format!("FLIGHT\tPLAN\t{PLAN}\t2"),
        "FLIGHT\tADDRESS".into(),
        "FLIGHT\tASSIGNED\t12000".into(),
    ] {
        events.push(row(SECOND, SECOND, seq, &body));
        seq += 1;
    }
    if with_owner {
        events.push(row(SECOND, SECOND, seq, "FLIGHT\tOWNER\tSECTOR1"));
    }
    regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: SECOND,
            source_clock_regressions: 0,
            events,
        },
    )
    .expect("seed frame");
    let frame = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 2 * SECOND,
            source_clock_regressions: 0,
            events: vec![position(2 * SECOND, 4)],
        },
    )
    .expect("confirmed frame");
    assert_eq!(frame.surveillance.tracks.len(), 1);
    assert!(
        frame
            .flights
            .first()
            .is_some_and(|flight| flight.view.is_some()),
        "command fixture lacks a current full-key flight view: report={:?}, flights={:?}",
        frame.flight_report,
        frame.flights
    );
    (engine, frame)
}

fn command(
    frame: &RegionalFrame,
    receipt: i64,
    seq: u64,
    actor: u32,
    operation: &str,
    value: Option<&str>,
) -> oa_regional_session::NormalizedEvent {
    command_at(frame, receipt, receipt, seq, actor, operation, value)
}

fn command_at(
    frame: &RegionalFrame,
    receipt: i64,
    source: i64,
    seq: u64,
    actor: u32,
    operation: &str,
    value: Option<&str>,
) -> oa_regional_session::NormalizedEvent {
    let target = frame.controller_views.first().expect("target view");
    let mut body = format!(
        "COMMAND\t{}\t{}\t{}\t{}\t{operation}",
        PLAN, target.generation, frame.sequence, actor
    );
    if let Some(value) = value {
        body.push('\t');
        body.push_str(value);
    }
    row(receipt, source, seq, &body)
}

fn step(
    engine: &mut RegionalRuntime,
    event: oa_regional_session::NormalizedEvent,
) -> RegionalFrame {
    let receipt_ns = event.receipt_ns;
    let position_sequence = u64::try_from(receipt_ns / SECOND)
        .expect("positive fixture receipt")
        .checked_add(2)
        .expect("bounded fixture sequence");
    regional_step(
        engine,
        &ReceiptBatch {
            receipt_ns,
            source_clock_regressions: 0,
            events: vec![position(receipt_ns, position_sequence), event],
        },
    )
    .expect("command frame")
}

fn owner(frame: &RegionalFrame) -> [u8; 8] {
    frame
        .flights
        .first()
        .and_then(|row| row.view)
        .expect("flight view")
        .owner_unit
}

// REQ: OA-REG-009
#[test]
fn handoff_offer_accept_then_take_preserves_owner_until_take() {
    let (mut engine, mut frame) = ready(true);
    assert_eq!(owner(&frame), *b"SECTOR1\0");
    frame = step(
        &mut engine,
        command(&frame, 3 * SECOND, 10, 1, "HO_OFFER", Some("2")),
    );
    assert_eq!(owner(&frame), *b"SECTOR1\0");
    assert_eq!(frame.controller_outcome, CommandOutcome::Accepted);
    frame = step(
        &mut engine,
        command(&frame, 4 * SECOND, 11, 2, "HO_ACCEPT", Some("2")),
    );
    assert_eq!(owner(&frame), *b"SECTOR1\0");
    assert_eq!(frame.controller_outcome, CommandOutcome::Accepted);
    frame = step(
        &mut engine,
        command(&frame, 5 * SECOND, 12, 2, "HO_TAKE", Some("2")),
    );
    assert_eq!(owner(&frame), *b"SECTOR2\0");
    assert_eq!(frame.controller_outcome, CommandOutcome::Accepted);
}

// REQ: OA-REG-009
#[test]
fn flight_command_flight_interleaving_validates_against_prior_state() {
    let (mut engine, frame) = ready(true);
    let command = command_at(
        &frame,
        3 * SECOND,
        2 * SECOND + 800_000_000,
        10,
        1,
        "ASSIGNED",
        Some("15000"),
    );
    let earlier_flight_update = row(
        3 * SECOND,
        2 * SECOND + 700_000_000,
        9,
        "FLIGHT\tASSIGNED\t11000",
    );
    let final_flight_update = row(
        3 * SECOND,
        2 * SECOND + 900_000_000,
        11,
        "FLIGHT\tASSIGNED\t13000",
    );
    let next = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![earlier_flight_update, command, final_flight_update],
        },
    )
    .expect("ordered mixed frame");
    assert_eq!(
        next.controller_outcome,
        CommandOutcome::Accepted,
        "seq10 executes before the seq11 update; a reordered command would be stale"
    );
    assert!(next
        .controller_dispatches
        .first()
        .is_some_and(|d| d.refusal.is_empty() && !d.terminal_failure));
    assert_eq!(
        next.flights
            .first()
            .and_then(|f| f.view)
            .expect("view")
            .assigned
            .a_ft,
        13000
    );
}

// REQ: OA-REG-009
#[test]
fn wrong_actor_old_view_old_command_and_reused_address_are_refused_visibly() {
    let (mut engine, frame) = ready(true);
    let wrong_actor = step(
        &mut engine,
        command(&frame, 3 * SECOND, 10, 2, "ASSIGNED", Some("15000")),
    );
    assert_eq!(wrong_actor.controller_outcome, CommandOutcome::Refused);
    assert!(wrong_actor.controller_dispatches[0]
        .refusal
        .contains("authority"));
    assert_eq!(wrong_actor.flights[0].view.unwrap().assigned.a_ft, 12000);

    let old_frame = command(&frame, 4 * SECOND, 11, 1, "ASSIGNED", Some("15000"));
    let old_view_batch = ReceiptBatch {
        receipt_ns: 4 * SECOND,
        source_clock_regressions: 0,
        events: vec![old_frame],
    };
    // The selection check refuses stale frame sequences before queueing; its typed refusal remains visible.
    let old_view = regional_step(&mut engine, &old_view_batch).expect("old view refusal");
    assert_eq!(old_view.controller_outcome, CommandOutcome::Stale);

    let fresh = old_view;
    // Restore a live surveillance view before testing the old command payload so the expected
    // refusal is the stale frame sequence, not the runtime's feed-loss fallback.
    regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 9 * SECOND,
            source_clock_regressions: 0,
            events: vec![position(9 * SECOND, 11)],
        },
    )
    .expect("current report before stale-command check");
    let old_command = command_at(
        &fresh,
        10 * SECOND,
        4 * SECOND,
        12,
        1,
        "ASSIGNED",
        Some("15000"),
    );
    let stale = regional_step(
        &mut engine,
        &ReceiptBatch {
            receipt_ns: 10 * SECOND,
            source_clock_regressions: 0,
            events: vec![old_command],
        },
    )
    .expect("stale command refusal");
    assert_eq!(stale.controller_outcome, CommandOutcome::Stale);
    assert!(
        !stale.flights.is_empty(),
        "current surveillance should supply a flight row: automation={:?}, tracks={}, feeds={:?}, tracker={:?}, quarantine={:?}",
        stale.automation,
        stale.surveillance.tracks.len(),
        stale.surveillance.feeds,
        stale.surveillance.tracker_stats,
        stale.quarantine.records
    );
    assert_eq!(stale.flights[0].view.unwrap().assigned.a_ft, 12000);

    let (mut reused_engine, old) = ready(true);
    let changed = parse_event_line(&format!(
        "v1\t{}\t{}\t9\tFLIGHT\t{FLIGHT:032X}\tABC102\tZAU1\tFLIGHT\tADDRESS",
        3 * SECOND,
        3 * SECOND
    ))
    .expect("address replacement event");
    let aimed_at_old = command(&old, 3 * SECOND, 10, 1, "ASSIGNED", Some("15000"));
    let reused = regional_step(
        &mut reused_engine,
        &ReceiptBatch {
            receipt_ns: 3 * SECOND,
            source_clock_regressions: 0,
            events: vec![changed, aimed_at_old],
        },
    )
    .expect("address change refusal");
    assert_eq!(reused.controller_outcome, CommandOutcome::Refused);
    assert!(reused.controller_dispatches[0]
        .refusal
        .contains("authoritative identity"));
}

// REQ: OA-REG-009, OA-UI-020
#[test]
fn unowned_flight_cannot_be_bootstrapped_by_an_interactive_command() {
    let (mut engine, frame) = ready(false);
    assert_eq!(owner(&frame), [0; 8]);
    let refused = step(
        &mut engine,
        command(&frame, 3 * SECOND, 10, 1, "ASSIGNED", Some("15000")),
    );
    assert_eq!(refused.controller_outcome, CommandOutcome::Refused);
    assert_eq!(owner(&refused), [0; 8]);
    assert_eq!(refused.flights[0].view.unwrap().assigned.a_ft, 12000);
}
