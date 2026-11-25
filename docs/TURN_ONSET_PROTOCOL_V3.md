# Turn-onset delay protocol v3 (experimental)

Status: post-development, outcome-informed experimental definition, 2026-11-25. Earlier v2 scoring
of two frozen two-minute 14Z development windows yielded zero shared truth events. That observation
motivated examining a gradual-transition event definition and a longer development window. V3 is
therefore not a preregistered, outcome-independent comparison. It is not a validated metric,
accuracy claim, or basis for changing a frozen hold-out. No new data export or result is authorized
by this document. The scorer remains `v2` by default; use `--protocol v3` only for a separately
authorized analysis. Existing v2 definitions/results remain archived and must not be relabeled as
v3.

## Purpose and estimand

V3 changes only the transition between an established straight interval and a sustained turn-rate
event. It makes that state transition explicit so a gradual ramp through the midband can qualify.
This does not establish that the smoothed reference is physical truth, that the estimator is
accurate, or that a new event set is preferable. Any V3 comparison is an experimental
source-time algorithmic proxy and must be reported under this version label.

This is ground-track course, derived from east/north horizontal velocity. It is not aircraft
heading, bank angle, pilot intent, controller perception, or OA's internal coordinated-turn model
state (`mu_ct`). The reference is the retrospective split-B ADS-B smoothed course, sampled on its
complete 1 Hz grid. It uses future and past reports and is a reference proxy, not independent
physical truth. Future reference values can confirm labels only; they are never supplied to OA, CV,
SFDPS detection, or detector decision times.

## Frozen thresholds and event times

- Truth/reference eligibility requires a finite course, finite ground speed of at least 30 knots,
  and consecutive one-second reference samples. A missing/invalid sample or speed below 30 knots
  clears state and cancels any unconfirmed transition.
- A qualified straight interval is ten elapsed seconds with every signed wrapped course-rate
  sample satisfying `abs(rate) < 0.25 degrees/second`. The rate at a sample is the circular
  course difference from its immediately preceding valid sample divided by the actual elapsed
  time; reference intervals must be exactly one second. The first valid rate interval begins at
  the prior sample time. Qualification is checked at the prior sample time when a departure rate
  arrives: the departure interval itself cannot complete the ten-second straight baseline.
- After qualification, the first same-direction rate with `abs(rate) >= 0.25 degrees/second`
  starts a transition candidate and is the candidate's departure/onset timestamp. Rates from 0.25
  up to but not including 0.5 of that sign preserve the candidate; they do not count as turn
  confirmations.
- Confirmation requires three consecutive same-sign rates with `abs(rate) >= 0.5
  degrees/second`, reached no later than 30 elapsed seconds after the first departure rate. A
  same-sign midband rate resets the consecutive confirmation count but leaves the candidate
  alive. A candidate can therefore ramp through 0.3 and 0.4 before three threshold rates.
- For truth, future reference samples confirm the candidate and the truth event time is the first
  departure rate (first rate at or above 0.25), backdated only after the three confirming rates.
  For an estimator, the alarm time is the third distinct observable update that confirms the
  threshold run. The detector is causal; it never backdates its alarm.
- A rate below 0.25 returns to straight state: cancel any candidate and start a new quiet interval
  at the beginning of that low-rate interval. A same-direction candidate is not preserved through
  a quiet return. A rate at or above 0.25 in the opposite direction cancels the candidate and
  clears straight qualification; it does not immediately start a candidate in the new direction.
  A candidate whose next same-direction update is more than 30 seconds after departure expires
  and clears straight qualification. At exactly 30 seconds it may still confirm.
- Invalid truth support cancels a truth candidate. A missing/nonconsecutive truth time interval
  cancels it and resets the straight qualification. For output updates, a distinct-state gap over
  30 seconds cancels the candidate and resets the state; the gap itself is not interpolated.
- Cancellation precedence in implementation is: invalid truth row / invalid or excessive input
  gap; after an update, quiet return below 0.25; opposite direction at or above 0.25; then
  candidate timeout; then same-direction persistence. These outcomes are counted as transition
  diagnostics, not truth events or estimator misses.
- After a confirmed event, the active-maneuver latch prevents repeated events until ten elapsed
  seconds of consecutive rate intervals below 0.25 have re-armed the state. Non-straight or
  midband rates reset that re-arm interval while active.

All rates are signed circular ground-track rates in `[-180,180)`. Positive means clockwise/right
course change; negative means counterclockwise/left. The candidate direction must remain the same
through confirmation. Matching remains nearest unused same-direction alarm in `[-5,+30]` seconds;
opposite-direction alarms cannot match. Circular wrapping must work in both directions across
north.

## Synthetic interpretation

With a constant course from `t=0..10`, speed 120 kt, and then one-second rates
`+0.1,+0.2,+0.3,+0.4,+0.5,+0.6,+0.7 degrees/second` at `t=11..17`, V3 retains the qualified
straight state through the first two sub-0.25 intervals, starts its candidate at `t=13` (`+0.3`),
and confirms at `t=17` after the three threshold rates at `t=15..17`. Truth time is `t=13`; a
causal detector alarm is `t=17`. This is an explanatory synthetic case, not a score or tuning
observation.

## Comparison, support, and caveats

Use the same V3 truth events for OA, CV, and SFDPS. An eligible event with missing/uncertain truth
support in its preceding ten-second baseline or through the existing 30-second delay horizon is a
shared reference-gap event. Estimator output absence does not exclude truth events. Keep
`truth_reference_gaps`, `missing_output_events`, `never_detected`, direction misses, early
detections, matched delays, gap-associated alarms, and unmatched-alarm counts. Reference-gap
alarms within `[-5,+30]` seconds are censored regardless of direction. The terminal event
partition remains `shared_truth_events = truth_reference_gaps + matched + never_detected`, with
the documented overlapping diagnostics retained.

V3 does not change source timing or make the three systems' delay clocks physically comparable:
OA/CV archive tracker ticks use ADS-B applicability time and lack a per-report ADS-B receive clock;
SFDPS has its own position applicability (`pos_ms`) and recorder receive (`recv_ms`) clocks. Keep
the estimator's distinct output cadence visible. Three distinct qualifying updates are required;
stale held SFDPS states cannot add confirmations. The existing >30-second estimator-gap rule
remains in force. The availability diagnostic (no output or a >30-second time gap) does not
guarantee three updates in the transition or confirmation window.

Truth speed is gated at 30 knots; the detector has no comparable speed gate in its frozen
course-only input schema. Low-speed output alarms may enter `false_turns`. That field means
unmatched alarms under this proxy definition, not established physical false turns or detector
errors. Report this asymmetry, reference support gaps, per-system output cadence, and source-time
clock limitations with every V3 result.

Capacity remains explicit and bounded. The standard profile is 250,000 rows and 32 MiB per CSV
for both the Rust exporter and Python scorer. It remains the default for v2 and v3. The optional
extended profile is four times those bounds: 1,000,000 rows and 128 MiB per CSV. In the Rust
example it requires `--turn-onset-extended-budget` together with `--turn-onset`; in the scorer it
requires `--extended-budget --protocol v3`. The Python scorer refuses extended mode for v2, and
neither tool accepts limits beyond the extended hard maximum. The Rust exporter checks row count
and actual accumulated UTF-8 CSV bytes before appending each truth, OA, CV, or SFDPS data row. It
formats each row through a bounded 512-byte scratch writer, so an oversized formatted row is
refused before it can grow the export buffer. The Python reader retains the production sealed-path
and per-row nanosecond checks and additionally counts bytes read from the guarded file descriptor
through EOF, not only the earlier path-size estimate. Before `csv` constructs a row dictionary,
the parser caps each physical UTF-8 record at 1,024 bytes, each record at 16 columns, and each field
at 512 characters. This matches the producer's scalar, single-line CSV schema and refuses
multiline quoted records; it bounds parser scratch independently of the whole-file cap. The
five-million-work matching bound,
schemas, event logic, windows, and matching rules do not change. The Rust provenance records the
selected profile, row/byte caps, and actual CSV byte lengths.

The Rust exporter and Python scorer run in separate sequential phases; do not add their allocations
as if simultaneous. For planning only, Python's two tables can contain up to two million materialized
rows total. At a rough 2 KiB per row this is about 3.82 GiB, plus parser/scoring scratch. The Rust
phase has up to 256 MiB of logical CSV bytes, a bounded SFDPS dedup map, and its existing clip,
tracker and SFDPS-loading working sets. These estimates are not measured RSS or formal allocator
bounds; the existing tracker/loader working sets are not bounded by this export profile. The
coordinator's 6 GiB / zero-swap guard must cover each actual phase. A guard kill or capacity refusal
is incomplete and cannot be reported as a complete comparison. Do not claim the total job fits
under 6 GiB from the export caps alone; record high-water memory for each authorized exporter and
scorer phase under the guard before declaring the comparison complete. The fourfold profile is not
unlimited capacity.

`--protocol v3` adds named per-leg/aggregate state diagnostics for transition candidates started,
confirmed, cancelled by quiet return, reversal, timeout, invalid truth, or gap, and candidates
pending at the end of a leg. Cancelled or pending transition candidates are not qualifying truth
events and must not be counted as detector misses. V2 output schema and default behavior remain
unchanged; V3 prints its protocol identity and V3-only diagnostic lines before the shared CSV
summary. Its standard capacity profile is also the default; extended limits require the explicit
V3 flag.

The previously selected two-minute 14Z windows had no shared reference events. The coordinator's
follow-up development window is ten minutes, `[14:00:00,14:10:00)Z`, on each of the same two
development days, frozen after the V3 source is frozen. This window expansion follows the zero
event-support observation and must be described as outcome-informed. The CSV adapter exports only
the selected half-open interval. Consequently, truth candidates near the end may lack the required
30-second confirmation/support tail; the current scorer will classify events lacking samples
through `onset+30s` as reference-gap events. That is window censoring, not evidence of missing
source observations. Report and interpret the terminal interval accordingly; do not call these
source gaps or detector misses. The first ten seconds likewise cannot label an onset without
baseline evidence inside the selected window. These edge conditions do not justify moving the
window after inspecting outputs.

## Optional scoring timing profile

The `vs_eram` Rust development/export example accepts the opt-in
`--timing-profile fixed-10-30-120-v1` only with `--turn-onset`; `turn_diag` accepts the same option
for its development tracker replay, whether or not `--rescore` is set (`--rescore` controls output
scoring and retention). It assigns expected interval 10 seconds, initiation interval 30 seconds, and
drop interval 120 seconds to both OpenATCA and its straight-only CV baseline; only `h_mode` differs
between those tracker configurations. The fixed profile is selected from versioned constants and
does not inspect the measurement interval distribution. The export provenance and text report
identify the profile, source, and exact nanosecond values. Without the option, the examples retain
their existing measurement-derived timing and empty-distribution fallback. This development
profile does not register a strict metric or alter V3 event definitions, metric definitions,
windows, thresholds, matching, or scorer defaults. It is not causal calibration: reference
smoothing and state-time fitting remain retrospective. Report the chosen profile with any
development result.

## Evidence boundary

V3 is an explicit, bounded design change motivated by the zero-event v2 development result. The
existing 0.25/0.5 degrees/second thresholds, ten-second baseline, three-sample confirmation, and
30-knot reference gate remain fixed; the new 30-second transition maximum is tied to the already
declared delay/update-gap budget. The version tests a defined alternative event-support rule and
cannot be presented as independently preregistered or better-performing. Coordinator review,
source validation, and separately authorized development analysis are required before interpreting
any V3 numbers. No hold-out read, frozen-C change, production-tracker change, or performance claim
is authorized by this protocol.
