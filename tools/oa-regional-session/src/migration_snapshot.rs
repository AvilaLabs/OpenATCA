//! Message-scoped comparison of candidate flight state with an SFDPS publication.
//!
//! The profile checks propagation of published facts; it is not an independent controller-action
//! oracle because the candidate flight table consumes the same SFDPS publication. It compares
//! fields explicitly present in a decoded row, matches only the actual positive GUFI, and keeps
//! source time, recorder receive time, and candidate model-stamp time separate. The SFDPS
//! `controllingUnit` value is split into the published unit and sector bytes. No event ordinal or
//! business key is inferred. Differences remain unresolved until separately reviewed with
//! attached evidence. Missing, malformed, unsupported, and not-yet-available values are counted.
//!
//! Class: offline shell comparison. No I/O, allocation, or tracker changes.

use oa_fixm::{
    ALT_FORM_AT_FIX, ALT_FORM_BLOCK, ALT_FORM_SIMPLE, ALT_FORM_VFR, ALT_FORM_VFR_PLUS,
    STATUS_ACTIVE, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_DROPPED, STATUS_PROPOSED,
};
use oa_flight::{
    Flight, Stamp, View, ALT_AT_FIX, ALT_BLOCK, ALT_NIL, ALT_SIMPLE, ALT_VFR, ALT_VFR_ON_TOP_PLUS,
    KEY_GUFI,
};
use oa_flight_sfdps::migration_projection::{
    AssignedAltitude, SfdpsRowFacts, SourceField, SourceFieldState,
};

pub const PROFILE_ID: &str = "M4-H2-SFDPS-PUBLISHED-STATE-V1";
pub const MAX_ROW_FIELDS: usize = 11;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparedField {
    SectorOwnerUnit = 1,
    SectorIdentifier = 2,
    FlightStatus = 3,
    FlightPlanId = 4,
    Callsign = 5,
    Departure = 6,
    Arrival = 7,
    AssignedAltitude = 8,
    InterimAltitude = 9,
    RequestedAltitude = 10,
    Route = 11,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldOutcome {
    Equivalent = 1,
    Different = 2,
    SourceAbsent = 3,
    SourceUnknown = 4,
    SourceUnsupported = 5,
    CandidateUnavailable = 6,
    CandidateNotYetAvailable = 7,
    CandidateNotModeled = 8,
    IdentityMismatch = 9,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewClass {
    Equivalent = 1,
    Unresolved = 2,
    NotComparable = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CanonicalValue {
    pub present: u8,
    pub kind: u8,
    pub len: u8,
    pub bytes: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldComparison {
    pub field: ComparedField,
    pub source_state: SourceFieldState,
    pub outcome: FieldOutcome,
    pub review_class: ReviewClass,
    pub source_value: CanonicalValue,
    pub candidate_value: CanonicalValue,
    pub source_time_ns: i64,
    pub source_receive_ns: i64,
    pub candidate_source_ns: i64,
    pub candidate_model_available_ns: i64,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowIdentityState {
    MatchedGufi = 1,
    MissingGufi = 2,
    CandidateDifferentGufi = 3,
    InvalidClock = 4,
    CandidateUnavailable = 5,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowComparison {
    pub source_row: oa_flight_sfdps::audit::RowIdentity,
    pub source_gufi: u128,
    pub candidate_gufi: u128,
    pub identity_state: RowIdentityState,
    pub field_count: u8,
    pub fields: [FieldComparison; MAX_ROW_FIELDS],
    pub source_fields_present: u16,
    pub source_fields_absent: u16,
    pub source_fields_malformed: u16,
    pub source_fields_unknown: u16,
    pub source_fields_unsupported: u16,
    pub exact_matches: u16,
    pub differences_unresolved: u16,
    pub candidate_unavailable: u16,
    pub candidate_not_yet_available: u16,
    pub candidate_not_modeled: u16,
}

const EMPTY_FIELD: FieldComparison = FieldComparison {
    field: ComparedField::SectorOwnerUnit,
    source_state: SourceFieldState::Absent,
    outcome: FieldOutcome::SourceAbsent,
    review_class: ReviewClass::NotComparable,
    source_value: CanonicalValue {
        present: 0,
        kind: 0,
        len: 0,
        bytes: [0; 32],
    },
    candidate_value: CanonicalValue {
        present: 0,
        kind: 0,
        len: 0,
        bytes: [0; 32],
    },
    source_time_ns: 0,
    source_receive_ns: 0,
    candidate_source_ns: 0,
    candidate_model_available_ns: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareError {
    InvalidSourceClock,
    InvalidCandidateClock,
    OutputInvariant,
}

fn bytes_value(bytes: &[u8]) -> Option<CanonicalValue> {
    if bytes.len() > 32 {
        return None;
    }
    let mut value = CanonicalValue {
        present: 1,
        kind: 1,
        len: u8::try_from(bytes.len()).ok()?,
        bytes: [0; 32],
    };
    value.bytes.get_mut(..bytes.len())?.copy_from_slice(bytes);
    Some(value)
}

fn integer_value(value: i64) -> CanonicalValue {
    let mut out = CanonicalValue {
        present: 1,
        kind: 2,
        len: 8,
        bytes: [0; 32],
    };
    if let Some(dst) = out.bytes.get_mut(..8) {
        dst.copy_from_slice(&value.to_be_bytes());
    }
    out
}

fn altitude_value(form: u8, a_ft: i32, b_ft: i32, fix: [u8; 12]) -> CanonicalValue {
    let mut out = CanonicalValue {
        present: 1,
        kind: 3,
        len: 21,
        bytes: [0; 32],
    };
    if let Some(dst) = out.bytes.get_mut(..21) {
        if let Some(form_slot) = dst.get_mut(0) {
            *form_slot = form;
        }
        if let Some(altitude_slot) = dst.get_mut(1..5) {
            altitude_slot.copy_from_slice(&a_ft.to_be_bytes());
        }
        if let Some(block_slot) = dst.get_mut(5..9) {
            block_slot.copy_from_slice(&b_ft.to_be_bytes());
        }
        if let Some(fix_slot) = dst.get_mut(9..21) {
            fix_slot.copy_from_slice(&fix);
        }
    }
    out
}

fn source_form_altitude(value: AssignedAltitude) -> Option<CanonicalValue> {
    let form = match value.form {
        ALT_FORM_SIMPLE => ALT_SIMPLE,
        ALT_FORM_VFR_PLUS => ALT_VFR_ON_TOP_PLUS,
        ALT_FORM_VFR => ALT_VFR,
        ALT_FORM_BLOCK => ALT_BLOCK,
        ALT_FORM_AT_FIX => ALT_AT_FIX,
        _ => return None,
    };
    let (a, b, fix) = match form {
        ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS => (value.value_ft?, 0, [0; 12]),
        ALT_VFR => (0, 0, [0; 12]),
        ALT_BLOCK => (value.block_above_ft?, value.block_below_ft?, [0; 12]),
        ALT_AT_FIX => (value.fix_pre_ft?, value.fix_post_ft?, value.fix_point?),
        _ => return None,
    };
    Some(altitude_value(form, a, b, fix))
}

fn source_requested(value: (u8, Option<i32>)) -> Option<CanonicalValue> {
    let form = match value.0 {
        ALT_FORM_SIMPLE => ALT_SIMPLE,
        ALT_FORM_VFR_PLUS => ALT_VFR_ON_TOP_PLUS,
        ALT_FORM_VFR => ALT_VFR,
        _ => return None,
    };
    let feet = if matches!(form, ALT_SIMPLE | ALT_VFR_ON_TOP_PLUS) {
        value.1?
    } else {
        0
    };
    Some(altitude_value(form, feet, 0, [0; 12]))
}

fn source_status(value: u8) -> Option<CanonicalValue> {
    let state = match value {
        STATUS_ACTIVE => oa_flight::ST_ACTIVE,
        STATUS_PROPOSED => oa_flight::ST_PROPOSED,
        STATUS_COMPLETED => oa_flight::ST_COMPLETED,
        STATUS_CANCELLED => oa_flight::ST_CANCELLED,
        STATUS_DROPPED => oa_flight::ST_DROPPED,
        _ => return None,
    };
    Some(integer_value(i64::from(state)))
}

fn candidate_status(value: u8) -> Option<CanonicalValue> {
    matches!(
        value,
        oa_flight::ST_ACTIVE
            | oa_flight::ST_PROPOSED
            | oa_flight::ST_COMPLETED
            | oa_flight::ST_CANCELLED
            | oa_flight::ST_DROPPED
    )
    .then(|| integer_value(i64::from(value)))
}

fn stamp_ns(stamp: Stamp) -> Result<(i64, i64), CompareError> {
    if stamp == Stamp::default() || stamp.ts_ms < 0 || stamp.avail_ms < stamp.ts_ms {
        return Err(CompareError::InvalidCandidateClock);
    }
    Ok((
        stamp
            .ts_ms
            .checked_mul(1_000_000)
            .ok_or(CompareError::InvalidCandidateClock)?,
        stamp
            .avail_ms
            .checked_mul(1_000_000)
            .ok_or(CompareError::InvalidCandidateClock)?,
    ))
}

fn candidate_view(flight: &Flight, centre: [u8; 4]) -> Option<&View> {
    flight
        .views
        .get(..usize::from(flight.n_views))?
        .iter()
        .find(|view| view.centre == centre && view.stamp != Stamp::default())
}

fn blank_report(row: &SfdpsRowFacts<'_>, candidate_gufi: u128) -> RowComparison {
    RowComparison {
        source_row: row.identity,
        source_gufi: row.gufi,
        candidate_gufi,
        identity_state: RowIdentityState::MatchedGufi,
        field_count: 0,
        fields: [EMPTY_FIELD; MAX_ROW_FIELDS],
        source_fields_present: 0,
        source_fields_absent: 0,
        source_fields_malformed: 0,
        source_fields_unknown: 0,
        source_fields_unsupported: 0,
        exact_matches: 0,
        differences_unresolved: 0,
        candidate_unavailable: 0,
        candidate_not_yet_available: 0,
        candidate_not_modeled: 0,
    }
}

fn source_clock(row: &SfdpsRowFacts<'_>) -> Result<(i64, i64), CompareError> {
    if row.source_time_ms < 0 || row.observed_time_ms < row.source_time_ms {
        return Err(CompareError::InvalidSourceClock);
    }
    Ok((
        row.source_time_ms
            .checked_mul(1_000_000)
            .ok_or(CompareError::InvalidSourceClock)?,
        row.observed_time_ms
            .checked_mul(1_000_000)
            .ok_or(CompareError::InvalidSourceClock)?,
    ))
}

fn compare_one(
    row: &SfdpsRowFacts<'_>,
    field: ComparedField,
    source_state: SourceFieldState,
    source_value: Option<CanonicalValue>,
    candidate: Option<(CanonicalValue, Stamp)>,
    source_unsupported: bool,
    candidate_not_modeled: bool,
) -> Result<FieldComparison, CompareError> {
    let (source_time_ns, source_receive_ns) = source_clock(row)?;
    let (candidate_value, candidate_source_ns, candidate_model_available_ns) = match candidate {
        Some((value, stamp)) => {
            let (source_ns, available_ns) = stamp_ns(stamp)?;
            (value, source_ns, available_ns)
        }
        None => (CanonicalValue::default(), 0, 0),
    };
    let (outcome, review_class) = match source_state {
        SourceFieldState::Absent => (FieldOutcome::SourceAbsent, ReviewClass::NotComparable),
        SourceFieldState::Malformed => (FieldOutcome::SourceUnknown, ReviewClass::NotComparable),
        SourceFieldState::Present if source_unsupported => {
            (FieldOutcome::SourceUnsupported, ReviewClass::NotComparable)
        }
        SourceFieldState::Present if candidate_not_modeled => (
            FieldOutcome::CandidateNotModeled,
            ReviewClass::NotComparable,
        ),
        SourceFieldState::Present => match (source_value, candidate) {
            (Some(_), Some(_)) if candidate_model_available_ns > source_receive_ns => (
                FieldOutcome::CandidateNotYetAvailable,
                ReviewClass::NotComparable,
            ),
            (Some(expected), Some(_)) if expected == candidate_value => {
                (FieldOutcome::Equivalent, ReviewClass::Equivalent)
            }
            (Some(_), Some(_)) => (FieldOutcome::Different, ReviewClass::Unresolved),
            (Some(_), None) => (
                FieldOutcome::CandidateUnavailable,
                ReviewClass::NotComparable,
            ),
            (None, _) => (FieldOutcome::SourceUnknown, ReviewClass::NotComparable),
        },
    };
    Ok(FieldComparison {
        field,
        source_state,
        outcome,
        review_class,
        source_value: source_value.unwrap_or_default(),
        candidate_value,
        source_time_ns,
        source_receive_ns,
        candidate_source_ns,
        candidate_model_available_ns,
    })
}

fn push(report: &mut RowComparison, item: FieldComparison) -> Result<(), CompareError> {
    let slot = report
        .fields
        .get_mut(usize::from(report.field_count))
        .ok_or(CompareError::OutputInvariant)?;
    *slot = item;
    report.field_count = report
        .field_count
        .checked_add(1)
        .ok_or(CompareError::OutputInvariant)?;
    match item.source_state {
        SourceFieldState::Present => report.source_fields_present += 1,
        SourceFieldState::Absent => report.source_fields_absent += 1,
        SourceFieldState::Malformed => report.source_fields_malformed += 1,
    }
    if item.outcome == FieldOutcome::SourceUnsupported {
        report.source_fields_unsupported += 1;
    }
    if item.outcome == FieldOutcome::SourceUnknown {
        report.source_fields_unknown += 1;
    }
    match item.outcome {
        FieldOutcome::Equivalent => report.exact_matches += 1,
        FieldOutcome::Different => report.differences_unresolved += 1,
        FieldOutcome::CandidateUnavailable => report.candidate_unavailable += 1,
        FieldOutcome::CandidateNotYetAvailable => report.candidate_not_yet_available += 1,
        FieldOutcome::CandidateNotModeled => report.candidate_not_modeled += 1,
        FieldOutcome::SourceAbsent
        | FieldOutcome::SourceUnknown
        | FieldOutcome::SourceUnsupported
        | FieldOutcome::IdentityMismatch => {}
    }
    Ok(())
}

fn candidate_field<const N: usize>(
    value: [u8; N],
    stamp: Option<Stamp>,
) -> Result<Option<(CanonicalValue, Stamp)>, CompareError> {
    let Some(stamp) = stamp.filter(|value| *value != Stamp::default()) else {
        return Ok(None);
    };
    Ok(Some((
        bytes_value(&value).ok_or(CompareError::OutputInvariant)?,
        stamp,
    )))
}

fn candidate_altitude(
    form: u8,
    a_ft: i32,
    b_ft: i32,
    fix: [u8; 12],
    stamp: Option<Stamp>,
) -> Option<(CanonicalValue, Stamp)> {
    stamp
        .filter(|value| *value != Stamp::default())
        .map(|value| (altitude_value(form, a_ft, b_ft, fix), value))
}

fn add_source_field<T: Copy>(
    report: &mut RowComparison,
    row: &SfdpsRowFacts<'_>,
    field: ComparedField,
    source: SourceField<T>,
    encode: impl FnOnce(T) -> Option<CanonicalValue>,
    candidate: Option<(CanonicalValue, Stamp)>,
    candidate_not_modeled: bool,
) -> Result<(), CompareError> {
    let encoded = match (source.state, source.value) {
        (SourceFieldState::Present, Some(value)) => encode(value),
        _ => None,
    };
    push(
        report,
        compare_one(
            row,
            field,
            source.state,
            encoded,
            candidate,
            source.state == SourceFieldState::Present
                && encoded.is_none()
                && !candidate_not_modeled,
            candidate_not_modeled,
        )?,
    )
}

const IDENTITY_FIELDS: [ComparedField; MAX_ROW_FIELDS] = [
    ComparedField::SectorOwnerUnit,
    ComparedField::SectorIdentifier,
    ComparedField::FlightStatus,
    ComparedField::FlightPlanId,
    ComparedField::Callsign,
    ComparedField::Departure,
    ComparedField::Arrival,
    ComparedField::AssignedAltitude,
    ComparedField::InterimAltitude,
    ComparedField::RequestedAltitude,
    ComparedField::Route,
];

/// Compare one published SFDPS row with the candidate flight table immediately after processing
/// that row. The comparison checks propagation of input facts, not an independent controller
/// decision. The row locator remains provenance; the only match identity is the actual GUFI.
pub fn compare_published_row(
    source: &SfdpsRowFacts<'_>,
    candidate: &Flight,
) -> Result<RowComparison, CompareError> {
    compare_published_row_inner(source, candidate, false, false, false)
}

/// Compare a valid source row when no candidate flight view was published at this time. Present
/// source fields are counted as unavailable, while source-absent/unknown/unsupported fields keep
/// their own outcome. This never fabricates a candidate GUFI or address.
pub fn compare_published_unavailable(
    source: &SfdpsRowFacts<'_>,
) -> Result<RowComparison, CompareError> {
    let empty = oa_flight::FLIGHT_FREE;
    compare_published_row_inner(source, &empty, false, false, true)
}

/// Compare source facts with the immutable output view retained by the regional runtime. This
/// projection marks requested altitude unavailable because `SharedFlightView` does not expose it;
/// it never invents a value from source input or a different table.
pub fn compare_published_view(
    source: &SfdpsRowFacts<'_>,
    view: &crate::flight::SharedFlightView,
) -> Result<RowComparison, CompareError> {
    let mut candidate = oa_flight::FLIGHT_FREE;
    candidate.key = oa_flight::key_gufi(view.gufi);
    candidate.address = view.address;
    candidate.owner = view.owner_unit;
    candidate.ho_state = view.handoff_state;
    candidate.ho_from = view.handoff_from;
    candidate.ho_to = view.handoff_to;
    candidate.ho_accepted_by = view.handoff_accepted_by;
    candidate.assigned = oa_flight::Alt {
        form: view.assigned.form,
        a_ft: view.assigned.a_ft,
        b_ft: view.assigned.b_ft,
    };
    candidate.interim = oa_flight::Alt {
        form: view.interim.form,
        a_ft: view.interim.a_ft,
        b_ft: view.interim.b_ft,
    };
    candidate.callsign = view.callsign;
    candidate.dep = view.departure;
    candidate.arr = view.arrival;
    candidate.n_views = 1;
    candidate.views[0] = oa_flight::View {
        centre: view.centre,
        state: view.plan_state,
        plan_id: view.plan_id,
        stamp: view.plan_stamp,
    };
    candidate.stamps[oa_flight::F_ASSIGNED] = view.assigned_stamp;
    candidate.stamps[oa_flight::F_INTERIM] = view.interim_stamp;
    candidate.stamps[oa_flight::F_OWNER] = view.owner_stamp;
    candidate.stamps[oa_flight::F_HANDOFF] = view.handoff_stamp;
    candidate.stamps[oa_flight::F_ADDRESS] = view.address_stamp;
    candidate.stamps[oa_flight::F_IDENTITY] = view.identity_stamp;
    let assigned_fix_not_modeled = source
        .assigned_altitude
        .value
        .is_some_and(|altitude| altitude.form == ALT_FORM_AT_FIX);
    compare_published_row_inner(source, &candidate, true, assigned_fix_not_modeled, false)
}

fn compare_published_row_inner(
    source: &SfdpsRowFacts<'_>,
    candidate: &Flight,
    requested_not_modeled: bool,
    assigned_fix_not_modeled: bool,
    candidate_unavailable: bool,
) -> Result<RowComparison, CompareError> {
    let candidate_gufi = if !candidate_unavailable && candidate.key.kind == KEY_GUFI {
        candidate.key.gufi
    } else {
        0
    };
    let mut report = blank_report(source, candidate_gufi);
    let identity_state = if source.source_time_ms < 0
        || source.observed_time_ms < source.source_time_ms
        || source.source_time_ms.checked_mul(1_000_000).is_none()
        || source.observed_time_ms.checked_mul(1_000_000).is_none()
    {
        RowIdentityState::InvalidClock
    } else if source.gufi == 0 {
        RowIdentityState::MissingGufi
    } else if candidate_unavailable {
        RowIdentityState::CandidateUnavailable
    } else if candidate_gufi != source.gufi {
        RowIdentityState::CandidateDifferentGufi
    } else {
        RowIdentityState::MatchedGufi
    };
    report.identity_state = identity_state;
    if identity_state == RowIdentityState::CandidateDifferentGufi {
        // Preserve the independently decoded source fields while refusing to compare them with
        // another GUFI's candidate values. Amendment-propagation accounting needs the source
        // before/after values even when candidate identity is wrong.
        let source_only = compare_published_row_inner(
            source,
            &oa_flight::FLIGHT_FREE,
            requested_not_modeled,
            assigned_fix_not_modeled,
            true,
        )?;
        for original in source_only
            .fields
            .iter()
            .take(usize::from(source_only.field_count))
        {
            let mut field = *original;
            if field.outcome == FieldOutcome::CandidateUnavailable {
                field.outcome = FieldOutcome::IdentityMismatch;
            }
            field.review_class = ReviewClass::NotComparable;
            field.candidate_value = CanonicalValue::default();
            push(&mut report, field)?;
        }
        return Ok(report);
    }
    if identity_state != RowIdentityState::MatchedGufi
        && identity_state != RowIdentityState::CandidateUnavailable
    {
        for field in IDENTITY_FIELDS {
            push(
                &mut report,
                FieldComparison {
                    field,
                    source_state: SourceFieldState::Malformed,
                    outcome: FieldOutcome::IdentityMismatch,
                    review_class: ReviewClass::NotComparable,
                    source_value: CanonicalValue::default(),
                    candidate_value: CanonicalValue::default(),
                    source_time_ns: 0,
                    source_receive_ns: 0,
                    candidate_source_ns: 0,
                    candidate_model_available_ns: 0,
                },
            )?;
        }
        return Ok(report);
    }

    let owner_stamp = candidate.stamps.get(oa_flight::F_OWNER).copied();
    let owner_candidate = candidate_field(candidate.owner, owner_stamp)?;
    let owner_bytes = source.controlling_unit.value;
    let source_unit_unknown = source.controlling_unit.state == SourceFieldState::Present
        && owner_bytes.is_some_and(|value| oa_fixm::unit_code(&value) == "???");
    let source_sector_unknown = source_unit_unknown
        || owner_bytes.is_some_and(|value| {
            value
                .get(4..)
                .is_none_or(|sector| sector.iter().all(|byte| *byte == 0))
        });
    let source_unit_state = source.controlling_unit.state;
    let source_sector_state = source.controlling_unit.state;
    let unit_value = if source_unit_unknown {
        None
    } else {
        owner_bytes.and_then(|value| bytes_value(value.get(..4)?))
    };
    let sector_value = if source_sector_unknown {
        None
    } else {
        owner_bytes.and_then(|value| bytes_value(value.get(4..)?))
    };
    let unit_candidate = owner_candidate.and_then(|(value, stamp)| {
        value
            .bytes
            .get(..4)
            .and_then(bytes_value)
            .map(|value| (value, stamp))
    });
    let sector_candidate = owner_candidate.and_then(|(value, stamp)| {
        value
            .bytes
            .get(4..8)
            .and_then(bytes_value)
            .map(|value| (value, stamp))
    });
    push(
        &mut report,
        compare_one(
            source,
            ComparedField::SectorOwnerUnit,
            source_unit_state,
            unit_value,
            unit_candidate,
            false,
            false,
        )?,
    )?;
    push(
        &mut report,
        compare_one(
            source,
            ComparedField::SectorIdentifier,
            source_sector_state,
            sector_value,
            sector_candidate,
            false,
            false,
        )?,
    )?;

    let view = candidate_view(candidate, source.centre);
    let view_status =
        view.and_then(|row| candidate_status(row.state).map(|value| (value, row.stamp)));
    add_source_field(
        &mut report,
        source,
        ComparedField::FlightStatus,
        source.flight_status,
        source_status,
        view_status,
        false,
    )?;
    let view_plan = view.and_then(|row| bytes_value(&row.plan_id).map(|value| (value, row.stamp)));
    add_source_field(
        &mut report,
        source,
        ComparedField::FlightPlanId,
        source.flight_plan_id,
        |value| bytes_value(&value),
        view_plan,
        false,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::Callsign,
        source.callsign,
        |value| bytes_value(&value),
        candidate_field(
            candidate.callsign,
            candidate.stamps.get(oa_flight::F_IDENTITY).copied(),
        )?,
        false,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::Departure,
        source.departure,
        |value| bytes_value(&value),
        candidate_field(
            candidate.dep,
            candidate.stamps.get(oa_flight::F_IDENTITY).copied(),
        )?,
        false,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::Arrival,
        source.arrival,
        |value| bytes_value(&value),
        candidate_field(
            candidate.arr,
            candidate.stamps.get(oa_flight::F_IDENTITY).copied(),
        )?,
        false,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::AssignedAltitude,
        source.assigned_altitude,
        source_form_altitude,
        candidate_altitude(
            candidate.assigned.form,
            candidate.assigned.a_ft,
            candidate.assigned.b_ft,
            candidate.assigned_fix,
            candidate.stamps.get(oa_flight::F_ASSIGNED).copied(),
        ),
        assigned_fix_not_modeled,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::InterimAltitude,
        source.interim_altitude,
        |value| {
            Some(if value.is_nil {
                altitude_value(ALT_NIL, 0, 0, [0; 12])
            } else {
                altitude_value(ALT_SIMPLE, value.value_ft?, 0, [0; 12])
            })
        },
        candidate_altitude(
            candidate.interim.form,
            candidate.interim.a_ft,
            candidate.interim.b_ft,
            [0; 12],
            candidate.stamps.get(oa_flight::F_INTERIM).copied(),
        ),
        false,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::RequestedAltitude,
        source.requested_altitude,
        source_requested,
        candidate_altitude(
            candidate.requested.form,
            candidate.requested.a_ft,
            candidate.requested.b_ft,
            [0; 12],
            candidate.stamps.get(oa_flight::F_REQUESTED).copied(),
        ),
        requested_not_modeled,
    )?;
    add_source_field(
        &mut report,
        source,
        ComparedField::Route,
        source.route,
        |_| None,
        None,
        true,
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{
        compare_published_row, compare_published_unavailable, compare_published_view,
        ComparedField, FieldOutcome, RowIdentityState,
    };
    use oa_flight::{Flight, Stamp, FLIGHT_FREE, F_OWNER};
    use oa_flight_sfdps::audit::RowIdentity;
    use oa_flight_sfdps::migration_projection::{
        AssignedAltitude, InterimAltitude, SfdpsRowFacts, SourceField, SourceFieldState,
    };

    fn absent<T>() -> SourceField<T> {
        SourceField {
            state: SourceFieldState::Absent,
            value: None,
        }
    }

    fn facts() -> SfdpsRowFacts<'static> {
        SfdpsRowFacts {
            identity: RowIdentity {
                stream: 1,
                decoded_offset: 10,
                recorder_sequence: 4,
                row: 2,
            },
            gufi: 0x1234,
            centre: *b"ZXXA",
            source_time_ms: 1_000,
            observed_time_ms: 1_100,
            message_sequence: absent(),
            plan_sequence: absent(),
            handoff_event: absent(),
            handoff_transferring: absent(),
            handoff_receiving: absent(),
            handoff_accepting: absent(),
            controlling_unit: SourceField {
                state: SourceFieldState::Present,
                value: Some(*b"ZXXA0102"),
            },
            flight_status: absent(),
            flight_plan_id: absent(),
            callsign: absent(),
            departure: absent(),
            arrival: absent(),
            assigned_altitude: SourceField {
                state: SourceFieldState::Present,
                value: Some(AssignedAltitude {
                    form: oa_fixm::ALT_FORM_SIMPLE,
                    value_ft: Some(12_000),
                    block_above_ft: None,
                    block_below_ft: None,
                    fix_pre_ft: None,
                    fix_post_ft: None,
                    fix_point: None,
                }),
            },
            interim_altitude: SourceField {
                state: SourceFieldState::Present,
                value: Some(InterimAltitude {
                    is_nil: false,
                    value_ft: Some(9_000),
                }),
            },
            requested_altitude: absent(),
            route: absent(),
        }
    }

    fn flight() -> Flight {
        let mut flight = FLIGHT_FREE;
        flight.key = oa_flight::key_gufi(0x1234);
        flight.owner = *b"ZXXA0102";
        flight.stamps[F_OWNER] = Stamp {
            ts_ms: 1_000,
            avail_ms: 1_050,
            msg: 7,
        };
        flight.assigned = oa_flight::Alt {
            form: oa_flight::ALT_SIMPLE,
            a_ft: 12_000,
            b_ft: 0,
        };
        flight.stamps[oa_flight::F_ASSIGNED] = Stamp {
            ts_ms: 1_000,
            avail_ms: 1_050,
            msg: 7,
        };
        flight.interim = oa_flight::Alt {
            form: oa_flight::ALT_SIMPLE,
            a_ft: 9_000,
            b_ft: 0,
        };
        flight.stamps[oa_flight::F_INTERIM] = Stamp {
            ts_ms: 1_000,
            avail_ms: 1_050,
            msg: 7,
        };
        flight
    }

    // REQ: OA-MIG-413
    #[test]
    fn runtime_view_comparison_marks_unexported_fields_not_modeled(
    ) -> Result<(), super::CompareError> {
        let mut row = facts();
        row.requested_altitude = SourceField {
            state: SourceFieldState::Present,
            value: Some((oa_fixm::ALT_FORM_SIMPLE, Some(8_000))),
        };
        row.route = SourceField {
            state: SourceFieldState::Present,
            value: Some("FIX1 FIX2"),
        };
        let stamp = Stamp {
            ts_ms: 1_000,
            avail_ms: 1_050,
            msg: 7,
        };
        let view = crate::flight::SharedFlightView {
            gufi: 0x1234,
            address: 0x123456,
            centre: *b"ZXXA",
            plan_id: [0; 12],
            plan_state: oa_flight::ST_ACTIVE,
            reserved_after_plan_state: [0; 3],
            plan_stamp: Stamp::default(),
            assigned: crate::flight::SharedAltitude {
                form: oa_flight::ALT_SIMPLE,
                reserved_form: [0; 3],
                a_ft: 12_000,
                b_ft: 0,
            },
            reserved_after_assigned: [0; 4],
            assigned_stamp: stamp,
            interim: crate::flight::SharedAltitude {
                form: oa_flight::ALT_SIMPLE,
                reserved_form: [0; 3],
                a_ft: 9_000,
                b_ft: 0,
            },
            reserved_after_interim: [0; 4],
            interim_stamp: stamp,
            effective_clearance: crate::flight::EffectiveClearance {
                kind: oa_flight::ALT_SIMPLE,
                reserved_before_altitude: [0; 3],
                altitude_ft: 9_000,
                supported_as_single_level: true,
                reserved_after_supported: [0; 3],
            },
            callsign: [0; 8],
            departure: [0; 12],
            arrival: [0; 12],
            reserved_before_identity_stamp: [0; 4],
            identity_stamp: Stamp::default(),
            address_stamp: Stamp::default(),
            owner_unit: *b"ZXXA0102",
            owner_stamp: stamp,
            handoff_state: oa_flight::HO_NONE,
            handoff_from: [0; 8],
            handoff_to: [0; 8],
            handoff_accepted_by: [0; 8],
            reserved_before_handoff_stamp: [0; 7],
            handoff_stamp: Stamp::default(),
            reserved_tail: [0; 8],
        };
        let report = compare_published_view(&row, &view)?;
        assert_eq!(report.identity_state, RowIdentityState::MatchedGufi);
        assert_eq!(report.fields[9].outcome, FieldOutcome::CandidateNotModeled);
        assert_eq!(report.fields[10].outcome, FieldOutcome::CandidateNotModeled);
        assert_eq!(report.candidate_not_modeled, 2);
        assert_eq!(report.candidate_unavailable, 0);
        Ok(())
    }

    // REQ: OA-MIG-408
    #[test]
    fn published_center_status_and_plan_id_compare_only_with_that_center_view(
    ) -> Result<(), super::CompareError> {
        let mut row = facts();
        row.flight_status = SourceField {
            state: SourceFieldState::Present,
            value: Some(oa_fixm::STATUS_ACTIVE),
        };
        row.flight_plan_id = SourceField {
            state: SourceFieldState::Present,
            value: Some(*b"PLAN12345678"),
        };
        let mut candidate = flight();
        candidate.n_views = 1;
        candidate.views[0] = oa_flight::View {
            centre: *b"ZXXA",
            state: oa_flight::ST_ACTIVE,
            plan_id: *b"PLAN12345678",
            stamp: Stamp {
                ts_ms: 1_000,
                avail_ms: 1_050,
                msg: 7,
            },
        };
        let report = compare_published_row(&row, &candidate)?;
        assert_eq!(
            report.fields.get(2).map(|field| field.outcome),
            Some(FieldOutcome::Equivalent)
        );
        assert_eq!(
            report.fields.get(3).map(|field| field.outcome),
            Some(FieldOutcome::Equivalent)
        );
        Ok(())
    }

    // REQ: OA-MIG-408
    #[test]
    fn unknown_published_owner_is_not_compared_as_a_concrete_sector(
    ) -> Result<(), super::CompareError> {
        let mut row = facts();
        row.controlling_unit = SourceField {
            state: SourceFieldState::Present,
            value: Some(*b"???\0A102"),
        };
        let report = compare_published_row(&row, &flight())?;
        assert_eq!(
            report.fields.first().map(|field| field.outcome),
            Some(FieldOutcome::SourceUnknown)
        );
        assert_eq!(
            report.fields.get(1).map(|field| field.outcome),
            Some(FieldOutcome::SourceUnknown)
        );
        assert_eq!(report.exact_matches, 2);
        Ok(())
    }

    // REQ: OA-MIG-413
    #[test]
    fn missing_candidate_view_preserves_source_absence_and_present_field_unavailability(
    ) -> Result<(), super::CompareError> {
        let report = compare_published_unavailable(&facts())?;
        assert_eq!(
            report.identity_state,
            RowIdentityState::CandidateUnavailable
        );
        assert_eq!(report.fields[0].outcome, FieldOutcome::CandidateUnavailable);
        assert_eq!(report.fields[2].outcome, FieldOutcome::SourceAbsent);
        assert_eq!(report.fields[7].outcome, FieldOutcome::CandidateUnavailable);
        assert!(report.candidate_unavailable > 0);
        assert!(report.source_fields_absent > 0);
        Ok(())
    }

    // REQ: OA-MIG-408
    #[test]
    fn sector_and_flight_fields_match_without_collapsing_their_clocks(
    ) -> Result<(), super::CompareError> {
        let report = compare_published_row(&facts(), &flight())?;
        assert_eq!(report.identity_state, RowIdentityState::MatchedGufi);
        assert_eq!(report.exact_matches, 4);
        assert_eq!(report.differences_unresolved, 0);
        assert_eq!(
            report.fields.first().map(|field| field.field),
            Some(ComparedField::SectorOwnerUnit)
        );
        assert_eq!(
            report.fields.first().map(|field| field.outcome),
            Some(FieldOutcome::Equivalent)
        );
        assert_eq!(
            report.fields.first().map(|field| field.source_receive_ns),
            Some(1_100_000_000)
        );
        assert_eq!(
            report
                .fields
                .first()
                .map(|field| field.candidate_model_available_ns),
            Some(1_050_000_000)
        );
        Ok(())
    }

    // REQ: OA-MIG-410
    #[test]
    fn flight_data_values_are_row_scoped_propagation_not_amendment_events(
    ) -> Result<(), super::CompareError> {
        let report = compare_published_row(&facts(), &flight())?;
        assert_eq!(
            report.fields.get(7).map(|field| field.outcome),
            Some(FieldOutcome::Equivalent)
        );
        assert_eq!(
            report.fields.get(8).map(|field| field.outcome),
            Some(FieldOutcome::Equivalent)
        );
        assert_eq!(report.differences_unresolved, 0);
        Ok(())
    }

    // REQ: OA-MIG-408
    #[test]
    fn differences_are_unresolved_and_malformed_absent_and_unmodeled_fields_remain_visible(
    ) -> Result<(), super::CompareError> {
        let mut candidate = flight();
        candidate.owner = *b"ZXXA0103";
        candidate.assigned.a_ft = 13_000;
        let mut row = facts();
        row.callsign = SourceField {
            state: SourceFieldState::Malformed,
            value: None,
        };
        let report = compare_published_row(&row, &candidate)?;
        assert_eq!(report.differences_unresolved, 2);
        assert!(report.source_fields_absent > 0);
        assert_eq!(report.source_fields_malformed, 1);
        assert_eq!(
            report.fields.get(1).map(|field| field.outcome),
            Some(FieldOutcome::Different)
        );
        assert_eq!(
            report.fields.get(1).map(|field| field.review_class),
            Some(super::ReviewClass::Unresolved)
        );
        assert_eq!(
            report.fields.get(4).map(|field| field.outcome),
            Some(FieldOutcome::SourceUnknown)
        );
        assert_eq!(
            report.fields.get(10).map(|field| field.outcome),
            Some(FieldOutcome::SourceAbsent)
        );
        Ok(())
    }

    // REQ: OA-MIG-408
    #[test]
    fn wrong_identity_and_future_model_value_are_not_counted_as_agreement(
    ) -> Result<(), super::CompareError> {
        let mut candidate = flight();
        candidate.key = oa_flight::key_gufi(0x5678);
        let mismatch = compare_published_row(&facts(), &candidate)?;
        assert_eq!(
            mismatch.identity_state,
            RowIdentityState::CandidateDifferentGufi
        );
        assert_eq!(mismatch.exact_matches, 0);
        assert_eq!(
            mismatch.fields.first().map(|field| field.outcome),
            Some(FieldOutcome::IdentityMismatch)
        );
        assert_eq!(
            mismatch.fields.first().map(|field| field.source_state),
            Some(SourceFieldState::Present)
        );
        assert_eq!(
            mismatch
                .fields
                .first()
                .map(|field| field.source_value.present),
            Some(1)
        );

        let mut candidate = flight();
        candidate.stamps[F_OWNER].avail_ms = 1_101;
        let future = compare_published_row(&facts(), &candidate)?;
        assert_eq!(
            future.fields.first().map(|field| field.outcome),
            Some(FieldOutcome::CandidateNotYetAvailable)
        );
        assert_eq!(future.candidate_not_yet_available, 2);
        Ok(())
    }
}
