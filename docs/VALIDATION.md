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
| Distinct identities and ambiguity stay visible | Existing address, Mode A, close-aircraft and cross-radar cases; exact-truth pair study; counted wrong, false, missing and duplicate tracks | A small study does not establish a whole-day identity-error rate |
| Lost work cannot produce a capacity pass | Every stats field classified; full per-second evidence; every measured second under budget; every measured-window loss counter zero | Results apply to the stated machine, load, layout, seed and measurement window |
| Overload is detected | The same traffic with deliberately insufficient track capacity must report counted loss and refuse a capacity pass, even if its process exits zero | This checks the detector, not capacity at every load |
| Traffic maintains the advertised load | Whole-horizon coverage and position tests, continuous speed and bounded turn rate; recorded generator configuration | Simulated radar remains a model of sensor behavior |
| Report fields remain machine-readable | Commas, quotes and newlines in text fields follow CSV escaping; an independent reader checks the study outputs | Tables have section-specific numeric columns |
| The observed synthetic accuracy meets its stated targets | Small exact-truth study, with nonempty samples and the published position/speed/heading thresholds | Too few samples for an ESASSP exit result; a missed target stays a failure |
| The core allocates nothing after initialization | Allocation instrumentation with a demonstrated positive control and explicit path coverage | **OPEN:** buffer stability alone does not establish C5; incomplete instrumentation cannot pass |
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
python3 tools/validate_candidate.py --repo /path/to/checkout \
  --revision FULL_COMMIT_SHA --target-dir /path/to/build-cache \
  --out /path/to/new-evidence-directory \
  --reference-env /path/to/explicit-reference-paths.json --clip /path/to/clip.oaf
```

The runner uses only Python's standard library. It runs builds and checks sequentially with one
Cargo build job, captures command output, and records evidence in `run.json`. The evidence
directory must be new and outside the checkout. Reference paths are explicit; no recording
directory is discovered. A missing clip is NOT_RUN. The full data suites can skip when their
reference paths are absent, so a test-suite pass must be read with its recorded data manifest.

The fixed capacity cases are stress layout, seed 41, three radars (`e,t,t`), 60 measured seconds:
1,200 and 2,400 aircraft with half ADS-B, and 1,200 radar-only aircraft. The first two warm up for
60 seconds, the radar-only case for 120. The negative control has 1,200 aircraft and only 40
track slots. Each actual per-second row and every required loss counter are checked independently
of the tool's headline verdict. These are measured cases, not a NAS-wide capacity claim.

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
