//! Synthetic M4 shell-adapter checks; no recordings or private data are read.
#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use oa_migration_shadow::{
    BusinessKey, EventState, Family, FieldId, FieldState, KeyState, SourceIdentity,
};
use oa_regional_session::migration_shadow::{
    alert_observation, project_conflict_alert, project_msaw_alert, AlertActivity,
    AlertProjectionRecord, ProjectionError, OA_ALERT_KIND_CONFLICT, OA_ALERT_KIND_MSAW,
    SFDPS_ALERT_COVERAGE_WITHOUT_OTHER_FEED,
};

fn source() -> SourceIdentity {
    SourceIdentity {
        stream: 7,
        decoded_offset: 64,
        row: 3,
        operation_index: 0,
    }
}

fn alert_key() -> BusinessKey {
    BusinessKey {
        entity_key: [0x51; 32],
        family: Family::Alert,
        ordinal: 9,
    }
}

fn empty_record() -> AlertProjectionRecord {
    AlertProjectionRecord {
        source: source(),
        key_state: KeyState::Missing,
        key: alert_key(),
        source_time_ns: None,
        observed_time_ns: None,
        event_state: EventState::Unknown,
        fields: [oa_migration_shadow::FieldValue {
            id: 0,
            state: FieldState::Unknown,
            kind: oa_migration_shadow::ValueKind::None,
            integer: 0,
            bytes: [0; 32],
        }; 5],
    }
}

fn conflict_alert() -> oa_ca::Alert {
    oa_ca::Alert {
        addr_a: 0x0001_0203,
        addr_b: 0x0004_0506,
        start_ns: 10_000,
        end_ns: 12_000,
        peak_ns: 11_000,
        ttl_first_s: 45.0,
        ttl_peak_s: 12.0,
        h_now_peak_m: 1_000.0,
        h_miss_peak_m: 500.0,
        min_h_miss_m: 400.0,
        min_v_miss_ft: 800.0,
        h_eff_m: 5_000.0,
        v_eff_ft: 1_000.0,
        alt_ft: 20_000.0,
        lat_e7: 410_000_000,
        lon_e7: -870_000_000,
        ticks_conflict: 4,
        ticks_current: 3,
        ticks_unevaluated: 1,
        flags: 0,
        ground_either: 0,
    }
}

fn msaw_alert() -> oa_msaw::Alert {
    oa_msaw::Alert {
        addr: 0x0001_0203,
        start_ns: 20_000,
        end_ns: 22_000,
        caution_ns: Some(20_000),
        warning_ns: None,
        peak: oa_msaw::Grade::Caution,
        lat_e7: 410_000_000,
        lon_e7: -870_000_000,
        alt_ft: 2_000.0,
        vrate_fpm: 0.0,
        speed_mps: 120.0,
        worst_margin_ft: -100.0,
        worst_floor_ft: 2_100,
        worst_kind: oa_msaw::FloorKind::Terrain,
        worst_lat_e7: 410_000_001,
        worst_lon_e7: -870_000_001,
        worst_radius_m: 1_000.0,
        worst_k: 1,
        worst_ns: 21_000,
        worst_flags: 0,
        r95_m: 500.0,
        vel_east_mps: 20.0,
        vel_north_mps: 100.0,
        ttv_min_s: 10.0,
        track_deg: 11,
        worst_kinds: 1,
        cycles: 3,
        flags: 0,
        end_reason: oa_msaw::END_CLEAR,
        protect_ns: 0,
    }
}

// REQ: OA-MIG-406
#[test]
fn candidate_conflict_alert_preserves_source_and_missing_availability_without_claiming_ack() {
    let alert = conflict_alert();
    let mut output = [empty_record()];
    project_conflict_alert(
        &alert,
        AlertActivity::Open,
        source(),
        None,
        None,
        None,
        &mut output,
    )
    .unwrap();

    let observation = alert_observation(&output[0]);
    assert_eq!(observation.source, source());
    assert_eq!(observation.key_state, KeyState::Missing);
    assert_eq!(observation.key.family, Family::Alert);
    assert_eq!(observation.event_state, EventState::Observed);
    assert_eq!(observation.source_time_ns, Some(alert.start_ns));
    assert_eq!(observation.observed_time_ns, None);
    assert_eq!(observation.fields.len(), 5);
    assert_eq!(observation.fields[0].id, FieldId::AlertKind as u16);
    assert_eq!(observation.fields[0].integer, OA_ALERT_KIND_CONFLICT);
    assert_eq!(observation.fields[1].id, FieldId::AlertActive as u16);
    assert_eq!(observation.fields[1].integer, 1);
    assert_eq!(observation.fields[2].state, FieldState::Unsupported);
    assert_eq!(observation.fields[3].state, FieldState::Unsupported);
    assert_eq!(observation.fields[4].state, FieldState::Unsupported);
    assert_eq!(
        SFDPS_ALERT_COVERAGE_WITHOUT_OTHER_FEED,
        oa_migration_shadow::Coverage::Missing
    );
}

// REQ: OA-MIG-406
#[test]
fn msaw_closed_alert_uses_only_supplied_key_and_recorded_availability_clock() {
    let alert = msaw_alert();
    let key = alert_key();
    let mut output = [empty_record()];
    project_msaw_alert(
        &alert,
        AlertActivity::Closed,
        source(),
        Some(key),
        Some(alert.end_ns + 1),
        Some(25_000),
        &mut output,
    )
    .unwrap();

    let observation = alert_observation(&output[0]);
    assert_eq!(observation.key_state, KeyState::Valid);
    assert_eq!(observation.key, key);
    assert_eq!(observation.source_time_ns, Some(alert.end_ns + 1));
    assert_eq!(observation.observed_time_ns, Some(25_000));
    assert_eq!(observation.fields[0].integer, OA_ALERT_KIND_MSAW);
    assert_eq!(observation.fields[1].integer, 0);
    assert_eq!(observation.fields[2].state, FieldState::Unsupported);
    assert_eq!(observation.fields[3].state, FieldState::Unsupported);
    assert_eq!(observation.fields[4].state, FieldState::Unsupported);
}

// REQ: OA-MIG-406
#[test]
fn alert_projection_refuses_bad_key_or_clock_without_replacing_existing_record() {
    let alert = conflict_alert();
    let mut output = [empty_record()];
    project_conflict_alert(
        &alert,
        AlertActivity::Open,
        source(),
        Some(alert_key()),
        None,
        Some(20_000),
        &mut output,
    )
    .unwrap();
    let original = output[0];
    let wrong_family = BusinessKey {
        family: Family::Handoff,
        ..alert_key()
    };
    assert_eq!(
        project_conflict_alert(
            &alert,
            AlertActivity::Open,
            source(),
            Some(wrong_family),
            None,
            Some(20_000),
            &mut output,
        ),
        Err(ProjectionError::KeyFamilyMismatch)
    );
    assert_eq!(
        project_conflict_alert(
            &alert,
            AlertActivity::Open,
            source(),
            Some(alert_key()),
            None,
            Some(9_999),
            &mut output,
        ),
        Err(ProjectionError::ClockInvalid)
    );
    assert_eq!(output[0], original);
    assert_eq!(
        project_conflict_alert(
            &alert,
            AlertActivity::Open,
            source(),
            None,
            None,
            None,
            &mut [],
        ),
        Err(ProjectionError::OutputCapacity)
    );
}

// REQ: OA-MIG-406
#[test]
fn closed_conflict_without_actual_closure_time_is_unknown_not_onset_or_last_violation() {
    let alert = conflict_alert();
    let mut output = [empty_record()];
    project_conflict_alert(
        &alert,
        AlertActivity::Closed,
        source(),
        None,
        None,
        Some(alert.end_ns + 5),
        &mut output,
    )
    .unwrap();
    let observation = alert_observation(&output[0]);
    assert_eq!(observation.event_state, EventState::Unknown);
    assert_eq!(observation.source_time_ns, None);
    assert_eq!(observation.observed_time_ns, Some(alert.end_ns + 5));
    assert_eq!(observation.fields[1].integer, 0);
}

// REQ: OA-MIG-406
#[test]
fn closed_conflict_refuses_closure_before_last_violation_and_observation_before_closure() {
    let alert = conflict_alert();
    let mut output = [empty_record()];
    assert_eq!(
        project_conflict_alert(
            &alert,
            AlertActivity::Closed,
            source(),
            None,
            Some(alert.end_ns - 1),
            None,
            &mut output,
        ),
        Err(ProjectionError::ClockInvalid)
    );
    assert_eq!(
        project_conflict_alert(
            &alert,
            AlertActivity::Closed,
            source(),
            None,
            Some(alert.end_ns + 1),
            Some(alert.end_ns),
            &mut output,
        ),
        Err(ProjectionError::ClockInvalid)
    );
}
