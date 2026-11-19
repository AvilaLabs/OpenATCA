# Frozen comparison protocol v1

Status: protocol frozen for implementation; results are not yet established. This document
defines what a comparison may claim and how the data are scored. It does not certify the current
tracker or make an operational performance promise.

## 1. Version and claim boundaries

The normative machine-readable companion is `validation/comparison-protocol-v1.json` (version
1.0.3). The initial 1.0.0 draft overlapped a development seed with the qualification set;
v1.0.2 froze initialization and post-initialization scoring; v1.0.3 adds paired-key scoring and
a separate dropout fault cohort before qualification measurements.
These corrections supersede earlier drafts; no qualification measurements have started. A run
MUST identify both files by SHA-256, plus the evaluator revision, candidate binary/build
identity, input-manifest hash, truth hash, configuration hash, and hardware/OS description. A
changed scoring rule, profile, or threshold requires a new protocol version; old results retain
the version under which they were produced.

The current first cohort is `adsb_kinematic_v1`, **controlled ADS-B kinematic tracking**: known scenario and flight
identity, simulated sensor noise/faults or explicitly qualified replay measurements, and a
common truth. It can compare position/velocity tracking, update/coast/missing status, and
receive-to-usable latency. It does not establish anonymous multi-sensor association, radar
fusion, full NAS capacity, live feed latency, or operational suitability. Those require later
profiles with their own evidence.

Benchmark stages are distinct. There is currently no qualified professional tracker artifact or
approved professional-reference run, and no authenticated legacy export. Those reference stages
remain `unknown` until the required evidence is recorded; a Python or controlled legacy output
cannot be labelled a professional tracker:

| Stage | Role | Claims allowed |
|---|---|---|
| `development_control` | Repeated development scenarios and tuning | Debugging and development-only comparisons; never an untouched holdout claim |
| `professional_tracker` | Named, independently qualified tracker reference on the same inputs | Relative component comparison, if provenance and reference qualification pass |
| `legacy_export` | Published/exported legacy product output, replayed with its original timestamps | Exported-output comparison only; no claim about internal sensor-rate state or latency hidden by export |
| `controlled_legacy` | Reproducible legacy-like implementation or behavior model under frozen parameters | Controlled-model comparison only; never labelled as a vendor system measurement |

The words “ERAM”, “STARS”, “ARTAS”, or another named system may be used as a measured baseline
only for an authenticated, permitted export/input set and documented scoring scope. `controlled_legacy`
and `professional_tracker` are not proxies for such a named system. Vendor comparative claims
must state which claims and metrics each stage supports.

## 2. References, inputs, and frozen qualification

The evaluator uses truth, measurements, and outputs in the CSV shape defined in the JSON
manifest. CSV headers are exact, UTF-8, comma-delimited, with one header row; timestamps are
integer nanoseconds on the profile's common monotonic scenario time axis. `scenario_id` and
`flight_id` are identifiers, not inferred from candidate output. Files are immutable during a
run and identified by content hash. Invalid, duplicate, or malformed rows are counted and fail
the strict gate unless a profile explicitly defines their disposition.

For an output row with `status=missing`, `score_ns` and scheduled `publish_ns` are both required
and equal. `source_ns`, `lat_deg`, `lon_deg`, `ve_mps`, and `vn_mps` are empty. The row itself is
required for each missing truth-grid point, so missing output remains in the denominator and
cannot disappear as an absent CSV record. This v1 implementation detail is clarified before any
qualification measurement; it does not change the score grid or thresholds.

Truth references are qualified before scoring: provenance and rights recorded; identity/leg
continuity checked independently of candidate output; coverage and uncertainty evaluated against
profile limits; all rejected truth samples and reasons counted. For simulation, truth is the
scenario generator's latent state and measurement noise uses an independent keyed random stream.
For physical truth, the reference instrument must have documented calibration, time alignment,
uncertainty budget, and independent verification. RTK is **not currently qualified** as an
independent reference. ADS-B-derived truth scored against ADS-B-fed output is correlated evidence,
not independent absolute accuracy.

Seed policy is fixed: development seeds are 0 through 31, defaulting to seed 1, and may be
inspected and used for tuning. Qualification uses seed IDs 1,000,001 through 1,000,100. Each
qualification seed covers 300 aircraft for 1,200 seconds, exactly 10,000 simulated
aircraft-hours total. Twenty seeds are assigned to each of the five nominal scenarios; none are
assigned to diagnostic-only dropout. This allocation fits the runner's per-run limits and the
qualification wrapper repeats the frozen run over all 100 seeds. A separate dropout fault cohort
reserves seeds 1,000,101 through 1,000,120, with 300 aircraft and 1,200 seconds per seed, adding
2,000 simulated aircraft-hours. The fault payload has not been generated or opened. Its required
gates are exact accounting against the frozen dropout schedule with zero unexpected loss, one
explicit status per grid point, no valid output after two seconds of source age, valid recovery at
the first grid after receipt of a fresh report, and known flight identity preserved through the
gap. These fault gates are not yet implemented and independently reviewed; all-quality claims
remain blocked until each gate passes on this separate cohort. It is separate from the nominal
10,000-hour performance set and the seeds are not independent reference observations. Generator,
distributions, and allocation are frozen before candidate freeze. Seeds are scenario replicates,
not independent aircraft-hours or
independent reference observations. They MUST NOT be presented as 100 independent references;
shared world and reference models constrain independence. After all
code, parameters, thresholds, and evaluator artifacts are frozen, the actual holdout is consumed
once. Its per-seed outcomes are not inspected for tuning: any inspection that informs a change
reclassifies the consumed set as development, and a new untouched holdout is required. Report
whether a holdout was untouched, consumed once, or reclassified; do not imply that held-out data
remain untouched after their outcomes have been viewed.

The event/load matrix is predeclared and hashed with the run. It covers, at minimum: geometry
(straight, coordinated turns, crossing, parallel, close spacing, high latitude, dateline and
local-frame boundaries); sensor quality (nominal, bounded noise, bias, quantization, dropouts,
duplicates, out-of-order and late delivery); motion (level, climb/descent, acceleration, turn,
stop/start where applicable); timing (jitter, clock offset, bounded drift, burst delay, reorder);
identity (known stable identity, address collision/change, anonymous identity only in a later
profile); and offered load (nominal, burst, capacity boundary, overload, recovery). Matrix cells
not applicable to this cohort are explicitly marked out of scope, not silently passed.

The score origin is the scenario manifest epoch. Candidate initialization is 2-of-3 distinct
input reports keyed by `event_id`; the development control begins usable after two distinct input
reports. A row is coast when its actual supporting source is more than 2 seconds old. The raw
cold-start and all-grid metrics are always retained. Separate post-initialization metrics exclude
only grid times before `epoch + 2 seconds`. For `straight`, `turn`, `acceleration`, `crossing`,
and `delayed`, post-initialization usable coverage must be at least 0.999, where `valid` is usable,
and the candidate coverage must be at least the baseline coverage. `dropout` is diagnostic only
for v1: the predeclared four absent source reports in twelve seconds yield two coast grid points
in twelve seconds under the fixed coast rule, so the 0.999 threshold is unattainable there. It is
reported in full and excluded from the qualification gate; its role cannot be changed after
candidate results are seen.

For nominal accuracy ratios and their paired intervals, the candidate and baseline must also
have identical sets of usable post-initialization grid keys `(scenario_id, flight_id, score_ns)`.
If the usable key sets differ, retain each system's raw metrics and report the intersection and
difference counts, but block a qualified accuracy claim; equal coverage fractions alone do not
make nonmatching samples comparable.

## 3. Causal truth, clocks, and latency

Truth and measurements share `scenario_id`, `flight_id`, and a common scenario time axis. Truth
is evaluated at the output's `score_ns`; a row is eligible only when qualified truth brackets
that time under the declared profile's interpolation rule and uncertainty bound. No truth is
extrapolated across an unsupported gap. Each output retains `source_ns` (the source measurement
time actually supporting the state) and `publish_ns` (publication time). Output provenance must
be causal: no future measurements, smoothed truth, or holdout labels may enter candidate state.
Smoothers may qualify truth after the run; they may not be used by the candidate.

For deterministic logical replay, scheduled comparisons use one common score grid and require
`score_ns == publish_ns`; this tests state quality at matched logical times and says nothing
about wall-clock latency. Physical elapsed cost is measured separately with a monotonic clock.
Receive-to-usable latency starts at a measurement's `receive_ns` and ends at the first published
valid output causally incorporating that measurement; a held, rejected, or unused event has no
usable completion and is counted by disposition. Replay cost MUST NOT be called live latency.
The evaluator records sensor-clock-to-common-clock calibration and uncertainty. If timestamp
uncertainty is material to a metric, report the uncertainty interval; do not subtract a fitted
lag that could erase real tracking delay. Candidate lag is never refit away. Any permitted fixed
clock correction is independently calibrated, frozen before candidate scoring, and applied
identically to all systems.

## 4. Scorecard and strict integrity gate

The published comparison includes horizontal position RMS and p99 absolute horizontal error;
velocity errors; update interval/coverage and valid/coast/missing fractions; receive-to-usable
latency p50/p95/p99; throughput at the stated quality and resource envelope; peak memory and
CPU; offered, accepted, rejected, held, expired, and otherwise unprocessed input counts; and
output validity and completeness. Report per-flight and per-scenario distributions as well as
pooled statistics. State units, denominators, truth coverage, uncertainty, and missing-data rules.

The strict integrity gate is zero unaccounted work: every measurement has exactly one terminal
disposition, and every required truth-grid output has exactly one status (`valid`, `coast`, or
`missing`). `missing` is explicit and scored; it cannot be omitted from denominators. Duplicates
are separately identified. Any unexplained input, output, identity, or disposition loss fails the
comparison regardless of accuracy or speed. Explicit rejection under a frozen, published policy
is not “lost work,” but is counted and included in quality/availability metrics. A successful
comparison also may not hide a tracking regression in any predeclared scenario/flight guardrail.

Competitive targets are relative goals on the same qualified cohort, inputs, truth, evaluator,
quality gate, hardware class, and resource envelope: 30% lower position RMS; 20% lower p99
position error; half the p99 receive-to-usable latency; and 2x throughput at the same quality and
resources. These are goals, not universal physical guarantees. Each target is reported with its
paired estimate and uncertainty. A target is met only when the prespecified confidence interval
supports the required direction and the strict integrity/no-regression gates pass. A missing or
unqualified reference means “not comparable,” not a pass. The published ESASSP 1.3 Recommended
values remain the normative surveillance floor recorded in `docs/BENCHMARKS.md`; this protocol
does not lower or replace those requirements. Relative goals do not waive absolute floor rows.

## 5. Statistics, exclusions, and anti-tuning controls

Comparisons are paired on scenario and flight. Confidence intervals use a hierarchical cluster
bootstrap: resample scenarios, then flights within scenario, retaining paired candidate/reference
observations. Report the interval method, replicate count, seed set, point estimate, interval,
and counts of clusters and observations. Seed replicates are not treated as independent aircraft
or independent physical references. Publish uncertainty intervals for truth and clock calibration
alongside statistical intervals.

Report all counts for uncertain truth, outliers, malformed rows, exclusions, unmatched outputs,
and every other exclusion reason, both before and after filtering. Eligibility rules are fixed
before candidate results are examined and apply identically to all systems. Outlier handling is
predeclared; primary results include all eligible observations, and any robust/trimmed analysis is
secondary and shown beside the untrimmed result. No post hoc removal based on candidate error,
reference disagreement, speed, or named-system identity is permitted. An empty denominator is
`n=0` / no estimate, never a zero-valued metric.

Each reference and candidate receives the same documented tuning budget: same development data,
number of submitted configurations, compute budget, access to per-scenario development results,
and freeze deadline. Record the budget and any deviations. The qualification set is not a tuning
set. The one-shot holdout is inaccessible for selection and tuning; reveal only the preregistered
aggregate after the frozen run. If holdout results lead to any change, label them development and
start a new holdout. Baselines may not be deliberately weakened, starved of inputs, or run with a
different resource cap to manufacture a margin.

## 6. How the tools hold the freeze

The SHA-256 of `validation/comparison-protocol-v1.json` is a constant in
`tools/comparison_evaluator.py` and `tools/run_comparison.py`. If the file does not match it, the
evaluator gives no thresholds and the status `unqualified_protocol_hash_mismatch`; a run cannot
qualify. Every threshold the evaluator applies is read from the verified file, not written in the
code. A run started from a source tree with uncommitted changes is `exploratory (source tree not
clean)` and can never pass. `run_comparison.py` exits 0 only for a completed run; every other
status, including a failed reference, exits 3. Every pooled accuracy figure carries its coverage:
rows used, rows expected and their ratio.

## 7. Current evidence boundary

No qualification measurement has been made under this protocol. The synthetic comparison runner
(`validation/oa-comparison-runner`) is not in the repository yet: it needs tracker work counters
that have not landed, so `run_comparison.py` stops with "missing runner crate" before writing
anything. There is no qualified independent RTK reference. Component tests need no external
data; a comparison whose source is unavailable stays unmeasured and does not change unrelated
component evidence. No result may claim one of these gaps closed before its frozen gate has
evidence.
