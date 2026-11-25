# Turn-onset detection delay (development only)

Status: source draft, 2026-11-24. No day has been scored. November 20–22 remain sealed. The
scorer and optional adapter source are present but await coordinator review and queued validation;
no metric result is ready to claim.

## Metric definition

This is observable ground-track-course-rate turn onset. It is distinct from `tools/motion_study.py`
`mu_ct`, which inspects OA's internal coordinated-turn model state. It uses no filter model
probability or internal turn flag. It makes no claim about aircraft heading, bank angle, or what a
controller perceives. All courses are ground-track course from east/north horizontal velocity.

Reference: the existing ADS-B smoothed reference built from split B, sampled at each UTC second.
It is a retrospective proxy, not independent physical truth; the smoother uses future and past
reports. The truth event is the first sample whose **signed** wrapped forward course rate is at
least 0.5 degrees/second in one direction for three consecutive one-second samples, after a
straight baseline of at least ten seconds with absolute course rate below 0.25 degrees/second.
Truth ground speed must be at least 30 knots. The onset label is the first sample of the
three-sample run, backdated only after the next two reference samples confirm it. This future
lookahead labels reference events only.

Estimator detector: derive signed wrapped course rate from each pair of successive, distinct
source/state-time updates, dividing by their actual elapsed time. Use the same 0.5 degrees/second
threshold, three consecutive updates in one direction, and the ten-second straight / 0.25
degrees/second re-arm rule. This is elapsed time, not a count of repeated held samples. Alarm time
is the third distinct update, never the first backdated sample. An update gap over 30 seconds
resets state. Direction is positive clockwise/right course change and negative
counterclockwise/left. Match only alarms of the same direction.

The primary result is an algorithmic source-time proxy, not physical observable detection latency.
OA/CV ticks use ADS-B measurement applicability time (`toa_ns`); the archive reader does not
retain per-report ADS-B receive times. Offline tracker tick time is a state timestamp, not a
wall-clock emission time, and a late measurement may cause a backdated state tick. SFDPS `pos_ms`
is its position applicability time. Preserve SFDPS recording receive time (`recv_ms`) separately
as an availability clock and report its cadence and receive-minus-position lag where valid. The
primary delay excludes SFDPS delivery time and ADS-B delivery time; it must never be presented as
an apples-to-apples physical detector latency comparison. No internal ERAM sensor/update rate or
pre-SFDPS processing latency is inferred.

OA is the normal tracker output. CV is an independent single straight-model constant-velocity
Kalman baseline using the same split-A ADS-B measurements (not dead reckoning renamed CV, and not
the IMM output). The existing tracker implementation supports a dedicated straight model with no
model mixing and no coordinated-turn state: `TrackerConfig.h_mode = 1`, whose fixed white
acceleration density is `oa_track::params::CV_ONLY_Q[0]` (the existing `H_Q_CV` value). Copy all
other scoring-run settings from the same `scoring_config`; do not tune them. CV's tracker instance
must receive the same measurements, timestamps, availability window and quality adapter as OA.
Record the exact configuration and input split hash. This provides an auditable CV baseline without
a new dependency or a new filter implementation.

SFDPS is ERAM's published output. Use `pos_ms` as source/state time and `recv_ms` as availability.
If `recv_ms` is `NO_TIME`, keep the source-time row with blank availability; do not substitute
`pos_ms` or invent publication time. If `recv_ms < pos_ms`, keep source state, blank availability,
and count the contradiction. Deduplicate multiple rows by address, truth leg and `pos_ms`; retain
identical courses and the earliest valid `recv_ms`. If courses conflict, mark that state ambiguous
and omit it from estimates rather than choosing one. Count duplicates and ambiguous states.
Report valid SFDPS receive cadence and receive-minus-position lag. No claim is made about ERAM's
internal sensors, internal update rate, processing delay or latency before SFDPS publication.

## Shared events and denominator

Every eligible reference event is a common event for OA, CV and SFDPS. A missing/uncertain
reference sample in the ten-second straight interval or through the 30-second delay horizon is a
reference-gap event, counted once for every system. Do not exclude an event because an estimator
has no output or misses it. Missing estimator rows/gaps are a per-system availability diagnostic
and remain `never_detected` if there is no same-direction alarm. This diagnostic checks for no
updates or a state-time gap over 30 seconds; it does not guarantee that three distinct updates are
present to satisfy detector persistence. `missing_output_events` is a subset of the shared
denominator and may overlap `never_detected`; the report must state this.

For each event, match the nearest unused same-direction alarm in [-5,+30] seconds. A negative
delay is an early detection. An opposite-direction alarm in this interval cannot match and is
counted in `opposite_direction_misses`, a subset of `never_detected`; the opposite alarm remains
among false turns unless matched to a later eligible opposite-direction event. Alarms within
censored reference-gap event windows are counted as `gap_associated_alarms` regardless of alarm
direction and excluded from false turns because truth cannot classify them. Count remaining
unmatched alarms under the protocol field name `false_turns` and report them per reference
flight-hour. This is an unmatched-alarm count under the declared proxy definition, not proof of a
physical false turn or detector error. Truth labels are gated at 30 knots, while the observable-
output detector has no speed gate because comparable output speed is not part of this frozen input
schema; consequently low-speed output alarms may contribute to `false_turns`. This truth/detector
eligibility asymmetry must be disclosed. Report
the shared event denominator, reference gaps, missing output events, gap-associated alarms,
matches, never-detected events, opposite-direction misses, early detections, false turns and the
median/p90 of signed delay among matched events. Keep per-leg counts in coordinator evidence.
The terminal event partition is `shared_truth_events = truth_reference_gaps + matched + never_detected`.
Missing-output and opposite-direction diagnostics overlap this partition as stated above. Delay
summaries alone are not a success metric.

## Files and bounded scorer

`tools/turn_onset.py` consumes two CSVs:

```text
truth.csv:     leg,time_ms,truth_course_deg,truth_speed_kt
estimates.csv: leg,state_time_ms,system,course_deg,availability_ms
```

Truth has a complete 1 Hz grid (blank course/speed values explicitly represent missing truth).
Estimate rows are distinct source/state updates; no forward-filled or interpolated rows.
`state_time_ms` is OA/CV tick state time or SFDPS `pos_ms`. `availability_ms` is blank for OA/CV
and SFDPS `recv_ms` where valid. Preserve both SFDPS fields and counts in the companion provenance
file. All courses
are degrees in `[0,360)`. Circular differences use [-180,180), so crossing north (359 to 1 degree)
is a 2 degree turn. Unique timestamps per leg/system are mandatory. Files are capped at 32 MiB
and 250,000 rows each; matching is capped at five million precharged work units. Each event prepays the full upper
bound of five scans/removals over its unused alarm list, and gap classification charges each
alarm/event visit before processing. Gap cleanup builds a retained list in one pass. All parsing and
matching work is offline and uses only the standard library.

The `vs_eram` example emits these files plus `turn-onset-provenance.txt` only when passed
`--turn-onset --turn-from-ms <utc-ms> --turn-to-ms <utc-ms>`. The required integer millisecond
endpoints must be increasing, whole-second aligned, and outside every sealed span; the interval is
half-open `[from,to)`. The ordinary comparison does not run the additional CV baseline or turn
exports. This bounds turn exports to one predeclared window while the OA, CV and truth estimators
still process their full source history for warmup. The export contains only selected-window rows;
the selected truth grid is complete at 1 Hz within each available reference leg. Report boundary
conditioning: the detector and truth label can arm only from baseline evidence in the selected
window, so events whose required straight baseline precedes the window are not eligible labels.
It builds truth from
split B, exports OA ticks, runs the existing `h_mode=1` straight-only CV filter as a separate
instance over the same split-A measurements and exports its ticks, then exports all associated
SFDPS state rows at `pos_ms` with `recv_ms` separately. It records unassociated rows, no-truth,
no-velocity, near-zero directions, missing/contradictory times, duplicate states and conflicting
courses in provenance. Ambiguous conflicting course states are omitted from the estimate stream.
The provenance records both endpoints and selected/outside row counts for truth and each system.
The adapter is bounded to 250,000 rows per export and the scorer to 32 MiB / 250,000 rows per
input. A capacity refusal is a hard stop: it must never be represented as a complete day. Freeze a
predeclared development window that fits all caps or revise caps through review before scoring;
do not silently batch and combine incomplete denominators. Its matcher uses a shared truth
denominator. The input files and
provenance hashes/cadence must be frozen before scoring any development day. Do not tune from the
resulting observations. A fresh authorized hold-out is required for any external or performance
claim; this draft authorizes no hold-out use.

## Commands queued for the coordinator

No command here has been executed. After source review, Sol should validate the synthetic source,
then run the diagnostic adapter and scorer on development-only inputs and inspect the outputs:

```sh
python3 -m unittest tools/test_turn_onset.py
cargo run --release -p oa-track --example vs_eram -- --adsb <development-clip.oaf> --sfdps-dir <development-sfdps-dir> --out <turn-onset-output-dir> --turn-onset --turn-from-ms <utc-ms> --turn-to-ms <utc-ms>
python3 tools/turn_onset.py --truth <turn-onset-output-dir/turn-onset-truth.csv> --estimates <turn-onset-output-dir/turn-onset-estimates.csv>
```

Run coordinator workloads serially under the workspace's memory guard. Archive code/input hashes,
day/window, SFDPS cadence, CV configuration, dedup counts, truth events and all denominator rows
with the development result.
