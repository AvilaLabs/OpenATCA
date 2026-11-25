# Dissimilar safety thread: design proposal

**Status:** Draft proposal for engineering review. This document does not approve an architecture,
implementation, safety case, or operational use.

## Proposal and boundary

Consider a small second surveillance tracker and narrowly scoped alert path, separately authored
and separately deployed from the primary. It would consume bounded, time-stamped surveillance
observations and only the adaptation needed for its approved functions. It would not share tracker
or alert state, implementation libraries, generated code, build products, or runtime processes
with the primary. The controller remains the authority: a separate manual selector chooses the
presented path. No software voter or network transfers authority automatically.

This is a design direction, not a claim that diversity makes failures independent. The existing
monitor and raw-view work may inform requirements, but it does not establish a second qualified
thread. A tracker and limited alert subset would not provide flight-plan or coordination services.
Any raw view must be labeled as raw observations, without implying track continuity, identity,
alert coverage, or separation support.

Start with a hazard analysis and select only safety functions with approved inputs, requirements,
alert semantics, adaptation, and evaluation data. Keep automatic voting, automatic takeover,
state averaging, flight-data processing, and a full automation-system clone outside this first
scope.

## Independence and common causes

Maintain a hazard-linked common-cause register. For each shared dependency, record how it could
affect both paths, its containment, evidence, and residual risk. At minimum cover:

| Dependency | Potential common failure | Evidence or control to resolve |
|---|---|---|
| Requirements and interpretation | Both implementations repeat an ambiguous or unsafe rule | Independently review testable requirements against authoritative sources; record assumptions and unresolved interpretations; derive boundary tests independently |
| Algorithms, code, generated data | Copied logic or shared helpers repeat a defect | Separate authorship and source trees; no shared tracker/alert implementation; inspect dependencies and source similarity |
| Surveillance source and timestamps | Missing, delayed, corrupted, or mis-timed input misleads both | Map sensor fan-out and publishers; retain source, sequence, time, and quality; test shared loss, poison, reorder, and delay cases |
| Adaptation and geometry | A common bad parameter set creates matching wrong results | Identify authoritative source and effective dates; validate independently; reject stale or malformed adaptation; retain residual common-source risk |
| Build chain and runtime | Compiler, toolchain, OS, firmware, clock, or deployment defects affect both | Name actual versions and hosts; separate where practical; preserve reproducible provenance; analyze unavoidable shared components |
| Facility and presentation | Shared power, cooling, network, console, display, or selector defeats both | Create a facility dependency diagram; identify common cut sets; test blank, stale, cross-wired, and failed display/selector cases |
| Queues and resource limits | Flood or poison inputs exhaust both paths | Pre-bound queues and per-cycle work; account drops; quarantine malformed records; prohibit unbounded retries; define degraded behavior |
| Maintenance and human procedure | One update, misunderstanding, or simultaneous maintenance disables both | Separate change approval and maintenance windows; train users; make active path and limitations persistent and obvious |
| Time reference | A shared time error breaks freshness and correlation | Identify time sources and holdover; monitor clock agreement; expose unknown time as unhealthy |

Different languages, teams, processors, or operating systems are controls to examine, not an
independence probability. Shared sensors, adaptation authority, specifications, facility, and
human procedures can dominate common-cause risk. Do not multiply failure probabilities or infer
combined availability until the service definition and each independence assumption are justified
for the relevant failure modes.

## Input, capacity, and degraded behavior

Use a narrow versioned record containing source identity and sequence, reception and measurement
times, position, altitude/quality, source health, and explicit validity. Do not add flight identity
or controller-entered state without separate approval. Record the exact interval and the counts of
missing sources, sequence gaps, invalid records, truncation, and clock uncertainty. Each path
should account for the observations it actually consumed so divergence can be separated into input
and computation differences.

Bound every queue, table, work budget, output, and log before implementation. Derive limits from
approved peak load plus a documented surge margin. Capacity misses must be counted and change
health to a defined degraded or unavailable state; silent truncation and “partial but healthy” are
not acceptable. Unknown or stale input remains unknown; it must not become a clear area or healthy
track. Report the population evaluated and the population omitted, with reasons.

The two pictures must not be averaged. A mismatch makes the active path unverified; it does not
prove the alternate is correct. A controller may select the alternate only after seeing its own
health, age, coverage, and unavailable services. If neither path is qualified, a raw view may be
shown only with explicit freshness and limitations; if its input is unhealthy, show unavailable.

## Manual transfer, alerts, and monitoring

The selector should be a simple, position-indicating control independent of both software paths.
Its electrical design, interlocks, transition timing, indication, and failure position need
hardware safety review. Do not choose an automatic fallback position before the facility's safe
state is approved. Define who owns the picture during transfer and how an aborted or failed switch
is annunciated.

Before transfer, show the destination's source, health, freshness, coverage, and unavailable
services. Log old and new source, operator, time, reason, observed health, outstanding alerts, and
any coverage gap. The receiving path presents its own alert, acknowledgement, and inhibit state;
an inhibit or acknowledgement must never transfer silently. Switch-back requires an explicit
operator action after the primary has passed an authority-approved health and run-in period. Do
not update or maintain both paths together.

Define measurable health predicates for input completeness, clock trust, adaptation validity,
capacity completion, process responsiveness, output freshness, display freshness, and alert
population reconciliation. A stale or missing monitor is `unknown` or `unavailable`, never healthy.
Test failures of the health monitor and annunciator themselves. Partial source loss, overload,
truncation, repeated restarts, and mismatch must have explicit degradation and annunciation rules.

## Evidence before implementation or claims

Before implementation, prepare a traceable hazard analysis and requirements set for misleading or
stale output, wrong source identity, clock or adaptation errors, queue overflow, resource
exhaustion, crashes, hangs, display freeze, selector failure, false health, and common facility or
network failures. Independently derive boundary tests; agreement with the primary is descriptive,
not an oracle.

The evidence plan should include synthetic malformed, missing, duplicate, reordered, late, and
poison inputs; identity, altitude, uncertainty, time, adaptation, and resource-limit faults; and
alert-state transitions. Later, under approved conditions, conduct fault-injection and manual
transfer exercises at approved peak and surge loads. Measure detection and annunciation latency,
switch time, stale interval, lost coverage, alert state, false alarms, and recovery/switch-back
behavior. Retain failed runs. Replay and synthetic success do not establish field availability.

No numeric availability or independence claim is supported by this proposal. The cited FAA
research basis includes SAF R4 on common-cause hazards and SAF R21, which is based on
FAA-HDBK-006A guidance. That handbook describes its availability/thread guidance as nonbinding;
the applicable binding NAS-SR-1000 text and target have not been established here. Do not treat
seven-nines guidance as a requirement, multiply component reliabilities, or claim a field target
without an approved service definition, justified model assumptions, and accepted operational
evidence. See FAA-HDBK-006A, §§6.3–6.5 and the availability guidance discussion; SAF R4 and R21
are research references, not approval of this proposal. The cited material is FAA's [ATO Safety
Management System Manual](https://www.faa.gov/air_traffic/publications/media/202607-ATO-SMS-Manual.pdf),
§3.8.2.1–.2, and FAA-HDBK-006A, *Reliability, Maintainability, and Availability Handbook*
(2008), §§6.3–6.5. The handbook itself says it is guidance, not a requirement; the referenced
binding NAS-SR-1000 target has not been established here.

## Decisions for engineering review

1. Which safety functions are in the first scope, and what does availability mean for each?
2. Which sensor inputs are truly independent, and what shared-feed failures remain uncovered?
3. What identity, altitude, uncertainty, and freshness evidence is required before manual selection?
4. Who owns adaptation, alert semantics, acknowledgement/inhibit rules, and coverage indication?
5. What hardware, power, network, clock, display, selector, and facility separation is feasible?
6. Who may switch and switch back, what training applies, and what is safe on selector/display failure?
7. Which performance or availability targets are actually binding, and which authority establishes them?
8. What assurance process, independent verification, and authority acceptance are required before
   any trial or operational use?

Until these decisions and reviews are complete, this remains a proposal. It does not close a safety
acceptance item or authorize an operational claim.
