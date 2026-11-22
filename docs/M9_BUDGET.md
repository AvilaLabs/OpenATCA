# M9 budget: one national hour on the reference laptop

Milestone M9 is the whole NAS at the busiest recorded hour, in real time, on one ordinary laptop,
using at most four of its eight cores (SCOPE.md). This file is where that claim is measured, one
function at a time, as each function is built. A real-time factor is the CPU time a function
spends divided by the hour of traffic it processed: 1.0 is one core exactly keeping up, and the M9
budget for everything together is 4.0.

`nas_hour` reports how many seconds in the 3,600-second window received qualifying measurements.
If a second has no qualifying input, or the loader reports damage or sealed rows, the whole-hour
real-time factor is unavailable. The reported work and counters still describe the partial run.

What is measured so far, on the busiest UTC hour of 19 November 2026 in a box around the
contiguous United States (15,107 airborne aircraft in the hour, up to 7,806 tracked at once):

| Function | Real-time factor, one core | Share of the 4.0 budget |
|---|---|---|
| Tracking, archived trace points (about 0.15 reports per aircraft per second) | 0.024 | 0.6 % |
| Tracking, synthetic 2 reports per aircraft per second | 0.189 | 4.7 % |
| Tracking and conflict alert, synthetic 2 per second | 0.236 | 5.9 % |

What these numbers do and do not show:

- They are the core computation only. Reading and decoding the input, and the quality filter, are
  outside the timed span.
- The input is ADS-B only. Radar fusion, which must associate plots that carry no identity from
  every radar, is not measured, and is expected to be the largest cost.
- The live rates are synthetic: trace points interpolated to 1 or 2 per second. A live ADS-B feed
  is up to about 2 position messages per aircraft per second.
- It is the busiest hour of one Thursday, not the busiest recorded hour; the Sunday after
  Thanksgiving is a better candidate, and will be measured when its archive is out.
- Conflict alert ran with development separation minima everywhere, and reported itself degraded
  on every cycle because about 400 aircraft a cycle were above the latitude its grid test is proved
  for. Its compute is measured, but not yet a complete national conflict alert.
- Every run is a single run on a laptop loaded by other work (load average 4.6 to 8.2), so the
  timings are upper figures, not best cases.
- Flight data processing, the conflict probe, MSAW at national scale, feed health and display
  scene generation are not measured yet.


| Machine | Value |
|---|---|
| CPU, memory | Intel Core i3-N305 (8 cores), 30 GB, shared with other jobs |
| Pinning | `taskset -c 0-3`; tracker and conflict alert single-threaded (99 % of one core) |
| Load average during the runs | 4.6 to 8.2, see each row; every timing is under load |

| Input row | Meaning |
|---|---|
| Archived points | the adsb.lol trace points as archived, about 0.145 reports per aircraft per second |
| 1 Hz, 2 Hz | synthetic rate from interpolated trace points: linear in position, altitude, vertical rate and velocity between consecutive points of one aircraft no more than 60 s apart; quality fields of the earlier point; no extrapolation past an aircraft's last point |

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

| Hour (Z) | Distinct airborne addresses | Trace rows in the hour | Rows per second | Peak minute (Z) | Distinct airborne addresses in the peak minute |
|---|---|---|---|---|---|
| 20 | 15,077 | 4,360,946 | 1,211.4 | 20:45 | 7,699 |
| 21 | 15,107 | 4,425,128 | 1,229.2 | 21:15 | 7,684 |
| 22 | 14,105 | 4,174,295 | 1,159.5 | 22:00 | 7,481 |
| 00 to 12 (peak minute of the span) | 10,024 (00Z) | 14,224,653 (span) | not computed | 00:00 | 5,745 |
| Hour beating 21Z's 15,107 | none | | | | |

## Tracking (`nas_hour track`)

| Measure | Run B (archived points, 10:57-11:18 runs) | Archived points, rerun | 1 Hz: synthetic rate from interpolated trace points | 2 Hz: synthetic rate from interpolated trace points |
|---|---|---|---|---|
| Machine load average before / after | 4.56 / 5.63 | 5.57 / 6.16 | 7.14 / 6.21 | 8.18 / 8.07 |
| Wall time, whole process | 110.5 s | 107.8 s | 396.3 s | 809.3 s |
| User+sys CPU, whole process | 110.1 s | 107.2 s | 393.5 s | 793.0 s |
| CPU of the measured hour | 89.0 s | 87.3 s | 335.3 s | 679.5 s |
| Real-time factor (measured-hour CPU / 3600 s) | 0.0247 | 0.0242 | 0.0931 | 0.1888 |
| Peak RSS including load | 931,060 kB | 931,152 kB | 931,256 kB | 1,066,872 kB |
| Working set RSS after clip decoded and dropped / start of hour / end of hour | not recorded | 447,620 / 610,732 / 667,000 kB | 447,648 / 634,188 / 866,032 kB | 447,716 / 634,240 / 1,066,872 kB |
| Reports per second of traffic, mean / max | 1,115.7 / 1,340 | 1,115.7 / 1,340 | 7,650.6 / 7,876 | 14,789.4 / 15,044 |
| Measured reports | 4,016,586 | 4,016,586 | 27,542,068 | 53,241,971 |
| Reports per CPU second | 45,130 | 46,030 | 82,142 | 78,349 |
| Confirmed tracks, max / mean | 7,772 / 7,707.3 | 7,772 / 7,707.3 | 7,805 / 7,739.3 | 7,806 / 7,741.3 |
| Tentative tracks, max / mean | 95 / 69.2 | 95 / 69.2 | 66 / 37.3 | 65 / 35.4 |
| Update calls without a tick (per report), mean / p50 / p99 / p99.9 / max | not recorded | 8.11 / 8.08 / 13.09 / 24.96 / 3,071.50 us | 9.07 / 9.04 / 14.82 / 27.15 / 10,077.26 us | 10.57 / 10.24 / 20.72 / 34.65 / 17,783.52 us |
| Tick-crossing calls (the per-second sweep), mean / p50 / p99 / p99.9 / max | not recorded | 15,095.54 / 14,962.15 / 17,306.74 / 20,390.91 / 24,216.21 us | 16,907.25 / 16,935.22 / 22,076.77 / 28,488.68 / 37,260.08 us | 19,734.79 / 19,427.46 / 29,720.92 / 45,473.39 / 54,080.03 us |
| All update calls together (Run B only), mean / p50 / p99 / p99.9 / max | 22.12 / 8.34 / 14.82 / 39.86 / 27,093.93 us | - | - | - |
| Mean sweep time per second | not recorded | 15.10 ms | 16.91 ms | 19.73 ms |
| Per-cycle (one second of reports) work, mean / p50 / p99 / p99.9 / max | 24.68 / 24.61 / 27.39 / 32.03 / 47.37 ms | 24.14 / 23.88 / 28.16 / 32.91 / 38.06 ms | 86.31 / 87.28 / 111.00 / 137.43 / 167.91 ms | 175.99 / 172.82 / 243.19 / 303.06 / 325.07 ms |
| Cycles longer than 1 s | 0 of 3,600 | 0 of 3,600 | 0 of 3,600 | 0 of 3,600 |
| Refused for capacity / output rows lost | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| Refused for altitude / duplicates dropped | 676 / 2,873 | 676 / 2,873 | 2,493 / 2,434 | 4,557 / 2,401 |
| Offered / applied | 4,671,037 / 4,584,871 | 4,671,037 / 4,584,871 | 31,918,825 / 31,219,405 | 61,693,260 / 60,022,254 |

## Conflict alert (`nas_hour ca`, tracker and conflict alert in one process)

| Measure | Run B (archived points, 10:57-11:18 runs) | Archived points, rerun | 1 Hz: synthetic rate from interpolated trace points | 2 Hz: synthetic rate from interpolated trace points | Mark |
|---|---|---|---|---|---|
| Machine load average before / after | 4.95 / 5.49 | 6.16 / 7.14 | 6.21 / 8.18 | 8.07 / 4.78 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Wall time, whole process | 364.3 s | 388.3 s | 757.8 s | 1018.9 s | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| User+sys CPU, whole process | 363.7 s | 387.2 s | 746.5 s | 1002.1 s | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| CPU of the measured hour, tracker and conflict alert | 310.3 s | 329.2 s | 643.3 s | 850.2 s | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Real-time factor of both | 0.0862 | 0.0914 | 0.1787 | 0.2362 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| `ca_cycle` summed | 197.8 s | 210.7 s | 245.1 s | 225.3 s | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Real-time factor of `ca_cycle` alone | 0.0550 | 0.0585 | 0.0681 | 0.0626 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Conflict-alert time with building the rows (70 min, includes warm-up) | 253.9 s | 272.4 s | 312.3 s | 291.5 s | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Peak RSS including load | 930,988 kB | 931,240 kB | 931,092 kB | 1,069,104 kB | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Working set RSS after clip decoded and dropped / start of hour / end of hour | not recorded | 447,676 / 613,424 / 669,764 kB | 447,604 / 636,304 / 868,268 kB | 447,688 / 636,380 / 1,069,104 kB | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Reports per second of traffic, mean / max | 1,115.7 / 1,340 | 1,115.7 / 1,340 | 7,650.6 / 7,876 | 14,789.4 / 15,044 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Tracks per cycle, max / mean | 7,769 / 7,705.5 | 7,769 / 7,705.5 | 7,801 / 7,737.5 | 7,803 / 7,739.5 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Pairs evaluated per cycle, max / mean | 15,732 / 14,674.7 | 15,732 / 14,674.7 | 15,776 / 14,650.7 | 15,942 / 14,804.7 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Pairs found by the grid per cycle, max / mean | 15,732 / 14,674.7 | 15,732 / 14,674.7 | 15,776 / 14,650.7 | 15,942 / 14,804.7 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Conflict-alert cycle, mean / p50 / p99 / p99.9 / max | 54.94 / 54.21 / 65.32 / 71.14 / 76.67 ms | 58.50 / 58.95 / 67.63 / 76.25 / 97.96 ms | 68.06 / 67.16 / 97.67 / 146.56 / 153.80 ms | 62.56 / 60.85 / 98.71 / 171.13 / 200.17 ms | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Cycles longer than 1 s | 0 of 3,601 | 0 of 3,601 | 0 of 3,601 | 0 of 3,601 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Cycles with work left undone by a budget | 0 | 0 | 0 | 0 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Cycles with tracks unscanned / candidates unevaluated while running | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Cycles the net reported not running | 3601 | 3601 | 3601 | 3601 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Rows above the flat-test latitude per cycle, max / mean | 853 / 398.3 | 853 / 398.3 | 853 / 400.4 | 1,439 / 400.9 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Rows with the radius held to the cap per cycle, max / mean | 20 / 6.5 | 20 / 6.5 | 14 / 5.5 | 16 / 5.5 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef |
| Alerts opened / closed / dropped (development minima, 70 min including warm-up, not a conflict rate) | 274,487 / 269,814 / 0 | 274,487 / 269,814 / 0 | 270,933 / 266,256 / 0 | 273,010 / 268,269 / 0 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef; development minima, 70 min including warm-up, not a conflict rate |
| Pair cycles evaluated / in conflict (development minima, 70 min including warm-up, not a conflict rate) | 61,223,242 / 18,985,626 | 61,223,242 / 18,985,626 | 61,264,599 / 19,252,449 | 61,905,580 / 19,453,508 | net DEGRADED on every cycle (latitude-limit and r95-cap rows); a fix is in lane-ca-ecef; development minima, 70 min including warm-up, not a conflict rate |

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
`T=$HOME/.cache/avila-night/openatca-nashour/release`. The archive parts are
`v2026.11.19-planes-readsb-prod-0.tar.aa` and `.ab`. `1795122000` is 2026-11-19T21:00:00Z.

```
systemd-run --user --scope -q -p MemoryMax=5G -p MemorySwapMax=0 env CARGO_BUILD_JOBS=2 \
  CARGO_TARGET_DIR=$HOME/.cache/avila-night/openatca-nashour cargo +1.89.0 build --release \
  -p oa-adsb -p oa-ca-run --example nas_hour

$T/oa-adsb-clip --out $D/conus-1200-2344.oaf --box 23,51,-127,-65 \
  --from 2026-11-19T12:00:00Z --to 2026-11-19T23:44:00Z $R/*.tar.aa $R/*.tar.ab 2> $D/conus-1200-2344.err
$T/oa-adsb-clip --out $D/conus-0000-1200.oaf --box 23,51,-127,-65 \
  --from 2026-11-19T00:00:00Z --to 2026-11-19T12:00:00Z $R/*.tar.aa $R/*.tar.ab 2> $D/conus-0000-1200.err
$T/oa-adsb-clip --out $D/conus-2000-2300.oaf --box 23,51,-127,-65 \
  --from 1795118400 --to 1795129200 $R/*.tar.aa $R/*.tar.ab 2> $D/conus-2000-2300.err
$T/examples/nas_hour census $D/conus-1200-2344.oaf > $D/census.txt
$T/examples/nas_hour census $D/conus-0000-1200.oaf > $D/census-0000-1200.txt
$T/examples/nas_hour census $D/conus-2000-2300.oaf > $D/census-2000-2300.txt

$T/oa-adsb-clip --out $D/conus-2050-2200.oaf --box 23,51,-127,-65 \
  --from 1795121400 --to 1795125600 $R/*.tar.aa $R/*.tar.ab 2> $D/conus-2050-2200.err

# each of track and ca, with no flag (archived points), --rate-hz 1 and --rate-hz 2:
systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=0 taskset -c 0-3 /usr/bin/time -v \
  $T/examples/nas_hour track $D/conus-2050-2200.oaf --start 1795122000 --max-tracks 65536 [--rate-hz 1] \
  > $D/track-<archive|1|2>.txt 2> $D/track-<...>.time
systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=0 taskset -c 0-3 /usr/bin/time -v \
  $T/examples/nas_hour ca $D/conus-2050-2200.oaf --start 1795122000 --max-tracks 65536 [--rate-hz 1] \
  --config $D/national.cfg > $D/ca-<archive|1|2>.txt 2> $D/ca-<...>.time
```
