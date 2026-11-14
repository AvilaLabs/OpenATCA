# M2 — Benchmark harness and legacy baseline: design

Status: approved by Connor 2026-11-14 (publishing: results stay private until the SCDS terms are read for derived aggregates; truth gates as stated, tuned from the leave-one-out result with the reason recorded).

## Goal

Measure the systems OpenATCA must beat (ERAM's published tracks, TBFM's and TFMS's time
estimates, ERAM's converted routes and handoffs) against independent truth, on one recorded
day, with scoring code that will later score OpenATCA's own output unchanged. SCOPE §7 M2 and
docs/BENCHMARKS.md §1 bar 2.

The point is not to criticise ERAM. It is to replace "best in class" claims with numbers anyone
can rerun, and to give every later milestone a target measured on the same traffic.

## Inputs

| Input | Source | Status |
|---|---|---|
| ERAM flight data and published track positions | SFDPS recording 2026-11-14/15 (`oa-fixm`) | recording |
| TBFM schedules and estimates | TBFM recording (`oa-tbfm`) | recording |
| TFMS estimates | TFMS recording (`oa-tfms`) | recording |
| ADS-B reports | adsb.lol history for the same window (`oa-adsb`, building) | fetched after the day ends |
| Airspace, fixes, airports, runways, ARTCC boundaries | M0 model | have |

## What SFDPS lets us measure, and what it does not

SFDPS `TH` messages carry ERAM's track position, position time, track velocity, reported
altitude and a coast indicator; `OH` and `boundaryCrossings` carry handoffs and sector
ownership; `HX` carries ERAM's expanded route. This is ERAM's *published* track, not its
internal track at radar update rate: SFDPS may decimate or smooth it, and it arrives about 60 s
late (UK-004). So M2 measures:

- position, velocity and altitude error of the published track at each published position time;
- how often SFDPS publishes a position per flight, and how stale it is when it does;
- track presence (does a flight in ZAU airspace have a published position within a window);
- association errors (a published track that matches another aircraft better).

It cannot measure ERAM's internal probability of update or internal track error. Every report
says so in its header. Where a benchmark row needs the internal value, BENCHMARKS.md keeps the
row open and the gap goes to outreach.

## 1. Truth: reference trajectories from ADS-B

ADS-B is not perfect truth, so the reference is built with care and its own error is measured
and published.

1. **Selection.** Reports with NACp ≥ 8 (95% position uncertainty under 0.05 NM, about 93 m)
   and NIC ≥ 7, SIL 3, ADS-B version 2 where the field exists; positions with a fault flag
   or a kinematically impossible jump are removed and counted.
   In adsb.lol traces the quality fields (NACp, NIC, SIL, version) appear only on rows that
   carry readsb's aircraft object, about a quarter of rows in the 2026-11-13 sample. They are
   carried forward within a trace for at most 60 s; rows with no quality value within that age
   are not truth. On that sample, 99% of rows with a known NACp had NACp ≥ 8, and 22 position
   jumps above 700 kt (largest 2,782 kt) were found in 3.2 million row pairs: the jump filter is
   needed.
2. **Legs.** Each aircraft (24-bit address) splits into legs at readsb's new-leg flag or a gap of
   more than 120 s.
3. **Smoother.** Per leg, in a local east-north-up frame, a fixed-interval Rauch–Tung–Striebel
   smoother with a nearly-constant-acceleration model. Output: position and velocity at any
   query time inside the leg, with a smoothed covariance.
4. **No extrapolation.** Truth exists at time t only if real reports lie within 10 s on both
   sides of t. Otherwise the comparison is recorded as "no truth", never filled in.
5. **Truth accuracy, measured.** Leave-one-out: hide every fifth report, predict it from the
   rest, publish the residual distribution. A comparison is used only where the truth's 95%
   horizontal uncertainty is below 50 m. ERAM errors are expected to be several times larger;
   if they are not, the report says the truth is not good enough to separate them.
6. **Altitude.** ADS-B barometric altitude is the same pressure altitude source ERAM uses
   (25 ft or 100 ft quantised), so altitude is compared directly, without smoothing.

**Timing.** At 450 kt, one second of clock offset is 230 m of along-track error. adsb.lol
timestamps come from its feeders' clocks; ERAM's position time from ERAM. So:
- **cross-track error is the primary horizontal metric**: it does not depend on time offset;
- along-track error is reported twice: raw, and after fitting one constant time offset over the
  whole day (the fitted offset is itself published);
- the fitted offset must be stable across the day and across feeders; if it drifts, along-track
  numbers are withheld with the reason.

## 2. Association: which ADS-B aircraft is which ERAM flight

1. The flight plan's `aircraftAddress` (24-bit address) where SFDPS carries it.
2. Otherwise callsign match (ADS-B flight ID against ERAM aircraft identification) plus gating:
   the ERAM position must be within 3 NM and 1,000 ft of that aircraft's truth for at least 80%
   of shared times.
3. Ambiguous candidates (two aircraft pass the gate) are not associated; they are counted.

The association method is stored per pair, and every metric can be split by it.

## 3. Metrics

All metrics are free functions over struct-of-arrays tables of paired samples. Each reports N,
mean, RMS, median, 95th and 99th percentile and maximum, and is split by flight phase (climb,
cruise, descent from the truth vertical rate), altitude band and coast indicator.

| ID | Metric | Pairing |
|---|---|---|
| T1 | Cross-track position error | ERAM TH position vs truth at ERAM position time |
| T2 | Along-track error, raw and offset-corrected | same |
| T3 | Speed error and heading error | ERAM track velocity vs truth velocity |
| T4 | Altitude error | ERAM reported altitude vs truth barometric altitude |
| T5 | Publication interval per flight | consecutive TH position times |
| T6 | Staleness | receive time minus position time (UK-004) |
| T7 | Presence | fraction of truth flight-time inside ZAU with an ERAM position within 30 s |
| T8 | Initiation delay | first ERAM position vs truth entry into ZAU or takeoff |
| T9 | Association errors | ERAM position closer to another aircraft's truth than to its own for 3+ consecutive samples |
| F1 | Meter-fix time error | TBFM estimate vs truth crossing time, by look-ahead in 5-minute bins to 60 min |
| F2 | Schedule delivery | TBFM scheduled time vs truth crossing time |
| F3 | Arrival and departure time error | TFMS estimates vs truth landing and takeoff, by look-ahead |
| R1 | Route conformance | truth cross-track distance from ERAM's expanded route; share of flight time beyond 2 NM, with each case listed (LS-201) |
| H1 | Handoff timing | handoff initiation and acceptance vs truth crossing of the sector or ARTCC boundary |

Truth crossing of a meter fix is the time of closest approach to the fix within 2 NM; if the
aircraft never passes within 2 NM the flight is excluded and counted. Landing is the last truth
point before the aircraft is on the ground within 1 NM of a runway threshold of the destination.

## 4. The harness checks itself

Before any real number is published, the metric code must recover known errors:
- a synthetic "legacy" stream generated from the truth with injected bias, noise, time offset,
  decimation, coasting and one deliberate track swap;
- each metric must recover each injected value within a stated tolerance, and the swap must be
  found. These are requirements with tests (OA-BENCH-…).

## 5. Output

One run produces a report with: input file hashes, code version, replay state hash, every metric
with N, and an exclusions table (no truth, truth too uncertain, not associated, unresolved meter
fix…) so nothing is dropped silently. The full report stays in `openatca-data/`. What we
publish is aggregate statistics only, after the SCDS terms have been checked for derived
aggregates (open question below).

## 6. Code

| Crate | Role |
|---|---|
| `oa-bench` | truth construction, association, metrics, synthetic self-test, report writer |
| `oa-bench` binary | `oa-bench --sfdps-dir … --tbfm-dir … --tfms-dir … --adsb … --out …` |

The core is pure: inputs are the replayed message stream and the ADS-B reports; time is an
input; same inputs give the same report byte for byte.

## Open questions for Connor

1. **Publishing.** SCDS data is "not for operational use". Publishing aggregate error statistics
   derived from it is not operational use, but the SCDS terms should be read for derived data
   before the first public benchmark. Until then, results stay private.
2. **Truth gate values** (NACp ≥ 8, 50 m truth uncertainty, 10 s bracket) are my starting
   choices; the leave-one-out result will show whether they are right.
