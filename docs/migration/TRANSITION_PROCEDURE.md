# Transition procedure

This procedure says how one function moves from the legacy system to OpenATCA at one facility. It
covers the steps, who decides each one, and how a move is undone. It applies to every swap unit
defined in `docs/MIGRATION_CORE.md`: one function, at one scope, for one window.

It is written to be adopted by a facility, not used as it stands. A facility replaces the role
names in section 2 with its own positions and adds its own local steps. Where this procedure and a
facility order disagree, the facility order governs.

## 1. Principles

1. **Legacy stays hot.** Until a function is retired (S5), the legacy system runs, holds full
   state, and is one action away. Undoing a move is a rollback, never a recovery.
2. **The switch is at the output.** A move changes which system feeds a consumer. It changes no
   state in either system.
3. **No move without evidence.** Each rung starts only when the previous rung's exit criteria are
   met, with numbers, and the evidence package has been reviewed.
4. **Anyone with authority can stop it.** Rollback needs one person and one action. It needs no
   meeting and no justification in advance.
5. **The software enforces what it can.** `oa-migration` refuses a promotion that lacks evidence or
   approvals. `oa-migration-switch` refuses a flip while a blocking mismatch is open. This
   procedure covers what software cannot: who the people are, and what they do.

## 2. Roles and decision authority

| Role | Decides | Cannot decide |
|---|---|---|
| Facility supervisor (operations) | Promotion to the next rung for a unit, within an approved plan. Starting and ending each window. Rollback at any time. | Retiring legacy (S5). Changing exit criteria after a unit has started. |
| Independent assessor | Whether the evidence package meets the exit criteria. Their rejection is a veto on promotion. | Starting a window. Overriding a rollback. |
| Technical operations (Tech Ops) | Rollback for a technical fault. Releasing quarantined inputs. Certifying the service (I1) before each window. | Promotion. |
| Controller on the position | Requesting a rollback, which the supervisor executes at once. Reporting any difference they see. | — |
| Controllers' representative | Agreeing the exit criteria before a unit starts. | Promotion of a single unit. |
| Facility retirement authority | Retiring legacy for a function and scope (S5). | — |
| OpenATCA project | Supplying software, drill results, and the generated evidence package. | Anything operational. The project holds no authority at any rung. |

`oa-migration` checks that the supervisor and the independent assessor are two distinct
principals for every rung, and that S5 also has the retirement authority. It checks identifiers,
not identities. Authenticating those people is the facility's responsibility (E1, E4).

## 3. Before a unit starts

1. **Define the unit.** Record the function, scope (sector or position set), window start and end,
   and the rung. Do this as a unit record, not prose.
2. **Set the exit criteria.** For each metric in MIGRATION_SPEC section 6 and each shadow
   comparison in section 7, record the threshold, its direction, the minimum sample and the minimum
   exposure. Agree them with the controllers' representative. Record the policy digest. Once the
   window opens, the criteria do not change. A changed criterion makes a new unit.
3. **Generate the evidence package.** Run `tools/migration_evidence.py` for the unit. The package
   carries the hazards for the function, exposure hours, every shadow difference by class,
   quarantine events, rollbacks and drill results (H3). Any difference classed "worse" or
   "unresolved" blocks the unit until it is resolved and reclassified.
4. **Assessor review.** The independent assessor reviews the package against the exit criteria and
   records an approval or rejection.
5. **Certify the service.** Tech Ops runs performance verification and records certification (I1)
   for every processor in scope. The software and adaptation digests per processor must match the
   issued manifests (G2). "Not certified" stops the unit.
6. **Brief the controllers.** Controllers working the scope know the window, what changes, how to
   request a rollback, and the differences list for the function. For S2 and later, they have
   already trained on their facility's recorded traffic at S0 and S1.
7. **Notify interfaced facilities.** At least the facility's agreed notice period before an S2 or
   later window, tell every interfaced facility the function, scope and window. The switch also
   emits a coordination record to each configured interfaced facility immediately before the flip
   (6343e5c). That record supplements this notice and does not replace it.

## 4. Starting a window (the flip)

1. The supervisor confirms that steps 3.1 to 3.7 are recorded.
2. The reconciliation check runs for the scope: every track, flight plan, handoff and active alert
   in both systems. A flip is refused while any blocking mismatch is open. The supervisor does not
   override a refusal. They resolve the mismatch or cancel the window.
3. The supervisor executes the flip. The selector records the decision time and the first output
   from OpenATCA.
4. Within one update interval, the supervisor and the controller on the position confirm that the
   display shows the same traffic, data blocks and alert states as before the flip.
5. The flip and its continuity metrics (MIGRATION_SPEC section 6) are recorded with the unit.

## 5. During a window

- Legacy runs throughout and its outputs continue to the shadow comparison.
- Tech Ops watches health and the quarantine queue. A quarantined input is handled manually and
  does not by itself trigger a rollback.
- Any controller observation is logged with its time, whether or not it leads to a rollback.

## 6. Rollback

**Triggers.** Any one of these:
- a blocking difference in the live comparison;
- a request from the controller on the position, for any reason;
- a health fault, as judged by Tech Ops;
- the supervisor's judgment, for any reason.

**Steps.**
1. The supervisor (or Tech Ops, for a technical fault) executes the rollback, which is the same
   flip in reverse. Legacy never stopped, so it holds full state.
2. The switch emits a coordination record to each interfaced facility. The supervisor also tells
   them directly, by the facility's normal coordination means.
3. The controller confirms that the display is continuous.
4. The rollback, its trigger and its continuity metrics are recorded with the unit.

A rollback never needs approval and is never treated as a failure of the people who called it.
The unit's next window does not start until the cause is classified in the evidence package.

## 7. Ending a window and promotion

1. At the window's end, the supervisor flips back to legacy unless the unit's rung is S4.
2. The evidence package is regenerated with the window's live record.
3. The assessor reviews it against the exit criteria for the next rung.
4. Promotion follows the ladder in MIGRATION_SPEC section 4, one rung at a time:
   - S2: one off-peak window.
   - S3: a new window of at least 48 hours, then runs of 24 hours over one to two weeks.
   - S4: a new window of at least 7 days before continuous operation.
   `oa-migration` refuses any skipped rung, stale evidence, or reuse of an expired window.

## 8. Retiring legacy (S5)

S5 is a separate decision by the facility retirement authority. It is never automatic and never
follows from S4 by elapsed time. Before S5:
- the S4 evidence covers at least 7 days of continuous operation;
- every interfaced facility has been told of the date, through the facility's own process;
- the facility has decided how it will recover if OpenATCA fails after legacy is gone, since
  rollback is no longer available.

## 9. Records

For each unit, keep:
- the unit record, policy digest and source-epoch digest;
- the evidence package and the assessor's decision;
- the certification record for each processor;
- the notice to each interfaced facility;
- every flip and rollback, with its continuity metrics;
- controller observations.

`oa-migration` keeps an append-only audit of each accepted and refused transition. The facility
keeps the rest under its own records policy.

## 10. What this procedure does not do

It does not authorize any live operation. Live inputs, live windows, the site adaptation, the
decision authority for each rung, neighbour agreements, and retiring legacy all belong to the FAA
and the facility (MIGRATION_SPEC section 14). This procedure is what OpenATCA brings to that
decision: a written method, enforced in software where possible, and already exercised in drills
on recorded traffic.
