# Capability and integration register

Reviewed against the current component source and the regional increments described below,
2026-11-24. This register separates implemented code, its
integration boundary, and the evidence still needed. A component or scripted fixture does not
close a milestone in [SCOPE.md](../SCOPE.md). OpenATCA remains a research prototype.

| Capability | Implemented entry point / evidence | Integration and qualification limit |
|---|---|---|
| Airspace adaptation | `oa-airspace`, NASR/CIFP import; [M0 report](M0_REPORT.md) | M0 is closed for its stated region/data; this does not establish every facility's operational adaptation. |
| Ingest and deterministic replay | `oa-feed`, `oa-replay`, ordered input fingerprints and state witnesses | Live ingestion and file replay exist. Authenticated peer distribution and autonomous facility operation are not established by those tools. |
| Surveillance | `oa-track`, `oa-perf`, recorded comparison tooling | Recorded and synthetic paths exist. [Radar qualification](RADAR_CAPACITY_2026-11-23.md) still fails strict latency and zero-loss criteria; gate coverage is not identity accuracy. |
| Flight state and handoffs | `oa-flight`, `oa-flight-sfdps`; [regional runner](../tools/oa-regional-session/README.md) | Authoritative normalized state and local typed controller requests compose with full identity, actor authority and handoff checks. A network coordination service and complete controller position remain open. |
| Normalized route conversion | `oa-airspace::core::route::convert_route`, `oa-adapt route`; [converter contract](../crates/oa-airspace/README.md) | Bounded caller-normalized fix/direct/airway/procedure steps resolve full identities and retain supported geometry and constraints. This is not a general filed-route parser, does not infer active progress, and has not been scored against ERAM's expanded routes. |
| Arrival trajectory experiment | `oa-legacy-traj::core::arrival`, `--arrival-meter-fix`; [method and limits](../tools/oa-legacy-traj/README.md) | Issue-time route/track/assignment evidence produces 5/10/20-minute forecasts and meter ETA, with cutoff revision selection and retained provenance. The experiment freezes its first eligible issue per flight. This uses supplied expanded routes, not an independent NASR/CIFP route converter. Explicit wind prediction exists in the core; a weather/airspeed source is not wired into the scored shell baseline. |
| Shared issue-time trajectory baseline | `oa-legacy-traj::core::trajectory`; [API and limits](../tools/oa-legacy-traj/SHARED_TRAJECTORY.md) | A separate fixed-capacity component selects bounded revision evidence and emits 5/10/20-minute constant-groundspeed/vertical-rate horizons plus exact-fix ETA from a full route key and explicit active cursor. It reports omitted wind, aircraft performance and forward uncertainty; altitude-crossing feasibility is not implemented. It is not composed into the earlier arrival experiment or consumed by conformance, coordination, conflict probe, MSAW or safety nets. |
| Shared operational trajectory | M4 design; offline regional composition | The runner binds converted routes, active cursors and frozen flight/track evidence to the shared fixed-motion baseline. It is not operationally qualified or shared by every downstream consumer; aircraft performance, wind, forward uncertainty and consumer integration remain open. |
| Conflict alert | `oa-ca`, `oa-ca-run` | Bounded geometric alerting, net status and replay tools exist. Development settings and finite tests do not qualify facility warning/miss/nuisance rates. |
| Minimum safe altitude warning | `oa-msaw`, `oa-msaw-run`; [offline encounter evaluator](../tools/oa-msaw/EVALUATION.md) | Terrain/obstacle/procedure-aware replay exists, including flight-data eligibility. The evaluator measures warning onsets against independent synthetic windows and refuses ambiguous compressed records. Complete cycle-level availability evidence and facility qualification remain necessary. |
| Other safety nets | `oa-safety` state/inhibition infrastructure; `oa-runway` geometry/occupancy | APW and approach-path monitoring are not complete nets. Runway occupancy does not implement a complete surface tracker or runway alerting service. |
| Controller display | `oa-display::interaction`, `oa-scope-replay` | Deterministic scene/raster, positive-identity selection, bounded typed command queue and explicit outcome presentation exist. The regional bridge applies actual flight operations. KMS presentation, a complete interactive position workflow and controller evaluation remain open. |
| Integrated offline session | [oa-regional-session](../tools/oa-regional-session/README.md) | Actual flight, tracker, route, forecast, observed CA, safety status and scope share a receipt cutoff. Public synthetic drills cover amendments, handoffs, refusals, loss/reacquisition and reconstruction. This normalized-input research envelope is not a live facility system or process checkpoint. |
| STARS/STDDS comparison | `oa-stdds-ref`; [cohort policy](../tools/oa-stdds-ref/README.md) | Recorded STDDS exists. Address-qualified comparison distinguishes flag classes and raw/calibrated agreement, exclusions and clock-fitting limits. Flag values do not establish independent radar truth; no completed real-data superiority result is asserted here. |
| Independent monitor and recovery | `oa-safety::monitor`, verified replay, raw fallback, [raw replay witness](../crates/oa-scope-replay/tests/RAW_WITNESS.md), process/transport drills | The raw publisher verifies a completed prefix, recomputes from original inputs and publishes the missing suffix under a same-host cooperative lock. It does not restore tracker/process state or establish distributed fencing. Production supervision, durable peer recovery and two-host qualification remain separate work. |
| Developer interface | [OpenATCA Core](../crates/openatca-core/README.md) | Experimental synchronous offline ADS-B replay API; not a live regional automation API. |

## Next integrated research milestone

One reproducible regional session should show traffic and positively identified flight state,
an amendment and handoff, arrival prediction, safety alerts and explicit degraded states, then
recover from a serving-process failure. Its audit trail must identify exact inputs, source,
binary, settings, receipt-time basis and every omission. The current independent tools and
fixtures now compose in the offline regional runner. Live serving-process recovery and operational
interfaces remain necessary to close the full milestone.

The source-ordered route API `forecast_latest_source_route` supplies a bounded building block:
availability gates the evidence, source time orders amendments within one full GUFI and reporting
centre, and successes and refusals retain complete candidate counts. The independent normalized
route converter and shared issue-time trajectory baseline now compose through a shell adapter
with cutoff/key/cursor binding and independent altitude/speed channels. The
new evidence selector does not change the frozen study selector. The visible-CA recovery fixture
also checks current CA
state before rasterizing, with matching tracker/display identities, altitude and coast/uncertainty
status. These remain component and same-host fixture evidence; the proposed composition is in
[the regional session plan](REGIONAL_SESSION_PLAN.md).

The next development increments are bounded:

1. Develop the shared prediction service beyond fixed motion: measured performance/wind inputs,
   vertical feasibility, uncertainty and explicit downstream consumer contracts. Preserve the
   frozen comparison study while validating these capabilities independently.
2. Run the reviewed STDDS comparison on explicitly authorized development recordings with full
   cohort, clock, unit and independence disclosures; pursue equivalent comparative evidence for
   warning behavior and operational workflows rather than treating position error as the system.
3. Evaluate MSAW warning onsets against explicit hazard windows, with timely/late/missed outcomes,
   nuisance records and terminal loss observations counted separately. Compressed alert episodes
   cannot establish continuous warning or all protection transitions; ambiguous cases are refused.
   Missing evidence remains unavailable.
4. Complete live controller input/presentation and authenticated supervision/recovery; integrate
   reviewed increments, run the software gate, and maintain a separately frozen
   surveillance qualification candidate. Feature development must not consume sealed hold-out
   data or change the candidate after results are seen.

Each increment gets an externally usable API or command, negative cases, an independent expected
answer and a stated boundary. Subsequent integration must preserve one flight identity and one
issue-time view throughout the session. No live system, availability, operational-safety or
legacy-superiority claim follows from this milestone alone.

## Evidence and stopping rules

The software gate checks regressions and the ruleset. Benchmark acceptance additionally needs
the declared cohort, full denominators, zero hidden work loss and every measured deadline.
Hold-out qualification follows the privately preregistered runbook; a successful local test or
push-tier proof run does not replace its full-run and provenance preconditions.

Performance tuning resumes only on a measured bottleneck with a bounded proposed change and an
explicit stop condition. Safety tuning requires a classified missed/late/nuisance case and a
check that the change does not conceal missing protection. A lane ends with reviewed code and
evidence or an explicit unresolved boundary, rather than silently expanding its remit.
