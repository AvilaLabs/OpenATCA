//! Same-input published-value amendment propagation accounting for the offline H2 runner.
//!
//! This compares changes between consecutive decoded SFDPS publications for one exact GUFI with
//! the actual candidate flight-table values captured at those same two receive-time observations.
//! It measures propagation of source values only. It does not measure an independent controller
//! decision, invent an amendment event identity, or classify any difference as better or worse.
//! Class: offline shell; fixed-capacity previous-value inventory and aggregate counters only.

use crate::migration_snapshot::{
    CanonicalValue, ComparedField, FieldComparison, FieldOutcome, RowComparison, RowIdentityState,
};
use oa_flight_sfdps::migration_projection::{SourceField, SourceFieldState};

pub const MAX_AMENDMENT_BASELINES: usize = 250_000;
pub const AMENDMENT_FIELD_COUNT: usize = 9;
pub const AMENDMENT_BASELINE_BYTES: usize = 720;
pub const MAX_LOOKUP_PROBES: usize = 64;

const AMENDMENT_FIELDS: [ComparedField; AMENDMENT_FIELD_COUNT] = [
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmendmentFieldCounts {
    pub rows_seen: u64,
    pub source_present: u64,
    pub source_absent: u64,
    pub source_malformed: u64,
    pub source_unsupported: u64,
    pub first_present_value: u64,
    pub source_unchanged: u64,
    pub source_changed: u64,
    pub propagated_equivalent: u64,
    pub different_unresolved: u64,
    pub candidate_unavailable: u64,
    pub candidate_not_yet_available: u64,
    pub candidate_not_modeled: u64,
    pub identity_mismatch: u64,
    pub clock_ambiguous: u64,
}
const _: () = assert!(core::mem::size_of::<AmendmentFieldCounts>() == 120);

impl AmendmentFieldCounts {
    pub fn reconciles(&self) -> bool {
        self.source_present
            .checked_add(self.source_absent)
            .and_then(|n| n.checked_add(self.source_malformed))
            .and_then(|n| n.checked_add(self.source_unsupported))
            == Some(self.rows_seen)
            && self
                .first_present_value
                .checked_add(self.source_unchanged)
                .and_then(|n| n.checked_add(self.source_changed))
                == Some(self.source_present)
            && self
                .propagated_equivalent
                .checked_add(self.different_unresolved)
                .and_then(|n| n.checked_add(self.candidate_unavailable))
                .and_then(|n| n.checked_add(self.candidate_not_yet_available))
                .and_then(|n| n.checked_add(self.candidate_not_modeled))
                .and_then(|n| n.checked_add(self.identity_mismatch))
                .and_then(|n| n.checked_add(self.clock_ambiguous))
                == Some(self.source_changed)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmendmentSummary {
    pub source_rows_seen: u64,
    pub source_rows_with_changes: u64,
    pub missing_gufi_rows: u64,
    pub source_clock_regression_rows: u64,
    pub baseline_entries: u64,
    pub fields: [AmendmentFieldCounts; AMENDMENT_FIELD_COUNT],
}
const _: () = assert!(core::mem::size_of::<AmendmentSummary>() == 1120);

impl AmendmentSummary {
    pub fn reconciles(&self) -> bool {
        self.fields.iter().all(AmendmentFieldCounts::reconciles)
            && self.baseline_entries <= self.source_rows_seen
            && self.source_rows_with_changes <= self.source_rows_seen
            && self.missing_gufi_rows <= self.source_rows_seen
            && self.source_clock_regression_rows <= self.source_rows_seen
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AmendmentValue {
    source_state: SourceFieldState,
    outcome: FieldOutcome,
    source: CanonicalValue,
    candidate: CanonicalValue,
}
const EMPTY_VALUE: AmendmentValue = AmendmentValue {
    source_state: SourceFieldState::Absent,
    outcome: FieldOutcome::SourceAbsent,
    source: CanonicalValue {
        present: 0,
        kind: 0,
        len: 0,
        bytes: [0; 32],
    },
    candidate: CanonicalValue {
        present: 0,
        kind: 0,
        len: 0,
        bytes: [0; 32],
    },
};
const _: () = assert!(core::mem::size_of::<AmendmentValue>() == 72);

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AmendmentBaseline {
    gufi: u128,
    source_time_ms: i64,
    receive_time_ms: i64,
    identity_state: RowIdentityState,
    source_stream: u32,
    source_offset: u64,
    source_sequence: u64,
    source_row: u64,
    fields: [AmendmentValue; AMENDMENT_FIELD_COUNT],
}
const _: () = assert!(core::mem::size_of::<AmendmentBaseline>() == 720);
const _: () = assert!(core::mem::size_of::<AmendmentBaseline>() == AMENDMENT_BASELINE_BYTES);

#[derive(Debug)]
pub enum AmendmentError {
    InvalidComparison,
    CounterOverflow,
    BaselineCapacity,
    LookupProbeCap,
    AllocationRefused,
}

/// A3 offline exception: compact latest-row records plus a fixed open-addressed index retain the
/// nine source/candidate values for exact per-GUFI pairing. The explicit ceiling bounds records
/// at 250,000 (180,000,000 bytes) and the index at 524,288 u32 slots (2,097,152 bytes).
pub struct AmendmentAccumulator {
    baselines: Vec<AmendmentBaseline>,
    slots: Vec<u32>,
    summary: AmendmentSummary,
}

/// Creates the fixed-capacity, sorted previous-value inventory.
pub fn amendment_accumulator_new() -> Result<AmendmentAccumulator, AmendmentError> {
    amendment_accumulator_new_with_capacity(MAX_AMENDMENT_BASELINES)
}

pub fn amendment_accumulator_new_with_capacity(
    capacity: usize,
) -> Result<AmendmentAccumulator, AmendmentError> {
    if capacity == 0 || capacity > MAX_AMENDMENT_BASELINES {
        return Err(AmendmentError::BaselineCapacity);
    }
    let mut baselines = Vec::new();
    baselines
        .try_reserve_exact(capacity)
        .map_err(|_| AmendmentError::AllocationRefused)?;
    let slot_count = capacity
        .checked_mul(2)
        .and_then(usize::checked_next_power_of_two)
        .ok_or(AmendmentError::BaselineCapacity)?;
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(slot_count)
        .map_err(|_| AmendmentError::AllocationRefused)?;
    slots.resize(slot_count, 0);
    Ok(AmendmentAccumulator {
        baselines,
        slots,
        summary: AmendmentSummary::default(),
    })
}

fn hash_gufi(gufi: u128) -> usize {
    let low = gufi as u64;
    let high = (gufi >> 64) as u64;
    let mut value = low ^ high.rotate_left(29);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    (value ^ (value >> 31)) as usize
}

fn lookup_slot(
    accumulator: &AmendmentAccumulator,
    gufi: u128,
) -> Result<(usize, Option<usize>), AmendmentError> {
    let mask = accumulator
        .slots
        .len()
        .checked_sub(1)
        .ok_or(AmendmentError::BaselineCapacity)?;
    if !accumulator.slots.len().is_power_of_two() {
        return Err(AmendmentError::BaselineCapacity);
    }
    let start = hash_gufi(gufi) & mask;
    for probe in 0..MAX_LOOKUP_PROBES.min(accumulator.slots.len()) {
        let slot = start.wrapping_add(probe) & mask;
        let index_plus_one = *accumulator
            .slots
            .get(slot)
            .ok_or(AmendmentError::BaselineCapacity)?;
        if index_plus_one == 0 {
            return Ok((slot, None));
        }
        let index =
            usize::try_from(index_plus_one - 1).map_err(|_| AmendmentError::BaselineCapacity)?;
        if accumulator
            .baselines
            .get(index)
            .map(|baseline| baseline.gufi)
            == Some(gufi)
        {
            return Ok((slot, Some(index)));
        }
    }
    Err(AmendmentError::LookupProbeCap)
}

fn field_index(field: ComparedField) -> Option<usize> {
    AMENDMENT_FIELDS
        .iter()
        .position(|candidate| *candidate == field)
}

fn next(counter: &mut u64) -> Result<(), AmendmentError> {
    *counter = counter
        .checked_add(1)
        .ok_or(AmendmentError::CounterOverflow)?;
    Ok(())
}

fn find_field(
    comparison: &RowComparison,
    field: ComparedField,
) -> Result<FieldComparison, AmendmentError> {
    let len = usize::from(comparison.field_count);
    if len > comparison.fields.len() {
        return Err(AmendmentError::InvalidComparison);
    }
    comparison
        .fields
        .iter()
        .take(len)
        .find(|item| item.field == field)
        .copied()
        .ok_or(AmendmentError::InvalidComparison)
}

fn field_value(comparison: &FieldComparison) -> AmendmentValue {
    AmendmentValue {
        source_state: comparison.source_state,
        outcome: comparison.outcome,
        source: comparison.source_value,
        candidate: comparison.candidate_value,
    }
}

fn source_value_with_route_digest(
    field: ComparedField,
    mut value: AmendmentValue,
    route: SourceField<&str>,
) -> Result<AmendmentValue, AmendmentError> {
    if field != ComparedField::Route || value.source_state != SourceFieldState::Present {
        return Ok(value);
    }
    let text = route.value.ok_or(AmendmentError::InvalidComparison)?;
    if route.state != SourceFieldState::Present {
        return Err(AmendmentError::InvalidComparison);
    }
    value.source = CanonicalValue {
        present: 1,
        kind: 4,
        len: 32,
        bytes: oa_sha256::sha256(text.as_bytes()),
    };
    value.outcome = FieldOutcome::CandidateNotModeled;
    value.candidate = CanonicalValue::default();
    Ok(value)
}

fn classify_change(
    counts: &mut AmendmentFieldCounts,
    previous: AmendmentValue,
    current: AmendmentValue,
    previous_identity: RowIdentityState,
    current_identity: RowIdentityState,
    clock_ambiguous: bool,
) -> Result<(), AmendmentError> {
    if clock_ambiguous {
        return next(&mut counts.clock_ambiguous);
    }
    if matches!(previous.outcome, FieldOutcome::CandidateUnavailable)
        || matches!(current.outcome, FieldOutcome::CandidateUnavailable)
    {
        return next(&mut counts.candidate_unavailable);
    }
    if matches!(previous.outcome, FieldOutcome::CandidateNotYetAvailable)
        || matches!(current.outcome, FieldOutcome::CandidateNotYetAvailable)
    {
        return next(&mut counts.candidate_not_yet_available);
    }
    if previous_identity != RowIdentityState::MatchedGufi
        || current_identity != RowIdentityState::MatchedGufi
        || matches!(previous.outcome, FieldOutcome::IdentityMismatch)
        || matches!(current.outcome, FieldOutcome::IdentityMismatch)
    {
        return next(&mut counts.identity_mismatch);
    }
    if matches!(previous.outcome, FieldOutcome::CandidateNotModeled)
        || matches!(current.outcome, FieldOutcome::CandidateNotModeled)
        || matches!(previous.outcome, FieldOutcome::SourceUnsupported)
        || matches!(current.outcome, FieldOutcome::SourceUnsupported)
    {
        return next(&mut counts.candidate_not_modeled);
    }
    if previous.candidate.present != 0
        && current.candidate.present != 0
        && previous.candidate == previous.source
        && current.candidate == current.source
    {
        return next(&mut counts.propagated_equivalent);
    }
    next(&mut counts.different_unresolved)
}

/// Adds one decoded SFDPS row and its candidate as-of snapshot. Only exact positive GUFI links
/// are admitted. Values are paired with the previous received publication for that GUFI; no
/// message-order ordinal or time-derived business key is created.
pub fn amendment_observe(
    accumulator: &mut AmendmentAccumulator,
    comparison: &RowComparison,
    source_gufi: u128,
    source_time_ms: i64,
    receive_time_ms: i64,
    route: SourceField<&str>,
) -> Result<(), AmendmentError> {
    if comparison.source_gufi != source_gufi {
        return Err(AmendmentError::InvalidComparison);
    }
    next(&mut accumulator.summary.source_rows_seen)?;
    if source_gufi == 0 {
        next(&mut accumulator.summary.missing_gufi_rows)?;
    }
    let mut current_fields = [EMPTY_VALUE; AMENDMENT_FIELD_COUNT];
    let row = comparison.source_row;
    let source_row = u64::try_from(row.row).map_err(|_| AmendmentError::InvalidComparison)?;
    for field in AMENDMENT_FIELDS {
        let current_comparison = find_field(comparison, field)?;
        let slot = field_index(field).ok_or(AmendmentError::InvalidComparison)?;
        let current =
            source_value_with_route_digest(field, field_value(&current_comparison), route)?;
        *current_fields
            .get_mut(slot)
            .ok_or(AmendmentError::InvalidComparison)? = current;
        let counts = accumulator
            .summary
            .fields
            .get_mut(slot)
            .ok_or(AmendmentError::InvalidComparison)?;
        next(&mut counts.rows_seen)?;
        match current.source_state {
            SourceFieldState::Absent => next(&mut counts.source_absent)?,
            SourceFieldState::Malformed => next(&mut counts.source_malformed)?,
            SourceFieldState::Present
                if matches!(current.outcome, FieldOutcome::SourceUnsupported) =>
            {
                next(&mut counts.source_unsupported)?
            }
            SourceFieldState::Present => next(&mut counts.source_present)?,
        }
    }
    if source_gufi == 0 {
        for (slot, current) in current_fields.iter().enumerate() {
            if current.source_state == SourceFieldState::Present
                && current.outcome != FieldOutcome::SourceUnsupported
            {
                let counts = accumulator
                    .summary
                    .fields
                    .get_mut(slot)
                    .ok_or(AmendmentError::InvalidComparison)?;
                next(&mut counts.first_present_value)?;
            }
        }
        if !accumulator.summary.reconciles() {
            return Err(AmendmentError::InvalidComparison);
        }
        return Ok(());
    }
    let gufi = source_gufi;
    let (hash_slot, found) = lookup_slot(accumulator, gufi)?;
    let is_new = found.is_none();
    let baseline_index = match found {
        Some(index) => index,
        None => {
            if accumulator.baselines.len() >= MAX_AMENDMENT_BASELINES {
                return Err(AmendmentError::BaselineCapacity);
            }
            accumulator.baselines.len()
        }
    };
    let mut changed_row = false;
    if !is_new {
        let previous = accumulator
            .baselines
            .get(baseline_index)
            .copied()
            .ok_or(AmendmentError::InvalidComparison)?;
        let duplicate_identity = row.stream == previous.source_stream
            && row.decoded_offset == previous.source_offset
            && row.recorder_sequence == previous.source_sequence
            && source_row == previous.source_row;
        if duplicate_identity {
            return Err(AmendmentError::InvalidComparison);
        }
        let clock_ambiguous =
            receive_time_ms < previous.receive_time_ms || source_time_ms < previous.source_time_ms;
        if source_time_ms < previous.source_time_ms || clock_ambiguous {
            next(&mut accumulator.summary.source_clock_regression_rows)?;
        }
        for (slot, current) in current_fields.iter().copied().enumerate() {
            let previous_value = *previous
                .fields
                .get(slot)
                .ok_or(AmendmentError::InvalidComparison)?;
            let counts = accumulator
                .summary
                .fields
                .get_mut(slot)
                .ok_or(AmendmentError::InvalidComparison)?;
            if current.source_state == SourceFieldState::Present
                && previous_value.source_state == SourceFieldState::Present
                && current.outcome != FieldOutcome::SourceUnsupported
                && previous_value.outcome != FieldOutcome::SourceUnsupported
            {
                if current.source == previous_value.source {
                    next(&mut counts.source_unchanged)?;
                } else {
                    next(&mut counts.source_changed)?;
                    changed_row = true;
                    classify_change(
                        counts,
                        previous_value,
                        current,
                        previous.identity_state,
                        comparison.identity_state,
                        clock_ambiguous,
                    )?;
                }
            } else if current.source_state == SourceFieldState::Present
                && current.outcome != FieldOutcome::SourceUnsupported
            {
                next(&mut counts.first_present_value)?;
            }
        }
    } else {
        for (slot, current) in current_fields.iter().enumerate() {
            if current.source_state == SourceFieldState::Present
                && current.outcome != FieldOutcome::SourceUnsupported
            {
                let counts = accumulator
                    .summary
                    .fields
                    .get_mut(slot)
                    .ok_or(AmendmentError::InvalidComparison)?;
                next(&mut counts.first_present_value)?;
            }
        }
    }
    if changed_row {
        next(&mut accumulator.summary.source_rows_with_changes)?;
    }
    let current = AmendmentBaseline {
        gufi,
        source_time_ms,
        receive_time_ms,
        identity_state: comparison.identity_state,
        source_stream: row.stream,
        source_offset: row.decoded_offset,
        source_sequence: row.recorder_sequence,
        source_row,
        fields: current_fields,
    };
    if is_new {
        accumulator.baselines.push(current);
        let index_plus_one = u32::try_from(baseline_index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or(AmendmentError::BaselineCapacity)?;
        let slot = accumulator
            .slots
            .get_mut(hash_slot)
            .ok_or(AmendmentError::BaselineCapacity)?;
        *slot = index_plus_one;
    } else {
        let target = accumulator
            .baselines
            .get_mut(baseline_index)
            .ok_or(AmendmentError::InvalidComparison)?;
        *target = current;
    }
    accumulator.summary.baseline_entries =
        u64::try_from(accumulator.baselines.len()).map_err(|_| AmendmentError::CounterOverflow)?;
    if !accumulator.summary.reconciles() {
        return Err(AmendmentError::InvalidComparison);
    }
    Ok(())
}

pub fn amendment_summary(accumulator: &AmendmentAccumulator) -> AmendmentSummary {
    accumulator.summary
}

#[cfg(test)]
mod tests {
    use super::{
        amendment_accumulator_new_with_capacity, amendment_observe, amendment_summary,
        AmendmentSummary, AMENDMENT_FIELDS,
    };
    use crate::migration_snapshot::{
        CanonicalValue, ComparedField, FieldComparison, FieldOutcome, ReviewClass, RowComparison,
        RowIdentityState, MAX_ROW_FIELDS,
    };
    use oa_flight_sfdps::audit::RowIdentity;
    use oa_flight_sfdps::migration_projection::{SourceField, SourceFieldState};
    use std::error::Error;

    fn value(number: u8) -> CanonicalValue {
        let mut bytes = [0; 32];
        if let Some(first) = bytes.get_mut(0) {
            *first = number;
        }
        CanonicalValue {
            present: 1,
            kind: 1,
            len: 1,
            bytes,
        }
    }

    fn empty_field(field: ComparedField) -> FieldComparison {
        FieldComparison {
            field,
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
        }
    }

    fn comparison(
        gufi: u128,
        row: u64,
        source_value: u8,
        candidate_value: Option<u8>,
        identity_state: RowIdentityState,
    ) -> RowComparison {
        let mut fields = [empty_field(ComparedField::SectorOwnerUnit); MAX_ROW_FIELDS];
        for (index, field) in AMENDMENT_FIELDS.iter().copied().enumerate() {
            let source = value(source_value);
            let candidate = candidate_value.map(value).unwrap_or(CanonicalValue {
                present: 0,
                kind: 0,
                len: 0,
                bytes: [0; 32],
            });
            if let Some(slot) = fields.get_mut(index) {
                *slot = FieldComparison {
                    field,
                    source_state: SourceFieldState::Present,
                    outcome: if candidate_value.is_none() {
                        FieldOutcome::CandidateUnavailable
                    } else if identity_state == RowIdentityState::CandidateDifferentGufi {
                        FieldOutcome::IdentityMismatch
                    } else if candidate == source {
                        FieldOutcome::Equivalent
                    } else {
                        FieldOutcome::Different
                    },
                    review_class: if candidate == source {
                        ReviewClass::Equivalent
                    } else {
                        ReviewClass::Unresolved
                    },
                    source_value: source,
                    candidate_value: candidate,
                    source_time_ns: 10_000_000_000,
                    source_receive_ns: 10_000_000_000,
                    candidate_source_ns: 10_000_000_000,
                    candidate_model_available_ns: 10_000_000_000,
                };
            }
        }
        if let Some(slot) = fields.get_mut(9) {
            *slot = empty_field(ComparedField::SectorOwnerUnit);
        }
        if let Some(slot) = fields.get_mut(10) {
            *slot = empty_field(ComparedField::SectorIdentifier);
        }
        RowComparison {
            source_row: RowIdentity {
                stream: 0,
                decoded_offset: row,
                recorder_sequence: row,
                row: 0,
            },
            source_gufi: gufi,
            candidate_gufi: if identity_state == RowIdentityState::MatchedGufi {
                gufi
            } else {
                gufi.saturating_add(1)
            },
            identity_state,
            field_count: 11,
            fields,
            source_fields_present: 9,
            source_fields_absent: 2,
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

    fn empty_route() -> SourceField<&'static str> {
        SourceField {
            state: SourceFieldState::Present,
            value: Some(""),
        }
    }

    // REQ: OA-MIG-416
    #[test]
    fn source_changes_compare_exact_candidate_before_and_after_values() -> Result<(), Box<dyn Error>>
    {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        for (row, source, candidate) in [(1, 1, 1), (2, 2, 2), (3, 3, 2)] {
            let comparison = comparison(
                44,
                row,
                source,
                Some(candidate),
                RowIdentityState::MatchedGufi,
            );
            amendment_observe(
                &mut accumulator,
                &comparison,
                44,
                i64::try_from(row)?,
                i64::try_from(row)?,
                empty_route(),
            )
            .map_err(|_| "synthetic amendment observe failed")?;
        }
        let summary = amendment_summary(&accumulator);
        assert_eq!(summary.source_rows_seen, 3);
        assert_eq!(summary.source_rows_with_changes, 2);
        assert_eq!(summary.baseline_entries, 1);
        let field = summary.fields.first().ok_or("field counts missing")?;
        assert_eq!(field.first_present_value, 1);
        assert_eq!(field.source_unchanged, 0);
        assert_eq!(field.source_changed, 2);
        assert_eq!(field.propagated_equivalent, 1);
        assert_eq!(field.different_unresolved, 1);
        assert!(summary.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn unavailable_identity_and_missing_key_stay_in_explicit_denominators(
    ) -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let first = comparison(55, 1, 1, None, RowIdentityState::CandidateUnavailable);
        amendment_observe(&mut accumulator, &first, 55, 1, 1, empty_route())
            .map_err(|_| "synthetic initial observation failed")?;
        let second = comparison(55, 2, 2, None, RowIdentityState::CandidateUnavailable);
        amendment_observe(&mut accumulator, &second, 55, 2, 2, empty_route())
            .map_err(|_| "synthetic unavailable transition failed")?;
        let missing = comparison(0, 3, 2, Some(2), RowIdentityState::MissingGufi);
        amendment_observe(&mut accumulator, &missing, 0, 3, 3, empty_route())
            .map_err(|_| "synthetic missing-key observation failed")?;
        let summary: AmendmentSummary = amendment_summary(&accumulator);
        assert_eq!(summary.missing_gufi_rows, 1);
        let field = summary.fields.first().ok_or("field counts missing")?;
        assert_eq!(field.candidate_unavailable, 1);
        assert_eq!(field.source_changed, 1);
        assert!(summary.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn mismatched_candidate_identity_never_becomes_equivalent() -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let first = comparison(66, 1, 1, Some(1), RowIdentityState::MatchedGufi);
        amendment_observe(&mut accumulator, &first, 66, 1, 1, empty_route())
            .map_err(|_| "synthetic initial observation failed")?;
        let second = comparison(66, 2, 2, Some(2), RowIdentityState::CandidateDifferentGufi);
        amendment_observe(&mut accumulator, &second, 66, 2, 2, empty_route())
            .map_err(|_| "synthetic identity-mismatch transition failed")?;
        let summary = amendment_summary(&accumulator);
        let field = summary.fields.first().ok_or("field counts missing")?;
        assert_eq!(field.identity_mismatch, 1);
        assert_eq!(field.propagated_equivalent, 0);
        assert!(summary.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn route_change_is_detected_without_exporting_text_or_claiming_candidate_support(
    ) -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let first = comparison(77, 1, 1, Some(1), RowIdentityState::MatchedGufi);
        let second = comparison(77, 2, 2, Some(2), RowIdentityState::MatchedGufi);
        let route_a = SourceField {
            state: SourceFieldState::Present,
            value: Some("FIXA DCT FIXB"),
        };
        let route_b = SourceField {
            state: SourceFieldState::Present,
            value: Some("FIXA DCT FIXC"),
        };
        amendment_observe(&mut accumulator, &first, 77, 1, 1, route_a)
            .map_err(|_| "synthetic route baseline failed")?;
        amendment_observe(&mut accumulator, &second, 77, 2, 2, route_b)
            .map_err(|_| "synthetic route change failed")?;
        let summary = amendment_summary(&accumulator);
        let route = summary.fields.get(8).ok_or("route counts missing")?;
        assert_eq!(route.source_changed, 1);
        assert_eq!(route.candidate_not_modeled, 1);
        assert_eq!(route.propagated_equivalent, 0);
        assert!(summary.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn source_clock_regression_is_counted_and_never_scored_as_propagated(
    ) -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let first = comparison(88, 1, 1, Some(1), RowIdentityState::MatchedGufi);
        let second = comparison(88, 2, 2, Some(2), RowIdentityState::MatchedGufi);
        amendment_observe(&mut accumulator, &first, 88, 20, 20, empty_route())
            .map_err(|_| "synthetic initial observation failed")?;
        amendment_observe(&mut accumulator, &second, 88, 10, 21, empty_route())
            .map_err(|_| "synthetic regressing source clock observation failed")?;
        let summary = amendment_summary(&accumulator);
        assert_eq!(summary.source_clock_regression_rows, 1);
        let field = summary.fields.first().ok_or("field counts missing")?;
        assert_eq!(field.clock_ambiguous, 1);
        assert_eq!(field.propagated_equivalent, 0);
        assert!(summary.reconciles());
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn separate_gufi_argument_must_match_decoded_row_identity() -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let row = comparison(99, 1, 1, Some(1), RowIdentityState::MatchedGufi);
        assert!(amendment_observe(&mut accumulator, &row, 100, 1, 1, empty_route()).is_err());
        assert_eq!(amendment_summary(&accumulator).source_rows_seen, 0);
        Ok(())
    }

    // REQ: OA-MIG-416
    #[test]
    fn source_absent_malformed_and_unsupported_fields_reconcile_separately(
    ) -> Result<(), Box<dyn Error>> {
        let mut accumulator = amendment_accumulator_new_with_capacity(4)
            .map_err(|_| "synthetic accumulator creation failed")?;
        let mut row = comparison(111, 1, 1, Some(1), RowIdentityState::MatchedGufi);
        let absent = row.fields.get_mut(0).ok_or("absent test field missing")?;
        absent.source_state = SourceFieldState::Absent;
        absent.outcome = FieldOutcome::SourceAbsent;
        let malformed = row
            .fields
            .get_mut(1)
            .ok_or("malformed test field missing")?;
        malformed.source_state = SourceFieldState::Malformed;
        malformed.outcome = FieldOutcome::SourceUnknown;
        let unsupported = row
            .fields
            .get_mut(2)
            .ok_or("unsupported test field missing")?;
        unsupported.outcome = FieldOutcome::SourceUnsupported;
        amendment_observe(&mut accumulator, &row, 111, 1, 1, empty_route())
            .map_err(|_| "synthetic source-state observation failed")?;
        let summary = amendment_summary(&accumulator);
        assert_eq!(
            summary.fields.first().map(|field| field.source_absent),
            Some(1)
        );
        assert_eq!(
            summary.fields.get(1).map(|field| field.source_malformed),
            Some(1)
        );
        assert_eq!(
            summary.fields.get(2).map(|field| field.source_unsupported),
            Some(1)
        );
        assert_eq!(
            summary.fields.get(2).map(|field| field.first_present_value),
            Some(0)
        );
        assert!(summary.reconciles());
        Ok(())
    }
}
