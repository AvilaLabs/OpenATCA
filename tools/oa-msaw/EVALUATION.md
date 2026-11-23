# Offline MSAW encounter evaluation

`oa_msaw_run::evaluation::evaluate` compares independently supplied synthetic hazard windows with
a complete alert log. It is a development measurement; its outputs do not qualify real-world risk,
operational nuisance, or warning parameters.

Each `HazardWindow` names the complete aircraft address and its absolute start, warning deadline,
and end times. `Alert` records are compared by full address. Every supplied encounter contributes
exactly one to the denominator, and `timely + late + missed == encounters` whenever both evidence
sources are available. A warning at the deadline is timely. A warning after the deadline and no
later than the encounter end is late. No warning by the end is missed. A warning onset recorded
before the encounter on an alert episode that continues into the encounter is ambiguous: the
compressed alert does not preserve whether its grade stayed at WARNING or returned to CAUTION. The
evaluator returns `AmbiguousPreWindowWarning` unless a warning onset inside the window already
establishes a timely result. These boundaries are explicit scenario inputs, not inferred from the
emitted alerts.

Nuisance episodes are alert records whose `[start_ns, end_ns]` interval overlaps no same-address
hazard window. The alert record count is the denominator for that measure. `loss_marked_alert_records`
counts records with terminal reason `END_LOST_ALTITUDE_STALE` or `END_LOST_ALTITUDE_MISSING`; an
`Alert` compresses an episode, so this is not a count of every protection-state transition.
`loss_observations_during_hazard` counts these records when their single `protect_ns` timestamp is
inside a same-address hazard window. It does not infer an interval from the timestamp.
`protect_ns` never extends warning evidence, and an episode's `end_ns` does not prove that its
earlier warning remained active.
A missing alert log means warning, nuisance, and loss metrics are unavailable. Missing ground truth
means encounter and nuisance metrics are unavailable, while a supplied alert log can still report
the loss-marked record count. A present but empty slice is evidence and reports a zero denominator
and zero counts.

Example callable from a Rust shell or integration test:

```rust
use oa_msaw_run::evaluation::{evaluate, EvaluationError, HazardWindow};

fn score_example() -> Result<(), EvaluationError> {
    let scenarios = [HazardWindow {
        addr: 0x00ab_cdef,
        start_ns: 1_000_000_000,
        warning_due_ns: 2_000_000_000,
        end_ns: 3_000_000_000,
    }];
    // Replace the empty complete log with Alert records emitted by the run being evaluated.
    let result = evaluate(Some(&scenarios), Some(&[]))?;
    if let (Some(timely), Some(late), Some(missed), Some(denominator)) =
        (result.timely, result.late, result.missed, result.encounters)
    {
        assert_eq!(timely + late + missed, denominator);
    }
    Ok(())
}
```

The evaluator refuses malformed intervals, same-address truth windows that overlap (identity is
ambiguous), and work exceeding `MAX_EVALUATION_PAIRS`. It does not infer a window from an alert,
resolve input conflicts by row order, or count an absent evidence source as zero.
