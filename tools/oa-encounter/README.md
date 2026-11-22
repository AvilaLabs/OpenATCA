# oa-encounter

Records every pair of aircraft that comes close, whether or not any alert would have sounded. It
runs the tracker on an ADS-B clip and feeds its one-second output to the `oa-encounter` core crate.
This is step 2 of the M5 build order (`docs/M5_DESIGN.md` §5). It needs no alerting logic, so it
exists before conflict alert does, and it gives conflict alert its evaluation set.

```
oa-encounter --adsb clip.oaf --out dir [--nasr nasr.zip] [--scorer-half] [--from <unix s> --to <unix s>]
             [--sfdps <recording dir> [--lag-floor-s <s>]] [--pick-newest-plan] [--hours]
```

- `--adsb`: an `.oaf` clip from `oa-adsb-clip`. By default every report in the clip is used.
- `--scorer-half`: use the scorer's half A instead, as `oa-track-run` does.
- `--nasr`: the FAA NASR subscription zip. Its public airports (`APT_BASE.csv`, site type airport,
  facility use public) feed the surface rule below. Without it, the surface rule uses the ground
  state alone.
- `--from`, `--to`: only the reports from `from` up to (excluding) `to`, Unix seconds.
- `--sfdps`: join each aircraft to the SFDPS flight record (below).
- `--hours`: print the clip's reports by UTC hour and stop.
- `--out`: a directory for `encounters.csv` and `summary.txt`; the summary is also printed.

Exit status: 0 on success, 2 on a refused argument or path, checked before anything is opened or
created.

## What is recorded

A pair is inside the record volume while its horizontal separation is under 5 NM and its
vertical separation is under 1,000 ft, or while either altitude is unknown. A pair with both
aircraft on the ground is not recorded and is counted. An encounter starts at the first second a
pair is inside and ends 10 s after the last, so a dropped report does not split one pass in two.

Each encounter has a class, the tightest any second with known altitudes met:

| Class | Horizontal | Vertical |
|---|---|---|
| C1 | under 1,500 ft | under 200 ft |
| C2 | under 1 NM | under 400 ft |
| C3 | under 3 NM | under 1,000 ft |
| C4 | under 5 NM | under 1,000 ft |

C1 and C2 are the bands the NTSB quoted from the FAA's ASIAS data for DCA (AIR-26-02). An
encounter with no second of known altitude is `ALT_UNKNOWN`.

The closest approach is reported twice: the second with the smallest horizontal separation, and
the minimum found by moving the pair's relative position in a straight line between two evaluated
seconds.

## Context

Each encounter has one context at its closest approach, tested in this order:

1. `SURFACE`: either aircraft is on the ground, or it is within 5 NM of a public airport and less
   than 200 ft above the field.
2. `DEGRADED_TRACK`: either track is coasting or uncertain.
3. `AIRBORNE_ALT_UNKNOWN`: either altitude is unknown.
4. `AIRBORNE`: both airborne with known altitude and healthy tracks.

Within `AIRBORNE`, a pair whose vertical separation is at least 975 ft (1,000 ft less one 25 ft
altitude step) with both vertical rates known and under 300 fpm is counted on its own line as
`AT_VERTICAL_MINIMUM`. Those are aircraft on levels 1,000 ft apart that fall inside the volume
only because altitude is reported in 25 ft steps. The summary's main tables cover the rest of
`AIRBORNE`: aircraft converging vertically or below the minimum.

The ground state comes from the clip's own reports, not from the tracker. A report whose altitude
is "ground" says on the ground, a barometric altitude says airborne, and the latest such report no
older than 30 s decides. Otherwise the state is unknown, which is never treated as on the ground.

## Nothing is skipped silently

The record runs under fixed capacities and work budgets (RULESET C2 and C7). Every shortfall is
counted and printed: tracks the grid scan did not reach, pairs not evaluated, candidates dropped
from a full table, rows over capacity, rows with duplicate addresses (all rows sharing an address
in one second are left out, since which is right is unknown), rows with no valid position, and
rows above the grid's latitude limit (85.8°). Any of them makes the summary's record state
`DEGRADED`. A shortfall never ends or splits an encounter: the encounter is kept and its
unevaluated seconds are counted in the `ticks_unevaluated` column.

## Offline safety-net qualification seam

`crates/oa-ca/tests/qualification.rs` and the synthetic terrain case in
`crates/oa-msaw/tests/msaw.rs` are companion offline qualification slices. They declare analytic
encounter or terrain-violation truth and alert desire before evaluating output, and define miss,
nuisance and lead-time denominators. Both check input degradation while preserving active warning
state. They do not consume this tool's recorded-traffic output, estimate operational rates or
establish readiness; M5's recorded-traffic evaluation remains a separate build step.

`oa_encounter_run::qualification` is a bounded pure matcher for callers that already have
independently labelled cases and warning intervals. It matches by track or canonical pair, counts
one hit per label, reports exact lead time, and keeps degraded/unavailable labels in explicit
denominators with their reasons. It neither creates truth labels nor adapts CA/MSAW output; the
synthetic matcher fixtures test only the scorer contract.

## Output

`encounters.csv` has one row per encounter, sorted by start time, with the two addresses (8 hex
digits), times, the closest approach (sampled and interpolated), class, context, flags and
`ticks_unevaluated`. It holds addresses and is not for publication; public output is aggregated
under the privacy rules in `docs/M5_DESIGN.md` §5.

`summary.txt` holds counts only: every shortfall, the context and class tables, counts per hour
and per 0.25° cell, a histogram of vertical separation from 900 to 1,000 ft, and a table of the
encounters at or above 18,000 ft without addresses. For those, the smallest separation between
the two aircraft's own reports is also given, interpolating the other aircraft's position between
reports up to 30 s apart.

The output is a deterministic function of the clip: two runs give identical bytes.

## The flight record (`--sfdps`)

```
oa-encounter --adsb clip.oaf --out dir --sfdps <recording dir> [--lag-floor-s 58] [--pick-newest-plan]
```

With `--sfdps`, each aircraft of each encounter is joined to the FAA SFDPS flight record as it
stood at the closest approach, and the summary gains a table of what the clearances say about
encounters that are climbing or descending near 1,000 ft.

**As of the closest approach.** An SFDPS message counts only from the later of the time it was
received and its own stamp plus the lag floor (`--lag-floor-s`, 58 s by default). Each question is
answered before the first message that becomes available after it, so no answer uses anything
received or stamped later. The tool also takes a second look 30 minutes later, to see whether
either flight ended or was refiled, but that look only feeds the diagnostic tables, never a
verdict.

**Finding the flight.** The aircraft's own ADS-B reports from the 60 s before the closest
approach give its address, callsign and position.

1. Every flight that states the address is taken, and only those flying (proposed, active or
   departed) are kept. A flight that has been dropped does not count.
2. If more than one is flying, they are narrowed in this order: the callsign the aircraft is
   broadcasting; active or departed over proposed; then the flight's own SFDPS position, at most
   120 s old and within 10 NM of the aircraft. A step that would leave none is skipped. If more
   than one is still left, the address join names no flight.
   The callsign comes first because one airframe flies its legs of the day under different
   flight numbers and broadcasts the current one. Taking the newest flight to state the address
   picked another leg of the same operator in 23 of 170 aircraft on the development hour.
3. If the address names no flight, the callsign is tried. That needs one flying flight with the
   callsign and a callsign that did not change in the 60 s. Where the flight has a recent SFDPS
   position it must be within 10 NM. `--pick-newest-plan` takes the newest plan when several
   flying flights share the callsign and the same departure and arrival. It is off by default
   because it moved one aircraft on the development hour.
4. Where both keys name a flight and the flights differ, the one whose position agrees with the
   aircraft is taken. If both agree or neither does, the address flight stands. Every such case
   is counted.

**The verdict.** For an airborne encounter whose vertical separation at the closest approach is
900 to 1,000 ft, with either aircraft climbing or descending at 300 fpm or more:

| Verdict | Meaning |
|---|---|
| level-off | the two aircraft are cleared to altitudes 1,000 ft or more apart |
| does not stop | they are cleared to altitudes less than 1,000 ft apart, or the same one |
| no flight record | one or both aircraft have no flight |
| no altitude | a flight with no assigned altitude |
| not one level | VFR, a block or an altitude at a fix |

The table is given twice: on the assigned altitude, and on the interim altitude where one is set.

**What is printed.** The summary prints:
- how every aircraft was joined, by key and by address outcome;
- why each callsign join was refused;
- the cross-check where both keys name a flight;
- the positions held, and any refused when the position store was full;
- record coverage (both aircraft, one, neither), with the weakest key used among those with
  both.

All of these are counts. The recordings open through the hold-out guard, and no flight
identifier is printed.

**Limits.**
- An aircraft that files no flight plan, or flies VFR, has no record to find. Those aircraft
  could account for many of the "no flight record" verdicts. That share has not been measured.
- The join cannot tell two flights apart when neither has a recent SFDPS position and their
  callsigns do not settle it. The address flight then stands, and the case is counted.
