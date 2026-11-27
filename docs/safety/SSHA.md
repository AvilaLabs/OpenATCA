# OpenATCA Subsystem Hazard Analysis

Status: draft 1, 2026-11-27. Owner: Avila Labs. Not reviewed or approved by the FAA.
Source analysed: `AvilaLabs/OpenATCA` at commit c15b5ca. Data: `docs/safety/analysis/ssha.csv`.

The Subsystem Hazard Analysis (SSHA) asks, for each component, how its outputs can fail. For each
failure it asks whether anything notices, what happens next, and whether a test proves the check
works. It is one of the safety deliverables in the Program Safety Plan (section 8). This draft
records what the code does today. Its main result is the list of gaps, which sets the safety work
still to do.

## 1. Method

1. **Components.** Every crate in the shipped workspace, plus four platform items that every copy
   of the system shares: the operating system, the kernel, the firmware and the Rust standard
   library. That is 87 components.
2. **Outputs.** For each component, the outputs another component or a controller depends on,
   chosen by reading the source. There are 811 output groups, each cited by function and
   `file:line`. The selection is by review, not an exhaustive list of every exported function.
3. **Failure modes.** Each output is examined in six ways:
   - wrong value;
   - missing;
   - late or stale;
   - misleading but plausible;
   - crash or panic;
   - resource exhaustion.

   That gives 4,866 rows.
4. **For each row**, the analysis records:
   - the hazards in the hazard log that the failure contributes to;
   - the code that detects the failure, if any;
   - the response: refuse, degrade, flag or alarm;
   - a test or proof that fails if the detection is removed.

   Where any of these is missing, the row says NONE, with the reason. Where an output could not yet
   be tied to a hazard, the hazard column says UNMAPPED, with the reason.

A detection only counts if a test shows it working. Under the plan's principle 4, a check with no
such test is not credited.

## 2. Results

| | Rows | Share |
|---|---:|---:|
| Output and failure-mode pairs analysed | 4,866 | 100% |
| No detection | 816 | 17% |
| No response | 1,598 | 33% |
| **Neither detection nor response** | **776** | **16%** |
| Detection exists, but no test fails when it is removed | 2,936 | 60% |
| Not yet mapped to a hazard (28 output groups) | 168 | 3.5% |

So about one failure in six goes unnoticed and unhandled. In most of the rest, the check exists
but nothing yet proves that it works.

### 2.1 Where the undetected failures are

The 776 rows with neither detection nor response, by component (top ten) and failure mode:

| Component | Wrong | Missing | Late/stale | Misleading | Crash | Exhaustion | Total |
|---|---:|---:|---:|---:|---:|---:|---:|
| oa-msaw | 21 | 19 | 34 | 18 | 25 | 13 | 130 |
| oa-airspace | 11 | 10 | 33 | 16 | 10 | 0 | 80 |
| oa-ca | 13 | 5 | 21 | 8 | 9 | 4 | 60 |
| oa-display | 14 | 6 | 12 | 6 | 12 | 4 | 54 |
| oa-runway | 8 | 3 | 9 | 4 | 12 | 2 | 38 |
| oa-geo | 11 | 5 | 13 | 3 | 4 | 1 | 37 |
| oa-safety | 22 | 0 | 2 | 0 | 6 | 2 | 32 |
| oa-feed | 14 | 1 | 9 | 2 | 1 | 0 | 27 |
| oa-techops | 8 | 0 | 5 | 0 | 8 | 0 | 21 |
| oa-math | 6 | 1 | 6 | 1 | 6 | 0 | 20 |
| All 73 components with a gap | 213 | 66 | 214 | 93 | 147 | 43 | 776 |

The gaps sit in the safety functions themselves: minimum safe altitude warning, airspace and
adaptation, conflict alert, the display, and runway safety. By hazard, the most affected are:

| Hazard | Severity candidate | Rows with neither detection nor response |
|---|---|---:|
| HZ-FEED-001 | Hazardous | 137 |
| HZ-GEOMETRY-001 | Catastrophic | 136 |
| HZ-MSAW-001 | Catastrophic | 133 |
| HZ-CA-001 | Catastrophic | 110 |
| HZ-SURV-TIME-001 | Catastrophic | 107 |
| HZ-NET-STATE-001 | Catastrophic | 103 |

(A row can link to more than one hazard.)

### 2.2 The shared platform

The operating system, kernel, firmware and Rust standard library each have five output groups.
Each of the four has 19 of its 30 rows with neither detection nor response. Where replicas
run the same platform versions, as copies of one release normally do, these are common-cause
failures. Under the SMS Manual, a
catastrophic hazard with a common cause is high risk even if extremely improbable. A second copy
of the same build cannot defend against them (Program Safety Plan, section 6).

## 3. What the gaps have in common

Three patterns recur in the 776 rows. Each needs one design answer, not 776 local fixes.

Some of these protections already exist for particular functions. For example, when MSAW loses
its heartbeat, the display marks it unavailable and shows that the picture is frozen. The rows
below are the outputs where no such check was found.

1. **Freshness is not checked.** The core functions are pure: they take the time from the
   caller and compute. If the inputs are old, the output is computed correctly from old data,
   and, for these outputs, nothing downstream notices. This pattern covers most of the 214 late
   or stale rows.
2. **Wrong but plausible values are not caught.** A missed conflict alert, or a floor altitude
   computed from the wrong tile, looks like a normal output. A check inside the same computation
   rarely catches this. The proposed answer is an independent computation to compare against.
   This pattern covers most of the 213 wrong value rows and the 93 misleading rows.
3. **Crashes are not supervised across the system.** Per-function heartbeats exist in places,
   but this analysis did not find process-wide crash supervision. A stopped process may go
   unnoticed unless its function has its own heartbeat. This pattern covers most of the 147
   crash rows, and the platform rows.

## 4. Safety requirements this analysis sets

These are proposed. Each will get a hazard link, a requirement ID in `REQUIREMENTS.md` and a test
that fails when the behaviour is broken.

1. **Source time on every safety output.** Every conflict alert, MSAW, runway and track output
   carries the oldest source time of the inputs it used.
2. **Stale outputs are refused at the consumer.** The display and every downstream consumer
   refuse or visibly flag a safety output older than a stated bound for its function. The bound
   is stated per function and traced to the hazard.
3. **Heartbeat on every safety function.** Extending what MSAW already has, each safety function
   publishes a heartbeat. Loss of
   heartbeat for longer than a stated time is alarmed to the controller and to Tech Ops, and the
   function's outputs are marked unavailable.
4. **Dissimilar check for catastrophic functions.** Conflict alert, MSAW and the track picture
   are each compared against an independent computation that shares no code with the primary.
   Disagreement beyond a stated bound is alarmed. This is the dissimilar thread and the
   independent monitor of the Program Safety Plan, section 6.
5. **Bounded resources fail visibly.** When a bounded table or buffer is full, the function
   reports that it is degraded. It does not drop work silently.
6. **Every credited detection has a failing test.** The 2,936 rows where a detection exists but
   no test proves it get one. The method is the one used for the Done rows: remove or invert the
   check, and the test must fail.

## 5. Limits

- The output selection is by source review, not exhaustive.
- Hazards are the 20 function-level hazards in the current log. The severity of each is an
  engineering candidate, not an assessed severity.
- No likelihood has been assigned. That comes after the severities are agreed.
- The analysis covers the code at c15b5ca. Every later change is analysed again for what it
  changes (Program Safety Plan, section 4).
- No FAA review of this analysis is recorded.
