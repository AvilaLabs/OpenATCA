# Performance harness capability

**Status:** Proposal, under review.

## Intended use and boundary

`oa-perf` in `tools/oa-perf` and the `oa-track` fusion layer is proposed for controlled engineering
measurements of specified synthetic workloads, bounded clip latency, work counters, memory use,
and explicitly instrumented timing boundaries. It is not evidence of national capacity, a live
facility deadline, radar accuracy, field availability, or production suitability.

## Configuration and admission record

Freeze the workload class and generator, seed, traffic layout, rates, sensor mix, warm-up,
measurement interval, timing boundaries, diagnostic settings, expected work, resource limits, and
acceptance criteria. Bind the record to source revision and clean/dirty state, exact binary hashes,
compiler/toolchain, host and OS, environment, command, output schema, and report hashes.

Prove source-to-binary closure with an isolated empty target directory, exact build invocation,
toolchain, lockfile and resolved dependency manifest, relevant environment, complete build log,
and artifact hashes. A source-tree hash does not prove that cached or stale objects were absent.
Retain raw timing/work outputs, resource records, complete logs, and discrepancies for independent
review.

## Evidence and decision criteria

The source includes checks for workload generation, bounded counters, and selected timing output.
The documented frozen synthetic radar stress procedure completed but failed its declared
work-completion and matching criteria. That is a negative qualification result and remains open;
short timing does not compensate for missing work. A separate diagnostic profile was observer-
affected and is not qualification evidence.

For a future credited use, the approved TEMP must state workload representativeness, loss policy,
duration, memory and CPU bounds, timing boundaries, and acceptance criteria before execution.
Independently review the generator and measurement method, then decide against the frozen gates.
Accept only when the procedure completes all required work and meets all predeclared criteria with
complete identity-bound evidence. Refuse or retain a negative result for incomplete work, exceeded
limits, missing instrumentation, observer effects, incomplete logs, or unbound source/build/input.
Any rerun after a failed criterion needs an approved prospective procedure; do not adjust thresholds
to fit observed outcomes.

## Limits and open questions

Capacity inputs are generated outside the timed region; the measured region includes offering work
and the inclusive fusion-advance call, while output draining is measured separately. Current code
exposes selected shell-boundary timings; requested internal fusion-stage timings remain unavailable.
Observer-heavy diagnostics are not equivalent to uninstrumented runs. Short synthetic runs on one
host cannot establish sustained or facility-wide performance.

Representative national workloads, real-data ingestion, alerts, recovery, long-duration behavior,
and field testing require separate evidence. Resolve or explicitly retain the missing stage-timing
limitation. Capability owner, independent measurement reviewer, and approval authority must be
assigned before another qualification run.
