# Capability and integration register

Reviewed against baseline source `4ad16fe` and the regional increments described below,
2026-11-23. This register separates implemented code, its
integration boundary, and the evidence still needed. A component or scripted fixture does not
close a milestone in [SCOPE.md](../SCOPE.md). OpenATCA remains a research prototype.

| Capability | Implemented entry point / evidence | Integration and qualification limit |
|---|---|---|
| Airspace adaptation | `oa-airspace`, NASR/CIFP import; [M0 report](M0_REPORT.md) | M0 is closed for its stated region/data; this does not establish every facility's operational adaptation. |
| Ingest and deterministic replay | `oa-feed`, `oa-replay`, ordered input fingerprints and state witnesses | Live ingestion and file replay exist. Authenticated peer distribution and autonomous facility operation are not established by those tools. |
| Surveillance | `oa-track`, `oa-perf`, recorded comparison tooling | Recorded and synthetic paths exist. [Radar qualification](RADAR_CAPACITY_2026-11-23.md) still fails strict latency and zero-loss criteria; gate coverage is not identity accuracy. |
| Flight state and handoffs | `oa-flight`, `oa-flight-sfdps`; [controller workflow](CONTROLLER_WORKFLOW.md) | Plan lifecycle, altitude forms, ownership and handoff operations exist. The composed workflow is scripted; a live coordination service and controller command path are not complete. |
| Arrival trajectory experiment | `oa-legacy-traj::core::arrival`, `--arrival-meter-fix`; [method and limits](../tools/oa-legacy-traj/README.md) | Issue-time route/track/assignment evidence produces 5/10/20-minute forecasts and meter ETA, with cutoff revision selection and retained provenance. The experiment freezes its first eligible issue per flight. This uses supplied expanded routes, not an independent NASR/CIFP route converter. Explicit wind prediction exists in the core; a weather/airspeed source is not wired into the scored shell baseline. |
| Shared operational trajectory | M4 design | The arrival experiment is not yet the single authoritative trajectory consumed by conformance, coordination, conflict probe and safety nets. Aircraft performance and uncertainty qualification remain open. |
| Conflict alert | `oa-ca`, `oa-ca-run` | Bounded geometric alerting, net status and replay tools exist. Development settings and finite tests do not qualify facility warning/miss/nuisance rates. |
| Minimum safe altitude warning | `oa-msaw`, `oa-msaw-run`; [offline encounter evaluator](../tools/oa-msaw/EVALUATION.md) | Terrain/obstacle/procedure-aware replay exists, including flight-data eligibility. The evaluator measures warning onsets against independent synthetic windows and refuses ambiguous compressed records. Complete cycle-level availability evidence and facility qualification remain necessary. |
| Other safety nets | `oa-safety` state/inhibition infrastructure; `oa-runway` geometry/occupancy | APW and approach-path monitoring are not complete nets. Runway occupancy does not implement a complete surface tracker or runway alerting service. |
| Controller display | `oa-display`, `oa-scope-replay` | Deterministic scene/raster and tracked/raw replay frames exist. KMS presentation, complete interactive position workflow and controller evaluation remain open. |
| Independent monitor and recovery | `oa-safety::monitor`, verified replay, raw fallback, [raw replay witness](../crates/oa-scope-replay/tests/RAW_WITNESS.md), process/transport drills | The monitor consumes caller-paired evidence. Same-host process loss and synthetic launcher transport are tested. The raw witness verifies repeated output by replaying from the beginning. Production supervision, durable peer recovery, authenticated remote fencing and two-host qualification remain separate work. |
| Developer interface | [OpenATCA Core](../crates/openatca-core/README.md) | Experimental synchronous offline ADS-B replay API; not a live regional automation API. |

## Next integrated research milestone

One reproducible regional session should show traffic and positively identified flight state,
an amendment and handoff, arrival prediction, safety alerts and explicit degraded states, then
recover from a serving-process failure. Its audit trail must identify exact inputs, source,
binary, settings, receipt-time basis and every omission. The current independent tools and
fixtures are ingredients; a single integrated regional runner is not yet delivered.

The source-ordered route API `forecast_latest_source_route` now supplies a bounded integration
building block: availability gates the evidence, source time orders amendments within one full
GUFI and reporting centre, and successes and refusals retain complete candidate counts. It does
not change the frozen study selector. The visible-CA recovery fixture also checks current CA
state before rasterizing, with matching tracker/display identities, altitude and coast/uncertainty
status. These remain component and same-host fixture evidence; the proposed composition is in
[the regional session plan](REGIONAL_SESSION_PLAN.md).

The next development increments are bounded:

1. Select the route revision actually available at an issue cutoff, reject conflicting revisions,
   and feed the existing arrival predictor with retained provenance.
2. Persist and verify raw-replay evidence across a restart, binding inputs, adaptation, receipt
   semantics and effective rendering settings. This is a recovery building block, not failover.
3. Evaluate MSAW warning onsets against explicit hazard windows, with timely/late/missed outcomes,
   nuisance records and terminal loss observations counted separately. Compressed alert episodes
   cannot establish continuous warning or all protection transitions; ambiguous cases are refused.
   Missing evidence remains unavailable.
4. Integrate reviewed increments, run the software gate, and maintain a separately frozen
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
