# Offline two-sector controller workflow

This is a deterministic simulation of a handoff between sectors `ZAU12` and `ZMP34`, using the
existing `oa-flight` state transitions. It is not connected to a control system. `HO_FAIL` is the
explicit timeout event supplied by the scenario; the flight core has no clock or automatic
timeout policy. The scope replay tool owns recovery and is not part of this fixture.

Run evidence for the fixture is asserted in
[`controller_workflow.rs`](../crates/oa-flight/tests/controller_workflow.rs):

| Step | Flight 901 | Flight 902 |
| --- | --- | --- |
| Initial | Owner `ZAU12`; no handoff | Owner `ZAU12`; no handoff |
| Offer to `ZMP34` | Owner stays `ZAU12`; offered | Owner stays `ZAU12`; offered |
| Receiving sector accepts | Owner stays `ZAU12`; accepted by `ZMP34` | — |
| Duplicate acceptance | Unchanged; one accepted event | — |
| Late acceptance after completion | Refused because no handoff is pending; owner remains `ZMP34` | — |
| Old but valid offer delivery | Refused as stale; completed transfer stays closed | — |
| Receiving sector takes control | Owner becomes `ZMP34`; handoff clears | — |
| Offer timeout (`HO_FAIL`) | — | Owner stays `ZAU12`; handoff clears |
| Acceptance after timeout | — | Refused because no handoff is pending |

The test also audits the table invariants and exact completion, timeout-failure, and refusal
counters. Sector ownership remains a single value on each flight throughout the scenario. The
NAM session's LAM/RLA/PLA responses acknowledge messages at the transport coordination layer;
they do not transfer a flight. In this workflow only the explicit `OP_HO_TAKE` flight operation
changes sector ownership. The legacy comparison is a declared interface boundary only: the
scenario exercises the internal
flight operation model and does not claim wire compatibility or compare against an authenticated
legacy emulator. NAM parsing/session envelopes remain available in `oa-nam`, but introducing a
new protocol adapter or dependency is outside this fixture.

The [replacement drill](../crates/oa-flight/tests/controller_workflow.rs) retains the synthetic
operation prefix for Flight 901, including its active plan, assigned altitude of 36,000 ft, interim
altitude of 24,000 ft, owner `ZAU12`, and handoff accepted by `ZMP34`. A fresh `FlightTable` replays
that prefix and matches the uninterrupted table's digest and current fields. Both tables refuse a
delayed interim-altitude change, transfer ownership to `ZMP34` on `OP_HO_TAKE`, and refuse a
repeated take without another completion. The fixture exercises deterministic reconstruction and
continuation through the flight table API with a retained synthetic operation sequence.

`oa_display::ownership` exposes a pure typed rendering seam for one selected flight: the caller
supplies the current owner plus `Current`, `Offered`, `Accepted`, or `StaleOffer`. An accepted status
still displays the existing owner alongside the accepting sector. A NAM logical response cannot be
passed as an ownership input. The public replay builders reserve one bounded ownership text slot
up front and accept an optional status value; omitting it preserves existing frame pixels. A valid
line that does not fit is omitted and reported as `FrameInfo::ownership_clipped`; malformed input
or a display-list failure is returned as a replay error. The display and replay crates do not
depend on `oa-flight`, so the caller remains responsible for mapping a flight record and refusal
evidence to this presentation value.


## Process takeover with the scope

The combined offline drill runs the tracker and scope in two separate processes and runs a
flight table beside each. A third flight table and an uninterrupted scope provide the
reference. The fixture's flight record carries the full address of a displayed target. The
controller witness supplies the owner, the accepted handoff, and both altitude clearances.
The test uses the owner and accepting unit from that witness in the scope's ownership line.

At the last synchronized record, the test kills both serving processes. The standby then
renders the accepted handoff and applies the take-control input. It refuses the repeated take.
The fixture sink refuses old-generation output and already accepted sequence numbers. The
drill checks the resulting frames against uninterrupted playback, reports the source-time
gap, and measures the local wall time until the first verified frame.

Build the controller fixture from the same revision and use its exact executable path from
the build's compiler-artifact output.

```sh
cargo +1.89.0 test --release --locked -p oa-flight --test controller_workflow --no-run --message-format=json
OA_CONTROLLER_TAKEOVER_EXECUTABLE=/absolute/path/to/controller_workflow-binary \
  cargo +1.89.0 test --release --locked -p oa-scope-replay --test outage_drill \
  controller_handoff_and_scope_continue_after_serving_processes_are_killed \
  -- --ignored --exact --nocapture
```

This evidence covers OS process failure on one host with a fixture that owns publication
authority. The two-host exercise requires a test transport and independent machines.
