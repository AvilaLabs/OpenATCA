//! Offline projection of actual regional-session flight and alert outputs into M4 records.
//!
//! This shell adapter does not change tracker, flight, CA or MSAW behavior and does not perform
//! I/O. It accepts side-local capture identity, stable business keys, and observed-time evidence
//! from the caller. It never derives event keys or ordinals from output order or timestamps.
//! Flight sector/plan/altitude/identity values remain state snapshots with `EventState::Unknown`
//! through `oa-flight-sfdps::candidate_projection`; they are not sector-change or amendment
//! events. Candidate CA/MSAW alerts are real alert episodes, but acknowledgement/suppression and
//! a cross-system reason taxonomy are not emitted by these models and remain `Unsupported`.
//! SFDPS source projection has no legacy alert stream: callers must mark that side's alert family
//! `Coverage::Missing`, not empty or equivalent. The output is evidence plumbing only and makes
//! no comparison, conformance, acceptance, or operational claim.
//! No point-out observation is emitted by the current source parser or candidate session model;
//! use `OPENATCA_POINT_OUT_COVERAGE = Coverage::Unsupported` rather than complete-empty.
//!
//! RULESET F1: Connor approved `tools/oa-regional-session -> oa-migration-shadow` on 2026-11-26.
//! This shell is the existing boundary that has the actual `oa-flight`, CA and MSAW values; no
//! real-time core gains a migration dependency.

use oa_flight::Flight;
use oa_flight_sfdps::candidate_projection::{
    project_candidate_flight, CandidateProjectionError, CandidateProjectionSummary,
    MAX_CANDIDATE_FLIGHT_RECORDS,
};
use oa_migration_shadow::{
    BusinessKey, EventState, Family, FieldId, FieldState, FieldValue, KeyState, Observation,
    SourceIdentity, ValueKind,
};

use crate::migration_shadow::ProjectionError::{ClockInvalid, KeyFamilyMismatch, OutputCapacity};
use oa_flight_sfdps::migration_adapter::{
    observation as projected_observation, ProjectedObservation,
    ProjectionError as FlightProjectionError,
};

pub const PROJECTION_VERSION: u16 = 1;
pub const OA_ALERT_KIND_CONFLICT: i64 = 1;
pub const OA_ALERT_KIND_MSAW: i64 = 2;
/// SFDPS source facts do not contain legacy conflict/MSAW alert observations.
pub const SFDPS_ALERT_COVERAGE_WITHOUT_OTHER_FEED: oa_migration_shadow::Coverage =
    oa_migration_shadow::Coverage::Missing;
pub const OPENATCA_POINT_OUT_COVERAGE: oa_migration_shadow::Coverage =
    oa_migration_shadow::Coverage::Unsupported;
pub const MAX_FLIGHT_SNAPSHOT_RECORDS: usize = MAX_CANDIDATE_FLIGHT_RECORDS;

const ALERT_FIELD_COUNT: usize = 5;
const EMPTY_FIELD: FieldValue = FieldValue {
    id: 0,
    state: FieldState::Unknown,
    kind: ValueKind::None,
    integer: 0,
    bytes: [0; 32],
};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertActivity {
    Closed = 0,
    Open = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionError {
    OutputCapacity = 1,
    KeyFamilyMismatch = 2,
    ClockInvalid = 3,
}

/// One owned candidate alert observation, with missing output-availability time represented as
/// `None` rather than a plausible numeric sentinel.
// A7.2: one-shot offline adapter record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertProjectionRecord {
    pub source: SourceIdentity,
    pub key_state: KeyState,
    pub key: BusinessKey,
    pub source_time_ns: Option<i64>,
    pub observed_time_ns: Option<i64>,
    pub event_state: EventState,
    pub fields: [FieldValue; ALERT_FIELD_COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AlertClockEvidence {
    onset_ns: i64,
    last_violation_ns: i64,
    closure_ns: Option<i64>,
    observed_ns: Option<i64>,
}

fn empty_field(id: FieldId, state: FieldState) -> FieldValue {
    FieldValue {
        id: id as u16,
        state,
        ..EMPTY_FIELD
    }
}

fn integer_field(id: FieldId, value: i64) -> FieldValue {
    FieldValue {
        id: id as u16,
        state: FieldState::Present,
        kind: ValueKind::Integer,
        integer: value,
        bytes: [0; 32],
    }
}

fn key_state(
    family: Family,
    key: Option<BusinessKey>,
) -> Result<(KeyState, BusinessKey), ProjectionError> {
    match key {
        Some(value) if value.family != family => Err(KeyFamilyMismatch),
        Some(value) if value.entity_key.iter().all(|byte| *byte == 0) => {
            Ok((KeyState::Invalid, value))
        }
        Some(value) => Ok((KeyState::Valid, value)),
        None => Ok((
            KeyState::Missing,
            BusinessKey {
                entity_key: [0; 32],
                family,
                ordinal: 0,
            },
        )),
    }
}

fn make_alert_record(
    kind: i64,
    activity: AlertActivity,
    source: SourceIdentity,
    key: Option<BusinessKey>,
    clocks: AlertClockEvidence,
) -> Result<AlertProjectionRecord, ProjectionError> {
    let (source_time_ns, event_state) = match activity {
        AlertActivity::Open => {
            if clocks.onset_ns < 0
                || clocks.last_violation_ns < clocks.onset_ns
                || clocks.closure_ns.is_some()
            {
                return Err(ClockInvalid);
            }
            (Some(clocks.onset_ns), EventState::Observed)
        }
        AlertActivity::Closed => match clocks.closure_ns {
            Some(closure_ns)
                if closure_ns >= clocks.last_violation_ns
                    && clocks.last_violation_ns >= clocks.onset_ns
                    && clocks.onset_ns >= 0 =>
            {
                (Some(closure_ns), EventState::Observed)
            }
            Some(_) => return Err(ClockInvalid),
            None if clocks.onset_ns >= 0 && clocks.last_violation_ns >= clocks.onset_ns => {
                (None, EventState::Unknown)
            }
            None => return Err(ClockInvalid),
        },
    };
    if clocks.observed_ns.is_some_and(|time| time < 0)
        || source_time_ns.is_some_and(|source_time| {
            clocks
                .observed_ns
                .is_some_and(|observed_time| observed_time < source_time)
        })
    {
        return Err(ClockInvalid);
    }
    let (key_state, key) = key_state(Family::Alert, key)?;
    let fields = [
        integer_field(FieldId::AlertKind, kind),
        integer_field(FieldId::AlertActive, i64::from(activity as u8)),
        empty_field(FieldId::AlertAcknowledged, FieldState::Unsupported),
        empty_field(FieldId::AlertSuppressed, FieldState::Unsupported),
        empty_field(FieldId::AlertReason, FieldState::Unsupported),
    ];
    Ok(AlertProjectionRecord {
        source,
        key_state,
        key,
        source_time_ns,
        observed_time_ns: clocks.observed_ns,
        event_state,
        fields,
    })
}

/// Project one actual OpenATCA conflict-alert episode.
///
/// `key` and `source` are externally supplied provenance and identity, never synthesized here.
/// For an open alert, `source_time_ns` is the model's onset time. For a closed alert,
/// `closure_time_ns` must be supplied by the producer as the actual close-event time and must be
/// no earlier than the final violation cycle. When unavailable, the close record remains
/// `EventState::Unknown` with no source time; `end_ns` is not treated as closure. Pass
/// `observed_time_ns` only when independently recorded; a cycle timestamp is not publication time.
pub fn project_conflict_alert(
    alert: &oa_ca::Alert,
    activity: AlertActivity,
    source: SourceIdentity,
    key: Option<BusinessKey>,
    closure_time_ns: Option<i64>,
    observed_time_ns: Option<i64>,
    output: &mut [AlertProjectionRecord],
) -> Result<(), ProjectionError> {
    let target = output.get_mut(0).ok_or(OutputCapacity)?;
    let record = make_alert_record(
        OA_ALERT_KIND_CONFLICT,
        activity,
        source,
        key,
        AlertClockEvidence {
            onset_ns: alert.start_ns,
            last_violation_ns: alert.end_ns,
            closure_ns: closure_time_ns,
            observed_ns: observed_time_ns,
        },
    )?;
    *target = record;
    Ok(())
}

/// Project one actual OpenATCA MSAW alert episode with the same conservative semantics as CA.
pub fn project_msaw_alert(
    alert: &oa_msaw::Alert,
    activity: AlertActivity,
    source: SourceIdentity,
    key: Option<BusinessKey>,
    closure_time_ns: Option<i64>,
    observed_time_ns: Option<i64>,
    output: &mut [AlertProjectionRecord],
) -> Result<(), ProjectionError> {
    let target = output.get_mut(0).ok_or(OutputCapacity)?;
    let record = make_alert_record(
        OA_ALERT_KIND_MSAW,
        activity,
        source,
        key,
        AlertClockEvidence {
            onset_ns: alert.start_ns,
            last_violation_ns: alert.end_ns,
            closure_ns: closure_time_ns,
            observed_ns: observed_time_ns,
        },
    )?;
    *target = record;
    Ok(())
}

/// Borrow the observation view for a fully observed flight snapshot projection.
pub fn flight_snapshot_observation(
    record: &ProjectedObservation,
) -> Result<Observation<'_>, FlightProjectionError> {
    projected_observation(record)
}

/// Export one actual regional flight-table row as unkeyed/unknown sector and flight-field state
/// snapshots. The output clocks come from the field stamps retained by `oa-flight`.
pub fn project_flight_output(
    flight: &Flight,
    source: SourceIdentity,
    output: &mut [ProjectedObservation],
) -> Result<CandidateProjectionSummary, CandidateProjectionError> {
    project_candidate_flight(flight, source, output)
}

/// Borrow the observation view for an alert record while preserving a missing availability clock.
pub fn alert_observation(record: &AlertProjectionRecord) -> Observation<'_> {
    Observation {
        source: record.source,
        key_state: record.key_state,
        key: record.key,
        source_time_ns: record.source_time_ns,
        observed_time_ns: record.observed_time_ns,
        event_state: record.event_state,
        fields: &record.fields,
    }
}

#[cfg(test)]
mod tests {
    use super::{make_alert_record, AlertActivity, AlertClockEvidence, ProjectionError};
    use oa_migration_shadow::EventState;

    fn source() -> oa_migration_shadow::SourceIdentity {
        oa_migration_shadow::SourceIdentity {
            stream: 1,
            decoded_offset: 32,
            row: 4,
            operation_index: 0,
        }
    }

    #[test]
    fn open_alert_uses_onset_and_keeps_availability_separate() -> Result<(), ProjectionError> {
        let record = make_alert_record(
            1,
            AlertActivity::Open,
            source(),
            None,
            AlertClockEvidence {
                onset_ns: 100,
                last_violation_ns: 140,
                closure_ns: None,
                observed_ns: Some(150),
            },
        )?;
        assert_eq!(record.source_time_ns, Some(100));
        assert_eq!(record.observed_time_ns, Some(150));
        assert_eq!(record.event_state, EventState::Observed);
        Ok(())
    }

    #[test]
    fn closed_alert_requires_actual_closure_time_or_remains_unknown() -> Result<(), ProjectionError>
    {
        let unknown = make_alert_record(
            1,
            AlertActivity::Closed,
            source(),
            None,
            AlertClockEvidence {
                onset_ns: 100,
                last_violation_ns: 140,
                closure_ns: None,
                observed_ns: Some(160),
            },
        )?;
        assert_eq!(unknown.source_time_ns, None);
        assert_eq!(unknown.observed_time_ns, Some(160));
        assert_eq!(unknown.event_state, EventState::Unknown);

        let closed = make_alert_record(
            1,
            AlertActivity::Closed,
            source(),
            None,
            AlertClockEvidence {
                onset_ns: 100,
                last_violation_ns: 140,
                closure_ns: Some(145),
                observed_ns: Some(160),
            },
        )?;
        assert_eq!(closed.source_time_ns, Some(145));
        assert_eq!(closed.observed_time_ns, Some(160));
        assert_eq!(closed.event_state, EventState::Observed);
        Ok(())
    }

    #[test]
    fn invalid_or_backdated_closure_and_availability_are_refused() {
        assert_eq!(
            make_alert_record(
                1,
                AlertActivity::Closed,
                source(),
                None,
                AlertClockEvidence {
                    onset_ns: 100,
                    last_violation_ns: 140,
                    closure_ns: Some(139),
                    observed_ns: None,
                },
            ),
            Err(ProjectionError::ClockInvalid)
        );
        assert_eq!(
            make_alert_record(
                1,
                AlertActivity::Open,
                source(),
                None,
                AlertClockEvidence {
                    onset_ns: 100,
                    last_violation_ns: 140,
                    closure_ns: None,
                    observed_ns: Some(99),
                },
            ),
            Err(ProjectionError::ClockInvalid)
        );
    }
}
