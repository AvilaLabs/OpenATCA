//! Synthetic parser-boundary fixtures for M4 source-fact projection; no recordings are read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_fixm::{clear_events, new_events, Decoder, Events, Limits};
use oa_flight_sfdps::audit::RowIdentity;
use oa_flight_sfdps::migration_projection::{row_facts, SourceFieldState};
use oa_xml::{Limits as XmlLimits, Parser};

const HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ns5:MessageCollection xmlns:ns5="http://www.faa.aero/nas/3.0" xmlns:ns2="http://www.fixm.aero/base/3.0" xmlns:ns3="http://www.fixm.aero/flight/3.0" xmlns:ns4="http://www.fixm.aero/foundation/3.0">"#;
const GUFI: &str = "11111111-2222-3333-4444-555555555555";

fn message(body: &str) -> String {
    format!(
        r#"<message xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="ns5:FlightMessageType"><flight xsi:type="ns5:NasFlightType" centre="ZXX" source="OH" system="SLC" timestamp="2030-01-02T03:04:05.678Z"><gufi codeSpace="urn:uuid">{GUFI}</gufi>{body}</flight></message>"#
    )
}

fn decode(body: &str) -> Events {
    let xml = format!("{HEAD}{}</ns5:MessageCollection>", message(body));
    let mut parser = Parser::new(XmlLimits::default()).unwrap();
    let document = parser.parse(xml.as_bytes()).unwrap();
    let mut decoder = Decoder::new(Limits::new(
        oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
    ));
    let mut events = new_events(4, 4096);
    clear_events(&mut events);
    decoder
        .decode(&document, xml.as_bytes(), &mut events)
        .unwrap();
    events
}

fn identity(row: usize) -> RowIdentity {
    RowIdentity {
        stream: 2,
        decoded_offset: 900,
        recorder_sequence: 71,
        row,
    }
}

// REQ: OA-MIG-403
#[test]
fn handoff_facts_keep_parser_state_and_source_receive_clocks_separate() {
    let events = decode(
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX" sectorIdentifier="12"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="03"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();

    assert_eq!(facts.identity, identity(0));
    assert_eq!(facts.source_time_ms, 1_893_553_445_678);
    assert_eq!(facts.observed_time_ms, 1_893_553_446_000);
    assert_eq!(facts.handoff_event.state, SourceFieldState::Present);
    assert_eq!(facts.handoff_event.value, Some(oa_fixm::HANDOFF_INITIATION));
    assert_eq!(facts.handoff_transferring.state, SourceFieldState::Present);
    assert_eq!(facts.handoff_receiving.state, SourceFieldState::Present);
    assert_eq!(facts.handoff_accepting.state, SourceFieldState::Absent);
    assert_eq!(facts.controlling_unit.state, SourceFieldState::Absent);
}

// REQ: OA-MIG-403
#[test]
fn malformed_role_is_not_reported_as_absent_or_valid() {
    let events = decode(
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX" sectorIdentifier="0 8"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="03"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_202_446_000).unwrap();

    assert_eq!(
        facts.handoff_transferring.state,
        SourceFieldState::Malformed
    );
    assert_eq!(facts.handoff_transferring.value, None);
    assert_eq!(facts.handoff_receiving.state, SourceFieldState::Present);
}

// REQ: OA-MIG-403
#[test]
fn unknown_controller_token_is_preserved_as_a_present_source_value() {
    let events = decode(r#"<controllingUnit unitIdentifier="???" sectorIdentifier="12"/>"#);
    let facts = row_facts(&events, 0, identity(0), 1_893_202_446_000).unwrap();

    assert_eq!(facts.controlling_unit.state, SourceFieldState::Present);
    assert_eq!(facts.controlling_unit.value, Some(*b"???\x0012\0\0"));
}

// REQ: OA-MIG-403
#[test]
fn flight_data_values_and_explicit_interim_nil_are_typed_without_delta_inference() {
    let events = decode(
        r#"<flightStatus fdpsFlightStatus="ACTIVE"/><flightIdentification aircraftIdentification="ABC123"/><assignedAltitude><simple uom="FEET">36000.0</simple></assignedAltitude><interimAltitude xsi:nil="true"/><requestedAltitude><vfr/></requestedAltitude><agreed><route nasRouteText="KAAA.TESTA1.KBBB"/></agreed>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_202_446_000).unwrap();

    assert_eq!(facts.flight_status.state, SourceFieldState::Present);
    assert_eq!(facts.callsign.state, SourceFieldState::Present);
    assert_eq!(facts.assigned_altitude.state, SourceFieldState::Present);
    assert_eq!(
        facts.assigned_altitude.value.map(|alt| alt.form),
        Some(oa_fixm::ALT_FORM_SIMPLE)
    );
    assert_eq!(
        facts.assigned_altitude.value.and_then(|alt| alt.value_ft),
        Some(36_000)
    );
    assert_eq!(facts.interim_altitude.state, SourceFieldState::Present);
    assert_eq!(
        facts.interim_altitude.value.map(|alt| alt.is_nil),
        Some(true)
    );
    assert_eq!(
        facts.interim_altitude.value.and_then(|alt| alt.value_ft),
        None
    );
    assert_eq!(
        facts.requested_altitude.value,
        Some((oa_fixm::ALT_FORM_VFR, None))
    );
    assert_eq!(facts.route.value, Some("KAAA.TESTA1.KBBB"));
}

// REQ: OA-MIG-403
#[test]
fn every_projected_field_has_one_explicit_state_per_source_row() {
    let events = decode(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
    let facts = row_facts(&events, 0, identity(0), 1_893_202_446_000).unwrap();
    let projected_states = [
        facts.message_sequence.state,
        facts.plan_sequence.state,
        facts.handoff_event.state,
        facts.handoff_transferring.state,
        facts.handoff_receiving.state,
        facts.handoff_accepting.state,
        facts.controlling_unit.state,
        facts.flight_status.state,
        facts.flight_plan_id.state,
        facts.callsign.state,
        facts.departure.state,
        facts.arrival.state,
        facts.assigned_altitude.state,
        facts.interim_altitude.state,
        facts.requested_altitude.state,
        facts.route.state,
    ];
    let absent = projected_states
        .iter()
        .filter(|state| **state == SourceFieldState::Absent)
        .count();
    let present = projected_states
        .iter()
        .filter(|state| **state == SourceFieldState::Present)
        .count();
    let malformed = projected_states
        .iter()
        .filter(|state| **state == SourceFieldState::Malformed)
        .count();
    assert_eq!(absent + present + malformed, projected_states.len());
    assert_eq!(projected_states.len(), 16);
    assert_eq!(present, 1);
    assert_eq!(absent, 15);
    assert_eq!(malformed, 0);
    assert_eq!(facts.flight_status.state, SourceFieldState::Present);
}

// REQ: OA-MIG-403
#[test]
fn missing_required_parser_column_refuses_projection_instead_of_becoming_absent() {
    let mut events = decode(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
    events.gufi.clear();

    let error = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("GUFI column missing"));
}

// REQ: OA-MIG-403
#[test]
fn missing_presence_or_error_bitmap_refuses_instead_of_fabricating_absence() {
    for missing_present in [true, false] {
        let mut events = decode(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
        if missing_present {
            events.present.clear();
        } else {
            events.errors.clear();
        }
        let error = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap_err();
        let expected = if missing_present {
            "present bitmap row missing"
        } else {
            "error bitmap row missing"
        };
        assert!(error.to_string().contains(expected));
    }
}

// REQ: OA-MIG-403
#[test]
fn route_reference_range_and_utf8_are_validated_before_present_value() {
    let body = r#"<agreed><route nasRouteText="KAAA.TEST.KBBB"/></agreed>"#;

    let mut missing_offset = decode(body);
    missing_offset.route_off.clear();
    let error = row_facts(&missing_offset, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("route offset missing"));

    let mut missing_length = decode(body);
    missing_length.route_len.clear();
    let error = row_facts(&missing_length, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("route length missing"));

    let mut out_of_bounds = decode(body);
    out_of_bounds.route_off[0] = u32::MAX;
    let error = row_facts(&out_of_bounds, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("route range invalid"));

    let mut empty_range = decode(body);
    empty_range.route_len[0] = 0;
    let error = row_facts(&empty_range, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("present route has empty range"));

    let mut invalid_utf8 = decode(body);
    let offset = invalid_utf8.route_off[0] as usize;
    invalid_utf8.strings[offset] = 0xff;
    let error = row_facts(&invalid_utf8, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("route is not UTF-8"));
}

// REQ: OA-MIG-403
#[test]
fn source_absence_and_decoder_malformed_route_remain_distinct() {
    let absent = decode(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
    assert_eq!(
        row_facts(&absent, 0, identity(0), 1_893_553_446_000)
            .unwrap()
            .route
            .state,
        SourceFieldState::Absent
    );

    let malformed = decode(r#"<agreed><route nasRouteText=""/></agreed>"#);
    assert_eq!(
        row_facts(&malformed, 0, identity(0), 1_893_553_446_000)
            .unwrap()
            .route
            .state,
        SourceFieldState::Malformed
    );
}

// REQ: OA-MIG-403
#[test]
fn present_interim_altitude_requires_its_nil_flags_row() {
    let mut events = decode(r#"<interimAltitude xsi:nil="true"/>"#);
    events.flags.clear();
    let error = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap_err();
    assert!(error.to_string().contains("flags row missing"));
}
