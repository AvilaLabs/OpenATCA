#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use oa_flight::*;
use oa_workstation::*;

fn seed() -> (FlightTable, Position, Position, Coordination) {
    let mut table = flights_new(&FlightConfig {
        capacity: 4,
        lag_floor_ms: 0,
        max_future_ms: 5000,
        refusal_log: 32,
        conflict_log: 8,
        removal_log: 8,
    })
    .unwrap();
    let mut plan = op_new(OP_SET_PLAN, key_gufi(1), *b"ZAU\0", 1000, 1000, 1);
    plan.state = ST_ACTIVE;
    assert_eq!(flights_apply(&mut table, &plan), Outcome::Applied);
    let mut identity = op_new(OP_IDENTITY, key_gufi(1), *b"ZAU\0", 1001, 1001, 2);
    identity.callsign = identifier("UAL901").unwrap();
    assert_eq!(flights_apply(&mut table, &identity), Outcome::Applied);
    let mut owner = op_new(OP_OWNER, key_gufi(1), *b"ZAU\0", 1002, 1002, 3);
    owner.unit_a = identifier("ZAU12").unwrap();
    assert_eq!(flights_apply(&mut table, &owner), Outcome::Applied);
    let coord = Coordination {
        id: identifier("UAL901").unwrap(),
        ..EMPTY_COORDINATION
    };
    (
        table,
        position_new(identifier("ZAU12").unwrap(), R_SIDE, 45).unwrap(),
        position_new(identifier("ZAU13").unwrap(), R_SIDE, 45).unwrap(),
        coord,
    )
}
fn apply(
    pos: &mut Position,
    t: &mut FlightTable,
    c: &mut Coordination,
    s: &str,
    stamp: i64,
) -> Result<&'static str, &'static str> {
    execute(
        pos,
        t,
        c,
        &parse(s)?,
        &Context {
            key: key_gufi(1),
            centre: *b"ZAU\0",
            at_ms: stamp,
            message: stamp as u64,
        },
    )
}

// REQ: OA-WS-001
#[test]
fn bounded_grammar_refuses_unknown_ambiguous_and_malformed_commands() {
    assert_eq!(parse("qz 300 ual901").unwrap().value, 30000);
    assert_eq!(parse("QU 101").unwrap().target, identifier("101").unwrap());
    for value in [
        "QZ 300 101 EXTRA",
        "QZ NaN 101",
        "QZ -1 101",
        "QZ 451 101",
        "FILTER 450 100",
        "DB 0 UAL901",
        "RANGE 999",
        "BOGUS UAL901",
        "QS ABCDEFGHIJKLMNOPQ UAL901",
        "QF ✈",
    ] {
        assert!(parse(value).is_err(), "{value}");
    }
    assert!(parse(&"X".repeat(97)).is_err());
}

#[test]
fn default_preview_gate_refuses_commands_before_any_workstation_mutation() {
    let kinds = [
        Kind::Readout,
        Kind::Route,
        Kind::Assigned,
        Kind::Interim,
        Kind::Annotation,
        Kind::Offer,
        Kind::Accept,
        Kind::Control,
        Kind::Retract,
        Kind::Voice,
        Kind::Offset,
        Kind::PointOut,
        Kind::Acknowledge,
        Kind::Range,
        Kind::Filter,
        Kind::History,
        Kind::Vector,
    ];
    assert!(kinds
        .iter()
        .copied()
        .all(|kind| !default_command_available(kind)));

    let (mut table, mut position, _, mut coordination) = seed();
    let before_table = flights_digest(&table);
    let before_position = position.clone();
    let before_coordination = coordination;
    let context = Context {
        key: key_gufi(1),
        centre: *b"ZAU\0",
        at_ms: 2000,
        message: 99,
    };
    for input in ["QZ 300 UAL901", "VCI UAL901", "FILTER 100 200"] {
        let receipt = execute_prototype_entry(
            input,
            &mut position,
            &mut table,
            &mut coordination,
            &context,
        );
        assert_eq!(receipt.outcome, EntryOutcome::Refused);
        assert!(receipt.fidelity.response_is_openatca_difference);
        assert_eq!(receipt.fidelity.response_row_id, None);
        assert_eq!(
            receipt.fidelity.response_confidence,
            EvidenceConfidence::OpenAtcaOnly
        );
        assert_eq!(flights_digest(&table), before_table);
        assert_eq!(position, before_position);
        assert_eq!(coordination, before_coordination);
    }
    let voice = execute_prototype_entry(
        "VCI UAL901",
        &mut position,
        &mut table,
        &mut coordination,
        &context,
    );
    assert_eq!(voice.kind, Some(Kind::Voice));
    assert_eq!(voice.fidelity.context_row_id, None);
}
// REQ: OA-WS-002
#[test]
fn shared_altitude_edits_require_ownership_and_preserve_interim_distinction() {
    let (mut t, mut a, mut b, mut c) = seed();
    apply(&mut a, &mut t, &mut c, "QZ 300 UAL901", 2000).unwrap();
    apply(&mut a, &mut t, &mut c, "QQ 290 UAL901", 2001).unwrap();
    let f = flight_get(&t, &key_gufi(1)).unwrap();
    assert_eq!(f.assigned.a_ft, 30000);
    assert_eq!(f.interim.a_ft, 29000);
    let before = flights_digest(&t);
    assert!(apply(&mut b, &mut t, &mut c, "QZ 280 UAL901", 2002).is_err());
    assert_eq!(flights_digest(&t), before);
    apply(&mut a, &mut t, &mut c, "QQ 0 UAL901", 2003).unwrap();
    assert_eq!(flight_get(&t, &key_gufi(1)).unwrap().interim.form, ALT_NIL);
    assert_eq!(flight_get(&t, &key_gufi(1)).unwrap().interim.a_ft, 0);
    let mut forged = parse("RANGE 30").unwrap();
    forged.value = 10000;
    assert!(execute(
        &mut a,
        &mut t,
        &mut c,
        &forged,
        &Context {
            key: key_gufi(1),
            centre: *b"ZAU\0",
            at_ms: 2004,
            message: 2004
        }
    )
    .is_err());
    assert_eq!(a.range_nm, 45);
}
// REQ: OA-WS-003
#[test]
fn handoff_and_pointout_have_distinct_shared_state() {
    let (mut t, mut a, mut b, mut c) = seed();
    apply(&mut a, &mut t, &mut c, "PO ZAU13 UAL901", 2000).unwrap();
    assert!(apply(&mut a, &mut t, &mut c, "ACKPO UAL901", 2001).is_err());
    apply(&mut b, &mut t, &mut c, "ACKPO UAL901", 2002).unwrap();
    assert_eq!(c.acknowledged, 1);
    assert_eq!(flight_get(&t, &key_gufi(1)).unwrap().owner, a.sector);
    assert!(apply(&mut b, &mut t, &mut c, "CONTROL UAL901", 2003).is_err());
    apply(&mut a, &mut t, &mut c, "HO ZAU13 UAL901", 2004).unwrap();
    apply(&mut b, &mut t, &mut c, "ACCEPT UAL901", 2005).unwrap();
    assert_eq!(flight_get(&t, &key_gufi(1)).unwrap().owner, a.sector);
    apply(&mut b, &mut t, &mut c, "CONTROL UAL901", 2006).unwrap();
    assert_eq!(flight_get(&t, &key_gufi(1)).unwrap().owner, b.sector);
    assert_eq!(t.stats.handoffs_completed, 1);
}
// REQ: OA-WS-004
#[test]
fn position_preferences_are_separate_while_annotations_are_shared() {
    let (mut t, mut a, _, mut c) = seed();
    let mut d = position_new(a.sector, D_SIDE, 45).unwrap();
    apply(&mut a, &mut t, &mut c, "RANGE 30", 2000).unwrap();
    apply(&mut a, &mut t, &mut c, "DB 7 UAL901", 2001).unwrap();
    assert_eq!(d.range_nm, 45);
    assert_eq!(block_of(&d, &identifier("UAL901").unwrap()).offset, 9);
    apply(&mut d, &mut t, &mut c, "QS H120/S380 UAL901", 2002).unwrap();
    assert_eq!(&c.annotation[..9], b"H120/S380");
    assert!(apply(&mut d, &mut t, &mut c, "VCI UAL901", 2003).is_err());
    apply(&mut a, &mut t, &mut c, "VCI UAL901", 2004).unwrap();
    assert_eq!(c.voice, 1);
}
