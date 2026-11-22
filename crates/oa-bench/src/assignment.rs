//! Bounded as-of assigned-altitude reducer (shell; no recording or capture I/O).
//!
//! The query retains one compact state per issue GUFI. Events are folded into that state as they
//! arrive; no per-flight assignment history is kept. Unknown or contradictory receipt times are
//! conservative uncertainty, while a definitive event received after an issue cutoff is excluded.

use std::collections::{BTreeMap, BTreeSet};

use crate::legacy::{ALT_FORM_INVALID, ALT_FORM_SIMPLE};

/// Maximum issue GUFIs held by one reducer.
pub const MAX_ASSIGNMENT_ISSUES: usize = 12_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AssignmentIssue {
    pub gufi: u128,
    pub cutoff_ms: i64,
}
// Budget: one issue boundary. Its collection is capped at 12,000 entries.
const _: () = assert!(core::mem::size_of::<AssignmentIssue>() == 32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AssignmentEvent {
    pub gufi: u128,
    pub source_ms: i64,
    pub received_ms: i64,
    pub form: u8,
    pub value_ft: i32,
}
// Budget: one event at a time in the shell reducer.
const _: () = assert!(core::mem::size_of::<AssignmentEvent>() == 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignmentEvidence {
    Available {
        value_ft: i32,
        available_ms: i64,
    },
    Missing,
    UnsupportedForm {
        form: u8,
        available_ms: i64,
    },
    SameTimeConflict {
        available_ms: i64,
    },
    MalformedForm {
        available_ms: i64,
    },
    InvalidSimpleValue {
        available_ms: i64,
    },
    /// For positive source times, `source_ms` is the maximum retained affecting witness, not
    /// necessarily the earliest. Unusable source times are retained separately and remain exact.
    UnknownAvailability {
        source_ms: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignmentQueryError {
    TooManyIssues { count: usize, limit: usize },
    InvalidGufi { index: usize, gufi: u128 },
    InvalidCutoff { index: usize, cutoff_ms: i64 },
    DuplicateGufi { gufi: u128 },
}

const LATEST_CONFLICT: u8 = 1;
const UNKNOWN_MAX_CONFLICT: u8 = 1;

/// One state per issue GUFI, plus a bounded lookup index. Public counters count events targeted to
/// an issue, not unrecognized GUFIs; repeated identical events count as observations too.
// A7.2: one offline query object; its bounded SoA columns hold at most 12,000 issue states.
#[derive(Debug)]
pub struct AssignmentQuery {
    cutoff_ms: Vec<i64>,
    latest_source_ms: Vec<i64>,
    latest_available_ms: Vec<i64>,
    latest_form: Vec<u8>,
    latest_value_ft: Vec<i32>,
    latest_flags: Vec<u8>,
    unknown_without_source_ms: Vec<i64>,
    unknown_without_source_present: Vec<u8>,
    unknown_positive_max_source_ms: Vec<i64>,
    unknown_positive_max_form: Vec<u8>,
    unknown_positive_max_value_ft: Vec<i32>,
    unknown_positive_max_flags: Vec<u8>,
    by_gufi: BTreeMap<u128, usize>,
    pub targeted_events: u64,
    pub targeted_known_events: u64,
    pub targeted_uncertain_events: u64,
    pub targeted_late_events: u64,
}

/// Validates all issue keys and cutoffs before constructing the lookup index.
pub fn assignment_query(
    issues: &[AssignmentIssue],
) -> Result<AssignmentQuery, AssignmentQueryError> {
    if issues.len() > MAX_ASSIGNMENT_ISSUES {
        return Err(AssignmentQueryError::TooManyIssues {
            count: issues.len(),
            limit: MAX_ASSIGNMENT_ISSUES,
        });
    }

    let mut seen = BTreeSet::new();
    for (index, issue) in issues.iter().enumerate() {
        if issue.gufi == 0 || issue.gufi == u128::MAX {
            return Err(AssignmentQueryError::InvalidGufi {
                index,
                gufi: issue.gufi,
            });
        }
        if issue.cutoff_ms <= 0 || issue.cutoff_ms == i64::MAX {
            return Err(AssignmentQueryError::InvalidCutoff {
                index,
                cutoff_ms: issue.cutoff_ms,
            });
        }
        if !seen.insert(issue.gufi) {
            return Err(AssignmentQueryError::DuplicateGufi { gufi: issue.gufi });
        }
    }

    let mut cutoff_ms = Vec::with_capacity(issues.len());
    let mut latest_source_ms = Vec::with_capacity(issues.len());
    let mut latest_available_ms = Vec::with_capacity(issues.len());
    let mut latest_form = Vec::with_capacity(issues.len());
    let mut latest_value_ft = Vec::with_capacity(issues.len());
    let mut latest_flags = Vec::with_capacity(issues.len());
    let mut unknown_without_source_ms = Vec::with_capacity(issues.len());
    let mut unknown_without_source_present = Vec::with_capacity(issues.len());
    let mut unknown_positive_max_source_ms = Vec::with_capacity(issues.len());
    let mut unknown_positive_max_form = Vec::with_capacity(issues.len());
    let mut unknown_positive_max_value_ft = Vec::with_capacity(issues.len());
    let mut unknown_positive_max_flags = Vec::with_capacity(issues.len());
    let mut by_gufi = BTreeMap::new();
    for issue in issues {
        let index = cutoff_ms.len();
        cutoff_ms.push(issue.cutoff_ms);
        latest_source_ms.push(0);
        latest_available_ms.push(0);
        latest_form.push(0);
        latest_value_ft.push(0);
        latest_flags.push(0);
        unknown_without_source_ms.push(0);
        unknown_without_source_present.push(0);
        unknown_positive_max_source_ms.push(0);
        unknown_positive_max_form.push(0);
        unknown_positive_max_value_ft.push(0);
        unknown_positive_max_flags.push(0);
        by_gufi.insert(issue.gufi, index);
    }
    Ok(AssignmentQuery {
        cutoff_ms,
        latest_source_ms,
        latest_available_ms,
        latest_form,
        latest_value_ft,
        latest_flags,
        unknown_without_source_ms,
        unknown_without_source_present,
        unknown_positive_max_source_ms,
        unknown_positive_max_form,
        unknown_positive_max_value_ft,
        unknown_positive_max_flags,
        by_gufi,
        targeted_events: 0,
        targeted_known_events: 0,
        targeted_uncertain_events: 0,
        targeted_late_events: 0,
    })
}

/// Folds one event into its issue state. A receive time is definitive only when both times are
/// positive and the source stamp is no later than receipt. Contradictory or absent receipt times
/// use the source cutoff; an unusable source time is potentially relevant at every cutoff.
pub fn observe_assignment(q: &mut AssignmentQuery, event: AssignmentEvent) {
    let Some(&index) = q.by_gufi.get(&event.gufi) else {
        return;
    };
    q.targeted_events = q.targeted_events.saturating_add(1);
    let Some(&cutoff_ms) = q.cutoff_ms.get(index) else {
        return;
    };

    let definitive =
        event.source_ms > 0 && event.received_ms > 0 && event.source_ms <= event.received_ms;
    if definitive {
        if event.received_ms > cutoff_ms {
            q.targeted_late_events = q.targeted_late_events.saturating_add(1);
            return;
        }
        q.targeted_known_events = q.targeted_known_events.saturating_add(1);
        observe_known(q, index, event);
        return;
    }

    if event.source_ms > 0 && event.source_ms > cutoff_ms {
        return;
    }
    q.targeted_uncertain_events = q.targeted_uncertain_events.saturating_add(1);
    observe_unknown(q, index, event);
}

fn observe_known(q: &mut AssignmentQuery, index: usize, event: AssignmentEvent) {
    let Some(current_source) = q.latest_source_ms.get(index).copied() else {
        return;
    };
    if current_source == 0 || event.source_ms > current_source {
        if let Some(value) = q.latest_source_ms.get_mut(index) {
            *value = event.source_ms;
        }
        if let Some(value) = q.latest_available_ms.get_mut(index) {
            *value = event.received_ms;
        }
        if let Some(value) = q.latest_form.get_mut(index) {
            *value = event.form;
        }
        if let Some(value) = q.latest_value_ft.get_mut(index) {
            *value = event.value_ft;
        }
        if let Some(value) = q.latest_flags.get_mut(index) {
            *value = 0;
        }
    } else if event.source_ms == current_source {
        let (Some(&form), Some(&value_ft), Some(&available_ms)) = (
            q.latest_form.get(index),
            q.latest_value_ft.get(index),
            q.latest_available_ms.get(index),
        ) else {
            return;
        };
        let conflict = event.form != form || event.value_ft != value_ft;
        if conflict {
            if let Some(value) = q.latest_flags.get_mut(index) {
                *value |= LATEST_CONFLICT;
            }
        }
        // Keep a stable timestamp even when the stamp conflicts; the evidence kind remains the
        // conflict, and the minimum receipt is independent of event iteration order.
        if event.received_ms < available_ms {
            if let Some(value) = q.latest_available_ms.get_mut(index) {
                *value = event.received_ms;
            }
        }
    }
}

fn observe_unknown(q: &mut AssignmentQuery, index: usize, event: AssignmentEvent) {
    if event.source_ms <= 0 {
        let (Some(source), Some(present)) = (
            q.unknown_without_source_ms.get_mut(index),
            q.unknown_without_source_present.get_mut(index),
        ) else {
            return;
        };
        if *present == 0 || event.source_ms < *source {
            *source = event.source_ms;
        }
        *present = 1;
        return;
    }
    let Some(max_source) = q.unknown_positive_max_source_ms.get(index).copied() else {
        return;
    };
    if max_source == 0 || event.source_ms > max_source {
        if let Some(value) = q.unknown_positive_max_source_ms.get_mut(index) {
            *value = event.source_ms;
        }
        if let Some(value) = q.unknown_positive_max_form.get_mut(index) {
            *value = event.form;
        }
        if let Some(value) = q.unknown_positive_max_value_ft.get_mut(index) {
            *value = event.value_ft;
        }
        if let Some(value) = q.unknown_positive_max_flags.get_mut(index) {
            *value = 0;
        }
    } else if event.source_ms == max_source {
        let (Some(&form), Some(&value_ft)) = (
            q.unknown_positive_max_form.get(index),
            q.unknown_positive_max_value_ft.get(index),
        ) else {
            return;
        };
        if event.form != form || event.value_ft != value_ft {
            if let Some(flags) = q.unknown_positive_max_flags.get_mut(index) {
                *flags |= UNKNOWN_MAX_CONFLICT;
            }
        }
    }
}

/// Returns the final as-of evidence for an issue GUFI, or `None` for an unqueried GUFI.
pub fn assignment_at_issue(q: &AssignmentQuery, gufi: u128) -> Option<AssignmentEvidence> {
    let index = *q.by_gufi.get(&gufi)?;
    let source_ms = unknown_that_could_change(q, index)?;
    if let Some(source_ms) = source_ms {
        return Some(AssignmentEvidence::UnknownAvailability { source_ms });
    }
    let latest_source_ms = *q.latest_source_ms.get(index)?;
    if latest_source_ms == 0 {
        return Some(AssignmentEvidence::Missing);
    }
    let available_ms = *q.latest_available_ms.get(index)?;
    let form = *q.latest_form.get(index)?;
    let value_ft = *q.latest_value_ft.get(index)?;
    let flags = *q.latest_flags.get(index)?;
    if flags & LATEST_CONFLICT != 0 {
        return Some(AssignmentEvidence::SameTimeConflict { available_ms });
    }
    if form == ALT_FORM_INVALID {
        return Some(AssignmentEvidence::MalformedForm { available_ms });
    }
    if form != ALT_FORM_SIMPLE {
        return Some(AssignmentEvidence::UnsupportedForm { form, available_ms });
    }
    if value_ft == i32::MIN || !(-2_000..=100_000).contains(&value_ft) {
        return Some(AssignmentEvidence::InvalidSimpleValue { available_ms });
    }
    Some(AssignmentEvidence::Available {
        value_ft,
        available_ms,
    })
}

fn unknown_that_could_change(q: &AssignmentQuery, index: usize) -> Option<Option<i64>> {
    if q.unknown_without_source_present.get(index).copied()? != 0 {
        return Some(Some(*q.unknown_without_source_ms.get(index)?));
    }
    let unknown_source_ms = *q.unknown_positive_max_source_ms.get(index)?;
    if unknown_source_ms == 0 {
        return Some(None);
    }
    let known_source_ms = *q.latest_source_ms.get(index)?;
    if known_source_ms == 0 {
        return Some(Some(unknown_source_ms));
    }
    let max_flags = *q.unknown_positive_max_flags.get(index)?;
    let differs = *q.unknown_positive_max_form.get(index)? != *q.latest_form.get(index)?
        || *q.unknown_positive_max_value_ft.get(index)? != *q.latest_value_ft.get(index)?;
    if unknown_source_ms > known_source_ms
        || (unknown_source_ms == known_source_ms
            && (max_flags & UNKNOWN_MAX_CONFLICT != 0 || differs))
    {
        Some(Some(unknown_source_ms))
    } else {
        Some(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::{ALT_FORM_AT_FIX, ALT_FORM_SIMPLE, ALT_FORM_VFR};

    fn event(source_ms: i64, received_ms: i64, form: u8, value_ft: i32) -> AssignmentEvent {
        AssignmentEvent {
            gufi: 1,
            source_ms,
            received_ms,
            form,
            value_ft,
        }
    }

    #[allow(clippy::unwrap_used)]
    fn query() -> AssignmentQuery {
        assignment_query(&[AssignmentIssue {
            gufi: 1,
            cutoff_ms: 100,
        }])
        .unwrap()
    }

    #[test]
    fn latest_definitive_received_by_cutoff_wins_and_late_receipt_is_excluded() {
        let mut q = query();
        observe_assignment(&mut q, event(20, 30, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut q, event(40, 90, ALT_FORM_SIMPLE, 9000));
        observe_assignment(&mut q, event(50, 101, ALT_FORM_SIMPLE, 10000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::Available {
                value_ft: 9000,
                available_ms: 90
            })
        );
        assert_eq!(q.targeted_known_events, 2);
        assert_eq!(q.targeted_late_events, 1);
    }

    #[test]
    fn identical_same_stamp_duplicates_are_harmless_but_conflict_resets_at_higher_stamp() {
        let mut q = query();
        observe_assignment(&mut q, event(20, 40, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut q, event(20, 35, ALT_FORM_SIMPLE, 8000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::Available {
                value_ft: 8000,
                available_ms: 35
            })
        );
        observe_assignment(&mut q, event(20, 50, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::SameTimeConflict { available_ms: 35 })
        );
        observe_assignment(&mut q, event(21, 60, ALT_FORM_SIMPLE, 10000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::Available {
                value_ft: 10000,
                available_ms: 60
            })
        );
    }

    #[test]
    fn same_stamp_conflict_and_unknown_witness_are_stable_under_event_permutation() {
        let conflicting = [
            event(20, 50, ALT_FORM_SIMPLE, 8000),
            event(20, 35, ALT_FORM_SIMPLE, 9000),
            event(20, 40, ALT_FORM_SIMPLE, 8000),
        ];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut q = query();
            for index in order {
                observe_assignment(&mut q, conflicting[index]);
            }
            assert_eq!(
                assignment_at_issue(&q, 1),
                Some(AssignmentEvidence::SameTimeConflict { available_ms: 35 })
            );
        }

        let uncertain = [
            event(30, 40, ALT_FORM_SIMPLE, 8000),
            event(50, i64::MIN, ALT_FORM_SIMPLE, 9000),
            event(45, i64::MIN, ALT_FORM_VFR, 0),
        ];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut q = query();
            for index in order {
                observe_assignment(&mut q, uncertain[index]);
            }
            assert_eq!(
                assignment_at_issue(&q, 1),
                Some(AssignmentEvidence::UnknownAvailability { source_ms: 50 })
            );
        }
    }

    #[test]
    fn latest_form_and_value_are_reported_without_reusing_an_older_assignment() {
        let mut q = query();
        observe_assignment(&mut q, event(20, 30, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut q, event(21, 40, ALT_FORM_VFR, 0));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::UnsupportedForm {
                form: ALT_FORM_VFR,
                available_ms: 40
            })
        );
        observe_assignment(&mut q, event(22, 50, ALT_FORM_AT_FIX, i32::MIN));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::UnsupportedForm {
                form: ALT_FORM_AT_FIX,
                available_ms: 50
            })
        );
        observe_assignment(&mut q, event(23, 60, ALT_FORM_SIMPLE, 100_001));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::InvalidSimpleValue { available_ms: 60 })
        );
        observe_assignment(&mut q, event(24, 70, ALT_FORM_INVALID, 9000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::MalformedForm { available_ms: 70 })
        );
    }

    #[test]
    fn unknown_or_contradictory_receipt_is_cutoff_gated_but_no_source_time_is_not() {
        let mut q = query();
        observe_assignment(&mut q, event(120, i64::MIN, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::Missing)
        );
        observe_assignment(&mut q, event(90, i64::MIN, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::UnknownAvailability { source_ms: 90 })
        );
        let mut contradictory = query();
        observe_assignment(&mut contradictory, event(95, 94, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&contradictory, 1),
            Some(AssignmentEvidence::UnknownAvailability { source_ms: 95 })
        );
        let mut positive_witnesses = query();
        observe_assignment(
            &mut positive_witnesses,
            event(40, i64::MIN, ALT_FORM_SIMPLE, 8000),
        );
        observe_assignment(
            &mut positive_witnesses,
            event(90, i64::MIN, ALT_FORM_VFR, 0),
        );
        assert_eq!(
            assignment_at_issue(&positive_witnesses, 1),
            Some(AssignmentEvidence::UnknownAvailability { source_ms: 90 })
        );
        let mut no_source = query();
        observe_assignment(
            &mut no_source,
            event(i64::MIN, i64::MIN, ALT_FORM_SIMPLE, 9000),
        );
        assert_eq!(
            assignment_at_issue(&no_source, 1),
            Some(AssignmentEvidence::UnknownAvailability {
                source_ms: i64::MIN
            })
        );
    }

    #[test]
    fn definitive_higher_stamp_resolves_older_uncertainty_but_same_stamp_disagreement_does_not() {
        let mut q = query();
        observe_assignment(&mut q, event(50, i64::MIN, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut q, event(40, 50, ALT_FORM_SIMPLE, 7000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::UnknownAvailability { source_ms: 50 })
        );
        observe_assignment(&mut q, event(51, 60, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&q, 1),
            Some(AssignmentEvidence::Available {
                value_ft: 9000,
                available_ms: 60
            })
        );
    }

    #[test]
    fn unknown_same_stamp_duplicate_is_harmless_but_disagreement_is_uncertain() {
        let mut same = query();
        observe_assignment(&mut same, event(50, 60, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut same, event(50, i64::MIN, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut same, event(50, i64::MIN, ALT_FORM_SIMPLE, 8000));
        assert_eq!(
            assignment_at_issue(&same, 1),
            Some(AssignmentEvidence::Available {
                value_ft: 8000,
                available_ms: 60
            })
        );

        let mut different = query();
        observe_assignment(&mut different, event(50, 60, ALT_FORM_SIMPLE, 8000));
        observe_assignment(&mut different, event(50, i64::MIN, ALT_FORM_SIMPLE, 9000));
        assert_eq!(
            assignment_at_issue(&different, 1),
            Some(AssignmentEvidence::UnknownAvailability { source_ms: 50 })
        );
    }

    #[test]
    fn issue_keys_and_bounds_are_validated_before_query_construction() {
        assert_eq!(
            assignment_query(&[
                AssignmentIssue {
                    gufi: 1,
                    cutoff_ms: 100,
                },
                AssignmentIssue {
                    gufi: 1,
                    cutoff_ms: 101,
                },
            ])
            .unwrap_err(),
            AssignmentQueryError::DuplicateGufi { gufi: 1 }
        );
        for issue in [
            AssignmentIssue {
                gufi: 0,
                cutoff_ms: 1,
            },
            AssignmentIssue {
                gufi: u128::MAX,
                cutoff_ms: 1,
            },
            AssignmentIssue {
                gufi: 1,
                cutoff_ms: i64::MIN,
            },
            AssignmentIssue {
                gufi: 1,
                cutoff_ms: i64::MAX,
            },
        ] {
            assert!(assignment_query(&[issue]).is_err());
        }
        let too_many = vec![
            AssignmentIssue {
                gufi: 1,
                cutoff_ms: 1,
            };
            MAX_ASSIGNMENT_ISSUES + 1
        ];
        assert!(matches!(
            assignment_query(&too_many),
            Err(AssignmentQueryError::TooManyIssues { .. })
        ));
    }
}
