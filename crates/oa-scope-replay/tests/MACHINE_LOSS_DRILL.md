# Offline machine-loss drill

Run with:

```sh
cargo test -p oa-scope-replay --test outage_drill serving_process_loss_promotes_synchronized_scope_replica -- --nocapture
cargo test -p oa-scope-replay --test outage_drill durable_prefix_recovers_in_fresh_process_and_refuses_damage -- --nocapture
cargo test -p oa-scope-replay --test outage_drill visible_tracker_ca_alert_survives_silent_durable_replacement -- --nocapture
cargo test -p oa-scope-replay --test outage_drill crash_after_apply_before_ack_replays_from_verified_prefix -- --nocapture
cargo test -p oa-scope-replay --test outage_drill checkpointed_apply_before_ack_recovers_and_continues -- --nocapture
cargo test -p oa-scope-replay --test outage_drill poison_input_is_quarantined_before_replacement_application -- --nocapture
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

The opt-in `partitioned_handoff_promotes_standby_and_fences_reconnected_primary` test composes
that bounded checkpoint and fence with the two transport launchers and the `oa-flight` controller
worker. Both controller copies first reach the accepted-handoff state while the old generation
serves three synthetic frames. The primary then stays alive without receiving records while the
standby replays the synced prefix at the next generation. The standby takes control and publishes
each remaining synthetic frame; after the first promoted frame, the old primary reconnects, makes
the same local handoff transition and emits the same source records. The sink accepts exactly one
publication per source sequence and rejects every old-generation publication. All accepted scope
output is compared with a local uninterrupted reference, and each accepted ownership witness names
`ZMP34` as the sole current owner.

Run this local, offline control with launchers pointing at the same test executable. The checkpoint
path is passed through the standby launcher, so both it and the worker must run on this host. The
60-second response and exit deadlines, 1 KiB protocol-line cap, three-record checkpoint, fixed
15-record stream and generated temporary checkpoint bound all waits and data.

```sh
cargo +1.89.0 test --release --locked -p oa-flight --test controller_workflow --no-run --message-format=json
cargo +1.89.0 test --release --locked -p oa-scope-replay --test outage_drill --no-run --message-format=json
OA_CONTROLLER_TAKEOVER_EXECUTABLE=/absolute/path/to/controller_workflow-binary \
OA_MACHINE_LOSS_TRANSPORT_LEFT_LAUNCHER=/absolute/path/to/outage_drill-binary \
OA_MACHINE_LOSS_TRANSPORT_LEFT_ARGS=$'--exact\nmachine_loss_worker_process\n--nocapture' \
OA_MACHINE_LOSS_TRANSPORT_RIGHT_LAUNCHER=/absolute/path/to/outage_drill-binary \
OA_MACHINE_LOSS_TRANSPORT_RIGHT_ARGS=$'--exact\nmachine_loss_worker_process\n--nocapture' \
  cargo +1.89.0 test --release --locked -p oa-scope-replay --test outage_drill \
  partitioned_handoff_promotes_standby_and_fences_reconnected_primary -- --ignored --exact --nocapture
```

This is a same-host synthetic transport-partition model: the primary's command stream is withheld
while the standby starts from the checkpoint. It does not cut an OS/network link, prove remote
checkpoint delivery, establish production publisher authority or leases, or test external side
effects. The flight owner and publication fence are independent test fixtures; together they verify
the stated transition and stale-generation rejection without claiming an operational controller
failover implementation.

The `crash_after_apply_before_ack_replays_from_verified_prefix` fixture restores a three-step
synthetic prefix into a fresh child, processes the next tracker-to-scope/raster/CA step, and exits
before returning its success acknowledgement. A distinct bounded marker records that the injected
point was reached and includes the computed raster and CA identity; it is not an acknowledgement.
A second fresh child restores the same retained prefix, replays that step, and continues. Its
ordered outputs are compared with an uninterrupted in-process reference. In this fixture all
effects are process-local; no external publication or durable post-apply effect can escape. This
shows deterministic reconstruction from the last fixture checkpoint, not production exactly-once
processing or a rule for resolving an externally visible but unacknowledged effect. That ambiguous
case remains open. The fixture uses synthetic source times and makes no wall-clock recovery claim.

The `checkpointed_apply_before_ack_recovers_and_continues` fixture starts a child from a verified,
synced two-step visible-CA prefix, then applies the third step through the same
tracker/scope/raster/CA path. Before exiting without the success ACK, the child creates a separate
bounded checkpoint containing the three-step synthetic input prefix with `create_new`, writes it
and calls `sync_all`. The following marker includes the checkpoint digest and applied output
identity. The parent observes the marker and worker EOF. A fresh worker verifies that exact file
through the existing checkpoint reader, replays all three steps, then continues the suffix against
the uninterrupted reference. A separate restored worker refuses redelivery of step three because
it already appears in the saved sequence.

The checkpoint stores a bounded, replayable synthetic input prefix. The drill checks process
ordering at the file-sync and acknowledgement boundary. Directory-entry durability, power-loss
recovery, external effects, and operational supervisor policy require separate evidence.

The separate `visible_tracker_ca_alert_survives_silent_durable_replacement` fixture obtains its
CA rows from the identities actually present in the scope target set, then retrieves those
confirmed tracks with `tracker_predict`; coordinates, velocity, covariance, altitude, vertical
rate and coast flags come from those rows. The two distinct synthetic addresses have finite
velocity, altitude and zero vertical rate, and their latitude inputs differ by 0.008 degrees
(about 0.9 km); the fixture CA horizontal/vertical thresholds are 1,300 m and 300 ft. It requires
that alert to open before process loss, remain active across an 18-second source-silent frame, and
match after a fresh process replays the durable prefix and resumes. This is a synthetic geometry
check, not a claim about operational traffic or controller display of CA alerts.

The `poison_input_is_quarantined_before_replacement_application` fixture injects an explicit
failure before applying one normal synthetic measurement. Its quarantine record binds the fixture
publisher/source, generation, sequence and SHA-256 payload digest; the record is synced before the
replacement starts. The replacement-side input gate rebuilds the accepted prefix and rejects
changed bytes under the same identity. For the exact retry, it sends only a sequence-skip control;
the poison payload never reaches the replacement worker. The worker then continues with the next
source sequence. Later warning and CA output is compared with an independent reference
that also omits the quarantined measurement. This tests failure-before-application only; it does
not resolve ambiguous crash-after-application, provide power-loss durability, or implement a
production supervisor, source identity, publisher authority or quarantine format.
