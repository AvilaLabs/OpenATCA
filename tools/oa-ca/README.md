# oa-ca

Runs conflict alert over a recorded ADS-B clip. It tracks the clip with the OpenATCA tracker,
gives the core crate `oa-ca` the live tracks once a second, and writes every alert the net raised.
This is step 3 of the M5 build order (`docs/M5_DESIGN.md` §5).

```
oa-ca --adsb clip.oaf --config file.cfg --out dir [--scorer-half]
```

- `--adsb`: an `.oaf` clip from `oa-adsb-clip`. By default every report in the clip is tracked.
- `--scorer-half`: track only the scorer's half A instead, as `oa-track-run` does.
- `--config`: the conflict alert configuration (below). It is required: the tool has no built-in
  values.
- `--out`: a directory for `alerts.csv`, `altitude_lost.csv` and `summary.txt`. The summary is
  also printed.

Exit status: 0 on success, 1 on a failed run, 2 on a refused argument or a sealed path. Every path
is checked against the sealed list, as given and resolved, before anything is opened or created.

## Configuration

A plain text file of `key = value` lines; `#` starts a comment. Every key is required and
nothing has a default.

| Key | Meaning |
|---|---|
| `look_ahead_s` | how far ahead a pair is predicted |
| `hysteresis_s` | how long an alert stays open after its last cycle in conflict |
| `default_h_nm`, `default_v_ft` | the minima of a pair outside every volume |
| `volume` | a volume with its own minima, repeatable: `box lat_lo lat_hi lon_lo lon_hi alt_lo_ft alt_hi_ft h_nm v_ft`, or `polygon alt_lo_ft alt_hi_ft h_nm v_ft lat,lon lat,lon lat,lon ...` with at least three vertices |
| `max_tracks`, `max_open`, `max_candidates`, `alert_log_capacity`, `event_log_capacity` | capacities, fixed at start |
| `work_budget`, `eval_budget` | the per-cycle work budgets of the grid scan and of the exact evaluation |
| `max_speed_mps` | a track faster than this is refused as bad |
| `r95_cap_m` | the cap on a track's 95 % position radius |
| `grid_band_m` | the height of a band of the candidate grid |
| `net_stale_limit_s`, `net_future_horizon_s` | when the net's published state is stale, and how far ahead of the clock it may be |

`testdata/dev.cfg` is the configuration for the development hour (ZAU, 13 November 2026, 18:00Z).
Its values are chosen to exercise the tool on that clip. None of them is an ERAM or STARS value,
a facility adaptation or a number from FAA order 7110.65.

## What an alert is

A pair is in conflict when its straight-line prediction from the current positions and
velocities breaks both minima at the same time within the look-ahead. The minima are the
volume's, widened so that a poor track makes an alert more likely and never less: the horizontal
minimum grows by both tracks' 95 % radii, and the vertical by 100 ft for each coasting or
uncertain track. A pair with an altitude unknown on either track is tested horizontally only and
flagged `ALT_UNKNOWN`.

One alert covers a pair from its first cycle in conflict to its last, held open for the
hysteresis so that one missed cycle does not split it in two. When a cycle's work budget runs
out, the pairs nearest each other horizontally are evaluated first; an open alert whose pair was
not reached stays open and is counted as unevaluated, never ended by the gap.

## Outputs

`alerts.csv` has one row per alert, sorted by start time, with the two addresses, the start, end
and peak times, the time to loss of separation when the alert started and at its peak, the
separations, the widened minima in force, the context and the flags. It holds aircraft addresses
and is not for publication.

`altitude_lost.csv` has one row for each time a track that had a known altitude lost it.

`summary.txt` gives the clip and configuration with their SHA-256 hashes, the net's state after
every cycle (running, degraded with the reason, or unavailable), the alert count by context, the
count of alerts having each flag, and how many alerts were kept alive over an unevaluated cycle.

Each alert has one context, from its flags, tested in this order:

1. `GROUND`: either aircraft on the ground.
2. `DEGRADED_TRACK`: either track coasting or uncertain at some cycle.
3. `ALT_UNKNOWN`: an altitude unknown on either track at some cycle.
4. `AIRBORNE`: everything else.

## Result on the development hour

On the development hour with `testdata/dev.cfg`, compared with the `oa-encounter` record of the
same clip: every C2 encounter (28 of 28) and every C3 encounter (174 of 174) had an alert. Of the
244 C4 encounters (3 to 5 NM), 150 had one. All 94 without an alert are inside the ORD terminal
box, whose configured horizontal minimum is 3 NM, so a pass between 3 and 5 NM there is not a
conflict under that configuration.

This is a development result on development values. It says the net finds what it is configured
to find. It does not say those values are right for any airspace.
