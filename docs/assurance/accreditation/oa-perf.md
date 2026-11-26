# Performance harness capability

**Status:** Proposal, under review.
**Accreditation authority:** External; independent accreditation is pending.

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

### Source-backed checks and known-answer evidence

- Test: `tools/oa-perf/src/capacity.rs::a_small_run_processes_all_traffic_and_reports_every_figure`
  exercises complete synthetic work accounting (OA-PRF-003; introduced by
  `4c5653354519fb1d4941c873e799ebfe5b5ae320`).
- Test: `tools/oa-perf/src/capacity.rs::every_loss_counter_fails_a_run_and_a_clean_run_passes`
  checks that loss counters prevent a pass (OA-PRF-009).
- Test: `tools/oa-perf/src/capacity.rs::one_slow_second_in_a_hundred_is_not_a_pass` checks the
  per-second latency criterion.
- Test: `tools/oa-perf/src/capacity.rs::two_runs_of_one_configuration_count_the_same_things`
  checks repeatable accounting for one synthetic configuration.
- Test: `tools/oa-perf/src/capacity.rs::a_small_run_processes_all_traffic_and_reports_every_figure`
  pins the timing-independent vector, in order: measured seconds, expected and represented
  aircraft, expected and generated ADS-B reports, ADS-B and plot input counts, processed count,
  late and refused plot counts, tracker applied count, output overflow, and confirmed tracks.
  The values are `[20, 100, 100, 0, 0, 1470, 1489, 2959, 0, 0, 2698, 0, 114]`. Its canonical
  digest input is ASCII `OA-PERF-ACCOUNTING-V1`, NUL, then those 13 unsigned 64-bit values in
  little-endian order. The exact 126-byte input is
  `4f412d504552462d4143434f554e54494e472d56310014000000000000006400000000000000640000000000000000000000000000000000000000000000be05000000000000d1050000000000008f0b000000000000000000000000000000000000000000008a0a00000000000000000000000000007200000000000000`.
  SHA-256 is `8475f8cf3e812357b4175f494b915875a9798394ef73a99a819fec8ae68d294b`. It was recorded
  by guarded request `openatca-L1-D1-accreditation-20261126-233932-1b42fd` using
  `oa-perf test:--lib -- --nocapture`. This is a work-accounting golden, not a host-timing golden.
  The output was introduced in source commit `8c4a196d826be9cf6b2a14d2e1c7365c7f44e503`.

The repository does not pin host timing values; they are host- and run-specific. The documented
synthetic radar stress qualification remains a failed qualification, not an accredited result.

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
