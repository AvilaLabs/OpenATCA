# Safety-state, conflict-alert and MSAW API proposal

**Tier:** L3 Experimental; outside the 0.1 library release scope. This is a draft proposal. No thresholds or API names here are approved FAA values or frozen compatibility commitments.

## Current source modules and orchestration

`oa-safety` provides core shared state in `net`, `inhibit`, `geom`, and `monitor`. `oa-ca` exports `ca_new`, `ca_check`, `ca_cycle`, `ca_finish`, `ca_publish_refused`, its caller-provided config/types, and budget/loss statistics. `oa-msaw` exports `msaw_new`, `msaw_check`, `msaw_plan`, `msaw_points`, `msaw_evaluate`, `msaw_finish`, `msaw_publish_refused`, and typed report/config structures. These are separate cores; a public orchestration layer still needs a source-neutral adapter that converts track rows and source adaptation into their plain input values.

`NetId` currently enumerates four net slots (conflict alert, MSAW, area proximity, approach path); the latter nets having IDs does not mean their operational algorithms are implemented. A net publishes `NetReport` with timestamp; the shared state table marks never-reported/stale status unavailable. Inhibitions have bounded geometry, altitude/time ranges, reason/author text, create/remove/expire operations and suppression logging. This state machine does not provide user identity or role authentication; “author” is a value provided by caller, not authenticated identity.

## Inputs, units, limits, and outcomes

- Conflict alert receives `CaTrack`: latitude/longitude decimal degrees, east/north velocity m/s, 95% radius metres, altitude feet, vertical rate feet/minute, flags and ground state. `CaConfig` supplies track/open-alert/candidate capacities, look-ahead and hysteresis in ns, grid/evaluation work budgets, speed/uncertainty bounds, default volume and at most 32 configured volumes. It has no design-default minima; caller owns values. `ca_cycle` returns `CycleOut` and updates bounded alert/event logs and shared net status. Budget exhaustion is counted/degraded; a non-increasing cycle time is refused and published unavailable.
- MSAW receives similar track values, with altitude datum explicitly the datum of the floors after shell correction. `FloorSample` carries terrain/obstacle/MVA and clearance in feet MSL; `SamplePoint` uses E7 coordinates, metres query radius and degrees true heading. The core receives precomputed floor samples from the shell; it does not load terrain or obstacles. `MsawConfig` has no safety defaults: it carries max tracks/open alerts, per-cycle track budget, look-ahead/warning times in seconds, 2..32 path samples, hysteresis ns, clearances in feet, corridors (hard cap 16,384), speed/uncertainty limits, altitude-age policy and degradation shares. Hard ceilings include 600 s look-ahead, 32 samples/track, 1,048,576 track/open table limits, 100 km corridor length, 200 km query radius and latitude up to 85 degrees.
- MSAW planning ranks lower predicted altitudes first when its track budget is exhausted. Unusable rows, missing/stale altitude, ground exclusions, coverage gaps, lost upstream rows, per-track skips and dropped log entries are counted. It never treats missing terrain/obstacle coverage as safe; incomplete coverage degrades the net. Trajectory is straight-line; turning prediction is not established here.
- Shared safety-state capacities include 64 concurrent inhibitions, 4,096 suppression rows and 256 inhibition events. Time uses signed Unix nanoseconds; altitude is feet; inhibition positions are E7. Full bounded logs must remain observable through overflow counts.

## Candidate API boundary

Keep three separate public packages or modules: `safety-state` for net state/inhibitions; `conflict-alert` for CA; and `msaw` for MSAW. A provisional high-level `cycle(input, config, state) -> CycleReport` can be considered, but do not imply it exists: current source exports `ca_cycle` and the split `msaw_plan`/shell-lookups/`msaw_evaluate` protocol. Preserve the split in MSAW so terrain queries stay outside the pure core. Caller-owned `CaConfig` and `MsawConfig` should require explicit operational values and should not gain unreviewed defaults. A public `NetReport` must preserve `Running`/`Degraded`/`Unavailable` semantics and all loss counters.

Profiles must identify who supplies minima, MVA, terrain, obstacles, approach procedures, altimeter/QNH and cold-temperature adjustments. Do not call an inhibition author a principal until a separate authenticated user/account layer exists. Do not infer approved controller inhibition policy from the current fixed-capacity table.

## Errors, determinism and evidence status

Config checks return `Result<(), &'static str>` today; there is no structured stable error code. Runtime refusal/degradation is mostly represented by `CycleStats`, skip/event logs and `NetReport`, not a single exception. Converting to a public API should give stable typed errors without hiding this loss accounting.

Source describes fixed-array/no-allocation, deterministic cycle cores and pure calculations. MSAW and CA are sensitive to caller time order, limits, floor datum and caller parameters. This source review does not establish no-panic behavior, validated operational thresholds, full vector/reference coverage, fuzz duration, mutation review, Kani completeness or cross-target bit identity. Synthetic test values are not FAA minima or operational performance evidence. Remain Experimental pending the full §8 module evidence and independent safety review.
