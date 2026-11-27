# OpenATCA Operating and Support Hazard Analysis

Status: draft 1, 2026-11-27. Owner: Avila Labs. Not reviewed or approved by the FAA.
Procedures analysed: the transition procedure (`docs/migration/TRANSITION_PROCEDURE.md`), Tech Ops
(`docs/design/TECH_OPS.md`) and the Program Safety Plan, as of commit c15b5ca.
Data: `docs/safety/analysis/osha.csv`.

The SSHA and SHA cover how the software and its connections fail. The Operating and Support
Hazard Analysis (O&SHA) covers how people using and looking after it can go wrong:
- cutting a function over from the legacy system and promoting it through the stages;
- issuing and changing adaptation;
- certifying the service;
- handling alarms, the clock and records.

## 1. Method

The analysis covers 35 tasks from the procedures. For each task it records:
- who does it;
- the document that defines it;
- what can go wrong;
- the hazards it contributes to;
- the control that prevents or catches the error.

Where a procedure does not say who does a task, or what the control is, the analysis says
UNKNOWN, with the source lines it checked.

## 2. Results

We grouped the tasks as follows. The grouping is ours; the data lists each task.

| Group | Tasks | Main hazards |
|---|---:|---|
| Cutover and promotion: defining the unit, fixing exit criteria, evidence, review, briefing, notice, reconciliation, the flip, rollback, the stage promotions S0 to S5 | 19 | HZ-CUTOVER-001, HZ-EVIDENCE-001 |
| Adaptation: issuing, field changes, verifying, fleet check, activation, restoring | 6 | HZ-AIRSPACE-001 |
| Certification: performance verification before cutover, full and abbreviated performance verification, the certification decision | 4 | HZ-NET-STATE-001, HZ-CA-001 |
| Alarms, clock, thresholds, records | 6 | HZ-SURV-TIME-001, HZ-DISPLAY-001, HZ-ARCHIVE-001 |

The cutover is the best-defined part. Each stage needs:
- fixed exit criteria agreed before the window;
- an evidence package;
- review by an independent assessor, who can veto;
- a briefing for controllers;
- notice to neighbouring facilities;
- reconciliation of tracks, plans, handoffs and alerts before the flip.

Rollback is one reverse flip while the legacy system still runs. A worse or unresolved
difference from the legacy function blocks promotion.

### 2.1 Tasks with no defined control

Nine tasks have a control that the procedures do not define. In five of them, nobody is named to
do the task either.

| Task | Who | What is missing |
|---|---|---|
| Restore the prior adaptation after a failed field change | Not named | No restore procedure at all |
| Activate processors after an adaptation release | Not named | No activation procedure |
| Acknowledge or clear an alarm | Not named | No acknowledgement control or record |
| Daily clock comparison against a GPS-derived reference | Not named | Only the per-window comparison is defined |
| Export or import service, adaptation and evidence records | Not named | Retention is defined, export and import are not |
| Release quarantined inputs | Tech Ops | The release control |
| Field adaptation change | Issuer and approver | The exact procedure, its reversal and approval criteria |
| Promote S0 to S1 | Facility supervisor | Stage-specific thresholds |
| Start the S0 shadow window | Facility supervisor | Entry criteria for S0 |

In our judgement, the first three matter most:
- **Adaptation restore and processor activation.** These are how a facility recovers from a bad
  adaptation change, and wrong or stale adaptation is a catastrophic hazard candidate
  (HZ-AIRSPACE-001).
- **Alarm acknowledgement.** An alarm cleared for the wrong instance, or with no record, can
  leave a controller without a warning of a degraded state. How serious that is depends on the
  alarm: for a conflict alert or MSAW alarm, it touches a catastrophic hazard candidate.

## 3. What this analysis sets

1. **Write the missing procedures.** Each of the nine gets:
   - a named role;
   - the steps;
   - the record it leaves;
   - where software can enforce it, a check that refuses the wrong action. One example is
     refusing activation when a processor's loaded adaptation does not match the issued digest.
2. **Adaptation restore is a tested drill.** The restore of the prior adaptation is run end to
   end as a drill, the way rollback is. Processors that do not report the restored digest are
   found by the fleet check.
3. **Alarm acknowledgement leaves a durable record.** The record holds who acknowledged, which
   alarm instance, and when. An alarm whose condition still holds cannot be cleared.
4. **Thresholds stay under profile control.** This is already required: a threshold change
   without a profile review is refused.

## 4. Limits

- The tasks come from OpenATCA's own procedures. Facility procedures, controller procedures and
  FAA orders that govern the same work are not analysed here. They are assessed with the FAA at
  each site (Program Safety Plan, section 9).
- Human error rates are not estimated.
- The hazards are the function-level hazards in the log. Task-level causes are recorded in the
  data, not split into separate hazards.
- No FAA review of this analysis is recorded.
