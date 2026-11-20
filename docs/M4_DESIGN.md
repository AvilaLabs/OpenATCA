# M4 — Flight data processing and trajectory: design

Status: draft for Connor's approval, 2026-11-19. The decoders it builds on exist; none of the M4
functions is built yet.

## Goal

Flight data processing that keeps every flight's plan correct through any sequence of amendments,
and one 4D trajectory per flight that every function uses: conformance monitoring, the conflict
probe, arrival times at meter fixes, and the intent the safety nets need. Prediction error better
than the legacy estimates measured on the same traffic. SCOPE §7 M4; shortcomings LS-001, LS-007,
LS-201, LS-202, LS-204, LS-601 to LS-603, LS-607 and LS-608.

ERAM's flight data processing is its largest part, and most of the documented failures of en
route automation live here rather than in tracking:

- **LA Center, 2014 (LS-001).** A U-2 plan filed with no altitude made conflict processing consider
  every altitude. The system ran out of memory and kept restarting; flights were stopped for up to
  about two and a half hours.
- **NATS, 2023 (LS-007).** A plan containing two waypoints with the same name failed processing,
  and the backup failed on the same plan.
- **Route changes that did not arrive (LS-201, LS-602).** Controllers report aircraft flying routes
  other than the one displayed, and amendments that never reached the next sector or facility.
- **Two tools, two trajectories (SCOPE §2a, LS-608).** ERAM's conflict probe and its trajectory
  tools were built separately with incompatible requirements (NTRS 20130010914), so the probe
  and the controller's other tools can disagree about the same aircraft.

So the design starts from four rules, before any prediction:

1. **Every input has a stated cost, and no input can exceed it.** A plan with a missing altitude, an
   unknown fix or a route of ten thousand points is refused and reported, never processed partly
   and never allowed to grow the work of anything else (INTERFACES IF-R1, IF-R2). The U-2 plan and
   the duplicate-waypoint plan are replay tests from the first build.
2. **Any sequence of amendments leaves a valid plan.** The plan is a state machine whose
   transitions are checked by property tests and, where the state space allows, proved (LS-202).
   A plan is never silently removed or duplicated (LS-204, LS-602).
3. **There is one trajectory per flight.** Every function reads the same predicted trajectory with
   the same uncertainty. No function keeps a private prediction.
4. **A change reaches everyone it affects.** A route or altitude change is delivered to every
   sector and function whose airspace the old or new trajectory touches, and the delivery is
   acknowledged and visible (LS-201, LS-601).

## 1. Inputs

| Input | What it gives | Decoded today | Limits |
|---|---|---|---|
| SFDPS (FIXM 3.0, `oa-fixm`) | ERAM's flight data: assigned altitude, route, aircraft, departure and arrival, controlling sector, handoffs, status | 27 fields per message (SFDPS_SURVEY) | ERAM output, about 60 s late (UK-004); meanings of the `source` codes unconfirmed |
| TFMS (`oa-tfms`) | Filed plans, amendments, cancellations, departure and arrival times, ETAs, traffic management programmes | Flight and flow rows (TFMS_SURVEY) | Programmes only as free text; 41% of flights have no `gufi` |
| TBFM (`oa-tbfm`) | Arrival estimates and scheduled times at meter, delay and scheduling fixes and at the runway | `flt`, `mrp`, `eta`, `sta`, `sch` sections | 12 of ZAU's meter points are adaptation names that are not public |
| NASR and CIFP (`oa-nasr`, `oa-arinc424`, `oa-airspace`) | Fixes, navaids, airways with MEAs, SIDs, STARs, approaches, special use airspace | Yes (M0) | Radar-vector SIDs have no coded legs (UK-003); preferred routes not parsed |
| RAP and HRRR winds (`oa-wind`, `oa-grib2`) | Wind and temperature at a point and time | Yes | Refuses rather than extrapolates |
| ADS-B climbs and descents (our own performance model) | Climb and descent profiles by aircraft type, altitude and temperature, fitted from recorded US traffic, with the FAA registry for the type | Not built | Empirical: no mass or thrust; rare types fall back to their class |
| NOTAMs (`oa-notam`) | Temporary restrictions and airspace status | Yes | — |

The flight data we hold is a national SFDPS, TFMS, TBFM and FNS recording from 16:13Z on
14 November to about 16Z on 15 November 2026, alongside the ADS-B days. Two more windows are
scheduled, recording all five SCDS feeds (SFDPS, TFMS, TBFM, FNS and STDDS), and both end at a
fixed time because the laptop's disk is the only storage:

- **24 November 2026, 00:00Z to 24:00Z: the sealed hold-out day.** It is sealed from the start,
  with that day's ADS-B, and is read only to score a frozen candidate. The development data is the
  14 to 15 November recording.
- **30 November 05:00Z to 1 December 06:00Z: a flight day.** It covers a domestic flight whose
  phone GPS log gives one aircraft's position independently of every feed.

Together they are about 30 GB. Each stops all feeds if free disk space falls below 50 GB.

The first measurement is already in the survey. ERAM's assigned altitude is in 92% of SFDPS
messages (13,719 of 14,851 in the sample), and handoff fields are in about a fifth. The interim
altitude, block altitudes and altitude-at-fix forms are present but rarer. Most of what the safety
nets need from flight data, above all the cleared altitude, is therefore available from SFDPS
before anything else in M4 is built.

### SFDPS is ERAM's output, not ours

In shadow operation our flight data is ERAM's, received a minute late. That has three
consequences, and the design states them rather than hiding them:

- **It is not independent.** A plan ERAM processed wrongly arrives already wrong, and SFDPS stops
  when the centre's ERAM stops (UK-007). Comparing our route conversion with ERAM's is a check
  of agreement, not of correctness, and every disagreement is examined by hand.
- **It is late.** No function may treat SFDPS data as current to better than the measured lag
  (UK-004). In replay the lag is applied, so a replayed safety net sees the assigned altitude when
  ERAM would have published it, not earlier.
- **In service the source is different.** A deployed system receives controller entries and
  facility coordination directly (the NAS interface documents, UK-007), not SFDPS. The plan model
  is therefore built on the operations a controller performs (create, amend, assign altitude,
  hand off, cancel), and SFDPS is one adapter that turns ERAM's published state into those
  operations.

## 2. The flight object and its lifecycle

One record per flight, keyed by the GUFI where it exists, with the TFMS key
(callsign, departure, arrival) as the documented fallback. The record holds the current plan, the
assigned, interim and requested altitudes as their own fields (a block altitude is a range, VFR
and VFR-on-top are states, never a number), the route as filed and as converted, the owning
sector, any pending handoff, and the time each field last changed and from which message.

The lifecycle is a state machine: proposed, active, departed, arrived or completed, cancelled,
dropped. Every message becomes one or more operations on it. An operation that does not fit the
state (an amendment to a cancelled plan, a second active plan for one flight) is refused, counted
and listed, never applied quietly. Two plans that claim one flight, or one plan claimed by two
tracks, are listed as conflicts (LS-204, LS-603).

Properties to check over every sequence of operations up to a stated length: a plan is valid after
every step, there is at most one active plan per flight, a removal is always explicit and logged,
and replaying the same operations gives the same plan bit for bit (RULESET B6).

## 3. Route conversion

A route string (fixes, airways, SIDs, STARs, latitude-longitude points, speed and altitude changes)
becomes an ordered list of points against the airspace model. There is no route conversion code
yet; the model has every ingredient.

- **Duplicate names (LS-007).** A name that matches more than one fix is resolved by the route
  around it (the previous and next points), and only when one candidate is clearly consistent. An
  ambiguity that remains is refused with both candidates named, never guessed. A plan that fails
  conversion keeps its last good route and is flagged; it never takes the system down with it.
- **Procedures.** SIDs and STARs are expanded from CIFP legs with their transitions. A radar-vector
  SID has no coded path (UK-003), so the route after it starts at its first fix and the gap is
  marked as vectors.
- **Preferred routes** need a NASR parser that does not exist yet.
- **The legacy comparison is in the data.** SFDPS carries ERAM's converted route (`expandedRoute`,
  with an estimated time at each point) on about 2% of messages. Every flight that has one gives a
  point-by-point comparison of our conversion with ERAM's, and the same points give ERAM's own
  predicted times to compare with when the aircraft actually got there.

## 4. One trajectory per flight

For each active flight: a 4D path from the current track state along the converted route to the
destination, with climb and descent from our own performance model for the aircraft type, winds and
temperature from RAP or HRRR, and the constraints of the procedures and the cleared altitudes.
Each predicted point carries an uncertainty along track, across track and vertically, growing with
look-ahead, so a consumer can ask not only where the aircraft will be but how sure that is.

The trajectory is recomputed when its inputs change (a new track state beyond its bounds, an
amendment, a new wind hour), within a stated budget per flight, and it is deterministic like the
tracker. Every consumer reads it through one interface:

- the conflict probe (up to 20 minutes);
- conformance monitoring;
- arrival times at meter fixes;
- the safety nets' intent (the cleared altitude a climbing aircraft will stop at, the turn at the
  next route point), which can sharpen a prediction and never suppress an alert (M5 design).

## 5. Ownership, handoffs and point-outs

Ownership is a state machine per flight: one owning sector at any moment, a handoff as an explicit
offered, accepted or refused transition with a timeout, and a point-out as a separate object that
never changes ownership. Its properties (exactly one owner, no handoff accepted twice, no flight
left without an owner when a handoff times out) are proved with the same tools as the tracker's
proofs. The handoff target comes from the trajectory, not from the current altitude alone
(LS-601).

ERAM's sector boundaries are not public. They are derived from the ownership changes SFDPS
reports (the controlling unit and the handoff events), and the derived boundaries are kept as data
with their evidence, so the gap stays visible.

## 6. Conformance monitoring

Route, cleared level and speed adherence, measured against the trajectory: an aircraft off its
route, through its cleared altitude or outside its assigned speed is flagged with how far and for
how long, in graded steps like the safety nets. A route change raises a check that every
affected sector has received it (LS-201). Thresholds come from measurement on recorded traffic
before they are fixed, and the nuisance rate is reported per hour per position.

## 7. Conflict probe

Aircraft to aircraft and aircraft to airspace up to 20 minutes ahead, on the shared trajectory and
its uncertainty, with airspace status from NOTAMs and schedules (LS-607, LS-608). The probe and the
safety nets read the same trajectory and the same airspace state, so they cannot disagree about
where an aircraft is going. The legacy floor is the factory acceptance figures and the Chicago
Center Tech Center study in BENCHMARKS §3.2.

## 8. Arrival times at meter fixes

The trajectory's crossing time at each meter fix, compared with TBFM's estimate for the same flight
and with when the aircraft actually crossed. A meter point that is only an adaptation name is
listed as unresolved, not approximated.

## 9. Measurement

None of the legacy trajectory baselines is measured yet (BENCHMARKS §3.2). The recordings hold
them, and measuring them comes before building the predictor, so the target is known before
there is anything to tune:

- ERAM's predicted times along its converted route, against when the aircraft actually reached
  each point;
- TBFM's meter-fix and runway estimates, by look-ahead, against the actual crossing;
- TFMS's ETAs, by look-ahead, against the actual arrival.

The actual path comes from ADS-B truth. Its independence problem (UK-008) matters far less here: a
trajectory error is measured in hundreds of metres and tens of seconds, and the bias in
question is tens of metres at most (RT-288 simulation).

## Build order

1. **SFDPS flight state.** Decode the altitude forms that are left out today (interim, block,
   altitude at fix, requested), the handoff event and accepting unit, and the ADS-B address SFDPS
   carries. Build the per-flight record from SFDPS operations, with the measured lag applied. Then
   rerun the close-encounter record on an hour of 15 November with the assigned altitude, which
   divides the climbing and descending encounters near 1,000 ft (145 on the development hour)
   into level-offs and climbs that do not stop.
2. **Legacy measurement** (section 9), on the recorded day.
3. **The plan model and its properties** (section 2), with the U-2 and duplicate-waypoint replays.
4. **Route conversion**, compared with ERAM's on every flight that carries a converted route.
5. **The trajectory predictor**, scored by look-ahead against the legacy figures from step 2.
6. **Conformance monitoring.**
7. **Ownership and handoffs**, with the derived sector boundaries.
8. **The conflict probe and meter-fix times.**

## Aircraft performance

Decided by Connor on 2026-11-19: the performance model is our own, fitted to ADS-B climbs and
descents of US traffic. OpenAP's data files are GPL-3.0, and a deployed system would have to ship
them beside an Apache-2.0 one; its climb and descent data were themselves fitted from ADS-B,
mostly European. OpenAP is kept as an outside reference that the benchmarks score against, read
from `openatca-data` and never shipped. The model is empirical (ADS-B carries no mass or thrust),
and types with too few recorded flights fall back to their class; both limits are measured and
stated with every result.

## Questions decided by Connor

1. **More flight data.** Decided by Connor 2026-11-20: two bounded windows, a sealed hold-out
   day on 24 November and the 30 November flight day (section 1), rather than a continuous
   capture.
