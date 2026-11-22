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
| Stale acceptance delivery | Refused as stale; owner remains `ZMP34` after transfer | — |
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
