# Benchmarks: how "best in class" is measured

Status: first pass, 2026-11-14. SCOPE §3a sets the standard: every function must beat the state
of the art. This file says what that means in numbers, where each number comes from, and which
numbers we still need.

## 1. Three bars per metric

Each metric has up to three bars. A milestone closes only when all bars that exist are beaten,
or when a missed bar has a written diagnosis and next step (never a lowered bar).

1. **Floor: the published standard.** EUROCONTROL ESASSP for surveillance, the EUROCONTROL
   safety-net specifications, FAA NAS requirements. Where a standard has Mandatory and
   Recommended values we take **Recommended**: Airways New Zealand already uses it as its
   acceptance bar, so Mandatory is not the state of the art.
2. **Legacy, measured by us on the same traffic.** SWIM carries ERAM's and TBFM's own outputs:
   ERAM track positions in SFDPS, TBFM's estimated and scheduled times, TFMS's estimates. We
   recorded them for the same 24 hours as the ADS-B truth. So the legacy number is not taken
   from a report; we measure it, on identical traffic, with the same scoring code as our own
   output. This is the strongest comparison available and the one a reviewer can rerun.
3. **Best published research result.** The best number in the open literature for that metric.
   Where test conditions differ from ours, we rerun the published method on our data where
   possible, or say why the comparison is approximate.

Every result is published with the data window, the scoring code version and the state hash of
the replay, so anyone can reproduce it.

## 2. Truth

- **ADS-B truth** for the recorded day comes from adsb.lol history (ODbL, redistributable with
  attribution and share-alike), later also from our own receiver. ADS-B is not perfect truth:
  we keep only reports whose NACp is 8 or better (estimated position uncertainty under 0.05 NM)
  and score against a smoothed reference built after the fact from the whole trajectory, never
  against single reports.
- **Event truth** (meter-fix crossing times, runway threshold times, top of climb, top of
  descent) is derived from the same smoothed reference.
- **Synthetic truth** for things real traffic does not contain often enough (near mid-air
  collisions, spoofing, sensor faults) comes from encounter models and injected faults, labelled
  as synthetic in every report.

## 3. Metrics and current numbers

**Source marks.** P is a primary source that was read; S is secondary; U is unverified. A dash
means not found yet.

### 3.1 Surveillance tracking (M3)

| Metric | Floor | Legacy (to measure, M2) | Best published | Notes |
|---|---|---|---|---|
| Probability of update per aircraft (ESASSP I-4) | ≥97% Recommended for 5 NM, measurement interval ≤6 s Recommended (≤8 s Mandatory) (P: EUROCONTROL ESASSP slides, ICAO APAC 2024; Airways NZ WP13 2025) | ERAM via SFDPS | — | Measured per aircraft, not pooled |
| Long gaps (ESASSP 5N_N-R3) | Per ESASSP Vol 1, value to fetch | ERAM | — | The metric that failed a new radar in Airways NZ's evaluation (P) |
| Horizontal position RMS (I-6) | ESASSP Vol 1, value to fetch | ERAM track vs truth | IMM: −59% position error vs constant-velocity Kalman (U, 2024, no absolute value) | Absolute metres on our data is what counts |
| Velocity RMS, speed and heading (I-16) | ESASSP Vol 1, value to fetch | ERAM | IMM: −77% vs CV Kalman (U) | Hardest metric in the one real evaluation found (P: WP13) |
| False tracks near true tracks (I-18) | ESASSP Vol 1, value to fetch | — | — | Target zero on the recorded day |
| Track swaps | — | — | — | Our own definition, published with the code |
| Track initiation delay | — | ERAM | — | From first truth report to first confirmed track |
| Spoof detection | — | ERAM's behaviour unknown | ≥95% detection (NDSS 2021); false positive rate 0 to 1.06e-3 on open datasets (LoVe, 2023) (S, abstracts) | Injected spoofs over real traffic; publish the confusion matrix |
| Capacity | ERAM: 1,900 simultaneous tracks, 64 radars per centre (S: FAA ERAM page, archived) | — | — | Ours: all 20 centres at once, real time, one machine |

The ESASSP numeric tables have not been read yet: the Volume 1 PDF (edition 26 June 2023) is free
from EUROCONTROL. **Fetching it is the first step of M2.**

### 3.2 Flight data and trajectory prediction (M4)

| Metric | Floor | Legacy (to measure) | Best published | Notes |
|---|---|---|---|---|
| Meter-fix crossing time error | — | TBFM on our recording. Published: undelayed delivery mean +11 s, std 75.5 s at Denver, Aug 2013; std 40.6–129.1 s across four airports (P: NASA/AIAA 2015-3182) | — | We measure TBFM's own estimates against actual crossings, by look-ahead |
| Top of climb / bottom of descent time error | — | — | 5.2 s climb, 6.6 s descent mean absolute error; frozen BADA 95.6 s / 112.2 s (P: arXiv 2601.03075, UK radar, Sept 2019) | Event-based, not fixed look-ahead |
| Same, distance error | — | — | 3.9 nmi climb, 4.3 nmi descent (same paper) | |
| Along-track, cross-track and vertical error at 5, 10 and 20 minutes | — | ERAM conflict probe: published Tech Center studies exist (Paglione et al.) but are scans without a text layer | — | Read the scans; until then legacy is measured from TFMS and TBFM estimates only |
| Route conversion agreement | — | ERAM's converted route in SFDPS | — | Every disagreement explained |
| Conflict probe missed and false alerts | — | — | MTCD about 0.8% missed, 3.6% false (U: workshop slides, labels ambiguous); "less than 5%" false (S: Czech ATC paper) | Placeholders until a primary source is read |

### 3.3 Safety alerts (M5)

Few numbers are public, so this section is mostly method. The method is the one ACAS X used
to replace TCAS II: Monte Carlo runs over published encounter models, scored by risk ratio
(collision or near-miss risk with the alert divided by risk without), alert rate and nuisance
rate, against a baseline run of the legacy logic.

| Metric | Floor | Legacy | Best published | Notes |
|---|---|---|---|---|
| Conflict alert warning time | EUROCONTROL-SPEC-122 §4.3.2, value to fetch; STCA look-ahead about 2 min (U) | ERAM and STARS conflict alert: FAA orders, value to fetch | — | |
| Nuisance alert rate | Definition: "correctly generated according to the rule set but … operationally inappropriate"; alerts lasting a few seconds are generally nuisances (S: EUROCONTROL STCA guidance) | NTSB A-06-44 to A-06-47 (2006, 10 terrain accidents and 1 midair) asked for fewer unwarranted MSAW alerts and better attention capture (P) | ACAS X vs TCAS II: about 52% fewer alerts per aircraft, about 33% fewer 500 ft corrective alerts (U: review paper table, headers unclear) | Missed and nuisance rates always reported together |
| Missed alerts | — | — | — | Scored on encounter models and on recorded losses of separation |
| Risk ratio | — | — | ACAS X validation reports (MIT LL ATC-408), to read | Applied to ground safety nets as a new benchmark |
| Formal proof | — | None published for ERAM or STARS | DAIDALUS (DO-365 reference): algorithms proved in PVS; 2023 NASA/TU Wien work verified the floating-point C code and proved absence of overflow (P) | We prove the geometric core and its implementation |

Encounter models: MIT Lincoln Laboratory's manned correlated and uncorrelated models are public
on GitHub (Airspace-Encounter-Models), reportedly BSD-2 licensed; confirm the licence file
before use.

### 3.4 Platform (M6, M9)

| Metric | Floor | Legacy | Notes |
|---|---|---|---|
| Availability, safety-critical services | 0.99999 (P: FAA NAS-RD-2013 excerpt), about 5.3 min a year | ERAM: outages LS-001 to LS-003 | Shown by fault injection and failover measurement, not asserted |
| Availability, essential services | 0.999 (same) | | |
| Failover | — | ERAM and STARS: not published | Target: no lost state, no gap in surveillance updates |
| Incident replays | — | — | Each incident in LEGACY_SHORTCOMINGS.md passes as a scenario |

## 4. Numbers still needed

| Need | Where | Next step |
|---|---|---|
| ESASSP Vol 1 numeric requirements | EUROCONTROL website, free | Fetch the PDF (small) |
| ERAM conflict probe trajectory accuracy | FAA Tech Center reports by Paglione and others; scans | Read the scanned PDFs page by page |
| TBFM ETA error by look-ahead | NASA ATD-2 material | Find a working copy; meanwhile measure it ourselves |
| OpenAP validation numbers | Sun, Hoekstra, Ellerbroek, Aerospace 2020 | Retry the publisher page |
| ESASSP 3 NM values | ESASSP Vol 1 | With the PDF |
| ERAM and STARS failover time | Not public | Outreach question |
| STCA warning time and alert-rate targets | EUROCONTROL-SPEC-122, GUID-123 | Fetch the current editions |
| FAA conflict alert and MSAW parameters | JO 7110.65, JO 7210.3 (public; faa.gov refused automated fetch) | Download by hand |
| ACAS X risk ratio, induced risk | MIT LL ATC-408 (a copy is already saved locally) | Read it |
