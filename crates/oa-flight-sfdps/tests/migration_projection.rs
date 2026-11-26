//! Synthetic parser-boundary fixtures for M4 source-fact projection; no recordings are read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_fixm::{clear_events, new_events, Decoder, Events, Limits};
use oa_flight_sfdps::audit::RowIdentity;
use oa_flight_sfdps::migration_adapter::{
    empty_projected_observation, observation, project_row, ProjectionError, MAX_ROW_RECORDS,
};
use oa_flight_sfdps::migration_projection::{row_facts, SourceField, SourceFieldState};
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

// REQ: OA-MIG-404
#[test]
fn projection_maps_explicit_handoff_and_keeps_sector_snapshot_unknown() {
    use oa_migration_shadow::{
        BusinessKey, EventState, Family, FieldId, FieldState, KeyState, ValueKind,
    };

    let events = decode(
        r#"<controllingUnit unitIdentifier="ZXX" sectorIdentifier="12"/><enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX" sectorIdentifier="12"/><receivingUnit unitIdentifier="ZYY" sectorIdentifier="03"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();
    let key = BusinessKey {
        entity_key: [0x11; 32],
        family: Family::Handoff,
        ordinal: 44,
    };
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    let count = project_row(&facts, Some(key), &mut output).unwrap();
    assert_eq!(count, 2);

    let handoff = observation(&output[0]).unwrap();
    assert_eq!(handoff.key_state, KeyState::Valid);
    assert_eq!(handoff.key, key);
    assert_eq!(handoff.event_state, EventState::Observed);
    assert_eq!(handoff.fields.len(), 6);
    assert_eq!(handoff.fields[0].id, FieldId::HandoffKind as u16);
    assert_eq!(handoff.fields[0].integer, 1);
    assert_eq!(
        handoff.fields[1].id,
        FieldId::HandoffTransferringUnit as u16
    );
    assert_eq!(handoff.fields[1].kind, ValueKind::Bytes32);
    assert_eq!(handoff.fields[4].id, FieldId::HandoffPhase as u16);
    assert_eq!(
        handoff.fields[4].integer,
        i64::from(oa_fixm::HANDOFF_INITIATION)
    );
    assert_eq!(handoff.fields[5].state, FieldState::Present);

    let sector = observation(&output[1]).unwrap();
    assert_eq!(sector.key_state, KeyState::Missing);
    assert_eq!(sector.key.family, Family::SectorAssignment);
    assert_eq!(sector.event_state, EventState::Unknown);
    assert_eq!(sector.fields[0].id, FieldId::SectorOwnerUnit as u16);
    assert_eq!(sector.fields[0].state, FieldState::Present);
    assert_eq!(sector.fields[1].id, FieldId::SectorIdentifier as u16);
    assert_eq!(sector.fields[1].state, FieldState::Present);
}

// REQ: OA-MIG-404
#[test]
fn flight_facts_are_unknown_keyless_snapshots_not_amendment_deltas() {
    use oa_migration_shadow::{EventState, Family, FieldId, FieldState, KeyState, ValueKind};

    let events = decode(
        r#"<flightStatus fdpsFlightStatus="ACTIVE"/><assignedAltitude><simple uom="FEET">36000.0</simple></assignedAltitude><agreed><route nasRouteText="KAAA.TESTA1.KBBB"/></agreed>"#,
    );
    let facts = row_facts(&events, 0, identity(3), 1_893_553_446_000).unwrap();
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    let count = project_row(&facts, None, &mut output).unwrap();
    assert_eq!(count, 3);
    for record in output.iter().take(count) {
        let observation = observation(record).unwrap();
        assert_eq!(observation.key_state, KeyState::Missing);
        assert_eq!(observation.key.family, Family::FlightAmendment);
        assert_eq!(observation.event_state, EventState::Unknown);
        assert_eq!(observation.fields[0].id, FieldId::AmendmentKind as u16);
        assert_eq!(observation.fields[0].state, FieldState::Unsupported);
        assert_eq!(observation.fields[1].id, FieldId::AmendmentField as u16);
        assert_eq!(observation.fields[2].id, FieldId::AmendmentValue as u16);
    }
    assert_eq!(output[0].fields[1].integer, 1); // versioned source-fact slot: flight status
    assert_eq!(output[0].fields[2].kind, ValueKind::Integer);
    assert_eq!(output[1].fields[1].integer, 6); // assigned-altitude snapshot, not a delta
    assert_eq!(output[2].fields[1].integer, 9); // route snapshot, not a delta
    assert_eq!(output[2].fields[2].kind, ValueKind::Bytes32);
}

// REQ: OA-MIG-404
#[test]
fn projection_requires_full_fixed_buffer_and_family_correct_explicit_event_key() {
    use oa_migration_shadow::{BusinessKey, Family, KeyState};

    let events = decode(
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX"/><receivingUnit unitIdentifier="ZYY"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();
    let mut too_small = [empty_projected_observation(); MAX_ROW_RECORDS - 1];
    assert_eq!(
        project_row(&facts, None, &mut too_small),
        Err(ProjectionError::OutputCapacity)
    );
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    let wrong_family = BusinessKey {
        entity_key: [0x11; 32],
        family: Family::SectorAssignment,
        ordinal: 4,
    };
    assert_eq!(
        project_row(&facts, Some(wrong_family), &mut output),
        Err(ProjectionError::KeyFamilyMismatch)
    );

    let zero_entity = BusinessKey {
        entity_key: [0; 32],
        family: Family::Handoff,
        ordinal: 4,
    };
    let count = project_row(&facts, Some(zero_entity), &mut output).unwrap();
    assert_eq!(count, 1);
    assert_eq!(output[0].key_state, KeyState::Invalid);
}

// REQ: OA-MIG-404
#[test]
fn long_route_snapshot_is_explicitly_unsupported_without_truncation() {
    use oa_migration_shadow::FieldState;

    let events = decode(
        r#"<agreed><route nasRouteText="KAAA.ABCDEFGHIJKLMNOPQRSTUVWXYZ123456789.KBBB"/></agreed>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();
    assert_eq!(facts.route.state, SourceFieldState::Present);
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    let count = project_row(&facts, None, &mut output).unwrap();
    assert_eq!(count, 1);
    assert_eq!(output[0].fields[2].state, FieldState::Unsupported);
}

// REQ: OA-MIG-404
#[test]
fn malformed_handoff_event_is_invalid_and_never_becomes_an_observed_phase() {
    use oa_migration_shadow::{EventState, Family, FieldId, FieldState, KeyState};

    let events = decode(
        r#"<enRoute><boundaryCrossings><handoff event="UNKNOWN"><transferringUnit unitIdentifier="ZXX"/><receivingUnit unitIdentifier="ZYY"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();
    assert_eq!(facts.handoff_event.state, SourceFieldState::Malformed);
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    let count = project_row(&facts, None, &mut output).unwrap();
    assert_eq!(count, 1);
    let handoff = observation(&output[0]).unwrap();
    assert_eq!(handoff.key_state, KeyState::Missing);
    assert_eq!(handoff.key.family, Family::Handoff);
    assert_eq!(handoff.event_state, EventState::Invalid);
    assert_eq!(handoff.fields[4].id, FieldId::HandoffPhase as u16);
    assert_eq!(handoff.fields[4].state, FieldState::Invalid);
}

// REQ: OA-MIG-404
#[test]
fn adapter_rejects_unrepresentable_clock_without_partial_record() {
    let events = decode(r#"<flightStatus fdpsFlightStatus="ACTIVE"/>"#);
    let mut facts = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();
    facts.source_time_ms = i64::MAX;
    let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
    assert_eq!(
        project_row(&facts, None, &mut output),
        Err(ProjectionError::ClockOverflow)
    );
}

// REQ: OA-MIG-404
#[test]
fn direct_handoff_facts_with_missing_or_unrecognized_phase_are_invalid_events() {
    use oa_migration_shadow::{EventState, FieldId, FieldState};

    let events = decode(
        r#"<enRoute><boundaryCrossings><handoff event="INITIATION"><transferringUnit unitIdentifier="ZXX"/><receivingUnit unitIdentifier="ZYY"/></handoff></boundaryCrossings></enRoute>"#,
    );
    let base = row_facts(&events, 0, identity(0), 1_893_553_446_000).unwrap();

    for value in [None, Some(255)] {
        let mut facts = base;
        facts.handoff_event = SourceField {
            state: SourceFieldState::Present,
            value,
        };
        let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
        let count = project_row(&facts, None, &mut output).unwrap();
        assert_eq!(count, 1);
        let handoff = observation(&output[0]).unwrap();
        assert_eq!(handoff.event_state, EventState::Invalid);
        assert_eq!(handoff.fields[4].id, FieldId::HandoffPhase as u16);
        assert_eq!(handoff.fields[4].state, FieldState::Invalid);
    }

    for value in [
        oa_fixm::HANDOFF_INITIATION,
        oa_fixm::HANDOFF_ACCEPTANCE,
        oa_fixm::HANDOFF_UPDATE,
        oa_fixm::HANDOFF_RETRACTION,
        oa_fixm::HANDOFF_FAILURE,
        oa_fixm::HANDOFF_TAKE_CONTROL,
    ] {
        let mut facts = base;
        facts.handoff_event = SourceField {
            state: SourceFieldState::Present,
            value: Some(value),
        };
        let mut output = [empty_projected_observation(); MAX_ROW_RECORDS];
        let count = project_row(&facts, None, &mut output).unwrap();
        assert_eq!(count, 1);
        let handoff = observation(&output[0]).unwrap();
        assert_eq!(handoff.event_state, EventState::Observed);
        assert_eq!(handoff.fields[4].state, FieldState::Present);
        assert_eq!(handoff.fields[4].integer, i64::from(value));
    }
}
