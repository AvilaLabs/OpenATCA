# Proposed accreditation record: `oa-perf`

**Status: DRAFT / NOT ACCREDITED / NOT APPROVED FOR system verification.** This record does not
qualify national capacity, operational latency, or radar performance.

## Identification and intended use

- **Capability:** `oa-perf`, implemented in `tools/oa-perf` and the `oa-track` fusion layer.
- **Proposed use:** controlled engineering measurements of specified synthetic workloads, bounded
  clip latency, work counters, memory use, and explicitly instrumented timing boundaries.
- **Not intended to establish:** whole-system national capacity, a live-facility deadline, radar
  accuracy, field availability, or production suitability.
- **Configuration:** bind source/build, compiler, host and OS, seed, workload layout, rates, sensor
  mix, warm-up, measurement duration, limits, diagnostic settings, and command for every run.

## Measurement model and limits

The synthetic capacity workload simplifies traffic geometry, movement, and sensor schedules. In
the capacity path, per-second inputs are generated outside the timed region; the measured region
includes offering the work and the inclusive fusion-advance call, while output draining is timed
separately. The current implementation exposes only selected shell-boundary timing measurements;
requested internal fusion-stage timings remain unavailable. Observer-heavy diagnostic runs are not
equivalent to uninstrumented runs. A short synthetic run on one host cannot establish sustained or
facility-wide performance.

## Evidence and deficiencies

The source includes tests for workload generation, bounded counters, and selected timing output.
No tests were run for this draft. A frozen synthetic radar stress procedure completed, but its
declared work-completion and matching qualification criteria failed. This remains a negative
qualification result; fast timing cannot override missing work. The result does not demonstrate
operational radar detection rates. A separate diagnostic profile was observer-affected and is not
qualification evidence. Neither finding is a reason to change thresholds after seeing outcomes.

## Required entrance and exit evidence

The approved TEMP must define workload classes, workload fidelity, loss policy, duration, memory
and CPU limits, timing boundaries, and acceptance criteria before testing. Independently review the
generator and measurement method; freeze source/build, host, seed, workload, diagnostic profile,
and output validation. Treat the failed stress criterion as unresolved until causes are reviewed
and a separately approved rerun meets the predeclared gates. Complete or explicitly retain the
missing stage-timing limitation. Separate representative national workloads, real-data ingestion,
alerts, recovery, long-duration, and field testing require their own plans and evidence. Capability
owner, independent measurement reviewer, and approval authority remain unassigned.
