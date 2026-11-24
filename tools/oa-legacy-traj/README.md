# oa-legacy-traj

## What it measures

How accurate the FAA's own time predictions are, measured against what the aircraft then did. It
reads the predictions from recorded SWIM messages:

- ERAM (SFDPS): the predicted time at each point of a flight's converted route;
- TBFM: the estimated and scheduled times at the meter fix (`eta_mfx`, `sta_sfx`) and at the runway
  (`eta_rwy`, `sta_rwy`). The scheduled times are metering targets, not forecasts, and are reported
  apart;
- TFMS: the estimated arrival time.

The truth is the aircraft's ADS-B path. The time it reached a point is its closest approach to the point,
and the time it landed is its first ground report after a descent that holds up. The result is the
baseline OpenATCA's own trajectory predictions have to beat (milestone M4).

## Usage

```
oa-legacy-traj --sfdps <dir> --tbfm <dir> --tfms <dir> --adsb <clip.oaf> [--adsb <clip.oaf> ...] --out <dir>
               [--nasr <zip>] [--places <csv>] [--cut-nm <nm>] [--runway-nm <nm>] [--landing-gap-s <s>] [--step-s <s>]
```

Running it with no arguments prints every default and every fixed setting. Without `--nasr` or
`--places`, no point or airport has a place, so nothing can be scored. Every input goes through the
sealed-data guard.

### ORD arrival forecast experiment

The core also exposes `forecast_latest_source_route` for a receiving application that needs
source-ordered amendments. It takes `OperationalRouteCandidate` values carrying full GUFI,
explicit reporting centre, plan identifier, source timestamp, receive availability and message
identity. Within the 256-candidate limit, it first excludes future receipts, then selects the
newest source event for that flight and centre. A delayed older event cannot roll the route
back. Contradictory same-source-time payloads refuse the issue; equivalent duplicates retain
deterministic provenance. Invalid selected geometry never falls back to an older route.
Payload comparison uses canonical floating-point bits: signed-zero differences under the same
source stamp are explicit contradictions; duplicate invalid NaNs still fail geometry validation.
Successful values carry `selection` counts; `OperationalRouteRefusal` carries the same input
denominator and disposition counts alongside `reason`. Callers can account for every candidate
even when the forecast is withheld or the global budget prevents any candidate scan.

This API does not reconstruct historical flight state, convert filed routes or change the
experiment below. Its caller must supply the flight/track snapshot actually available at that
cutoff. It is an integration building block, not a shared operational trajectory service.

```
oa-legacy-traj --sfdps <dir> --tbfm <dir> --adsb <clip.oaf> --nasr <zip> --out <dir> --arrival-meter-fix <NAME|auto>
```

This experiment issues one forecast per eligible ORD flight using the latest route revision
received by that issue cutoff and the latest eligible ADS-B observation. Route revisions are
selected only within one full SFDPS flight GUFI; their plan ID, receive time and GUFI are carried
beside each forecast and written to `arrival-report.txt`. Future revisions are excluded. Equal-time
conflicting revisions, missing identity, revision-capacity overflow, and conflicting same-stamp
ERAM ETA payloads are reported as refusals; exact duplicate revisions are harmless. Malformed or
over-256-fix rows are counted and skipped as issue candidates, but stay in the revision history so
they refuse a forecast if available by its cutoff. The cohort still freezes the first eligible issue
snapshot per flight. At that cutoff, every captured revision for the GUFI participates, including
one whose arrival is no longer ORD or whose route omits the selected meter fix; a selected revision
without usable geometry is refused instead of falling back. Each flight history is sorted once;
cutoff selection binary-searches its eligible prefix and refuses more than 256 revisions before
resolving geometry. Amendments received after the frozen issue cutoff are excluded, so this
experiment does not model later in-flight amendments. It writes position and
altitude coverage and error at 5, 10 and 20 minutes, plus selected-fix ETA errors for OpenATCA,
ERAM and TBFM when comparable estimates exist. `auto` selects a resolvable fix by the number of
distinct ORD flight plans with a TBFM estimate, before scoring. OpenATCA uses the received route's
geometry and observed motion. Its ETA does not depend on the legacy time predictions. The ADS-B
archive records observation time and has no comparable receive clock. This baseline has no winds or
aircraft performance model. The experiment freezes one issue cutoff per eligible flight before
rereading SFDPS for those flights' typed assigned-altitude evidence. The guarded second pass verifies
the same input manifest and keeps bounded state for each selected flight. It reports scan and decode
refusals, which limit the evidence to admitted records. A valid `simple` form supplies a scalar.
A later VFR, block or at-fix form supersedes the scalar and is reported as unsupported. Malformed
assignments, same-stamp conflicts and unknown receipt times that could change the selected value
leave the scalar unavailable. When the source time is known, it bounds whether an uncertain record
could have been available by cutoff. An unusable source time remains uncertain at every cutoff.
The issue snapshot carries the assignment's provenance and receipt time. Receipts after cutoff are
excluded.
The legacy route table omits published procedure altitude windows. Synthetic checks exercise waypoint crossing
feasibility; this is not a fitted vertical solver. Coverage at each horizon is divided by every issued
forecast; missing predicted positions and missing truth interpolation are counted separately.
After flow selection, issue eligibility uses cutoff-time route and track observations.
Scoring queries future truth only after freezing the issue cohort.

The pure core also exposes an opt-in wind route forecast that requires explicit true airspeed and
one typed, cutoff-safe wind vector for every leg. It cancels crosswind in the wind triangle and
refuses impossible crosswind or nonpositive along-route speed. All horizon positions, waypoint
crossing checks, and meter ETA use the same leg speeds. The shell does not connect a weather source
or infer airspeed from ADS-B ground speed, so the scored baseline remains unchanged.

## Method

1. **Join.** Each flight is joined to one ADS-B aircraft by its address, or by its callsign when it has
   no address and the callsign names one aircraft. Every flight gets one join outcome, and none is
   dropped.
2. **Bind.** Each message is bound to one leg of the flight's own: the leg in progress at the issue
   time if the aircraft is airborne on it and it is this flight's, otherwise the next one. Taxi-in legs
   and other flights' legs are refused and counted.
3. **The crossing.** The path is searched once, within 5 NM of the point. Each time the aircraft comes
   back towards the point (a hold, a loop) is a separate passage. Of the passages after the issue time
   the first within 0.5 NM is taken, else the closest. The estimate never chooses the passage.
4. **Score.** The error is the estimate minus the actual time, in seconds: positive is late. It is
   binned by the stated lead, the estimate minus the issue time: 0–5, 5–10, 10–20, 20–40, 40–60 and
   60–120 minutes. The headline counts a passage within 2 NM that is not at a path end or a hole, for a
   flight joined by address and airborne at issue. The same passage is also scored at 0.5, 1 and 5 NM,
   and a row scored at one cut is scored at every larger one.
5. **Statistics.** Each bin gives n, the median, 95th and 99th percentile of the absolute error
   (nearest rank; the two upper percentiles only from 100 rows), the signed median and mean as bias, and
   a spread. Exact duplicate messages are dropped first, and counted.

## Output

`report.txt` and CSV files in the output directory, written under temporary names and renamed only
when all are written. Per system the report gives:
- the join outcomes and the binding refusals;
- every exclusion reason with its count, and per bin the share of offered rows excluded because the
  aircraft never came within the cut, the passage was at an edge, or there was no ground report;
- the headline;
- the same rows one message per flight and bin;
- the pre-departure and callsign classes;
- the cut sensitivity tables;
- the passages 2 to 5 NM abeam the point;
- the headline without the rows whose place was resolved against the flown path.

## Limits

- **Only predictions the aircraft flew are scored.** A point the aircraft was vectored away from
  never gets a crossing, so it is excluded, and those are often the worst predictions. The headline is
  the error given that the aircraft flew the point, and the exclusion shares beside it show how much is
  left out.
- **Rows are weighted by message.** A flight that was re-sent often weighs more. The one-per-flight
  table is the check.
- **A point with several places** takes the one nearest the filed route. Where the route gives
  nothing, it takes the one nearest the flown path; the sensitivity line removes those rows.
- **The truth is the thinned adsb.lol archive,** with reports about 8 s apart. A crossing time is
  interpolated between them, and the truth answers only within 10 s of a real report.
- **The data covers one day of Chicago Center** (ZAU), 2026-11-14, a development day.
- **ADS-B legs carry no flight-plan identity.** Binding uses the airports and times, and a shuttle
  aircraft flying the same city pair twice can still be bound to the wrong leg in a case the airport
  rule does not separate.
