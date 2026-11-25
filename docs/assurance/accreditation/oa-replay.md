# Proposed accreditation record: offline replay

**Status: DRAFT / NOT ACCREDITED / NOT APPROVED FOR system verification.** This describes a test
capability for review; it does not authorize use of its outputs as accepted verification evidence.

## Identification and intended use

- **Capability:** `oa-replay`, implemented in `crates/oa-replay`.
- **Proposed use:** deterministic offline ingestion and replay of specified recorded surveillance
  feeds through the flight-state pipeline for software verification and controlled scenario
  evaluation.
- **Not intended to establish:** live-interface behavior, real-time latency, field availability,
  absolute truth accuracy, facility readiness, or operational suitability.
- **Configuration:** each run must bind the exact source/build, toolchain, parser and pipeline
  settings, input files, ordering, platform, limits, command, and expected outputs.

## Model and limits

Replay advances on recorded frame time rather than a live clock. Feed ordering, malformed frames,
clock steps, stale data, decode refusals, table limits, and drop accounting are behaviors of the
software model. They do not represent every live network condition or operational recovery path.
The shell bounds frame size, read chunk size, retained frame-error detail, and pipeline tables;
these software bounds are not evidence of a facility workload limit.

## Evidence and gaps

The source contains tests for deterministic replay, clock behavior, parsing, tables, and state/input
fingerprints. Source presence alone is not proof execution or cross-architecture verification.
Any credited proof or replay-digest comparison must link a stable public evidence record with its
exact source, inputs, assumptions, platforms, and configuration. This draft did not run tests or
establish an approved qualification package bound to the reviewed source.

Replay cannot establish source authenticity: unkeyed digests detect accidental differences only
when the expected digest is trusted. Recorded file replay does not measure transport delay, live
source outage behavior, or production-scale retention.

## Required entrance and exit evidence

Before credited use, the approved TEMP must define the replay use, feed profiles, fidelity classes,
acceptance limits, and discrepancy handling. Freeze source/build and platform identities, parser
versions, exact inputs, expected outcomes, resource limits, and the execution procedure. An
independent reviewer must check representative ordering and clock anomalies, malformed and dropped
frames, completeness, determinism, and resource behavior against a separately controlled
reference. All failures and omissions remain in the report. Approval roles and signatures are
pending; no authority is assigned in this draft.
