# Offline replay capability

**Status:** Proposal, under review.

## Intended use and boundary

`oa-replay` in `crates/oa-replay` is proposed for deterministic offline ingestion and replay of
identified recorded surveillance feeds through the flight-state pipeline. Candidate uses are
software verification and controlled scenario evaluation against a separately defined reference.
The result describes this software pipeline and the admitted feed; it does not establish live
network behavior, transport delay, absolute truth, field availability, or facility readiness.

## Configuration and admission record

For each credited run, freeze the feed profile and ordering, exact input identities and hashes,
parser/pipeline settings, source revision and tree cleanliness, compiler/toolchain, build command,
platform, limits, expected outputs, and report schema. Record frame-time policy, malformed-frame
handling, clock-step treatment, stale-data rules, decode refusals, table bounds, and drop accounting.

The admission package must bind the executable to an isolated clean build: empty target directory,
exact invocation, toolchain, lockfile and resolved dependency manifest, relevant build environment,
complete build log, produced artifact hashes, and source-to-artifact provenance. A source revision
or binary digest without dependency and build closure is not sufficient. Preserve the run command,
exit status, complete outputs, resource record, and all discrepancies.

## Evidence and decision criteria

The source contains tests for deterministic replay, clock behavior, parsing, tables, and state/input
fingerprints. Source presence is evidence of implemented checks, not an execution record. The
reviewer must inspect a run-specific evidence package and independently compare representative
ordering and clock anomalies, malformed and dropped frames, completeness, determinism, and resource
behavior to a controlled reference. The TEMP must define the use-specific acceptance limits before
execution. The decision records accepted, rejected, or restricted use and the reasons.

Refuse admission if any source/build/input/reference binding is missing, target isolation or build
closure cannot be shown, input completeness is unknown, expected outcomes or limits were set after
execution, outputs are incomplete, or a material discrepancy remains unresolved. Preserve the
failed or refused run as evidence; do not relabel it as a pass.

## Limits and open questions

Recorded replay advances on frame time rather than a live clock. Feed ordering, malformed frames,
clock steps, stale data, decode refusals, table limits, and drop accounting are behaviors of the
software model, not a representation of every live network condition or recovery path. Shell bounds
on frame size, read chunk size, retained error detail, and pipeline tables are software limits, not
facility workload limits.

Unkeyed digests detect differences only when the expected digest is trusted; replay does not prove
source authenticity. It does not measure transport delay, live-source outage response, or
production-scale retention. A representative feed qualification, live ingestion, recovery testing,
and approval roles need a separate plan and evidence.
