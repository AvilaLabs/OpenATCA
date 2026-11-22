# Offline machine-loss drill

Run with:

```sh
cargo test -p oa-scope-replay --test outage_drill serving_process_loss_promotes_synchronized_scope_replica -- --nocapture
cargo test -p oa-scope-replay --test outage_drill durable_prefix_recovers_in_fresh_process_and_refuses_damage -- --nocapture
cargo test -p oa-scope-replay --test outage_drill visible_tracker_ca_alert_survives_silent_durable_replacement -- --nocapture
```

The test launches two child copies of its own Rust integration-test executable. Each child
consumes the same synthetic track measurements at the same source seconds and runs the real
`oa-track` tracker, `oa-scope-replay` row adaptation, `replay_build_with_safety` display path,
and `oa-ca::ca_cycle` conflict alert core. Both replicas receive the same three-report tracker
warmup before the fixed 15-step synthetic stream, so the first compared frame contains confirmed
tracks.
The parent also runs an uninterrupted in-process reference. At source second 10, the serving OS
process is killed after both child replicas agree on their frame digest and Mode S address list.
With no records delivered during the synthetic outage, the standby is promoted and resumes at
second 40, followed by recovery seconds 41 through 43. Every resumed frame is checked against the
reference digest, visible identities, and emitted FEED/PICTURE/MSAW warning count. The CA fixture
has an early conflict pair that is open at the takeover checkpoint and closes after promotion,
plus a second pair that opens after recovery. It records ordered `OPEN` and `CLOSE` identities
(pair addresses and first-alert time), the currently open alert identity, and compares every
event token on every frame with the uninterrupted reference. Both pre-kill and post-recovery
opening transitions are required. Each worker validates the fixed 15-step stream and command
order. Child protocol lines are limited to 1 KiB, responses have a 60-second deadline, and
shutdown is bounded. The test prints the 30-second logical frame gap separately from wall-clock
time from process kill to the first promoted frame.

This proves deterministic replay and process-loss behavior for this test slice. It does not
implement a production supervisor, durable checkpoint transfer, network feed reconnection,
fencing, or system-level failover. CA rows are synthetic and independent of the visible tracker
targets; the tracker/display path remains active on its own synthetic identities. The test-only
`oa-ca` dependency is approved under F1. No callsign conflict or terrain warning source is present,
so those warning types are not exercised.
No live or sealed input is opened.

The companion test `durable_prefix_recovers_in_fresh_process_and_refuses_damage` adds a bounded,
test-only recovery checkpoint: up to three synthetic source steps in the existing OAF1 frame
format, wrapped in a versioned header and SHA-256 digest. A fresh child process validates the
digest, frame CRCs, record count, order and source timestamps before replaying the prefix through
the same tracker, scope, display and CA path. Its output and alert identities are compared with an
uninterrupted in-process reference at each source second, then the child continues the suffix.
The fixture also retains a publisher/source identity, generation, sequence and source-time
watermark. A newer timestamp cannot revive a lower generation, a same-generation sequence replay
is refused, and only a caller-supplied higher-generation step can advance the copied fixture
fence. Fixture sequence numbers continue across generations. Recovery checks that replayed prefix
state matches the stored fence before continuing.
The file is exclusively created and `sync_all` completes before the serving process is killed;
the reader uses `oa_sealed::open`. Wrong-digest and internally truncated prefixes are refused by
the fresh worker and reported with a reason. Source timestamps are synthetic; the test does not
measure wall-clock recovery time or claim power-loss durability, production checkpoint
compatibility, a production publisher-generation schema, or operational failover.

The separate `visible_tracker_ca_alert_survives_silent_durable_replacement` fixture obtains its
CA rows from the identities actually present in the scope target set, then retrieves those
confirmed tracks with `tracker_predict`; coordinates, velocity, covariance, altitude, vertical
rate and coast flags come from those rows. The two distinct synthetic addresses have finite
velocity, altitude and zero vertical rate, and their latitude inputs differ by 0.008 degrees
(about 0.9 km); the fixture CA horizontal/vertical thresholds are 1,300 m and 300 ft. It requires
that alert to open before process loss, remain active across an 18-second source-silent frame, and
match after a fresh process replays the durable prefix and resumes. This is a synthetic geometry
check, not a claim about operational traffic or controller display of CA alerts.
