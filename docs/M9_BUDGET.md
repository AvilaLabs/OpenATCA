# M9 budget: one national hour on the reference laptop

First measurement. Tables and commands only. Machine: Intel Core i3-N305 (8 cores), 30 GB, shared with
other builds, load average 4.6 to 5.9 during every run below (so every timing is under load and is
labelled with it). Runs pinned with `taskset -c 0-3`. The tracker and conflict alert are single-threaded:
each run used one core (99 % of CPU), so the one-core figures are the budget actually used.

## Input

| Item | Value |
|---|---|
| Source | adsb.lol, 2026-11-19 (tune day), worldwide archive, two tar parts |
| Area | box 23 to 51 N, 127 W to 65 W (CONUS and margin) |
| Clip searched for the busiest hour | 12:00:00Z to 23:44:00Z, 40,864,430 reports, 3.27 GB (the full-day clip is refused by the clip tool: more than 41,943,040 reports) |
| Hours not examined | 23:44Z to 24:00Z |
| Busiest hour (whole UTC hours) | 21:00:00Z to 22:00:00Z, 15,107 distinct airborne addresses |
| Peak minute | 21:15Z, 7,684 distinct airborne addresses |
| Clip measured | 20:50:00Z to 22:00:00Z, 5,159,535 reports, 413 MB; first 10 minutes warm-up, not measured |
| Measured reports (after the quality filter, 21:00 to 22:00Z) | 4,016,586 |
| Airborne | a report with a barometric altitude and not on the ground |

## Distinct airborne addresses per UTC hour

| Hour (Z) | 00 | 01 | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 09 | 10 | 11 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Addresses | 10,024 | 8,231 | 6,945 | 5,577 | 4,128 | 2,674 | 1,691 | 1,216 | 1,037 | 1,127 | 1,414 | 2,420 |

| Hour (Z) | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 (to 23:44) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Addresses | 4,391 | 7,342 | 10,232 | 12,444 | 13,715 | 14,158 | 14,501 | 14,735 | 15,077 | 15,107 | 14,105 | 10,708 |

| Census | Value |
|---|---|
| Clip 00:00Z to 12:00Z (same box) | 14,224,653 reports, 1.14 GB |
| Peak minute of 00:00Z to 12:00Z | 00:00Z, 5,745 distinct airborne addresses |
| Hour beating 21Z's 15,107 | none; the 21:00Z measurement stands |

## Tracking (`nas_hour track`)

| Measure | Run A | Run B |
|---|---|---|
| Machine load average before / after | 4.86 / 4.71 | 4.56 / 5.63 |
| Wall time, whole process (reading, decoding, 70 min replay, 60 min measured) | 99.8 s | 110.5 s |
| User + sys CPU, whole process | 99.4 s | 110.1 s |
| CPU of the measured hour | 80.9 s | 89.0 s |
| Real-time factor (measured-hour CPU / 3600 s) | 0.0225 | 0.0247 |
| Peak RSS (mostly the decoded clip) | 931,072 kB | 931,060 kB |
| Reports per second of traffic, mean / max | 1,115.7 / 1,340 | 1,115.7 / 1,340 |
| Reports processed per CPU second | 49,700 | 45,100 |
| Confirmed tracks held, max / mean (sampled once per second) | 7,772 / 7,707.3 | 7,772 / 7,707.3 |
| Tentative tracks held, max / mean | 95 / 69.2 | 95 / 69.2 |
| Per-report latency (one `tracker_update` call), p50 | 7.50 us | 8.34 us |
| p99 | 12.61 us | 14.82 us |
| p99.9 | 27.95 us | 39.86 us |
| max | 28,560 us | 27,094 us |
| Per-cycle (one second of reports) work, p50 | 22.33 ms | 24.61 ms |
| p99 | 24.77 ms | 27.40 ms |
| p99.9 | 27.62 ms | 32.03 ms |
| max | 42.55 ms | 47.37 ms |
| Cycles longer than 1 s | 0 of 3,600 | 0 of 3,600 |

Run A's track-count labels were printed swapped by the first build of the tool; the timings are valid and
the counts are those of Run B (the same input, deterministic).

### Tracker capacity and refusal counters (Run B)

| Counter | Value |
|---|---|
| Track slots (`max_tracks`) | 65,536 |
| Refused for capacity | 0 |
| Output rows lost (table overflow) | 0 |
| Refused for position, covariance, velocity, address, time | 0, 0, 0, 0, 0 |
| Refused for altitude | 676 |
| Late (window or ring), numerical refusals, time held | 0, 0, 0 |
| Duplicates dropped | 2,873 |
| Offered / applied | 4,671,037 / 4,584,871 |
| Tracks started / confirmed (70 min) | 25,993 / 23,546 |
| Rows not representable, NaN covariance | 0, 0 |

## Conflict alert (`nas_hour ca`, tracker and conflict alert in one process)

| Measure | Run A | Run B |
|---|---|---|
| Machine load average before / after | 5.05 / 5.87 | 4.95 / 5.49 |
| Wall time, whole process | 339.5 s | 364.3 s |
| User + sys CPU, whole process | 338.9 s | 363.7 s |
| CPU of the measured hour, tracker and conflict alert together | 289.4 s | 310.3 s |
| Real-time factor of both (measured-hour CPU / 3600 s) | 0.0804 | 0.0862 |
| Conflict-alert cycles, summed time (`ca_cycle` only) | 183.6 s | 197.9 s |
| Real-time factor of `ca_cycle` alone | 0.0510 | 0.0550 |
| Conflict-alert time with building the rows (70 min, includes warm-up) | 236.8 s | 254.0 s |
| Peak RSS | 930,988 kB (Run B) | 930,988 kB |
| Tracks per cycle, max / mean | 7,769 / 7,705.5 | 7,769 / 7,705.5 |
| Pairs evaluated per cycle, max / mean | 15,732 / 14,674.7 | 15,732 / 14,674.7 |
| Pairs found by the grid per cycle (candidates), max / mean | 15,732 / 14,674.7 | 15,732 / 14,674.7 |
| Cycle time, p50 | 50.80 ms | 54.21 ms |
| p99 | 57.19 ms | 65.32 ms |
| p99.9 | 61.37 ms | 71.14 ms |
| max | 68.04 ms | 76.67 ms |
| Cycles longer than 1 s | 0 of 3,601 | 0 of 3,601 |
| Cycles with work left undone by a budget (`work_short`) | 0 | 0 |
| Cycles with tracks unscanned | 0 | 0 |
| Cycles with candidates not evaluated | 0 | 0 |
| Cycles the net reported not running | 3,601 of 3,601 | 3,601 of 3,601 |
| Net state of those cycles | DEGRADED: INPUT MISSING | DEGRADED: INPUT MISSING |

### Why the net reported degraded (conflict alert, Run B)

| Count | Value |
|---|---|
| Rows above the latitude the grid's flat test is proved for, per cycle, max / mean | 853 / 398.3 |
| Cycles with any such row | 3,601 of 3,601 |
| Rows whose 95 % radius was held to the cap, per cycle, max / mean | 20 / 6.5 |
| Rows refused for capacity, bad, duplicate | 0, 0, 0 |
| Pairs skipped as unable to converge vertically (70 min) | 153,882,714 |
| Pairs evaluated, in conflict (pair cycles, 70 min) | 61,223,242, 18,985,626 |
| Alerts opened / closed / dropped | 274,487 / 269,814 / 0 |

Conflict alert refuses no area: every row is used. The latitude limit is `atan(0.02 / dphi)`, with `dphi` the
cycle's largest pair radius over 6,356,752 m (`crates/oa-ca/src/lib.rs`); rows above it are used and counted
and the net reports degraded (`rows_high_lat`). Configuration: `national.cfg` (development minima of
`tools/oa-ca/testdata/dev.cfg`; capacities `max_tracks` 65,536, `max_open` 65,536, `max_candidates`
4,194,304; budgets `work_budget` 1,000,000,000, `eval_budget` 4,000,000; none is an ERAM or STARS value).

## Not measured

| Function | State |
|---|---|
| Fusion with radar | not measured |
| MSAW | not measured |
| Flight data processing | not measured |
| Feed health | not measured |
| Comparator against ERAM | not measured |
| Display scene generation | not measured |
| Quiet machine (no other jobs) | not measured |

## Commands

Data directory `D=/home/connoravila/Documents/Avila-Labs/openatca-data/derived/nas-hour-20261119`,
archive `R=/home/connoravila/Documents/Avila-Labs/openatca-data/raw/adsblol/2026.11.19`,
`T=$HOME/.cache/avila-night/openatca-nashour/release`.

```
systemd-run --user --scope -q -p MemoryMax=5G -p MemorySwapMax=0 env CARGO_BUILD_JOBS=2 \
  CARGO_TARGET_DIR=$HOME/.cache/avila-night/openatca-nashour cargo +1.89.0 build --release \
  -p oa-adsb -p oa-ca-run --example nas_hour

$T/oa-adsb-clip --out $D/conus-1200-2344.oaf --box 23,51,-127,-65 \
  --from 2026-11-19T12:00:00Z --to 2026-11-19T23:44:00Z $R/*.tar.aa $R/*.tar.ab 2> $D/conus-1200-2344.err

$T/examples/nas_hour census $D/conus-1200-2344.oaf > $D/census.txt

$T/oa-adsb-clip --out $D/conus-2050-2200.oaf --box 23,51,-127,-65 \
  --from 1795121400 --to 1795125600 $R/*.tar.aa $R/*.tar.ab 2> $D/conus-2050-2200.err

systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=0 taskset -c 0-3 /usr/bin/time -v \
  $T/examples/nas_hour track $D/conus-2050-2200.oaf --start 1795122000 --max-tracks 65536 > $D/track.txt 2> $D/track.time

systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=0 taskset -c 0-3 /usr/bin/time -v \
  $T/examples/nas_hour ca $D/conus-2050-2200.oaf --start 1795122000 --max-tracks 65536 \
  --config $D/national.cfg > $D/ca.txt 2> $D/ca.time
```

`1795122000` is 2026-11-19T21:00:00Z. The archive parts are the two files
`v2026.11.19-planes-readsb-prod-0.tar.aa` and `.ab`.
