//! Offline encounter scoring (RULESET B1/B2/C6): a pure comparison of independently supplied
//! synthetic hazard windows and a complete emitted-alert log. This is a development measure, not
//! an operational risk or nuisance qualification.

use oa_msaw::{Alert, Grade, END_CLEAR, END_LOST_ALTITUDE_MISSING, END_LOST_ALTITUDE_STALE};

/// Maximum input rows per side. The pair budget below is the tighter bound on comparison work.
pub const MAX_EVALUATION_ROWS: usize = 100_000;
/// Maximum aggregate pair checks across three hazard/alert passes and truth-window validation
/// (RULESET C7).
pub const MAX_EVALUATION_PAIRS: usize = 10_000_000;

/// An independently specified synthetic encounter interval. `warning_due_ns` is its warning
/// deadline; all times are absolute nanoseconds on the same timeline as [`Alert`].
// A3/C6: a one-shot offline oracle boundary uses whole records, consumed together; this is not
// a per-cycle real-time collection. Row storage and total comparison work are bounded above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct HazardWindow {
    pub addr: u32,
    pub start_ns: i64,
    pub warning_due_ns: i64,
    pub end_ns: i64,
}
// A7: bounded scenario rows are processed in quantity. Four bytes of alignment padding.
const _: () = assert!(core::mem::size_of::<HazardWindow>() == 32);

/// Invalid inputs or a comparison that exceeds the named offline-work budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationError {
    TooManyRows,
    TooManyComparisons,
    InvalidHazardWindow,
    OverlappingHazardWindows,
    InvalidAlertInterval,
    InvalidAlertTransition,
    InvalidProtectionTime,
    InvalidAlertState,
    AmbiguousPreWindowWarning,
}

/// Counts from a deterministic offline comparison. `None` means the required source evidence is
/// unavailable; zero is used only when that evidence was supplied and the observed count is zero.
/// Encounter warning counts share `encounters` as their denominator, and conserve it exactly.
// A7.2: cold struct, size guard waived (offline report output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evaluation {
    /// Number of independently supplied hazard windows, or `None` when ground truth is absent.
    pub encounters: Option<usize>,
    /// Warning onsets recorded in the window at or before its deadline.
    pub timely: Option<usize>,
    /// First warning after the deadline, but by the end of the hazard window.
    pub late: Option<usize>,
    /// No warning by the end of the hazard window, when the alert log is complete.
    pub missed: Option<usize>,
    /// Number of emitted alert records when alert evidence is available.
    pub alert_episodes: Option<usize>,
    /// Alert records with no overlapping same-address hazard window.
    pub nuisance_episodes: Option<usize>,
    /// Alert records whose terminal reason is a lost-altitude marker. Alerts compress episodes,
    /// so this is not a count of every protection transition during the episode.
    pub loss_marked_alert_records: Option<usize>,
    /// Loss-marked records whose recorded `protect_ns` observation falls inside a same-address
    /// hazard window, when both sources are present. `loss_marked_alert_records` is its record
    /// denominator. No interval is inferred from this timestamp.
    pub loss_observations_during_hazard: Option<usize>,
}

fn is_protection_lost(alert: &Alert) -> bool {
    alert.end_reason == END_LOST_ALTITUDE_MISSING || alert.end_reason == END_LOST_ALTITUDE_STALE
}

fn overlaps(start_a: i64, end_a: i64, start_b: i64, end_b: i64) -> bool {
    start_a <= end_b && start_b <= end_a
}

fn work_required(hazard_count: usize, alert_count: usize) -> Result<usize, EvaluationError> {
    let pair_passes = hazard_count
        .checked_mul(alert_count)
        .and_then(|n| n.checked_mul(3))
        .ok_or(EvaluationError::TooManyComparisons)?;
    let hazard_pairs = hazard_count
        .checked_mul(hazard_count.saturating_sub(1))
        .and_then(|n| n.checked_div(2))
        .ok_or(EvaluationError::TooManyComparisons)?;
    pair_passes
        .checked_add(hazard_pairs)
        .ok_or(EvaluationError::TooManyComparisons)
}

fn preflight_work(
    hazard_count: usize,
    alert_count: usize,
    budget: usize,
) -> Result<(), EvaluationError> {
    if work_required(hazard_count, alert_count)? > budget {
        return Err(EvaluationError::TooManyComparisons);
    }
    Ok(())
}

fn validate(
    hazards: Option<&[HazardWindow]>,
    alerts: Option<&[Alert]>,
) -> Result<(), EvaluationError> {
    if hazards.is_some_and(|rows| rows.len() > MAX_EVALUATION_ROWS)
        || alerts.is_some_and(|rows| rows.len() > MAX_EVALUATION_ROWS)
    {
        return Err(EvaluationError::TooManyRows);
    }
    if let Some(rows) = hazards {
        preflight_work(
            rows.len(),
            alerts.unwrap_or(&[]).len(),
            MAX_EVALUATION_PAIRS,
        )?;
        for row in rows {
            if row.start_ns > row.warning_due_ns || row.warning_due_ns > row.end_ns {
                return Err(EvaluationError::InvalidHazardWindow);
            }
        }
        for (i, row) in rows.iter().enumerate() {
            for other in rows.iter().skip(i + 1) {
                if row.addr == other.addr
                    && overlaps(row.start_ns, row.end_ns, other.start_ns, other.end_ns)
                {
                    return Err(EvaluationError::OverlappingHazardWindows);
                }
            }
        }
    }
    if let Some(rows) = alerts {
        for alert in rows {
            if alert.start_ns > alert.end_ns {
                return Err(EvaluationError::InvalidAlertInterval);
            }
            let Some(caution_ns) = alert.caution_ns else {
                return Err(EvaluationError::InvalidAlertTransition);
            };
            if caution_ns != alert.start_ns
                || alert
                    .warning_ns
                    .is_some_and(|at| at < caution_ns || at > alert.end_ns)
                || (alert.peak == Grade::Warning) != alert.warning_ns.is_some()
            {
                return Err(EvaluationError::InvalidAlertTransition);
            }
            if alert.cycles == 0
                || (alert.end_reason != END_CLEAR
                    && alert.end_reason != END_LOST_ALTITUDE_MISSING
                    && alert.end_reason != END_LOST_ALTITUDE_STALE)
            {
                return Err(EvaluationError::InvalidAlertState);
            }
            if is_protection_lost(alert) {
                if alert.protect_ns == 0 || alert.protect_ns <= alert.end_ns {
                    return Err(EvaluationError::InvalidProtectionTime);
                }
            } else if alert.protect_ns != 0 {
                return Err(EvaluationError::InvalidProtectionTime);
            }
        }
    }
    Ok(())
}

/// Compare supplied truth with a complete alert log. `None` for either input means that source is
/// unavailable, never an empty source. A present empty slice is valid evidence with denominator 0.
///
/// A timely warning has its recorded first-warning timestamp inside the encounter and at or before
/// the supplied deadline. A late warning is first seen after the deadline and no later than the
/// encounter end. A warning first recorded before the encounter on an alert record that extends
/// into the encounter is ambiguous: the compressed episode does not preserve whether it remained
/// at warning grade, so this returns [`EvaluationError::AmbiguousPreWindowWarning`] unless an
/// in-window warning already determines a timely result. Nuisance episodes are alert records that
/// do not overlap any same-address hazard window. Terminal loss-marked alert records and their
/// recorded loss observation (`protect_ns`) are reported separately. The observation is a point,
/// not an interval, and is not used to extend warning or alert-episode evidence. This evaluator
/// makes no claim about real-world risk.
pub fn evaluate(
    hazards: Option<&[HazardWindow]>,
    alerts: Option<&[Alert]>,
) -> Result<Evaluation, EvaluationError> {
    validate(hazards, alerts)?;
    let loss_marked_alert_records =
        alerts.map(|rows| rows.iter().filter(|a| is_protection_lost(a)).count());

    let (Some(hazards), Some(alerts)) = (hazards, alerts) else {
        return Ok(Evaluation {
            encounters: hazards.map(|rows| rows.len()),
            timely: None,
            late: None,
            missed: None,
            alert_episodes: alerts.map(|rows| rows.len()),
            nuisance_episodes: None,
            loss_marked_alert_records,
            loss_observations_during_hazard: None,
        });
    };

    let mut timely = 0usize;
    let mut late = 0usize;
    let mut missed = 0usize;
    for hazard in hazards {
        let mut first_warning_in_window: Option<i64> = None;
        let mut ambiguous_pre_window_warning = false;
        for alert in alerts.iter().filter(|a| a.addr == hazard.addr) {
            if let Some(at) = alert.warning_ns {
                if at < hazard.start_ns && alert.end_ns >= hazard.start_ns {
                    ambiguous_pre_window_warning = true;
                } else if at >= hazard.start_ns && at <= hazard.end_ns {
                    first_warning_in_window =
                        Some(first_warning_in_window.map_or(at, |prior| prior.min(at)));
                }
            }
        }
        match (first_warning_in_window, ambiguous_pre_window_warning) {
            (Some(at), _) if at <= hazard.warning_due_ns => timely += 1,
            (Some(_), true) | (None, true) => {
                return Err(EvaluationError::AmbiguousPreWindowWarning);
            }
            (Some(_), false) => late += 1,
            (None, false) => missed += 1,
        }
    }

    let nuisance_episodes = alerts
        .iter()
        .filter(|alert| {
            !hazards.iter().any(|hazard| {
                alert.addr == hazard.addr
                    && overlaps(alert.start_ns, alert.end_ns, hazard.start_ns, hazard.end_ns)
            })
        })
        .count();
    let loss_observations_during_hazard = alerts
        .iter()
        .filter(|alert| {
            is_protection_lost(alert)
                && hazards.iter().any(|hazard| {
                    alert.addr == hazard.addr
                        && alert.protect_ns >= hazard.start_ns
                        && alert.protect_ns <= hazard.end_ns
                })
        })
        .count();

    Ok(Evaluation {
        encounters: Some(hazards.len()),
        timely: Some(timely),
        late: Some(late),
        missed: Some(missed),
        alert_episodes: Some(alerts.len()),
        nuisance_episodes: Some(nuisance_episodes),
        loss_marked_alert_records,
        loss_observations_during_hazard: Some(loss_observations_during_hazard),
    })
}

#[cfg(test)]
mod tests {
    use super::{preflight_work, work_required, EvaluationError};

    #[test]
    fn comparison_budget_counts_all_three_hazard_alert_passes_and_window_checks() {
        // Three pairwise alert passes (3 * 2 * 3) plus one unordered hazard-window pair.
        assert_eq!(work_required(2, 3), Ok(19));
        assert_eq!(preflight_work(2, 3, 19), Ok(()));
        assert_eq!(
            preflight_work(2, 3, 18),
            Err(EvaluationError::TooManyComparisons)
        );
    }
}
