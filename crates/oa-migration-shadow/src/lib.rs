#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! Pure bounded comparison of two normalized migration-shadow evidence streams (H2/M4).
//!
//! This crate compares supplied observations; it does not decode SFDPS, generate a legacy
//! reference, reconstruct controller intent, or claim independent prediction accuracy. SFDPS
//! observed-state replay is compatibility evidence only. Missing legacy alert evidence remains
//! unobservable. Handoff, point-out, sector, amendment, and alert families share one explicit
//! schema so every present, absent, unknown, unsupported, invalid, missing, duplicate, and
//! out-of-window outcome is retained.
//! `FIELD_PROJECTION_VERSION` 1 requires a field record for every family-specific `FieldId`;
//! explicit `Absent` is a known value, while an omitted required field is `MissingRequired` and
//! makes that pair unobservable. The projection is a normalized comparison contract, not proof
//! that a source system exposes each field.
//!
//! Policy `M4-H2-EXACT-1` compares event timestamps and numeric values exactly (0 ns / 0 value
//! margin). This is an engineering comparison policy, not an FAA limit. Event/source time and
//! observed/receive time are independent fields; neither is silently substituted for the other.
//! Inputs are bounded and sorted by business key. Repeated business keys are ambiguous duplicates
//! and are never paired first-wins. The caller supplies normalized values and canonical digests
//! for values longer than 32 bytes.
//!
//! Each side is capped at 250,000 records and 16 fields per record; each `FieldValue` is 48 bytes,
//! so the two sides' maximum field payload is 384,000,000 bytes. The caller owns input and output
//! buffers; this crate allocates nothing. Comparison is linear in the record and field counts.
//!
//! Class: pure core (RULESET B1, B7). No I/O, clocks, decoder, allocation, or external dependency.

#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

pub const POLICY_ID: &str = "M4-H2-EXACT-1";
pub const POLICY_VERSION: u16 = 1;
pub const EVENT_TIME_MARGIN_NS: i64 = 0;
pub const NUMERIC_VALUE_MARGIN: i64 = 0;
pub const MAX_RECORDS_PER_SIDE: usize = 250_000;
pub const MAX_FIELDS_PER_RECORD: usize = 16;
pub const MAX_COMPARISON_FIELD_ISSUES: usize = MAX_FIELDS_PER_RECORD * 2;
pub const FAMILY_COUNT: usize = 5;
pub const FIELD_PROJECTION_VERSION: u16 = 1;

const HANDOFF_FIELD_IDS: [u16; 6] = [1, 2, 3, 4, 5, 6];
const POINT_OUT_FIELD_IDS: [u16; 3] = [10, 11, 12];
const SECTOR_FIELD_IDS: [u16; 2] = [20, 21];
const AMENDMENT_FIELD_IDS: [u16; 3] = [30, 31, 32];
const ALERT_FIELD_IDS: [u16; 5] = [40, 41, 42, 43, 44];

/// Required v1 normalized field projection. Every listed ID must be represented on each
/// observed record; known absence uses `FieldState::Absent`, while no field record is missing.
pub const fn required_field_ids(family: Family) -> &'static [u16] {
    match family {
        Family::Handoff => &HANDOFF_FIELD_IDS,
        Family::PointOut => &POINT_OUT_FIELD_IDS,
        Family::SectorAssignment => &SECTOR_FIELD_IDS,
        Family::FlightAmendment => &AMENDMENT_FIELD_IDS,
        Family::Alert => &ALERT_FIELD_IDS,
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Handoff = 1,
    PointOut = 2,
    SectorAssignment = 3,
    FlightAmendment = 4,
    Alert = 5,
}

pub const fn family_index(family: Family) -> usize {
    match family {
        Family::Handoff => 0,
        Family::PointOut => 1,
        Family::SectorAssignment => 2,
        Family::FlightAmendment => 3,
        Family::Alert => 4,
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    CompleteWithEvents = 1,
    CompleteEmpty = 2,
    Partial = 3,
    Missing = 4,
    Unsupported = 5,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Valid = 1,
    Missing = 2,
    Invalid = 3,
    Ambiguous = 4,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventState {
    Observed = 1,
    Unknown = 2,
    Unsupported = 3,
    Invalid = 4,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldState {
    Present = 1,
    Absent = 2,
    Unknown = 3,
    Unsupported = 4,
    Invalid = 5,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    None = 0,
    Integer = 1,
    Bytes32 = 2,
    Digest32 = 3,
}

/// Stable semantic field codes in the M4-H2 normalized event schema. A code outside these
/// constants must be recorded as `Unsupported`, not silently assigned an existing meaning.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldId {
    HandoffKind = 1,
    HandoffTransferringUnit = 2,
    HandoffReceivingUnit = 3,
    HandoffAcceptingUnit = 4,
    HandoffPhase = 5,
    HandoffOwner = 6,
    PointOutOrigin = 10,
    PointOutReceiving = 11,
    PointOutState = 12,
    SectorOwnerUnit = 20,
    SectorIdentifier = 21,
    AmendmentKind = 30,
    AmendmentField = 31,
    AmendmentValue = 32,
    AlertKind = 40,
    AlertActive = 41,
    AlertAcknowledged = 42,
    AlertSuppressed = 43,
    AlertReason = 44,
}

/// Side-local source provenance. It is never used as the cross-system match key.
// A7.2: offline evidence record, size guard waived.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceIdentity {
    pub stream: u32,
    pub decoded_offset: u64,
    pub row: u32,
    pub operation_index: u16,
}

/// Caller-supplied stable cross-side event key. `entity_key` is a canonical identifier or its
/// SHA-256; `ordinal` is a versioned event order within the entity and family.
// A7.2: offline evidence key, size guard waived.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusinessKey {
    pub entity_key: [u8; 32],
    pub family: Family,
    pub ordinal: u64,
}

/// Canonical field value. Non-present states carry `ValueKind::None` and all-zero value bytes.
// A7.2: offline field record, size guard waived.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldValue {
    pub id: u16,
    pub state: FieldState,
    pub kind: ValueKind,
    pub integer: i64,
    pub bytes: [u8; 32],
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldIssueKind {
    Different = 1,
    Unobservable = 2,
    MissingRequired = 3,
}

/// Field-level explanation for one compared pair.
// A7.2: offline issue record, size guard waived.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldIssue {
    pub id: u16,
    pub kind: FieldIssueKind,
}

const EMPTY_FIELD_ISSUE: FieldIssue = FieldIssue {
    id: 0,
    kind: FieldIssueKind::Unobservable,
};

/// One normalized observation. `fields` must be sorted by strictly increasing `id`.
// A7.2: offline evidence record, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation<'a> {
    pub source: SourceIdentity,
    pub key_state: KeyState,
    pub key: BusinessKey,
    pub source_time_ns: Option<i64>,
    pub observed_time_ns: Option<i64>,
    pub event_state: EventState,
    pub fields: &'a [FieldValue],
}

/// Provenance and explicit coverage for one side. Coverage is per family; a complete empty family
/// is distinct from missing or unsupported data. `epoch_sha256` must identify the exact source
/// epoch used to produce this side's observations.
// A7.2: offline evidence header, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceHeader {
    pub function_id: u32,
    pub scope_id: u32,
    pub window_from_ns: i64,
    pub window_to_ns: i64,
    pub source_time_basis: u32,
    pub observed_time_basis: u32,
    pub epoch_sha256: [u8; 32],
    pub coverage: [Coverage; FAMILY_COUNT],
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Equivalent = 1,
    Different = 2,
    LegacyOnly = 3,
    CandidateOnly = 4,
    DuplicateAmbiguous = 5,
    Unobservable = 6,
    OutsideWindow = 7,
    MissingKey = 8,
    InvalidKey = 9,
    AmbiguousKey = 10,
}

/// One output row for a unique key group or keyless row. A duplicate group emits one row per
/// source identity; its group-size fields repeat the group totals on each such row.
// A7.2: offline report row, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub key: Option<BusinessKey>,
    pub family: Family,
    pub legacy: Option<SourceIdentity>,
    pub candidate: Option<SourceIdentity>,
    pub legacy_count: u32,
    pub candidate_count: u32,
    pub outcome: Outcome,
    pub field_differences: u16,
    pub field_unobservable: u16,
    pub field_issue_count: u8,
    pub field_issues: [FieldIssue; MAX_COMPARISON_FIELD_ISSUES],
    pub source_time_delta_ns: Option<i64>,
    pub observed_time_delta_ns: Option<i64>,
}

/// Reconciled denominators and pair outcomes, one counter set per family.
// A7.2: offline aggregate, size guard waived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FamilyCounts {
    pub legacy_records: u64,
    pub candidate_records: u64,
    pub legacy_eligible: u64,
    pub candidate_eligible: u64,
    pub legacy_missing_key: u64,
    pub candidate_missing_key: u64,
    pub legacy_invalid_key: u64,
    pub candidate_invalid_key: u64,
    pub legacy_ambiguous_key: u64,
    pub candidate_ambiguous_key: u64,
    pub legacy_missing_time: u64,
    pub candidate_missing_time: u64,
    pub legacy_outside_window: u64,
    pub candidate_outside_window: u64,
    pub legacy_unknown: u64,
    pub candidate_unknown: u64,
    pub legacy_unsupported: u64,
    pub candidate_unsupported: u64,
    pub legacy_invalid: u64,
    pub candidate_invalid: u64,
    pub legacy_out_of_order: u64,
    pub candidate_out_of_order: u64,
    pub key_groups: u64,
    pub exact_pairs: u64,
    pub different_pairs: u64,
    pub legacy_only: u64,
    pub candidate_only: u64,
    pub duplicate_groups: u64,
    pub duplicate_legacy_records: u64,
    pub duplicate_candidate_records: u64,
    pub unobservable_groups: u64,
    pub outside_only_groups: u64,
    pub field_differences: u64,
    pub field_unobservable: u64,
    pub source_time_differences: u64,
    pub observed_time_differences: u64,
    pub legacy_observed_time_missing: u64,
    pub candidate_observed_time_missing: u64,
    pub legacy_fields_present: u64,
    pub candidate_fields_present: u64,
    pub legacy_fields_absent: u64,
    pub candidate_fields_absent: u64,
    pub legacy_fields_unknown: u64,
    pub candidate_fields_unknown: u64,
    pub legacy_fields_unsupported: u64,
    pub candidate_fields_unsupported: u64,
    pub legacy_fields_invalid: u64,
    pub candidate_fields_invalid: u64,
}

// A7.2: offline result header, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComparisonReport {
    pub policy_version: u16,
    pub function_id: u32,
    pub scope_id: u32,
    pub window_from_ns: i64,
    pub window_to_ns: i64,
    pub legacy_epoch_sha256: [u8; 32],
    pub candidate_epoch_sha256: [u8; 32],
    pub legacy_coverage: [Coverage; FAMILY_COUNT],
    pub candidate_coverage: [Coverage; FAMILY_COUNT],
    pub families: [FamilyCounts; FAMILY_COUNT],
    pub comparison_rows: usize,
    pub out_of_order_side_families: u8,
    pub coverage_incomplete_families: u8,
    pub no_event_families: u8,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareError {
    InvalidWindow = 1,
    FunctionMismatch = 2,
    ScopeMismatch = 3,
    WindowMismatch = 4,
    TimeBasisMismatch = 5,
    MissingEpoch = 6,
    RecordCap = 7,
    OutputCap = 8,
    FieldCap = 9,
    FieldOrder = 10,
    FieldEncoding = 11,
    FieldIssueCap = 12,
    KeyOrder = 13,
    CoverageContradiction = 14,
    CounterOverflow = 15,
    TimeDeltaOverflow = 16,
    TimeBasisMissing = 17,
    InvalidBusinessKey = 18,
    ConservationFailure = 19,
}

fn key_cmp(a: &BusinessKey, b: &BusinessKey) -> i8 {
    for (left, right) in a.entity_key.iter().zip(b.entity_key.iter()) {
        if left < right {
            return -1;
        }
        if left > right {
            return 1;
        }
    }
    let af = a.family as u8;
    let bf = b.family as u8;
    if af < bf {
        -1
    } else if af > bf {
        1
    } else if a.ordinal < b.ordinal {
        -1
    } else if a.ordinal > b.ordinal {
        1
    } else {
        0
    }
}

fn zero_bytes(bytes: &[u8; 32]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn bump(value: &mut u64) -> Result<(), CompareError> {
    *value = value.checked_add(1).ok_or(CompareError::CounterOverflow)?;
    Ok(())
}

fn add_count(value: &mut u64, amount: usize) -> Result<(), CompareError> {
    let amount = u64::try_from(amount).map_err(|_| CompareError::CounterOverflow)?;
    *value = value
        .checked_add(amount)
        .ok_or(CompareError::CounterOverflow)?;
    Ok(())
}

fn validate_field(field: &FieldValue) -> bool {
    match field.state {
        FieldState::Present => match field.kind {
            ValueKind::Integer => zero_bytes(&field.bytes),
            ValueKind::Bytes32 | ValueKind::Digest32 => field.integer == 0,
            ValueKind::None => false,
        },
        FieldState::Absent
        | FieldState::Unknown
        | FieldState::Unsupported
        | FieldState::Invalid => {
            field.kind == ValueKind::None && field.integer == 0 && zero_bytes(&field.bytes)
        }
    }
}

fn validate_header(header: &EvidenceHeader) -> Result<(), CompareError> {
    if header.window_from_ns >= header.window_to_ns
        || header
            .window_to_ns
            .checked_sub(header.window_from_ns)
            .is_none()
    {
        return Err(CompareError::InvalidWindow);
    }
    if header.source_time_basis == 0 || header.observed_time_basis == 0 {
        return Err(CompareError::TimeBasisMissing);
    }
    if zero_bytes(&header.epoch_sha256) {
        return Err(CompareError::MissingEpoch);
    }
    Ok(())
}

fn validate_side(header: &EvidenceHeader, records: &[Observation<'_>]) -> Result<(), CompareError> {
    validate_header(header)?;
    if records.len() > MAX_RECORDS_PER_SIDE {
        return Err(CompareError::RecordCap);
    }
    let mut previous: Option<&BusinessKey> = None;
    for record in records {
        if record.fields.len() > MAX_FIELDS_PER_RECORD {
            return Err(CompareError::FieldCap);
        }
        let mut prev_field = None;
        for field in record.fields {
            if prev_field.is_some_and(|id| field.id <= id) {
                return Err(CompareError::FieldOrder);
            }
            if !validate_field(field) {
                return Err(CompareError::FieldEncoding);
            }
            prev_field = Some(field.id);
        }
        if record.key_state == KeyState::Valid {
            if zero_bytes(&record.key.entity_key) {
                return Err(CompareError::InvalidBusinessKey);
            }
            if let Some(prior) = previous {
                if key_cmp(prior, &record.key) > 0 {
                    return Err(CompareError::KeyOrder);
                }
            }
            previous = Some(&record.key);
        }
    }
    Ok(())
}

fn validate_coverage(
    header: &EvidenceHeader,
    records: &[Observation<'_>],
) -> Result<(), CompareError> {
    let mut in_window = [0u64; FAMILY_COUNT];
    let mut missing_time = [0u64; FAMILY_COUNT];
    for record in records {
        if record.source_time_ns.is_none() {
            let count = missing_time
                .get_mut(family_index(record.key.family))
                .ok_or(CompareError::CoverageContradiction)?;
            bump(count)?;
        }
        if record
            .source_time_ns
            .is_some_and(|time| header.window_from_ns <= time && time < header.window_to_ns)
        {
            let count = in_window
                .get_mut(family_index(record.key.family))
                .ok_or(CompareError::CoverageContradiction)?;
            bump(count)?;
        }
    }
    for (index, status) in header.coverage.iter().enumerate() {
        let records = *in_window
            .get(index)
            .ok_or(CompareError::CoverageContradiction)?;
        let status_contradiction = match *status {
            Coverage::CompleteEmpty => records != 0,
            Coverage::CompleteWithEvents => records == 0,
            Coverage::Partial => false,
            Coverage::Missing | Coverage::Unsupported => records != 0,
        };
        let complete_with_missing_time = matches!(
            *status,
            Coverage::CompleteEmpty | Coverage::CompleteWithEvents
        ) && missing_time.get(index).copied().unwrap_or(0) > 0;
        if status_contradiction || complete_with_missing_time {
            return Err(CompareError::CoverageContradiction);
        }
    }
    Ok(())
}

fn push_field_issue(
    issues: &mut [FieldIssue; MAX_COMPARISON_FIELD_ISSUES],
    count: &mut usize,
    issue: FieldIssue,
) -> Result<(), CompareError> {
    let mut index = *count;
    if index >= issues.len() {
        return Err(CompareError::FieldIssueCap);
    }
    while index > 0 {
        let previous_index = index.checked_sub(1).ok_or(CompareError::FieldIssueCap)?;
        let previous = issues
            .get(previous_index)
            .copied()
            .ok_or(CompareError::FieldIssueCap)?;
        if previous.id <= issue.id {
            break;
        }
        let target = issues.get_mut(index).ok_or(CompareError::FieldIssueCap)?;
        *target = previous;
        index -= 1;
    }
    let target = issues.get_mut(index).ok_or(CompareError::FieldIssueCap)?;
    *target = issue;
    *count = (*count).checked_add(1).ok_or(CompareError::FieldIssueCap)?;
    Ok(())
}

fn has_field(fields: &[FieldValue], id: u16) -> bool {
    fields.iter().any(|field| field.id == id)
}

fn is_required_field(family: Family, id: u16) -> bool {
    required_field_ids(family).contains(&id)
}

fn field_compare(
    family: Family,
    left: &[FieldValue],
    right: &[FieldValue],
) -> Result<(u16, u16, u8, [FieldIssue; MAX_COMPARISON_FIELD_ISSUES]), CompareError> {
    let (mut i, mut j, mut differences, mut unobservable) = (0usize, 0usize, 0u16, 0u16);
    let mut issue_count = 0usize;
    let mut issues = [EMPTY_FIELD_ISSUE; MAX_COMPARISON_FIELD_ISSUES];
    while i < left.len() || j < right.len() {
        let (a, b) = (left.get(i), right.get(j));
        match (a, b) {
            (Some(a), Some(b)) if a.id == b.id => {
                if a.state == FieldState::Unknown
                    || a.state == FieldState::Unsupported
                    || a.state == FieldState::Invalid
                    || b.state == FieldState::Unknown
                    || b.state == FieldState::Unsupported
                    || b.state == FieldState::Invalid
                {
                    unobservable = unobservable.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: a.id,
                            kind: FieldIssueKind::Unobservable,
                        },
                    )?;
                } else if a.state != b.state
                    || (a.state == FieldState::Present
                        && (a.kind != b.kind || a.integer != b.integer || a.bytes != b.bytes))
                {
                    differences = differences.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: a.id,
                            kind: FieldIssueKind::Different,
                        },
                    )?;
                }
                i = i.saturating_add(1);
                j = j.saturating_add(1);
            }
            (Some(a), Some(b)) if a.id < b.id => {
                if !is_required_field(family, a.id) {
                    unobservable = unobservable.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: a.id,
                            kind: FieldIssueKind::Unobservable,
                        },
                    )?;
                }
                i = i.saturating_add(1);
            }
            (Some(_), Some(b)) => {
                if !is_required_field(family, b.id) {
                    unobservable = unobservable.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: b.id,
                            kind: FieldIssueKind::Unobservable,
                        },
                    )?;
                }
                j = j.saturating_add(1);
            }
            (Some(a), None) => {
                if !is_required_field(family, a.id) {
                    unobservable = unobservable.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: a.id,
                            kind: FieldIssueKind::Unobservable,
                        },
                    )?;
                }
                i = i.saturating_add(1);
            }
            (None, Some(b)) => {
                if !is_required_field(family, b.id) {
                    unobservable = unobservable.saturating_add(1);
                    push_field_issue(
                        &mut issues,
                        &mut issue_count,
                        FieldIssue {
                            id: b.id,
                            kind: FieldIssueKind::Unobservable,
                        },
                    )?;
                }
                j = j.saturating_add(1);
            }
            (None, None) => break,
        }
    }
    for id in required_field_ids(family) {
        if !has_field(left, *id) || !has_field(right, *id) {
            unobservable = unobservable.saturating_add(1);
            push_field_issue(
                &mut issues,
                &mut issue_count,
                FieldIssue {
                    id: *id,
                    kind: FieldIssueKind::MissingRequired,
                },
            )?;
        }
    }
    Ok((
        differences,
        unobservable,
        u8::try_from(issue_count).map_err(|_| CompareError::FieldIssueCap)?,
        issues,
    ))
}

fn in_window(header: &EvidenceHeader, observation: &Observation<'_>) -> Option<bool> {
    observation
        .source_time_ns
        .map(|time| header.window_from_ns <= time && time < header.window_to_ns)
}

fn eligible(header: &EvidenceHeader, observation: &Observation<'_>) -> bool {
    in_window(header, observation) == Some(true) && observation.event_state == EventState::Observed
}

fn has_complete_projection(family: Family, fields: &[FieldValue]) -> bool {
    required_field_ids(family).iter().all(|id| {
        fields
            .iter()
            .find(|field| field.id == *id)
            .is_some_and(|field| {
                !matches!(
                    field.state,
                    FieldState::Unknown | FieldState::Unsupported | FieldState::Invalid
                )
            })
    })
}

fn preflight_time_deltas(
    header: &EvidenceHeader,
    legacy: &[Observation<'_>],
    candidate: &[Observation<'_>],
) -> Result<(), CompareError> {
    let (mut i, mut j) = (0usize, 0usize);
    while i < legacy.len() || j < candidate.len() {
        while legacy
            .get(i)
            .is_some_and(|record| record.key_state != KeyState::Valid)
        {
            i = i.checked_add(1).ok_or(CompareError::RecordCap)?;
        }
        while candidate
            .get(j)
            .is_some_and(|record| record.key_state != KeyState::Valid)
        {
            j = j.checked_add(1).ok_or(CompareError::RecordCap)?;
        }
        if i >= legacy.len() && j >= candidate.len() {
            break;
        }
        let left_key = legacy.get(i).map(|record| record.key);
        let right_key = candidate.get(j).map(|record| record.key);
        let (_key, left_count, right_count) = match (left_key, right_key) {
            (Some(left), Some(right)) if key_cmp(&left, &right) == 0 => (
                left,
                side_record_count(legacy, i, &left),
                side_record_count(candidate, j, &right),
            ),
            (Some(left), Some(right)) if key_cmp(&left, &right) < 0 => {
                (left, side_record_count(legacy, i, &left), 0)
            }
            (Some(_), Some(right)) => (right, 0, side_record_count(candidate, j, &right)),
            (Some(left), None) => (left, side_record_count(legacy, i, &left), 0),
            (None, Some(right)) => (right, 0, side_record_count(candidate, j, &right)),
            (None, None) => break,
        };
        if left_count == 1 && right_count == 1 {
            let left = legacy.get(i).ok_or(CompareError::RecordCap)?;
            let right = candidate.get(j).ok_or(CompareError::RecordCap)?;
            if eligible(header, left) && eligible(header, right) {
                right
                    .source_time_ns
                    .ok_or(CompareError::CounterOverflow)?
                    .checked_sub(left.source_time_ns.ok_or(CompareError::CounterOverflow)?)
                    .ok_or(CompareError::TimeDeltaOverflow)?;
                if let (Some(left_observed), Some(right_observed)) =
                    (left.observed_time_ns, right.observed_time_ns)
                {
                    right_observed
                        .checked_sub(left_observed)
                        .ok_or(CompareError::TimeDeltaOverflow)?;
                }
            }
        }
        i = i.checked_add(left_count).ok_or(CompareError::RecordCap)?;
        j = j.checked_add(right_count).ok_or(CompareError::RecordCap)?;
    }
    Ok(())
}

fn side_record_count(records: &[Observation<'_>], start: usize, key: &BusinessKey) -> usize {
    let mut end = start;
    while let Some(record) = records.get(end) {
        if record.key_state != KeyState::Valid || key_cmp(&record.key, key) != 0 {
            break;
        }
        end = end.saturating_add(1);
    }
    end - start
}

fn event_outcome(state: EventState) -> Option<Outcome> {
    match state {
        EventState::Observed => None,
        EventState::Unknown | EventState::Unsupported | EventState::Invalid => {
            Some(Outcome::Unobservable)
        }
    }
}

fn count_primary(
    header: &EvidenceHeader,
    observation: &Observation<'_>,
    counts: &mut FamilyCounts,
    legacy_side: bool,
) -> Result<(), CompareError> {
    if legacy_side {
        bump(&mut counts.legacy_records)?;
        if observation.observed_time_ns.is_none() {
            bump(&mut counts.legacy_observed_time_missing)?;
        }
    } else {
        bump(&mut counts.candidate_records)?;
        if observation.observed_time_ns.is_none() {
            bump(&mut counts.candidate_observed_time_missing)?;
        }
    }
    for field in observation.fields {
        let counter = match (legacy_side, field.state) {
            (true, FieldState::Present) => &mut counts.legacy_fields_present,
            (false, FieldState::Present) => &mut counts.candidate_fields_present,
            (true, FieldState::Absent) => &mut counts.legacy_fields_absent,
            (false, FieldState::Absent) => &mut counts.candidate_fields_absent,
            (true, FieldState::Unknown) => &mut counts.legacy_fields_unknown,
            (false, FieldState::Unknown) => &mut counts.candidate_fields_unknown,
            (true, FieldState::Unsupported) => &mut counts.legacy_fields_unsupported,
            (false, FieldState::Unsupported) => &mut counts.candidate_fields_unsupported,
            (true, FieldState::Invalid) => &mut counts.legacy_fields_invalid,
            (false, FieldState::Invalid) => &mut counts.candidate_fields_invalid,
        };
        bump(counter)?;
    }
    match observation.key_state {
        KeyState::Valid => {}
        KeyState::Missing => {
            if legacy_side {
                bump(&mut counts.legacy_missing_key)?;
            } else {
                bump(&mut counts.candidate_missing_key)?;
            }
            return Ok(());
        }
        KeyState::Invalid => {
            if legacy_side {
                bump(&mut counts.legacy_invalid_key)?;
            } else {
                bump(&mut counts.candidate_invalid_key)?;
            }
            return Ok(());
        }
        KeyState::Ambiguous => {
            if legacy_side {
                bump(&mut counts.legacy_ambiguous_key)?;
            } else {
                bump(&mut counts.candidate_ambiguous_key)?;
            }
            return Ok(());
        }
    }
    match in_window(header, observation) {
        None if legacy_side => bump(&mut counts.legacy_missing_time)?,
        None => bump(&mut counts.candidate_missing_time)?,
        Some(false) if legacy_side => bump(&mut counts.legacy_outside_window)?,
        Some(false) => bump(&mut counts.candidate_outside_window)?,
        Some(true) => match (legacy_side, observation.event_state) {
            (true, EventState::Observed) => bump(&mut counts.legacy_eligible)?,
            (false, EventState::Observed) => bump(&mut counts.candidate_eligible)?,
            (true, EventState::Unknown) => bump(&mut counts.legacy_unknown)?,
            (false, EventState::Unknown) => bump(&mut counts.candidate_unknown)?,
            (true, EventState::Unsupported) => bump(&mut counts.legacy_unsupported)?,
            (false, EventState::Unsupported) => bump(&mut counts.candidate_unsupported)?,
            (true, EventState::Invalid) => bump(&mut counts.legacy_invalid)?,
            (false, EventState::Invalid) => bump(&mut counts.candidate_invalid)?,
        },
    }
    Ok(())
}

fn count_coverage(status: Coverage) -> bool {
    matches!(
        status,
        Coverage::Partial | Coverage::Missing | Coverage::Unsupported
    )
}

fn family_complete(header: &EvidenceHeader, index: usize) -> Result<bool, CompareError> {
    let coverage = header
        .coverage
        .get(index)
        .copied()
        .ok_or(CompareError::CoverageContradiction)?;
    Ok(matches!(
        coverage,
        Coverage::CompleteEmpty | Coverage::CompleteWithEvents
    ))
}

/// Compare two bounded normalized streams. Both streams must be sorted by valid business key;
/// keyless rows may occur anywhere and are counted/emitted separately. Output must have room for
/// the worst case (`legacy.len() + candidate.len()`) before the call starts. No input is dropped.
/// The caller must discard the output slice if an error is returned.
pub fn compare(
    legacy_header: &EvidenceHeader,
    legacy: &[Observation<'_>],
    candidate_header: &EvidenceHeader,
    candidate: &[Observation<'_>],
    output: &mut [Comparison],
) -> Result<ComparisonReport, CompareError> {
    validate_header(legacy_header)?;
    validate_header(candidate_header)?;
    if legacy_header.scope_id != candidate_header.scope_id {
        return Err(CompareError::ScopeMismatch);
    }
    if legacy_header.function_id != candidate_header.function_id {
        return Err(CompareError::FunctionMismatch);
    }
    if legacy_header.window_from_ns != candidate_header.window_from_ns
        || legacy_header.window_to_ns != candidate_header.window_to_ns
    {
        return Err(CompareError::WindowMismatch);
    }
    if legacy_header.source_time_basis != candidate_header.source_time_basis
        || legacy_header.observed_time_basis != candidate_header.observed_time_basis
    {
        return Err(CompareError::TimeBasisMismatch);
    }
    if legacy.len() > MAX_RECORDS_PER_SIDE || candidate.len() > MAX_RECORDS_PER_SIDE {
        return Err(CompareError::RecordCap);
    }
    let worst = legacy
        .len()
        .checked_add(candidate.len())
        .ok_or(CompareError::RecordCap)?;
    if worst > output.len() {
        return Err(CompareError::OutputCap);
    }
    validate_side(legacy_header, legacy)?;
    validate_side(candidate_header, candidate)?;
    validate_coverage(legacy_header, legacy)?;
    validate_coverage(candidate_header, candidate)?;
    preflight_time_deltas(legacy_header, legacy, candidate)?;
    let mut families = [FamilyCounts::default(); FAMILY_COUNT];
    let mut out_of_order_side_families = 0u8;
    let mut coverage_incomplete_families = 0u8;
    let mut no_event_families = 0u8;
    let mut output_len = 0usize;

    // Record-level denominators are counted in one bounded linear pass per side.
    for (header, records, legacy_side) in [
        (legacy_header, legacy, true),
        (candidate_header, candidate, false),
    ] {
        let mut previous_entity: Option<([u8; 32], Family, Option<i64>)> = None;
        for observation in records {
            let index = family_index(observation.key.family);
            let counts = families
                .get_mut(index)
                .ok_or(CompareError::CounterOverflow)?;
            count_primary(header, observation, counts, legacy_side)?;
            if observation.key_state != KeyState::Valid {
                let row = Comparison {
                    key: None,
                    family: observation.key.family,
                    legacy: if legacy_side {
                        Some(observation.source)
                    } else {
                        None
                    },
                    candidate: if legacy_side {
                        None
                    } else {
                        Some(observation.source)
                    },
                    legacy_count: if legacy_side { 1 } else { 0 },
                    candidate_count: if legacy_side { 0 } else { 1 },
                    outcome: match observation.key_state {
                        KeyState::Missing => Outcome::MissingKey,
                        KeyState::Invalid => Outcome::InvalidKey,
                        KeyState::Ambiguous => Outcome::AmbiguousKey,
                        KeyState::Valid => return Err(CompareError::InvalidBusinessKey),
                    },
                    field_differences: 0,
                    field_unobservable: 0,
                    field_issue_count: 0,
                    field_issues: [EMPTY_FIELD_ISSUE; MAX_COMPARISON_FIELD_ISSUES],
                    source_time_delta_ns: None,
                    observed_time_delta_ns: None,
                };
                let slot = output.get_mut(output_len).ok_or(CompareError::OutputCap)?;
                *slot = row;
                output_len = output_len.checked_add(1).ok_or(CompareError::OutputCap)?;
            }
            if observation.key_state == KeyState::Valid
                && in_window(header, observation) == Some(true)
            {
                if let Some((entity, family, prior_time)) = previous_entity {
                    if entity == observation.key.entity_key
                        && family == observation.key.family
                        && observation
                            .source_time_ns
                            .zip(prior_time)
                            .is_some_and(|(current, prior)| current < prior)
                    {
                        if legacy_side {
                            bump(&mut counts.legacy_out_of_order)?;
                        } else {
                            bump(&mut counts.candidate_out_of_order)?;
                        }
                    }
                }
                previous_entity = Some((
                    observation.key.entity_key,
                    observation.key.family,
                    observation.source_time_ns,
                ));
            }
        }
    }

    let (mut i, mut j) = (0usize, 0usize);
    while i < legacy.len() || j < candidate.len() {
        while legacy
            .get(i)
            .is_some_and(|record| record.key_state != KeyState::Valid)
        {
            i = i.saturating_add(1);
        }
        while candidate
            .get(j)
            .is_some_and(|record| record.key_state != KeyState::Valid)
        {
            j = j.saturating_add(1);
        }
        if i >= legacy.len() && j >= candidate.len() {
            break;
        }
        let left_key = legacy.get(i).map(|record| record.key);
        let right_key = candidate.get(j).map(|record| record.key);
        let (key, left_count, right_count) = match (left_key, right_key) {
            (Some(left), Some(right)) if key_cmp(&left, &right) == 0 => {
                let left_count = side_record_count(legacy, i, &left);
                let right_count = side_record_count(candidate, j, &right);
                (left, left_count, right_count)
            }
            (Some(left), Some(right)) if key_cmp(&left, &right) < 0 => {
                (left, side_record_count(legacy, i, &left), 0)
            }
            (Some(_), Some(right)) => (right, 0, side_record_count(candidate, j, &right)),
            (Some(left), None) => (left, side_record_count(legacy, i, &left), 0),
            (None, Some(right)) => (right, 0, side_record_count(candidate, j, &right)),
            (None, None) => break,
        };
        let left = if left_count == 1 {
            legacy.get(i).copied()
        } else {
            None
        };
        let right = if right_count == 1 {
            candidate.get(j).copied()
        } else {
            None
        };
        let index = family_index(key.family);
        let counts = families
            .get_mut(index)
            .ok_or(CompareError::CounterOverflow)?;
        bump(&mut counts.key_groups)?;
        let is_duplicate = left_count > 1 || right_count > 1;
        if is_duplicate {
            bump(&mut counts.duplicate_groups)?;
            add_count(&mut counts.duplicate_legacy_records, left_count)?;
            add_count(&mut counts.duplicate_candidate_records, right_count)?;
            for offset in 0..left_count {
                let source_index = i.checked_add(offset).ok_or(CompareError::RecordCap)?;
                let source = legacy
                    .get(source_index)
                    .ok_or(CompareError::RecordCap)?
                    .source;
                let slot = output.get_mut(output_len).ok_or(CompareError::OutputCap)?;
                *slot = Comparison {
                    key: Some(key),
                    family: key.family,
                    legacy: Some(source),
                    candidate: None,
                    legacy_count: u32::try_from(left_count).map_err(|_| CompareError::RecordCap)?,
                    candidate_count: u32::try_from(right_count)
                        .map_err(|_| CompareError::RecordCap)?,
                    outcome: Outcome::DuplicateAmbiguous,
                    field_differences: 0,
                    field_unobservable: 0,
                    field_issue_count: 0,
                    field_issues: [EMPTY_FIELD_ISSUE; MAX_COMPARISON_FIELD_ISSUES],
                    source_time_delta_ns: None,
                    observed_time_delta_ns: None,
                };
                output_len = output_len.checked_add(1).ok_or(CompareError::OutputCap)?;
            }
            for offset in 0..right_count {
                let source_index = j.checked_add(offset).ok_or(CompareError::RecordCap)?;
                let source = candidate
                    .get(source_index)
                    .ok_or(CompareError::RecordCap)?
                    .source;
                let slot = output.get_mut(output_len).ok_or(CompareError::OutputCap)?;
                *slot = Comparison {
                    key: Some(key),
                    family: key.family,
                    legacy: None,
                    candidate: Some(source),
                    legacy_count: u32::try_from(left_count).map_err(|_| CompareError::RecordCap)?,
                    candidate_count: u32::try_from(right_count)
                        .map_err(|_| CompareError::RecordCap)?,
                    outcome: Outcome::DuplicateAmbiguous,
                    field_differences: 0,
                    field_unobservable: 0,
                    field_issue_count: 0,
                    field_issues: [EMPTY_FIELD_ISSUE; MAX_COMPARISON_FIELD_ISSUES],
                    source_time_delta_ns: None,
                    observed_time_delta_ns: None,
                };
                output_len = output_len.checked_add(1).ok_or(CompareError::OutputCap)?;
            }
            i = i.checked_add(left_count).ok_or(CompareError::RecordCap)?;
            j = j.checked_add(right_count).ok_or(CompareError::RecordCap)?;
            continue;
        }
        let left_ok = left.is_some_and(|event| eligible(legacy_header, &event));
        let right_ok = right.is_some_and(|event| eligible(candidate_header, &event));
        let mut field_differences = 0u16;
        let mut field_unobservable = 0u16;
        let mut field_issue_count = 0u8;
        let mut field_issues = [EMPTY_FIELD_ISSUE; MAX_COMPARISON_FIELD_ISSUES];
        let mut source_delta = None;
        let mut observed_delta = None;
        let outcome = if left.is_some() && right.is_some() && left_ok && right_ok {
            let l = left.ok_or(CompareError::CounterOverflow)?;
            let r = right.ok_or(CompareError::CounterOverflow)?;
            let ltime = l.source_time_ns.ok_or(CompareError::CounterOverflow)?;
            let rtime = r.source_time_ns.ok_or(CompareError::CounterOverflow)?;
            source_delta = Some(
                rtime
                    .checked_sub(ltime)
                    .ok_or(CompareError::TimeDeltaOverflow)?,
            );
            if l.observed_time_ns.is_some() && r.observed_time_ns.is_some() {
                observed_delta = Some(
                    r.observed_time_ns
                        .ok_or(CompareError::CounterOverflow)?
                        .checked_sub(l.observed_time_ns.ok_or(CompareError::CounterOverflow)?)
                        .ok_or(CompareError::TimeDeltaOverflow)?,
                );
            }
            if event_outcome(l.event_state).is_some() || event_outcome(r.event_state).is_some() {
                bump(&mut counts.unobservable_groups)?;
                Outcome::Unobservable
            } else {
                (
                    field_differences,
                    field_unobservable,
                    field_issue_count,
                    field_issues,
                ) = field_compare(key.family, l.fields, r.fields)?;
                counts.field_differences = counts
                    .field_differences
                    .checked_add(u64::from(field_differences))
                    .ok_or(CompareError::CounterOverflow)?;
                counts.field_unobservable = counts
                    .field_unobservable
                    .checked_add(u64::from(field_unobservable))
                    .ok_or(CompareError::CounterOverflow)?;
                if source_delta != Some(0) {
                    bump(&mut counts.source_time_differences)?;
                }
                if observed_delta.is_some_and(|delta| delta != 0) {
                    bump(&mut counts.observed_time_differences)?;
                }
                if field_unobservable > 0 || observed_delta.is_none() {
                    bump(&mut counts.unobservable_groups)?;
                    Outcome::Unobservable
                } else if field_differences > 0
                    || source_delta != Some(0)
                    || observed_delta != Some(0)
                {
                    bump(&mut counts.different_pairs)?;
                    Outcome::Different
                } else {
                    bump(&mut counts.exact_pairs)?;
                    Outcome::Equivalent
                }
            }
        } else if left_ok && right.is_none() {
            if family_complete(candidate_header, index)? {
                bump(&mut counts.legacy_only)?;
                Outcome::LegacyOnly
            } else {
                bump(&mut counts.unobservable_groups)?;
                Outcome::Unobservable
            }
        } else if right_ok && left.is_none() {
            if family_complete(legacy_header, index)? {
                bump(&mut counts.candidate_only)?;
                Outcome::CandidateOnly
            } else {
                bump(&mut counts.unobservable_groups)?;
                Outcome::Unobservable
            }
        } else if left.is_some() && right.is_some() {
            if !left_ok
                && !right_ok
                && in_window(legacy_header, &left.ok_or(CompareError::CounterOverflow)?)
                    == Some(false)
                && in_window(
                    candidate_header,
                    &right.ok_or(CompareError::CounterOverflow)?,
                ) == Some(false)
            {
                bump(&mut counts.outside_only_groups)?;
                Outcome::OutsideWindow
            } else {
                bump(&mut counts.unobservable_groups)?;
                Outcome::Unobservable
            }
        } else {
            let existing = left.or(right).ok_or(CompareError::CounterOverflow)?;
            if in_window(
                if left.is_some() {
                    legacy_header
                } else {
                    candidate_header
                },
                &existing,
            ) == Some(false)
            {
                bump(&mut counts.outside_only_groups)?;
                Outcome::OutsideWindow
            } else if event_outcome(existing.event_state).is_some()
                || existing.source_time_ns.is_none()
                || !has_complete_projection(existing.key.family, existing.fields)
            {
                bump(&mut counts.unobservable_groups)?;
                Outcome::Unobservable
            } else {
                let counterpart = if left.is_some() {
                    candidate_header
                } else {
                    legacy_header
                };
                if family_complete(counterpart, index)? {
                    if left.is_some() {
                        bump(&mut counts.legacy_only)?;
                        Outcome::LegacyOnly
                    } else {
                        bump(&mut counts.candidate_only)?;
                        Outcome::CandidateOnly
                    }
                } else {
                    bump(&mut counts.unobservable_groups)?;
                    Outcome::Unobservable
                }
            }
        };
        let slot = output.get_mut(output_len).ok_or(CompareError::OutputCap)?;
        *slot = Comparison {
            key: Some(key),
            family: key.family,
            legacy: if left_count == 1 {
                left.map(|event| event.source)
            } else {
                None
            },
            candidate: if right_count == 1 {
                right.map(|event| event.source)
            } else {
                None
            },
            legacy_count: u32::try_from(left_count).map_err(|_| CompareError::RecordCap)?,
            candidate_count: u32::try_from(right_count).map_err(|_| CompareError::RecordCap)?,
            outcome,
            field_differences,
            field_unobservable,
            field_issue_count,
            field_issues,
            source_time_delta_ns: source_delta,
            observed_time_delta_ns: observed_delta,
        };
        output_len = output_len.checked_add(1).ok_or(CompareError::OutputCap)?;
        i = i.checked_add(left_count).ok_or(CompareError::RecordCap)?;
        j = j.checked_add(right_count).ok_or(CompareError::RecordCap)?;
    }

    for (index, family_counts) in families.iter().enumerate() {
        let left_coverage = legacy_header
            .coverage
            .get(index)
            .copied()
            .ok_or(CompareError::CoverageContradiction)?;
        let right_coverage = candidate_header
            .coverage
            .get(index)
            .copied()
            .ok_or(CompareError::CoverageContradiction)?;
        if count_coverage(left_coverage) || count_coverage(right_coverage) {
            coverage_incomplete_families |= 1 << index;
        }
        if family_counts.legacy_out_of_order > 0 || family_counts.candidate_out_of_order > 0 {
            out_of_order_side_families |= 1 << index;
        }
        if left_coverage == Coverage::CompleteEmpty && right_coverage == Coverage::CompleteEmpty {
            no_event_families |= 1 << index;
        }
    }
    let report = ComparisonReport {
        policy_version: POLICY_VERSION,
        function_id: legacy_header.function_id,
        scope_id: legacy_header.scope_id,
        window_from_ns: legacy_header.window_from_ns,
        window_to_ns: legacy_header.window_to_ns,
        legacy_epoch_sha256: legacy_header.epoch_sha256,
        candidate_epoch_sha256: candidate_header.epoch_sha256,
        legacy_coverage: legacy_header.coverage,
        candidate_coverage: candidate_header.coverage,
        families,
        comparison_rows: output_len,
        out_of_order_side_families,
        coverage_incomplete_families,
        no_event_families,
    };
    validate_conservation(&report)?;
    Ok(report)
}

/// Verify that every source record has exactly one primary disposition and every keyed group has
/// exactly one comparison disposition. Field, observed-clock, and order counts are nonexclusive.
pub fn validate_conservation(report: &ComparisonReport) -> Result<(), CompareError> {
    let mut expected_rows = 0u64;
    for counts in &report.families {
        for (records, categories) in [
            (
                counts.legacy_records,
                [
                    counts.legacy_missing_key,
                    counts.legacy_invalid_key,
                    counts.legacy_ambiguous_key,
                    counts.legacy_missing_time,
                    counts.legacy_outside_window,
                    counts.legacy_unknown,
                    counts.legacy_unsupported,
                    counts.legacy_invalid,
                    counts.legacy_eligible,
                ],
            ),
            (
                counts.candidate_records,
                [
                    counts.candidate_missing_key,
                    counts.candidate_invalid_key,
                    counts.candidate_ambiguous_key,
                    counts.candidate_missing_time,
                    counts.candidate_outside_window,
                    counts.candidate_unknown,
                    counts.candidate_unsupported,
                    counts.candidate_invalid,
                    counts.candidate_eligible,
                ],
            ),
        ] {
            let mut sum = 0u64;
            for count in categories {
                sum = sum
                    .checked_add(count)
                    .ok_or(CompareError::CounterOverflow)?;
            }
            if sum != records {
                return Err(CompareError::ConservationFailure);
            }
        }
        let group_sum = [
            counts.exact_pairs,
            counts.different_pairs,
            counts.legacy_only,
            counts.candidate_only,
            counts.duplicate_groups,
            counts.unobservable_groups,
            counts.outside_only_groups,
        ]
        .into_iter()
        .try_fold(0u64, |sum, count| sum.checked_add(count))
        .ok_or(CompareError::CounterOverflow)?;
        if group_sum != counts.key_groups || counts.duplicate_groups > counts.key_groups {
            return Err(CompareError::ConservationFailure);
        }
        let keyless = counts
            .legacy_missing_key
            .checked_add(counts.legacy_invalid_key)
            .and_then(|sum| sum.checked_add(counts.legacy_ambiguous_key))
            .and_then(|sum| sum.checked_add(counts.candidate_missing_key))
            .and_then(|sum| sum.checked_add(counts.candidate_invalid_key))
            .and_then(|sum| sum.checked_add(counts.candidate_ambiguous_key))
            .ok_or(CompareError::CounterOverflow)?;
        let nonduplicate_groups = counts
            .key_groups
            .checked_sub(counts.duplicate_groups)
            .ok_or(CompareError::CounterOverflow)?;
        let duplicate_rows = counts
            .duplicate_legacy_records
            .checked_add(counts.duplicate_candidate_records)
            .ok_or(CompareError::CounterOverflow)?;
        expected_rows = expected_rows
            .checked_add(keyless)
            .and_then(|sum| sum.checked_add(nonduplicate_groups))
            .and_then(|sum| sum.checked_add(duplicate_rows))
            .ok_or(CompareError::CounterOverflow)?;
    }
    let written =
        u64::try_from(report.comparison_rows).map_err(|_| CompareError::CounterOverflow)?;
    if expected_rows != written {
        return Err(CompareError::ConservationFailure);
    }
    Ok(())
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewRole {
    OperationsEvaluator = 1,
    SafetyReviewer = 2,
    IndependentAssessor = 3,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgment {
    Better = 1,
    Worse = 2,
}

/// A review applies to one exact comparison row, source epochs, policy/scope/window, and must
/// bind a reviewer role and evidence digest. The digest identifies an attached evidence artifact
/// that the outer evidence-package verifier must resolve. It cannot alter raw comparisons or
/// their denominators.
// A7.2: offline review annotation, size guard waived.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReviewDecision {
    pub policy_version: u16,
    pub function_id: u32,
    pub scope_id: u32,
    pub window_from_ns: i64,
    pub window_to_ns: i64,
    pub legacy_epoch_sha256: [u8; 32],
    pub candidate_epoch_sha256: [u8; 32],
    pub key: Option<BusinessKey>,
    pub family: Family,
    pub legacy: Option<SourceIdentity>,
    pub candidate: Option<SourceIdentity>,
    pub reviewer_role: ReviewRole,
    pub evidence_sha256: [u8; 32],
    pub judgment: Judgment,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Equivalent = 1,
    BetterReviewed = 2,
    WorseReviewed = 3,
    Unresolved = 4,
}

pub fn classify(
    report: &ComparisonReport,
    comparison: &Comparison,
    review: Option<&ReviewDecision>,
) -> Classification {
    if comparison.outcome == Outcome::Equivalent {
        return Classification::Equivalent;
    }
    let Some(review) = review else {
        return Classification::Unresolved;
    };
    let matches_row = review.policy_version == report.policy_version
        && review.function_id == report.function_id
        && review.scope_id == report.scope_id
        && review.window_from_ns == report.window_from_ns
        && review.window_to_ns == report.window_to_ns
        && review.legacy_epoch_sha256 == report.legacy_epoch_sha256
        && review.candidate_epoch_sha256 == report.candidate_epoch_sha256
        && review.family == comparison.family
        && review.key == comparison.key
        && review.legacy == comparison.legacy
        && review.candidate == comparison.candidate
        && !zero_bytes(&review.evidence_sha256);
    let reviewable = matches!(
        comparison.outcome,
        Outcome::Different | Outcome::LegacyOnly | Outcome::CandidateOnly
    );
    if !matches_row || !reviewable {
        return Classification::Unresolved;
    }
    match review.judgment {
        Judgment::Better => Classification::BetterReviewed,
        Judgment::Worse => Classification::WorseReviewed,
    }
}

pub const fn blocks(classification: Classification) -> bool {
    matches!(
        classification,
        Classification::WorseReviewed | Classification::Unresolved
    )
}

/// A profile cannot clear an incomplete family or out-of-order source stream, even when an
/// individual pair is exact. Review decisions only classify the exact pair; they do not repair
/// source coverage or ordering.
pub const fn report_blocks(report: &ComparisonReport, classification: Classification) -> bool {
    blocks(classification)
        || report.coverage_incomplete_families != 0
        || report.out_of_order_side_families != 0
        || report.no_event_families != 0
}

/// Per-family gate used when an evidence unit scopes only one H2 function family.
pub const fn family_blocks(
    report: &ComparisonReport,
    family: Family,
    classification: Classification,
) -> bool {
    let bit = 1 << family_index(family);
    blocks(classification)
        || report.coverage_incomplete_families & bit != 0
        || report.out_of_order_side_families & bit != 0
        || report.no_event_families & bit != 0
}
