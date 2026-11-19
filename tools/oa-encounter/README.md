# oa-encounter

Records every pair of aircraft that comes close, whether or not any alert would have sounded. It
runs the tracker on an ADS-B clip and feeds its one-second output to the `oa-encounter` core crate.
This is step 2 of the M5 build order (`docs/M5_DESIGN.md` §5). It needs no alerting logic, so it
exists before conflict alert does, and it gives conflict alert its evaluation set.

```
oa-encounter --adsb clip.oaf --out dir [--nasr nasr.zip] [--scorer-half]
```

- `--adsb`: an `.oaf` clip from `oa-adsb-clip`. By default every report in the clip is used.
- `--scorer-half`: use the scorer's half A instead, as `oa-track-run` does.
- `--nasr`: the FAA NASR subscription zip. Its public airports (`APT_BASE.csv`, site type airport,
  facility use public) feed the surface rule below. Without it, the surface rule uses the ground
  state alone.
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
