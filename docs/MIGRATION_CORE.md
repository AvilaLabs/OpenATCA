# oa-migration core contract (H1)

`oa-migration` evaluates one explicitly identified swap unit. Its key is the tuple
`function_id`, `scope_id`, `window_start_ns`, and `window_end_ns`; IDs are caller-owned stable
values, and time is signed nanoseconds. The core stores no event/flight identity and cannot select
an output. H2 event identity remains the responsibility of the comparison lane.

Metric evidence names the swap-unit key and its own observation interval separately. The
observation interval may precede the planned operation window (for example, S0/S1 shadow evidence
used to qualify an S2 window); it still must satisfy its policy, source-epoch, completeness and
freshness checks. For operational stages S2–S5, each metric also binds the unit and activation
that authorized the measured exposure. Its interval must begin after that stage's recorded
activation and remain within the origin unit's window. A continued unit may use evidence from its
explicit prior-unit dependency only when the evidence identifies that exact origin receipt; it
cannot relabel pre-activation observations as later-stage exposure. S0→S1 is preparation and may
be evaluated before the planned window begins.
Transitions into S2, S3, S4 or S5 are window-scoped and must be evaluated in the half-open
`[window_start_ns, window_end_ns)` interval.

The transition ladder is strictly adjacent: S0 Shadow, S1 Advisory, S2 Window, S3 Extended,
S4 Continuous, S5 Retired. Promotion requires numeric evidence measured at the current stage,
matching exact unit key, policy version/digest and source-epoch digest. Each metric must satisfy its
predeclared direction/threshold, sample and denominator minima, exposure minimum, evidence
observation interval and maximum age. A sample count may not exceed its denominator. A dependency requirement must have a matching stage receipt, policy and
source epoch, within the freshness and dependency-window bounds. Every transition policy also
declares enabled rollback rules for blocking differences, controller requests, and health faults,
with permitted roles and a nonzero response-time target. These are declared targets; M1 does not
operate an output switch or assert that a rollback completed. M2 owns execution and delivery
receipts. Limits are fixed constants for evidence,
criteria, dependencies, approvals and audit capacity; over-cap input refuses.

S3 requires a new unit window of at least 48 hours; entering S4 requires a new 7-day unit window.
Evidence to leave S3 must cover at least 48 hours, and evidence to retire from S4 must cover at
least 7 days. `new_unit_continuation` accepts only the opaque result of a successful prior
transition, requires the next same-function/scope window to start no earlier than the prior
window's end, and embeds a required prior-unit dependency receipt. The next transition policy must
include that exact receipt in its dependency gates. This prevents an expired S2 window from
authorizing longer exposure by itself. The policy digest is an opaque handle: this core does not
hash or authenticate the policy
record. `TransitionResult` and `ContinuationStart` are opaque at the public boundary, so callers
cannot manufacture a successful progression token. Likewise, artifact and authority handles identify records retained by an external process;
their contents/signatures are not validated here. The caller must retain and review those records,
provide the approved policy and source-epoch identifiers, and supply deterministic `now_ns`.
`evaluate_transition` appends each accepted or refused attempt to a fixed-capacity `AuditLog`; its
private fields prevent overwriting earlier entries through the public API. It increments the
unit's audit sequence for refused attempts too, so callers must keep the returned unit even when
its stage did not advance. If the log is full it returns `AuditFull` without advancing or appending.

Every rung requires a facility-supervisor and a distinct independent-assessor principal. S5 also
requires the separate facility-retirement-authority role. Principal IDs are identifiers, not
authentication. This is an input consistency check over supplied approvals, not an identity or
credential service. Controller-representative and technical-operations roles can be added as
predeclared policy requirements. The independent assessor's required approval models a veto by
making its absence or rejection fail closed.

Refusals return the unchanged stage and a reason-bearing audit event. A successful result returns
the next unit value and event; this function performs no I/O, clock read, network operation, or
output switch. It provides no claim about the truth of source measurements, live readiness, zero
downtime, or facility authorization beyond the external evidence and approvals supplied.
