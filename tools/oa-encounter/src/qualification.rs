//! Bounded, offline matcher for independently labelled safety episodes and warning episodes.
//!
//! This module does not build truth or decide alert desire. The caller supplies both before
//! scoring. A degraded or unavailable net report makes the corresponding label unevaluable; it
//! never becomes a quiet control or a successful hit.

use oa_safety::net::{NetReport, Reason};

/// Maximum labels and warning episodes accepted by one scoring call (C7).
pub const MAX_CASES: usize = 64;
pub const MAX_WARNINGS: usize = 256;

/// A track (`b == 0`) or a canonical aircraft pair (`a < b`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subject {
    pub a: u32,
    pub b: u32,
}

/// Independently declared truth and evaluation window for one subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Case {
    pub subject: Subject,
    /// The interval in which the event is evaluated, or the whole no-alert control window.
    pub window_start_ns: i64,
    pub window_end_ns: i64,
    /// The independently declared violation interval; absent for a known no-alert control.
    pub violation: Option<(i64, i64)>,
    /// Whether the declaration calls for a warning.
    pub alert_desired: bool,
    /// Onset window for an eligible warning; used only for alert-desired cases.
    pub warning_from_ns: i64,
    pub warning_deadline_ns: i64,
    /// Evaluation state for the label's window. Any degraded or unavailable report is unknown.
    pub evaluation: NetReport,
}

/// An alert episode copied from the normal alert API. `warning_ns` is the warning-grade onset;
/// the episode start/end still count lower-grade alerts on no-alert controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertEpisode {
    pub subject: Subject,
    pub start_ns: i64,
    pub end_ns: i64,
    pub warning_ns: Option<i64>,
}

/// Outcome for one input label. `warning_index` points to the first eligible alert episode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    Miss,
    Quiet,
    Nuisance,
    Unavailable { reason: Reason, shortfall: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseResult {
    pub outcome: Outcome,
    pub warning_index: Option<u16>,
    /// Violation onset minus the selected warning onset; present for hits only.
    pub lead_ns: Option<i64>,
}

/// Explicit totals; unavailable cases remain in the all-label denominators.
///
/// Available-case miss rate is `misses / desired_evaluable`; available-case nuisance rate is
/// `nuisance_controls / controls_evaluable`. `desired_cases` and `no_alert_cases` include the
/// corresponding unavailable counts, so no incomplete evaluation can read as a clean result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub desired_cases: u16,
    pub desired_evaluable: u16,
    pub hits: u16,
    pub misses: u16,
    pub desired_unavailable: u16,
    pub no_alert_cases: u16,
    pub controls_evaluable: u16,
    pub nuisance_controls: u16,
    pub quiet_controls: u16,
    pub controls_unavailable: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreError {
    TooManyCases,
    TooManyWarnings,
    ResultsTooShort,
    InvalidSubject,
    InvalidInterval,
    InconsistentLabel,
    InvalidWarning,
}

/// Scores labels against alert episodes. Labels for a subject must be in window-start order. A
/// warning-grade onset can satisfy at most one desired case; duplicate records for the same
/// subject and warning-grade onset are treated as one onset. Work is bounded by
/// `MAX_CASES^2 + MAX_CASES * MAX_WARNINGS + MAX_WARNINGS^2` comparisons.
pub fn score(
    cases: &[Case],
    warnings: &[AlertEpisode],
    results: &mut [CaseResult],
) -> Result<Totals, ScoreError> {
    if cases.len() > MAX_CASES {
        return Err(ScoreError::TooManyCases);
    }
    if warnings.len() > MAX_WARNINGS {
        return Err(ScoreError::TooManyWarnings);
    }
    if results.len() < cases.len() {
        return Err(ScoreError::ResultsTooShort);
    }
    for (ix, case) in cases.iter().enumerate() {
        validate_subject(case.subject)?;
        if case.window_start_ns > case.window_end_ns {
            return Err(ScoreError::InvalidInterval);
        }
        match (case.alert_desired, case.violation) {
            (true, Some((start, end))) => {
                if start >= end || start < case.window_start_ns || end > case.window_end_ns {
                    return Err(ScoreError::InvalidInterval);
                }
                if case.warning_from_ns > case.warning_deadline_ns
                    || case.warning_from_ns < case.window_start_ns
                    || case.warning_deadline_ns >= start
                {
                    return Err(ScoreError::InconsistentLabel);
                }
                if start.checked_sub(case.warning_from_ns).is_none() {
                    return Err(ScoreError::InvalidInterval);
                }
            }
            (false, None) => {}
            _ => return Err(ScoreError::InconsistentLabel),
        }
        if cases.iter().take(ix).any(|previous| {
            previous.subject == case.subject && previous.window_start_ns > case.window_start_ns
        }) {
            return Err(ScoreError::InvalidInterval);
        }
    }
    for warning in warnings {
        validate_subject(warning.subject)?;
        if warning.start_ns > warning.end_ns
            || warning
                .warning_ns
                .is_some_and(|t| t < warning.start_ns || t > warning.end_ns)
        {
            return Err(ScoreError::InvalidWarning);
        }
    }

    let mut consumed = [false; MAX_WARNINGS];
    let mut totals = Totals::default();
    for (case, result) in cases.iter().zip(results) {
        let outcome = match case.evaluation {
            NetReport::Running => {
                if case.alert_desired {
                    totals.desired_cases += 1;
                    totals.desired_evaluable += 1;
                    let (event_start, _) = case.violation.ok_or(ScoreError::InconsistentLabel)?;
                    let mut selected: Option<(usize, i64)> = None;
                    for (warning_ix, (warning, &used)) in warnings.iter().zip(&consumed).enumerate()
                    {
                        let Some(warning_ns) = warning.warning_ns else {
                            continue;
                        };
                        if used
                            || warning.subject != case.subject
                            || warning_ns < case.warning_from_ns
                            || warning_ns > case.warning_deadline_ns
                        {
                            continue;
                        }
                        if selected.is_none_or(|(_, best_ns)| warning_ns < best_ns) {
                            selected = Some((warning_ix, warning_ns));
                        }
                    }
                    if let Some((warning_ix, warning_ns)) = selected {
                        for (duplicate, used) in warnings.iter().zip(&mut consumed) {
                            if duplicate.subject == case.subject
                                && duplicate.warning_ns == Some(warning_ns)
                            {
                                *used = true;
                            }
                        }
                        let lead_ns = event_start
                            .checked_sub(warning_ns)
                            .ok_or(ScoreError::InvalidInterval)?;
                        totals.hits += 1;
                        CaseResult {
                            outcome: Outcome::Hit,
                            warning_index: u16::try_from(warning_ix).ok(),
                            lead_ns: Some(lead_ns),
                        }
                    } else {
                        totals.misses += 1;
                        CaseResult {
                            outcome: Outcome::Miss,
                            warning_index: None,
                            lead_ns: None,
                        }
                    }
                } else {
                    totals.no_alert_cases += 1;
                    totals.controls_evaluable += 1;
                    let warned = warnings.iter().any(|warning| {
                        warning.subject == case.subject
                            && warning.start_ns <= case.window_end_ns
                            && warning.end_ns >= case.window_start_ns
                    });
                    if warned {
                        totals.nuisance_controls += 1;
                        CaseResult {
                            outcome: Outcome::Nuisance,
                            warning_index: None,
                            lead_ns: None,
                        }
                    } else {
                        totals.quiet_controls += 1;
                        CaseResult {
                            outcome: Outcome::Quiet,
                            warning_index: None,
                            lead_ns: None,
                        }
                    }
                }
            }
            NetReport::Degraded { reason, count, .. } => {
                if case.alert_desired {
                    totals.desired_cases += 1;
                    totals.desired_unavailable += 1;
                } else {
                    totals.no_alert_cases += 1;
                    totals.controls_unavailable += 1;
                }
                CaseResult {
                    outcome: Outcome::Unavailable {
                        reason,
                        shortfall: count,
                    },
                    warning_index: None,
                    lead_ns: None,
                }
            }
            NetReport::Unavailable { reason } => {
                if case.alert_desired {
                    totals.desired_cases += 1;
                    totals.desired_unavailable += 1;
                } else {
                    totals.no_alert_cases += 1;
                    totals.controls_unavailable += 1;
                }
                CaseResult {
                    outcome: Outcome::Unavailable {
                        reason,
                        shortfall: 0,
                    },
                    warning_index: None,
                    lead_ns: None,
                }
            }
        };
        *result = outcome;
    }
    Ok(totals)
}

fn validate_subject(subject: Subject) -> Result<(), ScoreError> {
    if subject.a == 0 || (subject.b != 0 && subject.a >= subject.b) {
        Err(ScoreError::InvalidSubject)
    } else {
        Ok(())
    }
}
