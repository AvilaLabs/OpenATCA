# Runway safety: design

Status: draft, 2026-11-20. Runway safety came into scope on 2026-11-20 (SCOPE §1, "In scope"). It
is built after the M5 airborne safety nets and follows the five rules of `docs/M5_DESIGN.md`.
Nothing here is built yet.

## Goal

Track aircraft on the airport surface from ADS-B and alert when a runway is about to be used by two
of them at once, or when an aircraft is about to land somewhere that is not a runway. The aim is the
airports with no surface surveillance at all, where nothing alerts today.

Why those airports:

- **Austin, 4 February 2023.** A FedEx 767 cleared to land on 18L came within a go-around of a
  Southwest 737 departing the same runway, at night in low visibility. The NTSB's probable cause
  includes the FAA's failure to require surface detection equipment at Austin and direct alerting
  for flight crews (DCA23FA149).
- **San Francisco, 7 July 2017.** Air Canada 759 lined up on taxiway C, where four aircraft were
  waiting, and flew over it at about 60 ft before going around. SFO had surface surveillance (ASSC)
  and it did not alarm; the NTSB recommended alerting for landings on taxiways (AIR-18/01).
- **Coverage today.** ASDE-X is at 35 US airports and ASSC at 9. The FAA's Surface Awareness
  Initiative (SAI) targets 220 towers that have no surface surveillance; 100 had it by 2 September
  2026. SAI is an ADS-B traffic display: as deployed it gives no conflict alerting (NBAA; AvWeb,
  2 September 2026). So about 220 towers have no surface alerting, and 120 of them no surface
  display either.

An ADS-B runway monitor costs no new sensor at any of them. That is the case for building it.

## What it is not

- **Not a replacement for ASDE-X.** ASDE-X fuses surface radar and multilateration; we receive
  neither. An aircraft or vehicle without ADS-B Out is invisible to us except where ASDE-X already
  sees it, through its output on STDDS (SCOPE, "Out of scope").
- **Not clearance-aware, at first.** We do not know which runway a tower cleared an aircraft to
  use, so a wrong-runway alert is out of reach until a clearance source exists. Every alert here is
  built from where aircraft are and where they are going.
- **Not a cockpit system.** SURF-IA (RTCA DO-323) is an ADS-B In cockpit application. This is a
  ground system for the tower, though nothing in it prevents the same logic serving both.

## 1. Inputs

- **ADS-B surface positions.** On the ground an aircraft broadcasts its position at least once a
  second while moving and at least every 5 s while stationary (14 CFR 91.227(d)). The air-ground
  state comes from the message type, and on the ground the direction reported is heading, not
  track over ground.
- **The archive is thinner than the feed.** The adsb.lol archive keeps a point every 2 to 5 s while
  taxiing, one every 80 s for a stationary aircraft, and about one a second only for the minute
  after takeoff or touchdown (readsb's trace rules, `globe_index.c`). The archive can show that the
  design works. It cannot measure alert timing on the ground. Timing is measured on the live feed
  and on STDDS.
- **ASDE-X output on STDDS (SMES).** At ASDE-X airports, the surface tracks ASDE-X publishes are
  already parsed by `oa-stdds` (`smes.rs`), with the runway each target is associated with. They are
  our position reference there, and they are only partly independent of ADS-B because ASDE-X fuses
  it, so every result is split by independence class as the terminal radar census does.
- **Runway geometry.** From NASR (`oa-nasr`, `APT_RWY` and `APT_RWY_END`): threshold positions,
  displaced thresholds, length, width, true alignment, elevations and declared distances. A runway
  is the rectangle between its two thresholds at its published width. CIFP runway records
  (`oa-arinc424`) take precedence where NASR has no threshold coordinate (OA-ASP-067).
- **Taxiway geometry, where it exists.** The FAA's Airport Diagram open data (taxiways, aprons, hot
  spots; first released with the 7 August 2025 cycle, 56-day updates) covers a minority of airports
  and has no hold-short lines. Its terms of use are not yet confirmed, and nothing from it ships
  until they are. OpenStreetMap has taxiways and holding positions under ODbL; if used at all it is
  fetched on the user's machine and never shipped in the repository.

Every alert in §3 works from runway geometry alone. Taxiway geometry, where present, sharpens two of
them; it is never required.

## 2. Surface tracking

The airborne tracker is tuned for aircraft that move fast and turn gently. On the surface they stop,
start, turn on the spot and sit still for minutes. Surface tracking is a mode of the same tracker,
not a second tracker, so a track keeps its identity through takeoff and landing.

- **State per target**, following NASA TM-2009-215696: stationary, taxi, lined up (on a runway,
  aligned within 10° of it, below 15 kt), takeoff roll (on a runway, aligned, accelerating above
  15 kt), airborne, approach (airborne, aligned with a runway end, within 3 NM and 1,000 ft of it,
  descending), rollout (on a runway after touchdown, decelerating). The thresholds are NASA's
  published defaults and are development values here, not FAA values.
- **Motion model.** Low-speed motion with heading taken from the report when the ground speed is too
  low for a track over ground to mean anything. A stationary target is held, not extrapolated.
- **Timeouts.** A target not renewed within 4 s while moving on a runway is marked stale and its
  uncertainty grows; the A-SMGCS surveillance requirement is a 4 s position renewal timeout and at
  least one update a second. A stale target still counts as occupying the runway (rule 4): missing
  data never clears a runway.
- **Occupancy.** A runway is occupied by every target whose position, widened by its 95 % radius, is
  inside the runway rectangle or its approach and departure areas. A poor position makes a runway
  look occupied more often, never less.

## 3. Alerts

Each alert has two grades, caution and warning, set by the predicted time to the conflict, as in
SURF-IA and the European RMCA. Every grade looks and sounds different (M5 rule 3).

1. **Arrival to an occupied runway.** An aircraft on approach to a runway end whose runway is
   occupied by another target that will not have cleared it when the arrival reaches the threshold.
   This is Austin.
2. **Departure onto an occupied runway.** A target in takeoff roll with another target on the same
   runway ahead of it, or an arrival predicted onto that runway before the departing aircraft is
   airborne.
3. **Intersecting runways.** Two targets in takeoff roll, rollout or on approach on runways that
   cross, predicted to reach the intersection within the separation NASA's tables give (TM-2009-215696,
   table B2).
4. **Lined up on something that is not a runway.** An aircraft on approach whose path is aligned
   with no runway end, or with a taxiway where taxiway geometry exists, below a height and distance
   from the field. This is San Francisco 2017. The FAA's own taxiway arrival prediction at Seattle
   alarmed within 20 s or 3,000 ft of landing with no false alarms in its 2018 test (NTSB AIR-18/01);
   that is the bar to meet or beat.
5. **Entering an active runway.** A target crossing onto a runway, from a taxiway or the side,
   while an arrival or departure is predicted to use it. Without hold-short geometry this uses the
   runway edge at its published width plus a margin; with it, the holding position.

Alerts are inhibited only through inhibition objects (M5 rule 2), for example a runway closed by
NOTAM, which `oa-notam` already decodes.

## 4. The five rules, on the surface

1. **State.** The net publishes running, degraded or unavailable per airport every cycle. An
   airport with no runway geometry, or with its ADS-B coverage lost, shows unavailable, never "no
   alerts".
2. **Inhibitions** are objects with an area, an end and a reason, as for the airborne nets.
3. **Grades** as in §3, with the nuisance rate published per airport and per grade.
4. **Missing data.** A target that stops reporting while on a runway keeps the runway occupied until
   it is seen leaving it or its uncertainty covers the whole runway, and the loss is an alert in
   itself. Vehicles reporting ADS-B (emitter categories C1 and C2) take part like aircraft.
5. **Bounded work.** Surface pairs are few per airport, but the work is still counted against a
   named budget, and an airport whose cycle cannot be completed is degraded with the count.

## 5. Evaluation

Protocol before evidence, as everywhere in OpenATCA: the airports, days, metrics and pass marks are
fixed in a hashed protocol before any result is seen.

- **A surface encounter record.** `oa-encounter` already records surface encounters (1,559 of the
  2,501 in the development hour). It is extended to record every runway occupancy conflict
  whether or not an alert would fire, which gives the evaluation set, as §5 of M5 does in the air.
- **Tracking accuracy against ASDE-X output.** At ASDE-X airports on the 24 November 2026
  development day and later days, our surface positions are compared with SMES, split by how
  independent each SMES target is of ADS-B.
- **Missed and false alerts.** Over whole days at many airports: every recorded runway incursion in
  the data must alert, and every alert is classified. The false-alert rate is reported per hour per
  airport, because that is what a tower lives with, and because false alerts are what kept runway
  status lights out of service at first (MIT Lincoln Laboratory).
- **Warning time,** as a distribution, against the Seattle taxiway-arrival figure and the pilot
  reaction times used in NASA's SURF-IA evaluation (5 s on approach, 3 s on rollout, 2 s taxiing and
  departing).
- **Replays of real events.** The adsb.lol archive starts on 16 February 2023 (its oldest release,
  a test instance), so JFK (13 January 2023) and Austin (4 February 2023) are not in it, and San
  Francisco 2017 is long before it. All three are rebuilt from the NTSB reports' timelines as
  synthetic cases and labelled as such. Events from 16 February 2023 on are replayed from the
  archive where the NTSB has published a timeline to check against.

## 6. Build order

1. Runway geometry per airport from NASR and CIFP, with the occupancy test and its proofs.
2. The surface mode of the tracker and the target state machine, measured against SMES at ASDE-X
   airports.
3. The surface encounter record.
4. Alerts 1 and 2 (arrival and departure to an occupied runway).
5. Alerts 3 to 5.
6. Taxiway geometry where its terms allow it.

## 7. Stated limits

- Targets without ADS-B Out are seen only where ASDE-X output exists.
- Alert timing cannot be measured on the archive; it needs the live feed or STDDS.
- No clearance data, so no wrong-runway alert.
- Taxiway and hold-short geometry is partial, and absent from many airports.
- The ASDE-X and RMCA stage timings are not public in anything read so far. Our grades are our own
  design, stated as such, never presented as the FAA's.
