# OpenATCA — Scope

Status: scoping, 2026-11-13; milestones and benchmarks revised 2026-11-14; runway safety added 2026-11-20. Owner: Connor Avila (Avila Labs).

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
- Runway safety (added 2026-11-20): tracking aircraft on the airport surface and alerting on
  runway hazards: a landing or departure onto an occupied runway, two aircraft cleared onto one
  runway at once, or an aircraft lined up to land on a taxiway. It is built on the
  surface positions ADS-B already carries, so it can serve airports that have no surface radar,
  as Austin did not in the 2023 near-collision there. Where an airport has ASDE-X, its output on
  STDDS is the reference the tracking is measured against. Sequenced after the M5 airborne
  safety nets.

### Out of scope

- Hardware: radars, radios, network routers, the telecom network itself.
- Voice communications.
- Traffic flow management (TFMS, the SMART tool) and NOTAMs: separate FAA systems, though our
  interfaces must exchange data with them.
- Replacing ASDE-X, the surface radar and multilateration system itself: its raw sensor data is
  not available to us, so aircraft and vehicles without ADS-B are seen only where ASDE-X output
  exists.
- Airborne systems.
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

### 2a. Where CAP stands (public sources, 2026-11-14)

- CAP is phase 2 of the FAA's "Brand New Air Traffic Control System" programme. Phase 1 (the
  $12.5B appropriated in July 2025) is mostly hardware; Peraton is its prime integrator
  (December 2025, $1.5B ceiling). GAO-26-107992 puts phase 2 at about $10.2B and says the FAA
  has no start or finish date for it. The FY2027 request holds $100M "to begin the transition
  toward CAP"; a further $7–10B is being sought.
- Shortlist (The Air Current, 2026-04-10): Collins Aerospace (RTX, maker of STARS), Leidos
  (maker of ERAM), Thales, Indra and Frequentis, down-selected to four by late August 2026
  (names not all public). Thales is offering TopSky-America, a cloud-hosted adaptation of its
  TopSky-ATC.
- No public commitment to source code ownership, escrow or open release has been found.

So CAP is not new code being written from scratch. It will almost certainly be one of these
companies' existing platforms, adapted to the US. **The bar is therefore the best of TopSky-ATC,
Indra's iTEC/AirCon, Frequentis, Collins AutoTrac and ERAM itself,** function by function, plus
the failures documented in docs/LEGACY_SHORTCOMINGS.md. Matching CAP is not enough; the target
is to beat what any of the bidders can show.

Functions the best platforms advertise that ERAM and STARS lack or keep separate: one 4D
trajectory shared by every tool (conflict probe, arrival and departure management, data link);
route and level conformance monitoring (Indra's Route Adherence Monitoring); controller–pilot
data link; stripless working. NASA notes that ERAM's conflict probe and the trajectory-based
tools were "developed completely independently" with "incompatible requirements" (NTRS
20130010914). One shared trajectory is therefore a design requirement here (M4), not an
add-on.

## 3. Design principles

1. **Migration first.** Every function can run in shadow beside a legacy system, be compared
   against it, be cut over on its own, and be rolled back. Coexistence is a mode the platform
   runs in, not a project phase.
2. **Overload is designed, not discovered.** Every queue is bounded. Every message carries a
   priority class. When load exceeds capacity the platform sheds in a written order and says so;
   it never passes on corrupted or inconsistent data silently. A fault in one site or one adapter
   cannot spread past its boundary.
2a. **Overload in systems we do not control** (Connor, 2026-11-14). We cannot fix another
   system's routers, but we can make sure we never cause their overload, see it first, survive
   it, and never pass on what it corrupts:
   - **Never the cause.** Every interface we send on has a published traffic contract (maximum
     rate, maximum burst, maximum message size) enforced inside our own sender. During shadow
     mode we measure what each interface can absorb and set the contract below it. A new system
     joined to an old one must prove it cannot flood it, which is exactly what failed on
     2026-11-13.
   - **See it first.** Every input feed reports its health continuously: rate, latency, sequence
     gaps, duplicate and out-of-order messages, and disagreement with independent sources. Rising
     latency and gaps are leading indicators; we alarm on trends, not only on outright loss, and
     we always distinguish "no traffic" from "feed dead".
   - **Never pass on corruption.** When a source starts disagreeing with independent sources
     (radar against ADS-B against flight plans), it is quarantined and flagged, not averaged in.
   - **Survive it.** Each facility's instance is autonomous for safety functions. Surveillance can
     come from receivers local to the facility (ADS-B ground stations independent of the
     inter-facility network), so a backbone outage degrades coordination between facilities, not
     each facility's own traffic picture. When the network returns, state re-synchronises.
   - **Show the margin.** As an independent monitor, the platform can report the load and health
     of the feeds it watches, giving operators warning before a collapse rather than after.
3. **Deterministic replay.** Every input is recorded with its time. Any run can be replayed bit
   for bit. This is how we test, how we compare against legacy output, and how an outage is
   investigated after the fact.
4. **Assurance built in.** Requirements are numbered and traced to code and tests from the first
   commit. Structural coverage is measured from the start. The DO-278A evidence is a by-product of
   how we work, not something added at the end.
5. **Open data model.** Every internal message has a published schema. External formats
   (ASTERIX, ARINC 424, AIXM, FIXM, the SWIM feeds) are handled at the edge by adapters, so the
   core never depends on any one source.
6. **Data-oriented design.** The engineering law is docs/RULESET.md (Connor's zat4 ruleset adapted
   to Rust): plain data in struct-of-arrays, typed indexes, compile-time size guards, a pure core
   with time as an input, no allocation after startup in the real-time core.
6a. **Minimal dependencies.** Core logic is written in-house. Outside code only for proven
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
   Presentation path (Connor, 2026-11-13: start with KMS, no throwaway backend): our renderer
   draws into a plain pixel buffer (pure, testable byte for byte against reference images).
   Linux KMS/DRM direct-to-display is the real backend from the start: no desktop stack between
   the data and the pixels, and page flipping guarantees every displayed frame is complete, never
   half old and half new. A headless image backend serves tests, CI and demo material. Input is
   read from Linux input devices directly. KMS needs a few raw OS calls (ioctl, mmap); they live
   in one isolated module under a recorded exception to the no-unsafe rule (RULESET.md A0).

8. **Every facility is a full node** (Connor, 2026-11-19; design in docs/NODE_NETWORK.md). If the
   whole NAS fits on one laptop (M9), the network no longer exists to divide the workload. It
   exists for resilience, locality and independent checking:
   - **The whole picture everywhere.** Every facility can compute the national picture from
     sensor data, so any facility can take over a neighbour's airspace at once. After the fire
     at Chicago Center in September 2014, neighbouring centres took over its airspace under
     improvised arrangements, and recovery took weeks.
   - **Verify, don't trust.** Processing is deterministic, so facilities fed the same inputs
     reach the same state bit for bit and can prove it by exchanging state hashes. A mismatch
     is detected and flagged, never resolved by averaging. With three or more independent nodes
     on the same inputs, agreement of the others can identify which node is faulty (the
     triple-redundancy pattern of fly-by-wire computers); it isolates that node and never
     decides who controls an aircraft. Membership is known and authenticated, so classical
     Byzantine fault tolerance applies; proof-of-work, which exists for open membership, does not.
   - **Control is assigned, never voted on.** Data and computation are replicated; authority is
     not. Exactly one position is responsible for each aircraft at any moment, and responsibility
     moves only by explicit handoff. No consensus protocol sits in any real-time path.
   - **Tamper-evident record.** Each facility keeps an append-only record of every input and
     every output, hash-chained and signed, so the record that an investigation relies on cannot
     be changed without detection. Optionally, a daily digest of the record is anchored in a
     public timestamping chain (for example Bitcoin through OpenTimestamps), giving anyone proof
     that the record existed in that form on that date.
   - **Run alone when the backbone fails.** A facility cut off from the shared network keeps
     working on its own local sensors (principle 2a), and facilities can exchange the essentials
     over a path independent of the backbone. Which path (satellite, HF radio, others) is open.
   - **Cost rule: a node is a deployment, not a subsystem.** The core does not change: the same
     deterministic engine runs with a wider input set. Node features (input distribution, hash
     exchange, signed record) live in the shell, each one small and added only when a milestone
     needs it. No consensus protocol, no replication machinery beyond the inputs themselves.
     First evidence (M9): two nodes on the same recording agree bit for bit, and a replayed
     takeover of one centre's airspace by another works with no hand edits.
   - **Judged against real failures** (Connor, 2026-11-19). When the node features exist, each
     real incident in docs/LEGACY_SHORTCOMINGS.md and the migration record is assessed one by
     one: would the node setup have prevented it, reduced it, or made no difference, and why,
     with sources. "No difference" results are reported as prominently as the others. Real
     incidents first, hypothetical scenarios only to fill gaps.
   - **Replicas catch broken machines, not broken code.** Identical deterministic nodes agree on
     a shared bug and die together on a shared poison input (NATS 2014, Ariane 501). Code faults
     need diversity, bought in this order of evidence: (1) an independent safety monitor on every
     node, a small separately written checker that bounds every output track against the raw
     measurements, physical limits, update ages and table limits (the Simplex runtime-assurance
     pattern), verified far more heavily than the tracker; (2) a raw-surveillance display path
     that does not pass through the tracker; (3) different compilers, hardware and operating
     systems across nodes, and staggered restarts; (4) a second implementation only of narrow,
     well-specified pieces such as the safety nets, used as a differential-testing oracle, never
     counted as an independent vote. Independently written versions are known not to fail
     independently (Knight and Leveson 1986 and later replications), so no reliability figure
     is ever multiplied out from a replica count.
   - **Fallback ladder:** full picture; picture annotated by the monitor; raw surveillance only
     (the role ERAM's EBUS and STARS's emergency service level play today). Every step down is
     logged and counted.
   - **Poison input:** an input that crashes or stalls processing is quarantined and the node
     runs on without it; it is never reprocessed on restart, and it becomes a permanent test.
   - **What may be claimed:** only measured results against a stated baseline: fault-detection
     rate and latency under injected faults, false-alarm rate, failover time, monitor escapes
     from a mutation study. No availability or failure probability is derived from the number
     of nodes.
   - **Limit:** taking over airspace also needs radio frequencies, radar feeds routed to the
     new site, and controllers trained on that airspace. The node design removes the automation
     and adaptation part of that problem, not the rest.

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

How each row is measured is in docs/BENCHMARKS.md: a published floor (the Recommended value of
the standard, not the Mandatory one), the legacy system measured by us on the same recorded
traffic, and the best published research result. SWIM carries ERAM's, TBFM's and TFMS's own
outputs, so the legacy number for tracking, arrival times and trajectory estimates is something
we measure, not something we quote.

## 3b. What openness and efficiency make possible

Connor, 2026-11-19. Three properties together (open code, a compute cost of about one percent of
a laptop core for national tracking, and bit-exact deterministic processing) allow practices
that a closed system needing racks of dedicated hardware cannot afford. Each is a planned
capability, not a side effect. Described as what these properties make possible, never as a
claim that the legacy systems are inefficient; the public record supports "old, costly and hard
to change" (DOT OIG AV-2020-040), and that is the comparison made.

1. **Exact replay for investigation.** With the signed record and the same version, anyone
   reproduces exactly what the system computed and displayed at any moment of an incident, and
   why each alert did or did not fire. Needs: the signed record (principle 8). Measured by:
   replay of recorded sessions reproducing every displayed state hash.
2. **The whole country on every change.** A recorded national day replays as a regression test
   on every change, and a release must reproduce the previous release's state hashes except
   where the change declares a difference. Needs: a recorded national fixture and the
   version-comparison job. In reach now.
3. **Rehearsal before change.** Before any software update, airspace change or procedure change,
   and for every 28-day aeronautical data cycle, the last month of real traffic replays through
   the new version or adaptation and every behaviour difference (alerts, handoffs, conflicts) is
   listed for review. In reach now for tracking; grows with each function.
4. **Fast-time simulation.** A day of traffic replays in minutes, so thousands of days, doubled
   traffic, a regional radar outage or a GNSS-jamming event can be run on one laptop and their
   effect on every metric measured.
5. **One codebase for training and operations.** Controller training runs the operational
   software on real recorded traffic, so nothing trained differs from what is operated. Needs:
   the display (M8).
6. **Contingency kit.** A portable machine with an ADS-B receiver gives a surveillance picture
   where a facility is lost or none exists (after a hurricane, at small island airports). It is
   situational awareness, not certified control, and says so on screen. Needs: the display and
   the LADD block.
7. **Several candidates in shadow at once.** Shadow running is cheap enough to run two or three
   candidate versions side by side on live data and choose on measured differences.
8. **No obsolete hardware.** Ordinary machines of any make run it, so no part becomes
   unobtainable; the hardware can be replaced on a commodity cycle.
9. **Fast safety fixes with evidence.** A fix ships with replay evidence that it changes only what
   it declares, so the review is of a small, proven difference.
10. **Public scrutiny.** Researchers, universities, security reviewers and the public observer
    network look for problems in the published code and data (SECURITY.md).

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

## 5. Data inventory

What each function needs, and whether we have it. "Have" means downloaded or recording and
decoded by our own code.

| Data | Used by | Source and licence | Status |
|---|---|---|---|
| Airspace, fixes, airways, airports, navaids, ARTCC boundaries | All | NASR 28-day subscription, public | **Have** (M0) |
| Procedures (SIDs, STARs, approaches) | M4, M5 | CIFP, ARINC 424, public | **Have** (M0) |
| ERAM flight data and track positions | M2 baseline, M4 | SWIM SFDPS, FAA account | **Have**, recording; about 60 s late (UK-004) |
| TBFM schedules and estimates | M2 baseline, M4 | SWIM TBFM | **Have**, recording |
| TFMS flight data, traffic management programmes | M2 baseline, M4 | SWIM TFMS | **Have**, recording |
| NOTAMs | M4, M5 (temporary restrictions) | SWIM FNS | **Have**, recording |
| Terminal surveillance and flight data | M3 terminal | SWIM STDDS | Account not approved |
| ADS-B history (truth, tracker input) | M2, M3 | adsb.lol history, ODbL | Format sample (2026-11-13, first part) **have**; the recorded days publish about 03:30Z the next day |
| ADS-B live, independent of the FAA | M3, M7 | Own receiver (about $40) | Purchase needs Connor |
| ADS-B history, research grade | M3 | OpenSky, research licence, non-redistributable | Application pending |
| Winds and temperature aloft | M4 | NOAA RAP 13 km (and HRRR sample), GRIB2, public domain | **Have** 2026-11-14 00–16Z; rest after the window; decoder building |
| METAR, TAF, PIREP, SIGMET, G-AIRMET | M4, M8 | aviationweather.gov API, public | METAR and PIREP **have** to 17Z and queryable for 30 days; TAF, SIGMET and G-AIRMET have no historical query, so an hourly snapshot runs until the window ends |
| Aircraft performance | M4 | OpenAP v2.6.2: code LGPL-3.0, **data files GPL-3.0**; read at run time from `openatca-data`, never copied into this Apache-2.0 repository. BADA is licensed and excluded | **Have** |
| Aircraft registry and types | M3, M4 | FAA releasable registry, public, daily | **Have** |
| Terrain | M5 | USGS 3DEP 1 arc-second, public domain | **Have** 73 of 99 tiles (3.5 GB, the ZAU core); 26 edge tiles (1.3 GB) deferred for space |
| Obstacles | M5 | FAA Digital Obstacle File, public, 56-day cycle | **Have** (cycle 260927) |
| Minimum vectoring and instrument altitudes | M5 | FAA MVA/MIA charts, AIXM 5.1 and PDF | **Have** C90, ZAU and 16 nearby TRACONs (FWA links broken at the FAA) |
| Encounter models | M5 | MIT LL em-model-manned-bayes v2.1.0, BSD-2-Clause, approved for public release | **Have** |
| Radar formats | M3 | ASTERIX specifications, EUROCONTROL, public | To fetch |
| Raw radar returns | M3 | Not public | Simulated from ADS-B truth with published error models |
| ERAM sector boundaries (high and low) | M4, M8 | Not public | Derived from SFDPS ownership changes in our recordings; gap recorded |
| TBFM adaptation meter points | M4 | Not public (12 of ZAU's do not resolve) | Gap recorded |
| ERAM and STARS interface documents | M7 | Partly: ERAM–non-US ACC interface document v4 is hosted by ICAO; NAS-MD-311 status unknown | Fetch the ICAO copy; outreach for the rest |
| Letters of agreement, local procedures | M4, M8 | Not public | Outreach |

| LADD list (aircraft whose owners asked that their data not be displayed or distributed) | All outputs | FAA ADX portal (adx.faa.gov), "IndustryLADD", monthly | **Needed**: an ADX account for Connor |

Account sign-ups, purchases and every download in this table go to Connor first.

### Obligations that come with the SWIM data (FAA Terms of Service, accepted per feed)

- **Compressed consumption only, JMS only.** The bridge connects over Solace JMS with compression
  level 9 (fixed 2026-11-14; before that it ran uncompressed).
- **LADD blocking.** No data, including historical data, may be displayed or distributed for an
  aircraft registration or call sign on the FAA LADD list, except at the owner's direction; we
  must update within five business days of each monthly list. So every display, report, replay
  export and screenshot filters LADD aircraft, and the filter is a tested platform function, not
  a manual step. Until we have the list, nothing that names individual aircraft leaves the
  laptop.
- **Not FAA data.** Anything we redistribute may not be characterised as FAA data: published
  results say "derived from SWIM data", never "FAA data".
- **Not NAS-impacting, not a sole safety source.** Consistent with every adoption rung before
  certification.
- Each feed's Service Description Document §4.5 may add terms; to be read for each feed.

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

Region for M0–M6 and M8: Chicago Center (ZAU), where the 2026-11-13 failure began, and its
TRACONs (C90 first), worked from recordings. Region for M7: Jacksonville Center (ZJX) and
Orlando TRACON (F11), because a live shadow needs our own ADS-B receiver and ours is in
Oviedo, Florida, under F11; a receiver only hears aircraft within about 150 to 250 NM. Before
M7 the M0 model, MVA charts and terrain are built for ZJX and F11 with the same code. M9
widens to the whole NAS.

Each milestone lists what it builds and the exit criteria that close it. Exit criteria use the
three bars in docs/BENCHMARKS.md. A milestone does not close on a missed bar; it closes on a
beaten bar or on a written diagnosis with a next step that Connor accepts. Every exit result is
reproducible from a recording, a scoring-code version and a replay state hash.

Relative size is given as S, M or L against M0 (S). No calendar estimates: the decoders went fast
because each had a published format; the tracker, flight data processing and safety nets need
design judgment and validation against real behaviour.

### M0 — Airspace foundation (done, 2026-11-14)

NASR and CIFP parsers, ZAU airspace model, requirements register and traceability tooling.
Report: docs/M0_REPORT.md.

### M1 — Record and replay (S, mostly done)

Built: SCDS adapter; decoders for SFDPS, TBFM, TFMS and FNS; recorder with per-feed health;
streaming deterministic replay (constant memory, same state hash every run).

To close:
- The 24-hour four-feed recording of 2026-11-14/15 completes and replays to a stable hash.
- ADS-B history for the same 24 hours and region (adsb.lol) decoded into the same replay.
- Weather for the same day captured (HRRR winds, aviationweather.gov products) before it ages
  out.
- STDDS added when the account is approved (does not block closing).

Exit: one command replays the day with all sources merged; two runs give the same hash; feed
health shows no unexplained gaps.

### M2 — Benchmark harness and legacy baseline (M)

Before building any function, measure the system it must beat, on the same traffic, with the
code that will later score us. This milestone is new: it turns "best in class" from a claim into
a number for every later milestone.

Builds:
- Smoothed reference trajectories from ADS-B (NACp 8 or better), with event extraction:
  meter-fix crossings, runway thresholds, top of climb and descent.
- Scoring code for every metric in BENCHMARKS §3, written once and used for legacy and ours.
- The legacy baseline from our recording:
  - ERAM track positions (SFDPS) against truth: position and velocity error, probability of
    update, gaps, at ERAM's own timestamps so the 60 s delivery delay does not count against it;
  - TBFM estimated times against actual meter-fix crossings, by look-ahead;
  - TFMS estimated times against actual departure and arrival;
  - ERAM route conversion against the route actually flown.
- The missing standards: ESASSP Vol 1 tables, EUROCONTROL STCA specification, JO 7110.65 and
  JO 7210.3 sections, ACAS X validation report, ERAM conflict probe accuracy studies.

Exit: BENCHMARKS §3 has a measured legacy value for every row where SWIM carries the legacy
output, and a sourced floor for every row a standard covers. This is the first public result
(rung 1): a reproducible measurement of ERAM and TBFM against independent truth.

### M3 — Surveillance tracking (L)

Builds:
- Multi-sensor tracker: interacting multiple models, ADS-B weighted by its own reported
  integrity (NACp, NIC, SIL), radar and multilateration as independent checks.
- ASTERIX categories 021 (ADS-B), 048 (radar) and 062 (system tracks), in and out.
- Simulated radar from ADS-B truth using published error models, so radar-only and mixed
  traffic can be tested without real radar.
- ADS-B integrity checking: kinematic plausibility, cross-sensor agreement, and receiver timing
  where we have our own receivers.
- Flight-plan-to-track correlation with every uncorrelated track shown.

Exit:
- Every ESASSP indicator meets the Recommended value on the recorded day.
- Position RMS, velocity RMS, probability of update and gaps all better than ERAM's measured
  values (M2) on the same traffic.
- Zero falsely confirmed tracks near true tracks over the day; track swaps and initiation delay
  reported.
- Injected spoofs: detection at least 95% with false positives at most 1e-3, confusion matrix
  published.
- Capacity: all 20 centres' peak traffic (ERAM: 1,900 tracks and 64 radars per centre) tracked
  in real time on one ordinary machine, with measured latency.
- Shortcomings LS-101 to LS-104 each have a passing test.

### M4 — Flight data processing and trajectory (L)

The largest part of ERAM, missing from the earlier plan.

Builds:
- Flight plan model and lifecycle: filing, amendment, cancellation, departure, arrival, with
  FIXM input and ICAO flight plan format; duplicate-plan detection.
- Route conversion against NASR and CIFP, including preferred routes and procedures, with
  duplicate waypoint names resolved and tested (LS-007).
- One 4D trajectory per flight, shared by every function: OpenAP performance, HRRR winds,
  published procedures and restrictions, with uncertainty carried along.
- Track and sector ownership, handoffs and point-outs as a state machine with properties proved.
- Conformance monitoring: route, cleared level and speed adherence, with route changes delivered
  to every affected sector (LS-201).
- Conflict probe (medium term, up to 20 minutes) on the shared trajectory.
- Arrival time estimates at meter fixes.

Exit:
- Trajectory error at 5, 10 and 20 minutes (along-track, cross-track, vertical) better than the
  legacy estimates measured in M2.
- Meter-fix time error smaller in spread than TBFM's measured on the same day; top of climb and
  descent errors at or better than the best published (5.2 s / 6.6 s mean absolute error).
- Route conversion agrees with ERAM's or each disagreement is explained.
- Conflict probe missed and false alert rates both reported against the legacy floor.
- Every input message type has a stated work bound; the U-2 plan (LS-001) and the duplicate
  waypoint plan (LS-007) pass as replays.

### M5 — Safety nets (L)

Builds: conflict alert (short term), minimum safe altitude warning, approach path monitoring and
area proximity warning (special use and temporary restricted airspace from NASR and NOTAMs),
on 3DEP terrain, the FAA obstacle file and MVA/MIA data. Terrain maxima used for warnings must be
conservative: every terrain cell that touches a protection volume counts (not only samples whose
centres fall inside it), and the possible height of terrain between samples is bounded, not
interpolated away. The plain `oa-tiff` radius and polygon queries count sample centres only and are
not the warning-grade queries; the `*_conservative` variants are (footprint of one full step, WGS 84
radius, typed incomplete result).

Exit:
- Monte Carlo evaluation over the MIT LL encounter models: risk ratio, missed alerts and
  nuisance alerts against a baseline run of legacy-style logic; nuisance alerts reduced without
  any increase in missed alerts.
- Warning time at least the EUROCONTROL specification value, and at least ERAM/STARS
  parameters where published.
- Recorded traffic: every recorded loss of separation and low-altitude event is alerted;
  nuisance alerts counted and each one classified.
- The geometric core of each safety net proved correct, and its implementation shown free of
  overflow, in the DAIDALUS manner.

After M5 the full public benchmark (rung 1) is published: tracking, prediction and alerting,
ours against legacy, on open data.

### M6 — Resilience and overload (M)

Builds: active-active replicas running the same deterministic state machine; staged updates
with automatic rollback; load shedding in a written order; feed degradation detection; legacy
interface emulators that misbehave on purpose (INTERFACES IF-P2).

Exit:
- Every incident in docs/LEGACY_SHORTCOMINGS.md replays as a scenario and passes: we neither
  cause nor amplify the failure, and surveillance and alerts stay consistent. That includes the
  2026-11-13 failure mode (a new high-rate source into a legacy-style network).
- Failover with no lost state and no gap in surveillance updates, under fault injection.
- Availability argued against 0.99999 for safety-critical services from measured failover and
  fault-injection results, not asserted.

### M7 — Live shadow (M)

Live operation in ZJX and F11 on SWIM and our own ADS-B receiver with the comparator running,
producing a daily difference report against ERAM by function. This is rung 3 in miniature, and the point
at which outreach for the gaps in section 6 starts, with results in hand.

Exit: 30 consecutive days live without an unexplained difference or an unhandled feed event;
daily reports published.

### M8 — Controller display (M)

En route and terminal positions on the KMS backend: data blocks placed without overlap in
holding and on final (LS-401), alerts on the data block (LS-302), settings that persist per
position (LS-402), degraded modes with the same display and commands (LS-005).

Exit: evaluation by people who have worked ERAM or STARS positions, on recorded traffic, with
their findings recorded and acted on. This makes the platform usable for training and research
(rung 2).

### M9 — Whole NAS on one laptop (M)

Adaptation for all 20 centres and the TRACONs generated from NASR and CIFP each AIRAC cycle,
with an automatic diff of what changed; whole-NAS replay and live shadow at real time on one
laptop.

The laptop is a showcase target and a design constraint (Connor, 2026-11-19): every function
OpenATCA has built by then, for the whole NAS at the busiest recorded hour, runs in real time on
one ordinary laptop. The reference machine is the project's development laptop: a Star Labs
StarBook Horizon, Intel Core i3-N305 (8 low-power cores, 3.8 GHz maximum), 30 GB of memory. If it
runs there, it runs on any machine an agency would buy.

What runs: surveillance tracking and fusion for every aircraft, from the national ADS-B feed and
from radar input at the real number and placement of FAA radars (simulated where recordings are
not available, and said so); flight data processing for every flight; the safety nets; feed
health; and the comparator against ERAM. Display scene generation is measured for a stated number
of positions; drawing for every position in the country is not part of the claim, because each
position has its own screen.

What the claim is, and is not: it compares compute and hardware. It is not a claim that one
laptop should run the NAS. Operations need redundant, separated machines and an assurance case.
ERAM's documented problems are cost, ageing hardware and parts that can no longer be repaired
(DOT OIG AV-2020-040), not an inability to keep up with traffic, and results are worded that way.

Interim marker, after M3: national surveillance tracking alone (every aircraft in the
continental US at the busiest hour of a recorded day, from ADS-B) in real time on the reference
laptop, with compute, memory and the share of one core measured.

Exit: every centre and TRACON adapts with no hand edits, or each hand edit is recorded with its
reason; whole-NAS peak load runs in real time on the reference laptop using at most half of its
cores, so the measured headroom is at least two to one; position accuracy and every alert
result are the same as on the per-region runs (the laptop costs nothing in accuracy); compute,
memory, power draw where measurable, and program size are published with the recording, the
scoring-code version and the replay state hash, so anyone can rerun it.

### Dependencies

M1 → M2 → M3 → M4 → M5 → M6 → M7, with M8 alongside from M3 onward (the display already exists in
early form) and M9 after M7. M3 and M4 can overlap once M2's scoring code exists.

## 8. Prior work to learn from

- BlueSky (TU Delft): open air traffic simulator; traffic generation and conflict detection
  algorithms.
- OpenSky Network: ADS-B data and published research on its quality.
- readsb / dump1090: ADS-B decoding.
- Eurocontrol specifications: ASTERIX, and the published SESAR material on service-based ATM.
- Commercial platforms named by the RFI's "Operationally Proven Platform" path (for example the
  systems used by other air navigation service providers): their public documentation sets the
  bar for function coverage.
