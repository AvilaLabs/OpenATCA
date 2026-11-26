//! Synthetic candidate-state projection checks; no recorded or private inputs are read.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use oa_flight::{
    Flight, Stamp, View, ALT_SIMPLE, FLIGHT_FREE, F_ASSIGNED, F_IDENTITY, F_OWNER, ST_ACTIVE,
};
use oa_flight_sfdps::candidate_projection::{
    candidate_observation, project_candidate_flight, CandidateProjectionError,
    MAX_CANDIDATE_FLIGHT_RECORDS,
};
use oa_flight_sfdps::migration_adapter::empty_projected_observation;
use oa_migration_shadow::{EventState, Family, FieldId, FieldState, KeyState, SourceIdentity};

// REQ: OA-MIG-405
#[test]
fn actual_flight_state_is_exported_as_unkeyed_unknown_snapshots_with_real_stamps() {
    let mut flight: Flight = FLIGHT_FREE;
    flight.owner = *b"ZXXA0102";
    flight.stamps[F_OWNER] = Stamp {
        ts_ms: 1_893_553_445_678,
        avail_ms: 1_893_553_446_000,
        msg: 71,
    };
    flight.assigned.form = ALT_SIMPLE;
    flight.assigned.a_ft = 36_000;
    flight.stamps[F_ASSIGNED] = Stamp {
        ts_ms: 1_893_553_445_000,
        avail_ms: 1_893_553_446_000,
        msg: 72,
    };
    flight.n_views = 1;
    flight.views[0] = View {
        centre: *b"ZXXA",
        state: ST_ACTIVE,
        plan_id: *b"PLAN12345678",
        stamp: Stamp {
            ts_ms: 1_893_553_444_000,
            avail_ms: 1_893_553_446_000,
            msg: 70,
        },
    };
    flight.callsign = *b"ABC123  ";
    flight.dep = *b"KAAA\0\0\0\0\0\0\0\0";
    flight.stamps[F_IDENTITY] = Stamp {
        ts_ms: 1_893_553_443_000,
        avail_ms: 1_893_553_446_000,
        msg: 69,
    };

    let source = SourceIdentity {
        stream: 4,
        decoded_offset: 1_024,
        row: 8,
        operation_index: 0,
    };
    let mut output = [empty_projected_observation(); MAX_CANDIDATE_FLIGHT_RECORDS];
    let summary = project_candidate_flight(&flight, source, &mut output).unwrap();

    assert_eq!(summary.records_written, 6);
    assert_eq!(summary.route_not_modeled, 1);
    assert_eq!(summary.unstamped_fields, 3);
    let sector = candidate_observation(&output[0]).unwrap();
    assert_eq!(sector.key_state, KeyState::Missing);
    assert_eq!(sector.key.family, Family::SectorAssignment);
    assert_eq!(sector.event_state, EventState::Unknown);
    assert_eq!(sector.source, source);
    assert_eq!(sector.source_time_ns, Some(1_893_553_445_678_000_000));
    assert_eq!(sector.observed_time_ns, Some(1_893_553_446_000_000_000));
    assert_eq!(sector.fields[0].id, FieldId::SectorOwnerUnit as u16);
    assert_eq!(sector.fields[0].state, FieldState::Present);
    assert_eq!(sector.fields[1].id, FieldId::SectorIdentifier as u16);
    assert_eq!(sector.fields[1].state, FieldState::Present);

    let status = candidate_observation(&output[1]).unwrap();
    assert_eq!(status.key.family, Family::FlightAmendment);
    assert_eq!(status.key_state, KeyState::Missing);
    assert_eq!(status.event_state, EventState::Unknown);
    assert_eq!(status.fields[0].state, FieldState::Unsupported);
    assert_eq!(status.fields[1].integer, 1);

    let assigned = candidate_observation(&output[2]).unwrap();
    assert_eq!(assigned.fields[1].integer, 6);
    assert_eq!(assigned.fields[2].state, FieldState::Present);
    assert_eq!(assigned.fields[2].bytes[0], oa_fixm::ALT_FORM_SIMPLE);
    assert_eq!(&assigned.fields[2].bytes[1..5], &36_000_i32.to_le_bytes());

    for item in output.iter().take(usize::from(summary.records_written)) {
        let projected = candidate_observation(item).unwrap();
        assert_eq!(projected.key_state, KeyState::Missing);
        assert_eq!(projected.event_state, EventState::Unknown);
    }
}

// REQ: OA-MIG-405
#[test]
fn capacity_invalid_view_and_clock_refusal_preflight_without_partial_output() {
    let source = SourceIdentity {
        stream: 4,
        decoded_offset: 1_024,
        row: 8,
        operation_index: 0,
    };
    let flight: Flight = FLIGHT_FREE;
    let mut short = [empty_projected_observation(); MAX_CANDIDATE_FLIGHT_RECORDS - 1];
    assert_eq!(
        project_candidate_flight(&flight, source, &mut short),
        Err(CandidateProjectionError::OutputCapacity)
    );
    assert!(short
        .iter()
        .all(|record| record.key_state == KeyState::Missing && record.field_count == 0));

    let mut invalid_views = flight;
    invalid_views.n_views = u8::MAX;
    let mut output = [empty_projected_observation(); MAX_CANDIDATE_FLIGHT_RECORDS];
    assert_eq!(
        project_candidate_flight(&invalid_views, source, &mut output),
        Err(CandidateProjectionError::ViewCountInvalid)
    );
    assert!(output.iter().all(|record| record.field_count == 0));

    let mut invalid_clock = flight;
    invalid_clock.stamps[F_OWNER] = Stamp {
        ts_ms: 2_000,
        avail_ms: 1_000,
        msg: 1,
    };
    assert_eq!(
        project_candidate_flight(&invalid_clock, source, &mut output),
        Err(CandidateProjectionError::ClockInvalid)
    );
    assert!(output.iter().all(|record| record.field_count == 0));
}

// REQ: OA-MIG-405
#[test]
fn unstamped_fields_are_counted_and_not_materialized_as_plausible_zeroes() {
    let flight: Flight = FLIGHT_FREE;
    let source = SourceIdentity {
        stream: 4,
        decoded_offset: 1_024,
        row: 8,
        operation_index: 0,
    };
    let mut output = [empty_projected_observation(); MAX_CANDIDATE_FLIGHT_RECORDS];
    let summary = project_candidate_flight(&flight, source, &mut output).unwrap();
    assert_eq!(summary.records_written, 0);
    assert_eq!(summary.unstamped_fields, 6);
    assert_eq!(summary.route_not_modeled, 1);
    assert!(output.iter().all(|record| record.field_count == 0));
}
