# Candidate validation

This is the acceptance gate for a bounded research benchmark, covering recorded Chicago Center
traffic, the scoring harness and the surveillance tracker. It does not establish operational
readiness, certification, absolute accuracy or whole-NAS capacity. The milestone criteria in
[SCOPE.md](../SCOPE.md) remain in force.

The initial source is `e84d93383a2a8c668a2b227210ec3220636854e9`, retained by the local
`validation-baseline-20261116` tag. A result belongs to its exact source, toolchain, binary,
configuration and input hashes. A successful command exit alone is not a passing result.

## Checks and limits

| Claim | Check and expected evidence | Remaining limit |
|---|---|---|
| Results identify what was run | Clean source at the requested revision; build with `OPENATCA_SOURCE_REV`; retain source, binary, input and output hashes and exact arguments; check source again at the end | A scoring-source digest alone does not identify the tracker binary |
| The scorer recovers known errors | Existing synthetic bias, time-offset, dropout and swap tests; repeated report bytes; independent common-time tie and tolerance-boundary fixture | Synthetic recovery does not establish real-world truth accuracy |
| Invalid clock values leave valid evidence usable | Impossible advance followed by valid reports; invalid finish/flush leaves live tracks, pending input and output unchanged, apart from one counted refusal | Valid clock jumps still follow the documented clock and caller contracts |
| Distinct identities and ambiguity stay visible | Per-source close-aircraft cases with absent, equal and conflicting Mode A; retained anonymous hypotheses and identity-warning flags; counted wrong, false, missing and duplicate tracks | A source-local hypothesis is not a distinct aircraft identity; a small study does not establish a whole-day identity-error rate |
| Initiation work is bounded across radars | Low-budget lookup/query/retirement cases; partial-pair rollback; unchanged scratch buffers after exhaustion and recovery; charged work and explicit refused-plot counters | Separate association and initiation allowances; other paths still need work arguments |
| Lost work cannot produce a capacity pass | Every stats field classified; full per-second evidence; every measured second under budget; every measured-window loss counter zero | Results apply to the stated machine, load, layout, seed and measurement window |
| Overload is detected | The same traffic with deliberately insufficient track capacity must report counted loss and refuse a capacity pass, even if its process exits zero | This checks the detector, not capacity at every load |
| Traffic maintains the advertised load | Whole-horizon kinematics and coverage; actual unique generated aircraft offered in the measured window; zero unknown input labels | Input coverage does not establish tracking accuracy or identity coverage |
| Report fields remain machine-readable | Commas, quotes and newlines in text fields follow CSV escaping; an independent reader checks the study outputs and per-metric class counts | Tables have section-specific numeric columns |
| The observed synthetic accuracy meets its stated targets | Small exact-truth study, with nonempty samples for each eligible metric class and the published position/speed/heading thresholds | Too few samples for an ESASSP exit result; variants can emit different cohorts; a missed target stays a failure |
| Nonlinear turn prediction follows its stated integration rule | Closed-form spread, discovery-floor and mirror cases; independent high-precision mean/second-moment fixtures; direct conditional-Gaussian reference on captured states | Cubature and the separate discovery regularization are approximations, not an exact nonlinear posterior or a calibrated covariance upper bound |
| Numerical failure cannot disappear as absent work | Typed forecast failures, counted tracker/fusion refusals, atomic history replay and per-field loss checks | The legacy optional forecast API cannot distinguish reasons; callers needing that distinction use the checked API |
| Named M3 paths make no allocator calls after initialization | Isolated counting allocator, standard-API positive controls, injected-allocation negative control, required tracker/fusion route evidence, source and binary fingerprints | Scoped to the named fixtures and generated program; excluded shell helpers and other inputs are unproved |
| The complete real-time core meets C5 | Extend allocation instrumentation and explicit path coverage across real-time crates | **OPEN:** the scoped M3 check and buffer stability do not establish the complete claim |
| Recorded ADS-B results reproduce | Explicit authorized clip, split and input counts, output digest, exclusions and repeated execution | Report-disjoint samples still share correlated GNSS error; disagreement is not an absolute-error bound |
| Acceptance data was not used for tuning | Recorded provenance and prior-use declaration for a reserved traffic window | **OPEN:** splitting a known recording does not make it an untouched acceptance dataset |
| ERAM and OpenATCA share a fair comparison | Authorized matched recordings, the same truth/scorer and common-time grid, with all exclusions | **NOT RUN:** an ADS-B-only tracker report does not establish this comparison |
| Numerical evidence covers the claimed domain | Exact or high-precision references, stated tolerances and convergence checks | Unresolved numerical cases remain a prioritized queue; passing vectors are not a full-domain proof |

Statuses are **PASS**, **FAIL**, **NOT_RUN** and **OPEN**. PASS means the named check actually ran
and its assertions hold. FAIL means an executed check violated them. NOT_RUN means execution or
required input is absent. OPEN means evidence or an argument is missing. Research benchmark
release readiness stays false while a mandatory claim is failed, not run or open. Passing the
software regression gate permits publishing corrected prototype source; it does not close the
benchmark acceptance gate.

## Running

From a clean checkout, with Rust 1.89.0 available:

```text
python3 tools/test_validate_candidate.py
python3 tools/test_allocator_validation.py
python3 tools/validate_candidate.py --repo /path/to/checkout \
  --revision FULL_COMMIT_SHA --target-dir /path/to/build-cache \
  --out /path/to/new-evidence-directory \
  --reference-env /path/to/explicit-reference-paths.json --clip /path/to/clip.oaf
```

The runner uses only Python's standard library. It runs builds and checks sequentially with one
Cargo build job, captures command output, and records evidence in `run.json`. `--target-dir` is a
cache root outside the checkout; the runner derives `checkouts/<sha256-of-resolved-checkout-path>`
under it for all Cargo builds, allocator artifacts, and binary checks. Different checkout paths
therefore never share build artifacts, while repeated runs of the same resolved checkout may reuse
them. A symlink alias resolves to the same checkout namespace. The evidence directory must be new
and outside the checkout. Reference paths are explicit; no recording directory is discovered. A
missing clip is NOT_RUN. The full data suites can skip when their reference paths are absent, so a
test-suite pass must be read with its recorded data manifest.
Manual comparisons also use a separate `CARGO_TARGET_DIR` for each checkout: a shared directory
can reuse a test executable from a different worktree.

The fixed capacity cases are stress layout, seed 41, three radars (`e,t,t`), 60 measured seconds:
1,200 and 2,400 aircraft with half ADS-B, and 1,200 radar-only aircraft. The first two warm up for
60 seconds, the radar-only case for 120. The negative control has 1,200 aircraft and only 40
track slots. Each actual per-second row and every required loss counter are checked independently
of the tool's headline verdict. These are measured cases, not a NAS-wide capacity claim.

The runner also invokes `tools/check_allocations.py`. This separately builds the excluded
`validation/oa-alloc-check` package, checks its isolation, runs its tests, and requires both a
zero-allocation normal run and a detected injected allocation. The scoped fixtures include the
default 4,096-track tracker capacity, small output/log overflow and reuse, and clock-gate hold,
forward confirmation and recovery, plus one `MAX_BUCKET_PLOTS` overload with counted loss and
subsequent existing-track recovery, and full ADS-B and plot pending queues with counted overflow,
drain and recovery. A source-backed route ledger requires positive runtime evidence and
injected-allocation detection in each armed window. The tracker fixture exercises history-ring
wrap, late replay and late-ring retention refusal. Fusion association-work/pair, candidate, record
and registration-history boundaries remain outside this scope. See the package's [scope and
fixtures](../validation/oa-alloc-check/README.md). A missing route, control or fingerprint fails
this scoped check. Its narrow unsafe-code exception is recorded in RULESET A0; product crates
retain their unsafe-code prohibition.

Each synthetic accuracy CSV metric row has an adjacent `<section>|samples` row with the same
label. Its nine integer columns are position, speed and heading counts, each in
`all, straight, turn` order. Eligible classes must be nonempty; the position all-count must match
the metric row's `n`. These counts describe emitted, matched samples for that variant. For the
fixed radar-only terminal case, the frozen `9536ef8` baseline's turning-speed counts differ
across variants (10, 35 and 71); the nonlinear turn predictor retains the original 71 and adds
four. Their unmatched RMS values alone do not establish comparative superiority. Common-sample
diagnostics do not replace the acceptance cohort or its unchanged target.

Each base-study variant also reports a `<section>|loss` row with named counts for
`tracker.numerical_refused`, `fusion.prediction_refused` and `tracker.rows_unrepresentable`.
These counts cover the whole run, once per world, independently of score subsets. The synthetic
accuracy gate requires all three counts to be present and zero for every eligible run. Historical
tables without these rows provide no evidence for these loss counts.
The gate's eligible rows are the all-aircraft `b` radar-only runs and `b`/`c` mixed-sensor runs.
Other variants and score subsets retain their reported target misses; passing this scope does
not establish that every subset meets every published study bar.

The input-load check requires every configured aircraft to appear in the measured inputs, using
ADS-B addresses and radar truth labels in the shell. Repeated sightings count once and unknown
labels fail the check. Confirmed track count is diagnostic: anonymous hypotheses can duplicate
one aircraft, so it cannot establish input load or distinct-aircraft coverage. This replaces the
earlier exact-track-count proxy; it does not change the traffic, timing or zero-loss thresholds.

Exit 1 means an executed check failed. Exit 2 means checks completed with mandatory claims still
open or not run. Exit 0 is reserved for an accepted scope with all mandatory claims closed;
the currently unverified claims cannot be overridden with a command-line flag.

## Candidate changes

Each fix gets a new candidate revision and a regression that fails on the original runtime code.
Expected answers and fixture assumptions are reviewed separately: an invalid fixture is not a
production defect. Check the same failure pattern in the affected module, then rerun the
dependent checks. Source, input, metric, generator or configuration changes invalidate the
evidence that depends on them. Run the full workspace gate when promoting a candidate.

Do not silently repin numeric answers, counts or digests. Explain every changed expectation.
Independent reviewers inspect the implementation, the expected answers and the retained evidence;
they do not approve their own fixes. No number of passing tests, surviving source changes or
clean review rounds substitutes for closing the declared claims.

The frozen baseline failed the invalid-time state-preservation checks and CSV field checks.
Those failures and their repairs must remain linked in the candidate evidence. Its small radar-only
study also missed the turning-speed target; that observation requires a separate disposition and
must not be hidden by successful unit tests or fast capacity runs.

The turn model integrates ten equally weighted spherical-radial points from its stored covariance
through the existing motion map to predict the mean. A second ten-point set applies the existing
turn-discovery variance floor and supplies the second moment about that mean, before process
noise is added. This replaces a first-order approximation that can overstate velocity under
uncertain turn rate, while keeping recurring discovery regularization from shifting the state
mean. The initial turn-rate prior, process noises, floor value and accuracy targets are unchanged.
The two sets do not describe one common posterior distribution when the floor is active, and
their second moment is not a statistically calibrated upper bound. Numerical factorization
failures refuse work explicitly. History updates roll back a partial replay, and fusion counts
failed eligible forecasts separately from absent tracks. These changes require a new candidate
run; prior-source results do not establish their accuracy or performance.
