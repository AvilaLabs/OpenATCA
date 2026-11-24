# Regional runtime process recovery drill

`oa-regional-session` has an integrated process recovery test at
[`tools/oa-regional-session/tests/recovery.rs`](../tools/oa-regional-session/tests/recovery.rs).
Run it on this host with the pinned toolchain:

```sh
cargo +1.89.0 test --release --locked -p oa-regional-session --test recovery
```

The test is enabled by default and needs no network, FAA input, environment setting, or external
service. Its helper process is `regional-recovery-worker`; that process constructs the same
`RegionalRuntime` used by the CLI, admits the checked-in normalized synthetic fixture through the
real session cursor, and invokes `regional_step` for every full `ReceiptBatch`. The adaptation is
the CLI's deterministic two-fix synthetic model. Each recovery request binds the exact admitted
prefix bytes, encoded adaptation, and explicit centre/range/capacity settings by hash.

## What the drill covers

An uninterrupted process supplies the continuation reference. A serving process applies and
publishes a three-frame prefix that ends during the `SECTOR1` to `SECTOR2` controller handoff.
Replacement generations replay this exact admitted prefix into a fresh runtime. One process is
killed before the next frame is applied; another is killed after applying the receiving-sector
acceptance but before publishing its output; a later process is killed after the independent
output authority has accepted a frame but before the worker receives its acknowledgment. The
replacement replays only the authority's committed prefix, then applies the same suffix. It must
match each reference frame hash, raster pixel hash, and complete frame-state hash. Fixture-level
assertions also check two-target confirmation, flight state, route output, observed-traffic CA,
and stale measurement age.

The output authority is a bounded test object with one current generation, one next sequence, and
one receipt-time watermark. It refuses stale generations, duplicate or skipped sequences, and
watermark regression. Replayed prefix outputs are compared as reconstruction evidence and are not
republished. This is a local single-authority fencing example; it makes no quorum, lease, remote
ownership, or production publication guarantee. Replaying an operator command restores
deterministic command state, but does not establish exactly-once external side effects.

The protocol accepts only bounded tab-separated records. A worker response has a two-second
deadline and an output line is capped at 1,024 bytes. The test exercises a changed fixture hash,
an out-of-range/missing retained prefix, malformed input, a stalled worker, process disconnect
and reconnect, duplicate output, and an old-generation output. Process recovery is recomputation
from retained admitted input and deterministic adaptation bytes. It does not serialize or restore
an in-memory runtime checkpoint.

Two observations are reported separately: wall-clock time across the injected process failures
through the first recovered output accepted by the authority (`availability_gap_ms`), and the maximum
age of the latest accepted surveillance measurement at a frame (`max_track_measurement_lag_ns`).
The first is a same-host synthetic observation, not a failover deadline claim. The second is
source freshness evidence, not process availability.
