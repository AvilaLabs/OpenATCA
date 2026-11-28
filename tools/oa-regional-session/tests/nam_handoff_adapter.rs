#![allow(clippy::expect_used, clippy::panic)]

use oa_flight::key_tfms;
use oa_nam::frame::frame;
use oa_nam::{bare, encode, parse, session, Facility, Field31, Header, Kind, MsgId};
use oa_regional_session::flight::{
    flight_apply_event, flight_driver_new, flight_driver_report, FlightDriverConfig,
};
use oa_regional_session::nam_handoff::{
    accepted_delivery_seq, accepted_message, nam_session_ingress_local_event,
    nam_session_ingress_new, nam_session_ingress_receive, nam_session_ingress_sequence_gap_count,
    nam_session_ingress_state, receive_response, receive_rti, NamHandoffError, NamSessionIngress,
    RlaRoute, RtiTarget, SessionAcceptedMessage,
};
use oa_regional_session::{
    EventKind, FlightOperation, HandoffOperation, Identity, NormalizedEvent, SourceKind,
};

const KZMP: Facility = Facility(*b"KZMP");
const CZWG: Facility = Facility(*b"CZWG");
const RTI: &str = "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034N0780521WN043327629F349)";
const RLA: &str = "(RLAKZBW/CZUL202CZUL/KZBW445-KBTV1A)";
const RTA: &str = "(RTACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-CZWGAA)";
const LRM: &str = "(LRMKZLC/CZWG035CZWG/KZLC021-RMK/06/07/AAL98295)";
const RTU: &str =
    "(RTUKZMP/CZWG000KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-13242934462034N0720521WN043327629F349)";

fn header(
    kind: Kind,
    sender: Facility,
    receiver: Facility,
    number: u16,
    reference: Option<MsgId>,
) -> Header {
    Header {
        kind,
        id: MsgId {
            sender,
            receiver,
            number,
        },
        reference,
    }
}
fn framed(template: &str, h: Header, f31: Option<Field31>) -> Vec<u8> {
    let mut message = parse(template.as_bytes()).expect("source fixture parses");
    message.header = h;
    if f31.is_some() {
        message.f31 = f31;
    }
    frame(&encode(&message).expect("synthetic message encodes")).expect("synthetic message frames")
}

fn driver() -> (
    oa_regional_session::flight::FlightDriver,
    oa_flight::FlightKey,
) {
    let key = key_tfms(*b"DLH499  ", *b"KMSP        ", *b"CYOW        ");
    let mut table = flight_driver_new(FlightDriverConfig {
        flight_capacity: 8,
        refusal_log_capacity: 16,
        conflict_log_capacity: 16,
    })
    .expect("bounded driver");
    for (time, sequence, operation) in [
        (
            1_000,
            1,
            FlightOperation::SetPlanByKey {
                key,
                plan_id: *b"PLAN00000001",
                status: 2,
            },
        ),
        (
            2_000,
            2,
            FlightOperation::SetOwnerByKey {
                key,
                unit: *b"KZMP4200",
            },
        ),
    ] {
        let event = NormalizedEvent {
            receipt_ns: time * 1_000_000,
            source_ns: time * 1_000_000,
            sequence,
            source: SourceKind::Flight,
            source_message_id: None,
            identity: Identity {
                flight_gufi: 0,
                aircraft_address: 0,
            },
            centre: *b"KZMP",
            kind: EventKind::Flight(operation),
        };
        let outcome = flight_apply_event(&mut table, &event).expect("ordered full-key seed");
        assert!(outcome.operation.is_some());
    }
    (table, key)
}

fn rti(state_number: u16) -> Vec<u8> {
    framed(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            CZWG,
            state_number,
            Some(MsgId {
                sender: KZMP,
                receiver: CZWG,
                number: 801,
            }),
        ),
        Some(Field31 {
            facility: CZWG,
            sector: Some(*b"1A"),
            text: None,
        }),
    )
}

fn framed_header(h: Header) -> Vec<u8> {
    frame(&encode(&bare(h)).expect("synthetic header message encodes")).expect("header frames")
}

fn accepted_offer() -> (
    oa_regional_session::flight::FlightDriver,
    oa_flight::FlightKey,
    oa_regional_session::nam_handoff::HandoffState,
) {
    let (mut table, key) = driver();
    let target = RtiTarget {
        key,
        expected_destination_sector: Some(*b"1A"),
        origin_unit: *b"KZMP4200",
        destination_unit: *b"CZWG1A00",
    };
    let (_origin, rti_number) = pending_rti_ingress(KZMP, CZWG);
    let mut ingress = active_ingress(CZWG, KZMP);
    let accepted = admit(&mut ingress, &rti(rti_number), 3);
    let (state, offer) = receive_rti(&accepted, target, 3_000_000_000, 3_000_000_000, 3)
        .expect("session-admitted RTI maps");
    let applied = flight_apply_event(&mut table, &offer).expect("RTI receipt applies");
    assert!(applied.operation.is_some());
    assert_eq!(flight_driver_report(&table).applied, 3);
    (table, key, state)
}

fn response(
    template: &str,
    kind: Kind,
    sender: Facility,
    receiver: Facility,
    number: u16,
    reference: MsgId,
    f31: Option<Field31>,
) -> Vec<u8> {
    framed(
        template,
        header(kind, sender, receiver, number, Some(reference)),
        f31,
    )
}

fn active_ingress(local: Facility, peer: Facility) -> NamSessionIngress {
    let config = session::default_config(local, peer);
    let mut ingress = nam_session_ingress_new(config);
    let actions = nam_session_ingress_local_event(&mut ingress, session::Event::Activate, 0)
        .expect("local activation event");
    let [session::Action::SendIrq { number }] = actions.as_slice() else {
        panic!("activation sends IRQ");
    };
    let irs = framed_header(header(
        Kind::Irs,
        peer,
        local,
        0,
        Some(MsgId {
            sender: local,
            receiver: peer,
            number: *number,
        }),
    ));
    let received = nam_session_ingress_receive(&mut ingress, &irs, 1)
        .expect("encoded IRS reaches NAM session");
    assert!(received.accepted.is_none());
    assert!(received.actions.as_slice().is_empty());
    assert_eq!(
        nam_session_ingress_state(&ingress).state,
        session::State::Active
    );
    ingress
}

fn pending_rti_ingress(local: Facility, peer: Facility) -> (NamSessionIngress, u16) {
    let mut ingress = active_ingress(local, peer);
    let actions =
        nam_session_ingress_local_event(&mut ingress, session::Event::SendAtm(Kind::Rti), 2)
            .expect("local RTI event");
    let [session::Action::SendAtm {
        kind: Kind::Rti,
        number,
    }] = actions.as_slice()
    else {
        panic!("RTI is numbered by session");
    };
    (ingress, *number)
}

fn admit(ingress: &mut NamSessionIngress, bytes: &[u8], now_ms: u64) -> SessionAcceptedMessage {
    nam_session_ingress_receive(ingress, bytes, now_ms)
        .expect("framed input reaches session")
        .accepted
        .expect("NAM session admitted an application message")
}

// REQ: OA-A7-006, OA-A7-008
#[test]
fn encoded_nam_rla_and_rta_apply_to_the_exact_full_flight_key() {
    let (mut table, key, mut state) = accepted_offer();
    let rla = response(
        RLA,
        Kind::Rla,
        CZWG,
        KZMP,
        20,
        state.rti_id(),
        Some(Field31 {
            facility: CZWG,
            sector: Some(*b"1A"),
            text: None,
        }),
    );
    let (mut ingress, generated_number) = pending_rti_ingress(KZMP, CZWG);
    assert_eq!(generated_number, state.rti_id().number);
    let accepted_rla = admit(&mut ingress, &rla, 4);
    let routed = receive_response(
        &accepted_rla,
        &mut state,
        Some(RlaRoute {
            facility: *b"CZWG",
            sector: Some(*b"1A"),
            unit: *b"CZWG1A00",
        }),
        4_000_000_000,
        4_000_000_000,
        4,
    )
    .expect("encoded RLA")
    .expect("RLA maps to update");
    assert!(
        matches!(routed.kind, EventKind::Flight(FlightOperation::HandoffByKey { key: k, operation: HandoffOperation::Update, .. }) if k == key)
    );
    flight_apply_event(&mut table, &routed).expect("RLA applies");
    let rta = response(RTA, Kind::Rta, CZWG, KZMP, 21, state.rti_id(), None);
    let accepted_rta = admit(&mut ingress, &rta, 5);
    let accepted = receive_response(
        &accepted_rta,
        &mut state,
        None,
        5_000_000_000,
        5_000_000_000,
        5,
    )
    .expect("encoded target RTA")
    .expect("RTA maps to acceptance");
    assert!(matches!(
        accepted.kind,
        EventKind::Flight(FlightOperation::HandoffByKey {
            operation: HandoffOperation::Accept,
            ..
        })
    ));
    flight_apply_event(&mut table, &accepted).expect("RTA applies");
    assert_eq!(flight_driver_report(&table).applied, 4);
}

// REQ: OA-A7-007, OA-A7-008
#[test]
fn encoded_lrm_fails_and_initiator_rta_retracts_as_distinct_flight_operations() {
    let (mut failed, _, mut fail_state) = accepted_offer();
    let lrm = response(LRM, Kind::Lrm, CZWG, KZMP, 30, fail_state.rti_id(), None);
    let (mut ingress, generated_number) = pending_rti_ingress(KZMP, CZWG);
    assert_eq!(generated_number, fail_state.rti_id().number);
    let accepted_lrm = admit(&mut ingress, &lrm, 4);
    let failure = receive_response(
        &accepted_lrm,
        &mut fail_state,
        None,
        4_000_000_000,
        4_000_000_000,
        4,
    )
    .expect("encoded LRM")
    .expect("LRM maps to failure");
    assert!(matches!(
        failure.kind,
        EventKind::Flight(FlightOperation::HandoffByKey {
            operation: HandoffOperation::Fail,
            ..
        })
    ));
    flight_apply_event(&mut failed, &failure).expect("LRM applies");
    assert_eq!(flight_driver_report(&failed).applied, 4);

    let (mut retracted, _, mut retract_state) = accepted_offer();
    let rta = response(
        RTA,
        Kind::Rta,
        KZMP,
        CZWG,
        31,
        retract_state.rti_id(),
        Some(Field31 {
            facility: KZMP,
            sector: Some(*b"42"),
            text: None,
        }),
    );
    let mut ingress = active_ingress(CZWG, KZMP);
    let accepted_rta = admit(&mut ingress, &rta, 4);
    let retract = receive_response(
        &accepted_rta,
        &mut retract_state,
        None,
        4_000_000_000,
        4_000_000_000,
        4,
    )
    .expect("encoded initiating-center RTA")
    .expect("RTA maps to retract");
    assert!(matches!(
        retract.kind,
        EventKind::Flight(FlightOperation::HandoffByKey {
            operation: HandoffOperation::Retract,
            ..
        })
    ));
    flight_apply_event(&mut retracted, &retract).expect("RTA retract applies");
}

// REQ: OA-A7-008
#[test]
fn encoded_session_sequence_and_response_correlation_gate_flight_mapping() {
    let (_, _, state) = accepted_offer();
    let target = RtiTarget {
        key: key_tfms(*b"DLH499  ", *b"KMSP        ", *b"CYOW        "),
        expected_destination_sector: Some(*b"1A"),
        origin_unit: *b"KZMP4200",
        destination_unit: *b"CZWG1A00",
    };
    let (_origin, first_number) = pending_rti_ingress(KZMP, CZWG);
    let mut inbound = active_ingress(CZWG, KZMP);
    let first = admit(&mut inbound, &rti(first_number), 4);
    assert_eq!(accepted_delivery_seq(&first), Some(session::Seq::First));
    assert_eq!(accepted_message(&first).header.kind, Kind::Rti);
    let (_state, event) = receive_rti(&first, target, 4_000_000_000, 4_000_000_000, 4)
        .expect("first session delivery maps");
    assert!(matches!(
        event.kind,
        EventKind::Flight(FlightOperation::HandoffByKey {
            operation: HandoffOperation::Offer,
            ..
        })
    ));
    let duplicate = admit(&mut inbound, &rti(first_number), 5);
    assert_eq!(
        receive_rti(&duplicate, target, 5_000_000_000, 5_000_000_000, 5),
        Err(NamHandoffError::SessionNotAdmitted)
    );
    let backwards_bytes = rti(first_number.wrapping_sub(1) % 1000);
    let backwards_result = nam_session_ingress_receive(&mut inbound, &backwards_bytes, 6)
        .expect("NAM session returns the sequence-gap delivery");
    assert_eq!(backwards_result.sequence_gap_count, 1);
    assert_eq!(nam_session_ingress_sequence_gap_count(&inbound), 1);
    assert!(backwards_result
        .actions
        .as_slice()
        .iter()
        .any(|action| matches!(
            action,
            session::Action::Deliver {
                seq: session::Seq::Gap(998),
                ..
            }
        )));
    let backwards = backwards_result
        .accepted
        .expect("gap delivery remains visible to application");
    assert!(receive_rti(&backwards, target, 6_000_000_000, 6_000_000_000, 6).is_ok());

    let (mut response_ingress, generated_number) = pending_rti_ingress(KZMP, CZWG);
    assert_eq!(generated_number, state.rti_id().number);
    let wrong_reference = MsgId {
        sender: KZMP,
        receiver: CZWG,
        number: state.rti_id().number.wrapping_add(1) % 1000,
    };
    let wrong = response(RLA, Kind::Rla, CZWG, KZMP, 20, wrong_reference, None);
    let refused = nam_session_ingress_receive(&mut response_ingress, &wrong, 7)
        .expect("bad reference is a parsed NAM event");
    assert!(refused.accepted.is_none());
    assert!(refused
        .actions
        .as_slice()
        .iter()
        .any(|action| matches!(action, session::Action::Discard { .. })));
    let wrong_peer = response(
        RLA,
        Kind::Rla,
        Facility(*b"KZBW"),
        KZMP,
        22,
        state.rti_id(),
        None,
    );
    let wrong_peer_result = nam_session_ingress_receive(&mut response_ingress, &wrong_peer, 8)
        .expect("wrong peer is rejected by session");
    assert!(wrong_peer_result.accepted.is_none());
    assert!(wrong_peer_result
        .actions
        .as_slice()
        .iter()
        .any(|action| matches!(action, session::Action::SendLrm { code: 1, .. })));
    let unmatched_lrm = response(LRM, Kind::Lrm, CZWG, KZMP, 21, wrong_reference, None);
    let unmatched = nam_session_ingress_receive(&mut response_ingress, &unmatched_lrm, 9)
        .expect("unmatched LRM is classified by session");
    assert!(unmatched.accepted.is_none());
    assert!(unmatched
        .actions
        .as_slice()
        .iter()
        .any(|action| matches!(action, session::Action::UnmatchedLrm { .. })));
}

// REQ: OA-A7-007
#[test]
fn encoded_rtu_is_not_forged_into_an_ownership_change() {
    let (table, _, mut state) = accepted_offer();
    let mut target_ingress = active_ingress(CZWG, KZMP);
    let rla = response(
        RLA,
        Kind::Rla,
        CZWG,
        KZMP,
        20,
        state.rti_id(),
        Some(Field31 {
            facility: CZWG,
            sector: Some(*b"1A"),
            text: None,
        }),
    );
    let (mut origin_ingress, generated_number) = pending_rti_ingress(KZMP, CZWG);
    assert_eq!(generated_number, state.rti_id().number);
    let accepted_rla = admit(&mut origin_ingress, &rla, 4);
    assert_eq!(accepted_delivery_seq(&accepted_rla), None);
    receive_response(
        &accepted_rla,
        &mut state,
        Some(RlaRoute {
            facility: *b"CZWG",
            sector: Some(*b"1A"),
            unit: *b"CZWG1A00",
        }),
        4_000_000_000,
        4_000_000_000,
        4,
    )
    .expect("RLA routes transfer");
    let rtu = response(RTU, Kind::Rtu, KZMP, CZWG, 22, state.rti_id(), None);
    let accepted_rtu = admit(&mut target_ingress, &rtu, 5);
    assert_eq!(
        receive_response(
            &accepted_rtu,
            &mut state,
            None,
            5_000_000_000,
            5_000_000_000,
            5
        ),
        Ok(None)
    );
    assert_eq!(flight_driver_report(&table).applied, 3);
}

// REQ: OA-A7-006
#[test]
fn changed_target_rti_does_not_silently_recoordinate_an_existing_offer() {
    let (mut table, key, _) = accepted_offer();
    let next_target = RtiTarget {
        key,
        expected_destination_sector: Some(*b"2B"),
        origin_unit: *b"KZMP4200",
        destination_unit: *b"KZNY2B00",
    };
    let bytes = framed(
        RTI,
        header(
            Kind::Rti,
            KZMP,
            Facility(*b"KZNY"),
            813,
            Some(MsgId {
                sender: KZMP,
                receiver: CZWG,
                number: 802,
            }),
        ),
        Some(Field31 {
            facility: Facility(*b"KZNY"),
            sector: Some(*b"2B"),
            text: None,
        }),
    );
    let mut ingress = active_ingress(Facility(*b"KZNY"), KZMP);
    let accepted = admit(&mut ingress, &bytes, 4);
    let (_, recoordination) = receive_rti(&accepted, next_target, 4_000_000_000, 4_000_000_000, 4)
        .expect("new RTI remains an explicit framed message");
    flight_apply_event(&mut table, &recoordination).expect("full-key event is staged");
    let report = flight_driver_report(&table);
    assert_eq!(report.applied, 3);
    assert!(report
        .refused_by_operation_reason
        .iter()
        .any(|count| *count > 0));
}
