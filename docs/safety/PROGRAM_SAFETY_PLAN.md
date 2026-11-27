# OpenATCA Program Safety Plan

Status: draft 1, 2026-11-26. Owner: Avila Labs (maintainer). Not approved by the FAA.

This plan says how OpenATCA finds, analyses, reduces and tracks the hazards of the system, which
safety documents it produces and when, and who decides what. It follows the FAA's safety risk
management (SRM) framework for NAS systems: FAA Order 8040.4C (Safety Risk Management Policy),
the ATO Safety Management System Manual (July 2026) and the ATO Safety Risk Management Guidance
for System Acquisitions (SRMGSA, October 2022).

The FAA approves a program safety plan; we cannot. This document is the plan we would put
forward, and the plan we work to now, so the evidence an FAA safety team needs is being produced
from the start rather than reconstructed later.

## 1. Scope

The system is OpenATCA: surveillance processing and tracking, flight data processing,
conflict alert, minimum safe altitude warning, runway and approach safety functions, the
controller display, coordination between facilities, recording and replay, and site adaptation.
It is meant to replace en route (ERAM) and terminal (STARS) automation one function at a time.

The plan covers:

- the software in the `AvilaLabs/OpenATCA` repository and its releases;
- adaptation data issued for a facility;
- the change from a legacy function to the OpenATCA function at a facility (the cutover),
  under the transition procedure (`docs/migration/TRANSITION_PROCEDURE.md`).

It does not cover the facility, its hardware, its communications or the controllers' procedures,
except where OpenATCA depends on them. Those are assessed with the FAA at each site (section 9).

## 2. Principles

1. **Severity is judged by the effect on the aircraft, not on controller workload.** A failure
   that could contribute to a collision, controlled flight into terrain or a runway incursion is
   classed by that outcome, as the SRMGSA requires.
2. **Identical software is not redundancy.** Two computers running the same build share the
   same bugs. Under the SMS Manual a catastrophic hazard from a common-cause failure is high
   risk even if extremely improbable. Every catastrophic hazard therefore needs a defence that
   does not share the cause (section 6).
3. **Evidence, not prediction.** Predicted residual risk is confirmed with objective evidence:
   tests that fail when the behaviour breaks, proofs, replays against recorded traffic, and
   field monitoring with targets. "Verified in test" is never a monitoring plan.
4. **Nothing is credited that has not been checked.** A test counts as verification only if it
   has been shown to fail when the behaviour it claims to check is broken. Limits used to judge
   a result are not raised to make it pass.
5. **The legacy system's field record counts.** Years of ERAM and STARS operation are stronger
   evidence than models. Each function is compared against the legacy function on the same
   recorded traffic before cutover and run in shadow alongside it after.

## 3. Organisation and decisions

| Role | Holder | Decides |
|---|---|---|
| Maintainer | Avila Labs | What is built, released and offered for a site; this plan |
| Safety engineer | Avila Labs | The hazard log, analyses and safety requirements; can stop a release |
| Independent reviewer | Someone who did not write the change | Whether a safety-related change meets the ruleset and its safety requirements |
| FAA program office | FAA | Whether the product meets the agreement |
| Risk acceptor | FAA official with positional authority (Order 8040.4C, Table 2-1) | Whether the predicted residual risk is acceptable; signs the risk acceptance |
| FAA safety approval (AJI) | FAA | Approves the PSP, OSA, CSA, PHA and SSAR |
| FAA development assurance (AJM) | FAA | Approves the SSHA, SHA, O&SHA and development assurance deliverables |

Any one of the FAA approvers can stop deployment. No function goes live at a facility without a
signed risk acceptance (Order 8040.4C, Ch. 2).

## 4. The SRM process

We follow the five steps of SRM for the system and for every change to it.

1. **Describe the system.** The function, its inputs and outputs, the people who use it and the
   environment. The descriptions are the requirements (`REQUIREMENTS.md`), the scope
   (`SCOPE.md`), the interface documents (`docs/interfaces/`) and the design documents
   (`docs/design/`).
2. **Identify hazards.** For each function, what can go wrong: wrong output, missing output,
   late output, output to the wrong place, and misleading output that looks right. Hazards are
   recorded in the hazard log (section 5).
3. **Analyse.** Causes, effects on the aircraft, existing controls, worst credible outcome.
4. **Assess.** Severity and likelihood, giving the initial risk on the FAA risk matrix.
5. **Treat.** Safety requirements that remove or reduce the hazard, each traced to verification.
   Predicted residual risk is stated, and confirmed by evidence and by field monitoring.

A new function, a changed function, a changed interface, a new site and a cutover each go
through the steps again for what they change.

## 5. The hazard log

- The hazard log is `docs/safety/hazard-log.json`, under version control with the code.
- Each hazard records: ID, function, cause, effect, severity class and its status, the rationale
  for the severity, the safety requirements that mitigate it, and references to source,
  verification and proofs.
- Every requirement in `REQUIREMENTS.md` is classified as safety or non-safety, with a stated
  reason. None may be left unclassified. Every active safety requirement traces to a hazard and
  to an exact test or registered proof (`oa-trace`, run in CI).
- CI refuses a hazard with no mitigation link.
- `tools/generate_safety_tables.py` produces the preliminary hazard analysis, operational safety
  assessment and SSAR verification tables from the log. They are generated in CI and attached to
  each release, never edited by hand.

## 6. Common-cause failure

Every catastrophic hazard must have a defence that does not share its cause. Three kinds of
defence are planned, alone or together. None is yet integrated into the product:

1. **A dissimilar safety thread.** A separately written, simple surveillance tracker and
   conflict alert path, with its own inputs and code, that can give an independently computed
   track picture and alert when the primary path fails or is judged unreliable
   (`docs/design/DISSIMILAR_THREAD.md`).
2. **Independent monitoring.** Checks that do not reuse the primary computation: raw input
   checks, a monitor that compares outputs between paths, and alarms when a path's outputs stop,
   go stale or disagree.
3. **Showing the effect is not catastrophic.** Where the controller has an independent source
   for the same information, and the failure is evident to them in time, the severity analysis
   may show a lower class. This must be argued from the effect on the aircraft, not assumed.

Replicas of the same build give availability. They are never credited as a defence against a
software fault.

## 7. Development assurance

- Software is developed to the objectives of DO-278A at the assurance level assigned to each
  function through the five-step process. The DO-278A gap analysis exists (2026-11-19).
- Levels are assigned after severity is approved, not before. Until then the work is done to
  the rules in `docs/RULESET.md`, which apply to every crate whatever its level.
- What the ruleset and tooling supply: requirements traced to tests and proofs (`oa-trace`);
  bounded memory and work, with allocation-free real-time core paths checked under a counting
  allocator; proofs with Kani for parsers, integer arithmetic and state machines; TLA+ models of
  protocols; fuzzing; mutation testing of the tests themselves; independent review of
  safety-related changes; reproducible builds (`docs/CONFIGURATION_MANAGEMENT_PLAN.md`).
- A Plan for Software Aspects of Approval (PSAA) and a Software Accomplishment Summary are
  produced for the FAA's development assurance approval.
- Third-party software: the shipped workspace uses no third-party crates. (The separate
  fuzzing workspace uses `libfuzzer-sys`; it is test tooling and never ships.) What remains in
  the shipped system is the operating system kernel, firmware and the Rust standard library. The safety functions must detect and
  contain their faults; that analysis is part of the SSHA.

## 8. Safety deliverables by gate

The SRMGSA (Table 2.1) names the deliverables at each acquisition gate. A route that is not a
normal FAA purchase still needs SRM: the SMS Manual makes new NAS systems used in ATC an SRM
trigger whatever their acquisition path. We plan to produce all of them. Any tailoring needs an
approved PSP, and we propose none.

| Gate | Deliverable | What it covers | State today |
|---|---|---|---|
| Concept and requirements | Program Safety Plan | This document | Draft 1 |
| | Operational Safety Assessment (OSA) | Hazards at the operational level and the safety requirements they set | Draft table generated from the hazard log; 20 function-level hazards |
| Initial investment | Comparative Safety Assessment (CSA) | Risk of each alternative, including keeping the legacy system | Not started |
| Final investment | Preliminary Hazard Analysis (PHA) | Hazards per function with causes, effects, severity and likelihood | Draft table generated; severities are engineering candidates; likelihood not assessed |
| Solution implementation | System Safety Program Plan | Our plan as developer, under this PSP | To be written from this document |
| | Subsystem Hazard Analysis (SSHA) | Hazards from each component and its failure modes, including the operating system and runtime | Draft 1 ([SSHA](SSHA.md)) |
| | System Hazard Analysis (SHA) | Hazards from interfaces and interactions between components and with other NAS systems | Draft 1 ([SHA](SHA.md)) |
| | Operating and Support Hazard Analysis (O&SHA) | Hazards from operating, maintaining, adapting and cutting over | Draft 1 ([O&SHA](OSHA.md)); the transition procedure is its starting point |
| | System Safety Assessment Report (SSAR) with the Safety Requirements Verification Table | All analyses and verification results; every safety requirement traced to origin and verification | Verification table generated; traceability complete (row C6); assessment not written |
| In service | Post-implementation safety assessment and monitoring | Whether predicted residual risk held in operation | Monitoring targets to be set per hazard (section 10) |

Each deliverable is updated at each later gate it carries into.

## 9. Sites and cutover

- Each facility installation follows a site implementation plan with its own SRM document; a
  site that differs from the generic plan gets a deviation SRM.
- Each cutover of a function at a facility follows the transition procedure: the legacy
  function stays available, the cutover has a stated rollback, and promotion through its stages
  waits on the stated soak periods and evidence. The O&SHA covers the procedure itself.
- Site adaptation is issued with recorded version, digests, issuer, approver and reason, and an
  installed package is verified locally at startup and on demand. Checking every processor on
  facility hardware, with authenticated reports, is deployment work not yet demonstrated.
  Misdecoded, incomplete or wrong-source adaptation is hazard HZ-AIRSPACE-001; stale adaptation,
  and a loaded set that differs from the issued one, are to be added to the log explicitly.

## 10. Monitoring in service

Every medium and high hazard from the PHA, every untraced medium or high hazard from the SSHA,
SHA and O&SHA, and every predicted residual medium hazard gets a monitoring target: what is
measured, from which data, by whom, how often and for how long. The SRMGSA guideline is at least
quarterly for initially high hazards and twice a year for medium, for at least a year for a
phased deployment. The system's own signed records and recordings are the data source. The
phrase "verified in test" does not appear in a monitoring plan.

## 11. Records

- The hazard log and every safety analysis are kept in the repository with the code they
  describe, so each release has the safety records for exactly that release.
- Generated safety tables, the trace report and verification evidence are attached to each
  release (`docs/CONFIGURATION_MANAGEMENT_PLAN.md`, section 8).
- Each change's safety impact (hazards whose mitigations changed) goes in its release's
  change package.

## 12. Where this stands

- The hazard log's status is `DRAFT_NOT_ASSESSED`. It holds 20 hazards at function level:
  12 Catastrophic and 8 Hazardous, all with severity as an engineering candidate. They are
  coarse; the SSHA and SHA will split them.
- Likelihood, external barrier credit and assurance levels are not yet assigned.
- Traceability is complete: every active safety requirement links to a hazard and to an exact
  test or proof.
- The dissimilar thread is a design proposal in this repository. A first alternate
  implementation, written in Ada, is in development in a separate repository: it compiles with
  assertion checks on and its first tests run, but it is not integrated with OpenATCA and is not
  qualified.
- No FAA review of any of this is recorded. The first step with the FAA is to agree this plan.
