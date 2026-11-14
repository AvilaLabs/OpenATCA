# OpenATCA — Scope

Status: scoping, 2026-11-13. Owner: Connor Avila (Avila Labs).

## 1. What this is

OpenATCA (Open Air Traffic Control Automation) is an open-source platform aimed at the same job as the FAA's
planned **Common Automation Platform (CAP)**: one system that replaces ERAM (en route, 20
centers) and STARS (terminal radar approach control and towers).

CAP is the largest unfunded software item in the administration's air traffic control rebuild,
estimated at about $10B of the roughly $19–20B still being requested on top of the $12.5B
already appropriated. The appropriated money is mostly hardware (telecom, radar, radios,
facilities). CAP is software.

We cannot deploy into the National Airspace System (NAS) ourselves. The goal is a platform good
enough, and assured well enough, that qualified people would want to test it. Everything is built
for that possibility from the first commit.

### How it gets used if it succeeds

We build for the operational end state, but adoption comes in rungs. Each rung needs less approval
than the next, and the same core serves all of them.

1. **Independent benchmark.** A public yardstick: the same recorded traffic run through our
   platform with published tracking, prediction and alerting metrics. Vendor bids and the FAA's
   own systems can be measured against it. Needs no approval.
2. **Training and research.** Controller training simulators and research testbeds (FAA Technical
   Center, NASA, universities, controller training programs). Simulation does not need
   operational certification, and it needs exactly our core plus the display.
3. **Independent safety monitor.** Runs beside ERAM/STARS on the same feeds as a dissimilar
   second system. It flags disagreements, and it keeps a coherent traffic picture when the primary
   or its network degrades, as on 2026-11-13. Advisory only, so the approval bar is lower than for
   a control system.
4. **Contingency system.** Takes over a defined set of functions when the primary fails.
5. **Primary platform.** CAP itself, carried by an integrator under the RFI's Enterprise
   Re-Architecture path, with the code owned by the public. Smaller ANSPs abroad, who cannot
   afford the big vendors' platforms, are a second home for it.

Precedent for open government-adjacent software becoming the standard: NASA's open-source
DAIDALUS is the reference implementation of the RTCA DO-365 detect-and-avoid standard.

Design consequence: live feeds, recorded feeds and simulated traffic enter the core through the
same interface and are indistinguishable to it. One core, five uses.

### In scope

- What ERAM and STARS do: surveillance processing and tracking, flight data processing,
  flight-plan/track correlation, conflict alert, terrain warning, sector and handoff logic,
  controller display, for en route and terminal airspace.
- Migration machinery: shadow mode, comparator, per-function cutover and rollback, adapters to
  legacy formats.
- The simulation and replay that rungs 1 and 2 need.

### Out of scope

- Hardware: radars, radios, network routers, the telecom network itself.
- Voice communications.
- Traffic flow management (TFMS, the SMART tool) and NOTAMs: separate FAA systems, though our
  interfaces must exchange data with them.
- Tower surface surveillance (ASDE-X) and airborne systems.
- Oceanic airspace (ATOP): later. The RFI mentions oceanic scale, so the data model must not
  rule it out.

### Why now

On 2026-11-13 the FAA stopped every departure in the country for about 4–5 hours after a
communications failure across multiple en route centers. By the officials' accounts, new equipment
pushed more data into old network routers, the routers overloaded and sent inconsistent data, and
radar and communications between facilities became unstable. That is a migration failure: new and
old systems joined with no flow control, no load shedding and no fault isolation. This project
treats safe migration and overload behaviour as core requirements, not afterthoughts.

## 2. What the FAA asked for (CAP RFI, 2025-11-19)

Source: FAA CAP Request for Information, responses were due 2025-12-19 (copy in `refs/`).
The questions that define the target:

- **Coverage:** "the majority of the operational capability already provided today across its
  en-route, and terminal domains."
- **Transition:** real-time data exchange with ERAM and STARS during transition; phased
  deployment; "zero-downtime transitions and legacy system coexistence."
- **Reliability:** high availability, fault tolerance, rapid recovery; real-time detection and
  reporting of safety-critical events.
- **Data quality:** normalization across heterogeneous sources; handling of gaps and
  inconsistencies in weather, surveillance and flight data.
- **Scale:** about 30 million square miles of domestic and oceanic airspace; over 45,000 flights a
  day.
- **Assurance:** development assurance levels, DO-278A / DO-178C / ED-109, hazard analysis,
  traceability, independent verification and validation, formal methods where applicable.
- **Architecture (Automation Evolution Strategy):** layered, service-based; separate compute,
  platform and mission applications; modules developed and replaced independently; Zero Trust.
- **No vendor lock-in:** open standards and data models, data export in non-proprietary formats,
  exit planning, source code escrow, API use without proprietary connectors, and periodic
  "portability fire drills."
- **Future users:** UAS, advanced air mobility and space operations without major redesign.

An open-source platform meets the lock-in section by construction, and most of the transition
section is what we intend to build first.

## 3. Design principles

1. **Migration first.** Every function can run in shadow beside a legacy system, be compared
   against it, be cut over on its own, and be rolled back. Coexistence is a mode the platform
   runs in, not a project phase.
2. **Overload is designed, not discovered.** Every queue is bounded. Every message carries a
   priority class. When load exceeds capacity the platform sheds in a written order and says so;
   it never passes on corrupted or inconsistent data silently. A fault in one site or one adapter
   cannot spread past its boundary.
3. **Deterministic replay.** Every input is recorded with its time. Any run can be replayed bit
   for bit. This is how we test, how we compare against legacy output, and how an outage is
   investigated after the fact.
4. **Assurance built in.** Requirements are numbered and traced to code and tests from the first
   commit. Structural coverage is measured from the start. The DO-278A evidence is a by-product of
   how we work, not something added at the end.
5. **Open data model.** Every internal message has a published schema. External formats
   (ASTERIX, ARINC 424, AIXM, FIXM, the SWIM feeds) are handled at the edge by adapters, so the
   core never depends on any one source.
6. **Minimal dependencies.** Core logic is written in-house. Outside code only for proven
   cryptography and entropy, each exception recorded with its reason.

7. **The controller display is safety over flash** (Connor, 2026-11-13). Data is shown with as
   little abstraction as possible. The display is a pure function from traffic state to a plain
   list of drawing commands, redrawn from current state every frame, so nothing on screen can
   drift from the data and the screen content is testable exactly. Characters are drawn with
   stroke fonts we define, not a font engine. A thin renderer draws the list; the only outside
   code is the minimal layer that opens a window. Stale, coasting and frozen states are always
   visible. General GUI toolkits (egui and similar) are acceptable only for internal engineering
   tools, never the controller display, because of their dependency trees and the layout
   decisions they make on our behalf.

Implementation language: Rust (memory safety without a garbage collector, deterministic timing,
already in use across Avila Labs). The core targets the standard library only.

## 3a. Standard: best in class, not parity

Connor, 2026-11-13: matching ERAM and STARS is not the goal. Every function must beat the
current state of the art, the way ACTINV replaced FISPACT's old solver with CRAM instead of
copying it. For each function we name the legacy method, the modern method that replaces it,
and the metric that proves it is better.

| Function | Legacy approach | Our approach | Proven by |
|---|---|---|---|
| Surveillance tracking | Single-model filters tuned per site; the DOT OIG found the same tracker problems in both ERAM and STARS | Multi-sensor fusion with interacting multiple models, weighting ADS-B by its own reported integrity (NIC/NACp), radar as cross-check | Position and velocity error against ADS-B truth; track swaps; track drops |
| ADS-B integrity | ADS-B largely trusted as reported | Every report checked for spoofing and faults: kinematic plausibility, cross-sensor agreement, signal timing where available | Detection rate on injected spoofs; false-flag rate on real traffic |
| Trajectory prediction | Mostly straight-line extrapolation plus flight-plan route | Physics-based prediction from flight plan, published procedures, open aircraft performance models (OpenAP, not licensed BADA) and live NOAA winds | Prediction error at 2, 5 and 20 minutes against what the aircraft actually flew |
| Conflict alert | Fixed geometric buffers; nuisance alerts are a known controller complaint | Intent-aware, probability-based alerting on top of a geometric core whose properties are proved (DAIDALUS-style formal verification) | Missed alerts and nuisance alerts on recorded traffic, both reported, neither traded silently for the other |
| Terrain warning (MSAW) | Site-tuned terrain and obstacle buffers | Terrain and obstacle data from public sources, with procedure-aware suppression instead of hand-drawn exclusion areas | Missed and nuisance warnings on recorded approaches |
| Site adaptation | Built and maintained by hand per facility | Generated from NASR, CIFP and AIXM each AIRAC cycle, with an automatic diff of what changed | Time to adapt a new facility; adaptation defects found by diff |
| Input safety | The 2014 Los Angeles Center ERAM outage came from one unusual flight plan (a U-2) exhausting memory | Every input bounded and validated at the edge; every parser fuzzed; no input can exhaust a shared resource | Fuzzing hours with zero crashes; stated bounds on every input |
| Overload | The 2026-11-13 routers passed inconsistent data under load | Written load-shedding order; inconsistent data is refused and flagged, never passed on | Replay of overload scenarios with surveillance and alerts staying consistent |
| Redundancy | Hot-standby channels | Active-active replicas running the same deterministic state machine; failover with no lost state | Failover time; state divergence under fault injection |
| Migration | Big-bang site cutovers over years | Per-function shadow, comparison, cutover and rollback | Difference reports during shadow; rollback time |
| Performance | Large dedicated hardware per facility | Whole-NAS real-time on one ordinary machine (peak airborne traffic is in the thousands; this is a small compute load) | Latency and throughput at full NAS load on a laptop |
| Verification | DO-278A process evidence | DO-278A evidence plus formal proofs of safety-critical cores, property-based testing, mutation testing and MC/DC coverage | Proof count; coverage; surviving mutants |
| New airspace users | Bolted on | Drones, advanced air mobility and space launch corridors as first-class object types from day one | Same metrics applied to mixed traffic |

Any row we cannot yet beat gets a diagnosis and a next step, not a lowered bar.

## 4. Architecture

```
  ingest adapters        normalized bus           core services                outputs
  ---------------        --------------           -------------                -------
  ADS-B (Beast/SBS)  ->                      ->  surveillance tracker     ->  controller display
  ASTERIX (radar)    ->   typed, versioned   ->  flight data processing   ->  shadow comparator
  SWIM FDPS/STDDS    ->   messages, bounded  ->  plan/track correlation   ->  safety event log
  NASR / CIFP        ->   priority queues,   ->  conflict alert           ->  replay archive
  weather (NOAA)     ->   recorded for       ->  terrain/obstacle (MSAW)  ->  legacy-format
  legacy adapters    ->   replay             ->  sector/handoff logic     ->    output adapters
                                              ->  airspace/adaptation
```

- **Adaptation data** (airspace, sectors, fixes, routes, procedures) comes from public NASR and
  CIFP data and is versioned on the 28-day AIRAC cycle.
- **The comparator** takes any two streams (ours against a legacy system, or ours against
  ground truth) and reports differences by function.
- **Health and load** of every service and queue are first-class outputs, so overload is visible
  before it becomes a failure.

## 5. Data we can get

| Source | Gives us | Access |
|---|---|---|
| NASR (28-day subscription) | Airspace, sector boundaries, fixes, airways, airports, navaids | Public download |
| CIFP | Every published procedure (SIDs, STARs, approaches) in ARINC 424 | Public download |
| SWIM via SCDS (FDPS, STDDS, TFMData, TBFM) | Live flight plans, terminal surveillance and flight data, flow management | Free, needs an FAA SWIM account |
| ADS-B (OpenSky Network, or our own receiver) | Live positions for most IFR traffic (mandated since 2020) | OpenSky research access, or a low-cost SDR receiver |
| NOAA MRMS and aviation weather | Weather for display and data-quality tests | Public |
| ASTERIX specifications | Radar data formats, so legacy-style feeds can be simulated | Public (Eurocontrol) |

Account sign-ups and any hardware purchase go to Connor first.

## 6. Gaps we cannot close alone

These are written down so the outreach later is specific.

| Gap | Why it matters | What we do meanwhile | Who could close it |
|---|---|---|---|
| Primary and secondary radar feeds | Non-ADS-B traffic; real radar error behaviour | Simulate radar from ADS-B truth with published error models; full ASTERIX support | FAA, an ANSP, or a research lab with radar access |
| ERAM and STARS interface control documents | Real legacy coexistence | Build clean adapter interfaces; test against our own simulated legacy | FAA (William J. Hughes Technical Center) |
| Real adaptation data | Exact sector configurations, local procedures | Derive from NASR/CIFP; note every assumption | FAA facilities |
| Controller evaluation | Display usability, workload | Prototype display; test with simulator users | Retired controllers, university ATC programs, NATCA |
| Assigned assurance levels | Which functions need which DO-278A level | Assume the highest plausible level per function | FAA safety assessment |

The assurance-level question is the main open one. Conflict alert and terrain warning are
safety-critical; until a safety assessment assigns levels, we hold them to the strictest level we
can justify and record the assumption.

## 7. Milestones

Region for M0–M5: Chicago Center (ZAU), where the 2026-11-13 failure began, and its TRACONs.

- **M0 — Airspace foundation.** NASR and CIFP parsers; ZAU airspace, sectors, routes and
  procedures in the internal model; requirements register and traceability tooling.
- **M1 — Record and replay.** ADS-B and SWIM ingest; a recorded day of ZAU traffic; bit-exact
  replay harness.
- **M2 — Tracking.** Surveillance tracker; flight-plan-to-track correlation; track quality
  metrics against ADS-B truth.
- **M3 — Safety alerts.** Conflict alert and minimum safe altitude warning, with published
  thresholds and test cases.
- **M4 — Shadow mode.** Live operation on the real feeds with the comparator running; per-function
  difference reports.
- **M5 — Overload and fault demonstration.** Reproduce the 2026-11-13 failure mode in replay:
  a new high-rate source added to a legacy-style network. Show the platform shedding load and
  isolating the fault while surveillance and alerts stay consistent.
- **M6 — Controller display.** Working display for en route and terminal positions.

After M3 the public benchmark (rung 1) can be published. After M4 the platform runs as a
safety monitor in shadow on live data (rung 3 in miniature), and is far enough along for outreach
to close the gaps in section 6. M6 makes it usable for training and research (rung 2).

## 8. Prior work to learn from

- BlueSky (TU Delft): open air traffic simulator; traffic generation and conflict detection
  algorithms.
- OpenSky Network: ADS-B data and published research on its quality.
- readsb / dump1090: ADS-B decoding.
- Eurocontrol specifications: ASTERIX, and the published SESAR material on service-based ATM.
- Commercial platforms named by the RFI's "Operationally Proven Platform" path (for example the
  systems used by other air navigation service providers): their public documentation sets the
  bar for function coverage.
