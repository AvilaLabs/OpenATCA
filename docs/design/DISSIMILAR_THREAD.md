# Dissimilar safety thread: design proposal

**Status:** Proposal, under review.

## Purpose and first scope

Develop a separately engineered surveillance thread that can provide an independently computed
track picture and conflict alert when the primary path is unavailable or judged unreliable. The
first scope is limited to ADS-B and radar plot ingestion, track formation and maintenance, and a
defined en-route conflict alert. It does not provide flight-plan correlation, coordination,
clearance processing, or a replacement for the full automation system. Minimum safe altitude
warning (MSAW) is a later, separately gated increment because it adds terrain, pressure/vertical
reference, and obstacle-data dependencies that are not needed to prove this first thread.

The first scope accepts only source-qualified observations with explicit source identity, source
time and reception time, quality, validity, and measured altitude provenance. A direct Mode S
address may associate radar and ADS-B observations; anonymous radar plots remain anonymous unless
approved evidence supports another identity rule. A missing altitude stays missing. The thread
must expose input age, source loss, coverage, capacity loss, and unsupported services alongside
its picture and alert status.

The primary and alternate pictures remain separate. A disagreement makes their outputs
unreconciled; it does not establish which one is correct. Operators may select a path only when
that path's own health and limitations are visible. Raw observations may be shown as a distinct
diagnostic view, with age and source coverage, but do not count as tracks or alert coverage.

## Dissimilar implementation choices

The current `oa-track` family uses covariance-based horizontal and vertical filters, multiple
models, and global assignment for anonymous radar association. The proposal is to use a different
family in the alternate: fixed-point constant-velocity extrapolation with bounded position and
velocity intervals, explicit interval intersection for association, two-observation confirmation,
and a deterministic bounded greedy association order. Conflict alerting would compare reachable
occupancy intervals over the alert horizon, rather than reuse the primary filter's covariance
gates or pair-cost assignment. Ambiguous matches remain unassigned and visible; they are not
resolved by proximity-only identity guesses. This simpler family is selected to reduce shared
algorithm structure and make each bound reviewable. It may create more coasts, duplicate tracks,
or nuisance alerts. Evaluation against independent truth and surge tests may show it is unsuitable;
if so, revise the algorithm before implementation rather than silently borrowing the primary's
logic.

Use an ARMv8-A, AArch64 Linux server with ECC memory and hardware watchdog for the first alternate
evaluation. This gives a different processor architecture from the x86-64 primary evaluation
target while retaining a hosted Linux environment for a separate display and sensor gateway.
Select the GNAT Pro for Ada 25 GCC toolchain family on its native Linux ARM64 platform. AdaCore's
GNAT Pro 25 release notes identify Linux ARM 64-bit native platform support and GNAT Pro for Ada's
ARM 64-bit cross-Linux support; this establishes a supported toolchain family/platform pairing,
not a board, distribution, certification, or performance qualification ([GNAT Pro 25 release
notes](https://docs.adacore.com/live/wave/gnat-release-notes/html/gnat_release_note/release_notes_25.html)).
Use the SPARK-eligible subset for bounded state and arithmetic where proof obligations are
well-defined. Constrained types, explicit checks, and proof-capable contracts offer a different
implementation and assurance path from Rust; a proof establishes only its stated property. Pin
the exact GNAT Pro maintenance release, board, Linux distribution, runtime profile, compiler
switches, libraries, and linker before the evaluation build. Change the platform choice if the
selected board lacks vendor-supported runtime/toolchain coverage, if the safety authority rejects
the assurance evidence, or if measured worst-case execution misses the target below. Do not port
the Rust implementation or share generated code as a shortcut.

Keep the alternate in its own repository and source history, with a separate build pipeline,
release process, reviewer set, issue tracking, and requirements-to-test trace. Requirements may
be shared as approved interface contracts, but each implementation's design, source, test vectors,
and expected results are authored independently. A reviewer who did not implement the alternate
must assess its requirements, source, proof claims, test results, and changes. Repository
separation reduces accidental code reuse; it does not remove common requirements, people, or
organizational hazards.

## Hardware, input, and common causes

The proposed deployment uses the selected AArch64 evaluation computer, a separate display endpoint,
and separately powered manual selector. Keep the alternate process, storage, watchdog, and health
indication out of the primary host. The two paths may receive the same sensor observations where no
independent feed is available; that shared fan-out, site, time source, adaptation authority, and
facility remain common causes and must be represented in the hazard analysis. Do not describe
separate computers as independent surveillance sources.

Maintain a hazard-linked common-cause register. For each dependency, identify affected functions,
containment, evidence, residual risk, and the next action.

| Dependency | Potential common failure | Proposed control and evidence |
|---|---|---|
| Requirements and interpretation | Both implementations repeat an ambiguous or unsafe rule | Trace each requirement to an authoritative source; use independent review and independently derived boundary cases; record unresolved semantics |
| Algorithms and code | Copied logic or shared helpers repeat a defect | Separate algorithm family, authorship, repository, dependencies, and source; review source similarity and design provenance |
| Surveillance source and timestamps | Shared missing, delayed, corrupted, or mis-timed feed misleads both | Map sensor fan-out; preserve source, sequence, event and reception time, and quality; inject shared loss, poison, reorder, delay, and clock faults |
| Adaptation and geometry | A shared bad parameter set creates matching wrong output | Identify data authority and effective dates; validate independently; reject stale or malformed inputs; retain shared-authority risk |
| Build chain and runtime | Compiler, OS, firmware, clock, or deployment defect affects both | Pin and reproduce distinct toolchains; identify host versions; review unavoidable shared runtime components; test restart and resource faults |
| Facility and presentation | Shared power, network, console, display, or selector defeats both | Draw facility dependencies and common cut sets; test blank, stale, misrouted, and failed display/selector cases |
| Queues and resource limits | Flood or poison input exhausts both paths | Bound queues and per-cycle work; reconcile every refused or deferred record; fault-test capacity; define degraded/unavailable status |
| Maintenance and procedure | One update, misunderstanding, or maintenance action disables both | Separate approval and maintenance windows; train operators; display active path and limitations persistently |
| Time reference | Shared time error breaks freshness and correlation | Identify source and holdover; compare time quality; report unknown time unhealthy; exercise disagreement and loss |

Language, teams, processors, and operating systems are controls to examine, not an independence
probability. Shared requirements, source feeds, adaptation, facility, time, and human procedures
can dominate common-cause risk. Do not multiply component reliabilities or infer combined
availability until the service definition and each relevant failure-mode assumption are justified.

## Capacity and degraded behavior

Use this concrete first evaluation envelope: one sector; 8,192 simultaneous tracks; one ADS-B
report per active track per one-second cycle; and three radar sites, each with no more than 75
plots per five-second scan. The first algorithm configuration uses fixed-point position and
velocity intervals, two-report ADS-B confirmation, three hits in four radar scans for anonymous
radar confirmation, coast after 5 seconds without an accepted update, drop after 30 seconds,
interval-overlap gating,
and deterministic source/time/plot-order greedy association. A plot with more than 64 eligible
tracks causes a counted ambiguous/refused result and an unhealthy cycle; it is not silently
truncated. The conflict-alert evaluation horizon is 120 seconds with the initial synthetic
scenario thresholds set to 5 NM horizontally and 1,000 ft vertically. Those values are engineering
test parameters only; the safety authority must approve operational alert semantics before they
can be used for a facility.

Use a fixed spatial grid with at most 128 track references per cell and inspect no more than nine
neighboring cells for one plot. A cell above 128 tracks is counted as overflow and degrades the
cycle; the grid cannot silently hide tracks. Cap refined association candidates at 64 per plot and
refuse an overfull ambiguous plot. The worst-case broad-phase bound is 225 plots × 9 cells × 128
references = 259,200 reference visits per five-second scan epoch. Bound conflict-alert candidate
evaluations separately at 524,288 per one-second cycle; exceeding either budget makes the cycle
incomplete.

Reserve fixed storage for 8,192 tracks, 8,192 ADS-B reports per one-second cycle, 75 plots per
radar site per scan, and the fixed grid and candidate limits above. Set all input, output, alert,
and diagnostic queues to explicit finite capacities sized for those maxima with no implicit
backlog; reject work once its pre-work budget is reached. The evaluation deadline is 200 ms from
cycle start to reconciled track and alert output, within a one-second update cycle. This target
leaves time for the display and operator path and is a design budget, not a measured result.

Every limit breach, ambiguous association, missing source, stale time, or refused record is
counted by reason and marks the cycle degraded or unavailable according to the approved health
table. Partial completion cannot be reported healthy. Reconcile every offered observation and
candidate evaluation with completed, refused, deferred, and omitted counts over the whole run.
Change the envelope if a source-backed facility traffic profile exceeds any input limit; change the
algorithm or target if the required worst-case execution cannot meet the 200 ms budget without
loss.

Unknown or stale input remains unknown; it does not become a clear area or healthy track. Report
the evaluated population and omitted population with reasons. The two track pictures must not be
averaged. An alternate that lacks its own current input, full required coverage, adaptation, or
capacity completion is unavailable for selection, even if it displays some tracks.

## Manual selection and alert continuity

Use a latched, position-indicating hardware selector that changes the presented picture only on an
explicit controller action. No health monitor or software path transfers authority automatically.
Before the change, the controller sees destination health, input age, source coverage, capacity
completion, and unavailable services. If the destination is not healthy or its state is unknown,
the interface refuses to label it a usable alternate and the facility procedure governs the
response.

Switching does not transfer track history, alert acknowledgement, inhibit, or ownership state.
The alternate presents its own alerts and requires the controller to review and acknowledge them
under approved procedures. Record the old and new selection, operator, time, reason, source health,
outstanding alert state, and any coverage gap. Returning to the primary also requires an explicit
action after an authority-approved healthy run-in. Neither path silently takes over when the
other fails.

The selector's power-loss position, interlocks, transition time, indication, and behavior during a
partial switch require hardware and human-factors review. A controller must be able to recognize a
failed or incomplete transition and use an approved local procedure. A visible alternate picture
does not itself authorize a transfer.

## Evidence and decision gates

1. **Define service and hazards.** Name the facility, supported surveillance sources, first-scope
   function, alert meaning, availability measure, unsupported services, and operational owner.
   Complete a hazard analysis for misleading, stale, missing, duplicated, or mismatched tracks and
   alerts, wrong source identity, time/adaptation error, resource exhaustion, process failure,
   display freeze, selector failure, false health, and shared facility/feed failures.
2. **Approve independent requirements.** Trace each behavior to an authoritative source or label
   it an explicit assumption. Independently derive edge cases, alert thresholds, identity rules,
   health predicates, and refusal semantics. Resolve disagreements before coding.
3. **Validate algorithms offline.** Compare both implementations against independently controlled
   truth across representative geometry, source rates, crossing traffic, maneuvers, altitude
   uncertainty, missing sources, and alert transitions. Measure track continuity, identity errors,
   alert detection and nuisance rates, latency, and mismatch causes. Use separate expected results;
   agreement with the primary is descriptive, not an oracle.
4. **Prove bounded behavior.** First require all named synthetic positive-conflict vectors to alert
   and all paired clear-control vectors not to alert; freeze those vectors and expected results
   before implementation. Then run the stated 8,192-track, 3-radar, 75-plots-per-site-per-scan
   profile on the selected AArch64 Linux target for 24 continuous hours, with synthetic ADS-B at
   one report per active track per second and the stated 120-second alert horizon. Require zero
   input, association, track, alert, or output loss; complete per-cycle accounting; and worst-case
   cycle completion at or below 200 ms. A single lost/refused/deferred record or deadline miss
   fails this load-evidence run. Separately inject malformed, reordered, duplicated, missing, delayed, poisoned, clock-fault,
   and capacity-exceeding inputs and require each failure to produce the specified count and
   degraded/unavailable state. Retain failed and incomplete runs. Exercise process restart, disk/log
   full, and stale adaptation.
5. **Review implementation assurance.** Independently review the Ada/SPARK proof boundary, source,
   compiler configuration, reproducible build, dependency inventory, and test evidence. Verify
   that claimed proofs cover the deployed binary and the requirements they are said to support.
6. **Assess facility transfer.** Only after authority approval, conduct hardware-in-loop and
   operationally controlled manual-transfer exercises. Measure detection and annunciation delay,
   switch time, stale interval, lost coverage, alert continuity, false alarms, failed transfer,
   and switch-back. Define acceptance values before seeing results.
7. **Authorize each increment.** A competent independent assurance authority decides whether the
   evidence supports a trial, limited deployment, or operational use. MSAW requires a new hazard
   analysis and evidence package; it is not included by acceptance of the first scope.

## Limits and open questions

- **Service definition and availability target.** The supported facility, service hours, required
  sources, peak/surge population, and meaning of availability are not established. Next step: the
  operational owner defines the service and authority-approved availability measure before
  requirements or sizing.
- **Alert semantics and identity rules.** The conflict-alert horizon, thresholds, Mode S evidence,
  and handling of anonymous radar remain to be approved against authoritative requirements. Next
  step: independent safety and surveillance specialists review proposed requirements and cases.
- **Independent surveillance feed.** The sensor fan-out, alternate feed, source timing, and failure
  isolation available at the intended facility are unknown. Next step: document the actual network,
  gateway, time, power, and facility dependency graph before selecting hardware.
- **Selected evaluation platform.** GNAT Pro 25 supports native Linux ARM64, but the exact board,
  distribution, runtime profile, maintenance release, compiler options, safety evidence, and
  execution performance remain open. Next step: pin those items and verify vendor support and
  procurement availability before producing an evaluation binary; return to x86-64 only if the
  selected ARM64 pairing is unsupported or misses the frozen deadline.
- **Facility load correspondence.** The 8,192-track/three-radar profile is a concrete initial
  evaluation envelope, not a confirmed site's traffic model. Next step: obtain an approved,
  privacy-safe source-rate and peak/surge profile; increase the envelope if that profile exceeds
  it, then freeze the limits and acceptance criteria before measurement.
- **Conflict-alert semantics.** The 120-second horizon and synthetic 5 NM/1,000 ft thresholds are
  engineering test values, not operational minima. Next step: have the responsible safety authority
  set and approve the alert thresholds before any operational code or evaluation claim.
- **Manual selector and display design.** Safe position, transition behavior, failed-switch
  indication, alert acknowledgement, and operator procedure require facility-specific human-factors
  and hardware review. Next step: model selector failures and review a concrete interface mock-up
  with operators and safety authority.
- **Adaptation authority.** Effective-date control, geometry sources, distribution, and independent
  validation responsibilities are undecided. Next step: define authoritative provenance and
  stale/mismatched data refusal requirements before any adaptation loader design.
- **MSAW increment.** Terrain and obstacle data, vertical reference, pressure handling, and alert
  acceptance are outside the first scope. Next step: open a separate hazard and evidence plan only
  after the first-scope service is defined.
- **Assurance authority and binding criteria.** The accepting authority, applicable assurance
  process, binding availability criteria, and permitted trial conditions have not been established.
  Next step: identify them before implementation authorization; advisory research guidance alone
  does not set binding criteria.
