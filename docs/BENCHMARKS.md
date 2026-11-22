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

**Comparing systems that publish at different rates.** The per-report numbers (T1 to T4) count
each published sample once, so a system that reports often while an error lasts and rarely when
it is easy scores worse than one that reports evenly with the same error at every instant. Those
numbers stay as the native description of each system's own output and are labelled that way.
A claim that one system beats another (ERAM against OpenATCA, in particular) must cite the
common-time numbers instead: both systems scored at the same truth times, on the aircraft and
grid times where both have a report, with the per-aircraft distribution next to the total (M2
design, section 3). Availability and data age are reported separately and are not part of it.

## 2. Truth

- **ADS-B truth** for the recorded day comes from adsb.lol history (ODbL, redistributable with
  attribution and share-alike), later also from our own receiver. ADS-B is not perfect truth:
  we keep only reports whose NACp is 8 or better (estimated position uncertainty under 0.05 NM)
  and score against a smoothed reference built after the fact from the whole trajectory, never
  against single reports.
- **Truth and input share data.** OpenATCA's tracker takes in the ADS-B reports that the truth
  is built from, so a score against ADS-B truth cannot see error carried by the reports
  themselves (a GNSS position bias, a wrong time stamp): the tracker and the reference both
  inherit it. The tracker's own scoring splits each aircraft's reports in two, even-numbered to
  the tracker and odd-numbered to the truth, which removes the direct reuse but not the error
  the halves share; GNSS error is correlated over seconds and is in both. ERAM fuses ADS-B with
  radar where it has both, so less of its error is shared with the truth, and a comparison
  against an ADS-B-only OpenATCA run favours OpenATCA by that difference. What the score
  measures is disagreement with a reference whose error is correlated with the tracker's and
  whose absolute bias is unknown. It is not a bound on the tracker's true error in either
  direction: the measured mean square is the tracker's plus the reference's, minus twice the
  covariance of their errors, so shared error lowers it and opposing error raises it. Every
  absolute accuracy number of OpenATCA against ADS-B truth is therefore labelled as
  disagreement with ADS-B truth, and every ERAM comparison states which inputs each side had.
  The checks with known error are the simulated sensors below, where the true position is
  given; radar-only runs scored against ADS-B truth share less error with the reference.
- **Event truth** (meter-fix crossing times, runway threshold times, top of climb, top of
  descent) is derived from the same smoothed reference.
- **Synthetic truth** for things real traffic does not contain often enough (near mid-air
  collisions, spoofing, sensor faults) comes from encounter models and injected faults, labelled
  as synthetic in every report.

## 3. Metrics and current numbers

**Source marks.** P is a primary source that was read; S is secondary; U is unverified. A dash
means not found yet.

### 3.1 Surveillance tracking (M3)

Floor: EUROCONTROL ESASSP, EUROCONTROL-SPEC-0147 Edition 1.3 (21 March 2024), Volume 1, Table 3
(5 NM) and Table 4 (3 NM), cooperative surveillance (P, read 2026-11-14; copy in
`openatca-data/raw/standards/esassp-vol1/`). We target the 3 NM Recommended column throughout,
since terminal airspace needs it and en route benefits from it.

| Metric (ESASSP id) | 5 NM Mand. / Rec. | **3 NM Mand. / Rec. (our floor)** | Legacy (M2) | Best published |
|---|---|---|---|---|
| Measurement interval (R1) | ≤8 s / ≤6 s | ≤5 s / **≤4 s** | SFDPS publication interval only | — |
| Probability of update, data age ≤7 s (R2) | ≥97% per flight and global / ≥99% global | ≥97% per flight / **≥99% global** | not measurable from SFDPS | — |
| Missed positions in long gaps (R3) | ≤0.5% | **≤0.5%** | — | — |
| Horizontal position RMS (R4) | ≤500 m / ≤350 m global | ≤300 m / **≤210 m global, <230 m per flight** | ERAM published track vs truth | IMM −59% vs CV Kalman (U, no absolute value) |
| Correlated errors, 3+ reports (R5) | — / ≤0.03% over 926 m | — / **≤0.03% over 555 m** | ERAM | — |
| Relative time of applicability, nearby aircraft (R6) | — / ≤0.3 s RMS | — / **≤0.3 s RMS within 6 NM** | — | — |
| Pressure altitude: update ≥96% (R7), mean age ≤4 s (R8), incorrect ≤0.1% (R10), error ≤200/300 ft in 99.9% level, ≤300 ft in 98.5% climbing (R11) | same | **same** | ERAM vs ADS-B barometric | — |
| Emergency/SPI change delay (R12), identity change delay (R13) | ≤12 s, ≤24 s | **≤7.5 s, ≤15 s** | — | — |
| Identity correct (R14), incorrect identity (R15) | ≥98%, ≤0.1% | **>98% per flight, ≤0.1%** | ERAM association errors | — |
| Vertical rate RMS (R16) | ≤250 ft/min level, ≤500 climbing | **same** | ERAM | — |
| Speed RMS (R17) | ≤6 / ≤4 m/s straight; ≤11 / ≤8 turning | **≤4 m/s straight, ≤8 m/s turning** | ERAM track velocity | IMM −77% vs CV Kalman (U) |
| Heading RMS (R18) | ≤10° straight, ≤25° turning | **same** | ERAM | — |
| False target density (R19) | — / <10 per 900 NM² over 450 intervals | — / **≤2 per 100 NM² over 720 intervals** | — | — |
| Falsely confirmed tracks near true tracks (R20) | — / ≤2 per hour within 7 NM | — / **≤1 per hour within 9 NM**; our target zero on the recorded day | ERAM swaps (T9) | — |
| Continuity (R21) | — / ≤2.5×10⁻⁵ critical failures per hour | **same** | — | — |
| Spoof detection | — | — | unknown | ≥95% detection (NDSS 2021); false positives 0 to 1.06×10⁻³ (LoVe 2023) (S, abstracts) |
| Capacity | — | — | ERAM: 1,900 tracks, 64 radars per centre (S) | ours: all 20 centres in real time on one machine; measured 7,424 confirmed tracks at about 2 % of one core (below) |

**National load, measured 20 November 2026** (`oa-track-run --realtime`, one thread, on the
development laptop). Input: one hour of the whole contiguous US from adsb.lol, 17 November 2026
21:00 to 22:00Z, the busiest hour of that day: 4,505,161 reports from 13,542 aircraft, of which
4,007,539 passed the quality filter, 1,113 a second on average and 1,591 at most. At the busiest
second the tracker held 7,424 confirmed and 1,436 tentative tracks, about four times ERAM's limit
for one centre, with the whole country's ADS-B traffic in one process.

| Measure | Rows discarded (2 runs) | Rows kept (2 runs) |
|---|---|---|
| Compute per second of traffic, p50 | 21.2 to 21.4 ms | 19.1 to 21.0 ms |
| p99 | 24.3 to 24.9 ms | 32.5 to 38.4 ms |
| Slowest second | 29 to 34 ms | 308 to 636 ms |
| Share of one core, updates only | 2.09 % | 2.03 to 2.23 % |
| Share of one core, with reading and decoding | 2.20 to 2.22 % | 2.43 to 2.66 % |
| Seconds that took longer than a second | 0 | 0 |

Tracker tables: 138.5 MiB for 65,536 track slots; process peak 0.69 GB, most of it the decoded
clip. Run-to-run spread is small; the slowest seconds with rows kept come from writing about 30
million output rows, not from tracking. The machine was not quiet (builds and an emulator were
running), which raises the figures; a quiet re-run on the landed build is owed. The input was
decoded, sorted and held in memory before timing, and processed on one thread: a live feed adds
network input, decoding and interrupts, which these figures do not include. Each second's reports
are timed as one batch processed after the second ends. The clip is
ADS-B only: radar, which ERAM also tracks, is not in it.

ESASSP notes: any flight failing R2, R4, R12 or R13 must be investigated (R22). Track initiation
has no numeric requirement ("verified by design"). A valid sample is at least 36,000 measurement
intervals; trajectories under 50 intervals may be excluded (§4.1.3.3.1). Non-cooperative
(primary radar) floors are lower (3 NM: interval ≤5 s, update >90%, RMS ≤300 m, Annex D).

FAA separation context (JO 7110.65BB §5-5-4): ERAM 5 NM below FL600, 3 NM up to FL230 within
40 NM of the preferred radar (60 NM with MSSR) or in track-based mode; ADS-B "allows expanded use
of 3 NM". JO 7210.3EE §3-6: WAM targets may not be used for 3 NM in ERAM.

#### Simulated sensor inputs (M3, `oa-sensorsim`)

Error models for simulated radar and multilateration (docs/M3_DESIGN.md §1). These are inputs to
the simulation, not bars. Most are stated requirements or assumptions, not measurements; each
simulation run records which row it used. Grades as in §2; "P (quoting X)" means read in the
cited document, which itself quotes X.

| Parameter | Value | Source | Grade |
|---|---|---|---|
| MSSR random error, range / azimuth (σ) | 70 m / 0.08° | NLR-CR-2004-472, *Wide Area Multilateration* v1.1 (EATMP TRS 131/04), §7.1 Table 4, quoting EUROCONTROL SUR.ET1.ST01.1000-STD-01-01 | P (quoting the 1997 standard) |
| Mode S sensor (FAA test assumption), range / azimuth (σ) | 7.6 m (25 ft) / 1 mrad (0.057°) | MIT LL ATC-302, Gertz and Kaminsky 2002, §4.1 | P (assumed, not measured) |
| Older SSR CD-2 (FAA test assumption), range / azimuth (σ) | 30.5 m (100 ft), 1/8 NM quantum / 3 mrad | MIT LL ATC-302 §4.1 | P (assumed) |
| ASR-9 primary radar, azimuth accuracy; range | 0.16° rms; 1/32 NM with 200 ft rms | Stone and Anderson, *Advances in Primary-Radar Technology*, Lincoln Laboratory Journal 2(3), Table 1 | P (requirement) |
| Scan period, terminal / en route | 4.8 s / 12 s | Stone and Anderson Table 1; ATC-302 §2 | P |
| SSR detection; false target reports; split reports; Mode A / Mode C code detection | >97%; <0.1%; <0.3%; >98% / >96% | ICAO SURICG/6 IP/04 (ROK, 2021) Table 1, quoting the 1997 EUROCONTROL standard | S |
| PSR detection; false reports | >90%; <20 per scan | same | S |
| Altitude resolution, Mode C / Mode S | 100 ft / 25 ft | NLR-CR-2004-472 §3 | P |
| WAM position accuracy (measured, 5 receivers, RTK reference) | about 6–7 m RMS (ADS-B about 20 m, Mode S radar about 200 m in the same test) | ICAO SURICG/7 IP14 (China, 2022) §2.3 | P |

Gaps: radar range and azimuth **bias** before registration (ATC-302 says azimuth bias dominated
real registration error but gives no usable value), time-stamp error, the primary 1997 standard
and ESASSP annex sensor tables, FAA ASR-11 / ARSR-4 / Mode S specifications, and any published
absolute accuracy of ARTAS or another tracker against ESASSP. Until a bias value is sourced,
simulations sweep bias over a stated range and report results per value rather than pick one.

### 3.2 Flight data and trajectory prediction (M4)

| Metric | Floor | Legacy (to measure) | Best published | Notes |
|---|---|---|---|---|
| Meter-fix crossing time error | — | TBFM on our recording. Published: undelayed delivery mean +11 s, std 75.5 s at Denver, Aug 2013; std 40.6–129.1 s across four airports (P: NASA/AIAA 2015-3182) | — | We measure TBFM's own estimates against actual crossings, by look-ahead |
| Top of climb / bottom of descent time error | — | — | 5.2 s climb, 6.6 s descent mean absolute error; frozen BADA 95.6 s / 112.2 s (P: arXiv 2601.03075, UK radar, Sept 2019) | Event-based, not fixed look-ahead |
| Same, distance error | — | — | 3.9 nmi climb, 4.3 nmi descent (same paper) | |
| Along-track, cross-track and vertical error at 5, 10 and 20 minutes | — | ERAM conflict probe: published Tech Center studies exist (Paglione et al.) but are scans without a text layer | — | Read the scans; until then legacy is measured from TFMS and TBFM estimates only |
| Route conversion agreement | — | ERAM's converted route in SFDPS | — | Every disagreement explained |
| Conflict probe missed and false alerts | ERAM factory acceptance limits: aircraft-to-aircraft missed alert rate 0.025; aircraft-to-airspace 0.02; warning time 854 s for conflicts over 10 min out, 104 s under (P: Vivona, Paglione, Cate, Enea, AIAA, Table 2; ERAM met them by November 2008). ERAM's basic function requires at least 75 s warning (P: Paglione, Oaks, Ryan) | Probe in Chicago Center, Tech Center study: hit rate 0.939, false alarm rate 0.126, positive predictive value 0.120 (baseline) (P: Rein et al., DOT/FAA/TC-TN-12/65, Tables 2–4) | MTCD about 0.8% missed, 3.6% false (U) | ZAU numbers are on our own centre: a direct target |

### 3.3 Safety alerts (M5)

The method is the one ACAS X used to replace TCAS II: Monte Carlo runs over published encounter
models, scored by risk ratio, alert rate and missed alerts against a baseline run of the legacy
logic, plus recorded traffic.

**Floor: EUROCONTROL safety-net guidelines** (GUID-159 STCA, GUID-160 MSAW, GUID-161 APW,
GUID-162 APM, 18 January 2017; Part II performance table, identical for all four; P). Encounters
are classed 1 (alert necessary), 2 (desirable), 3 (unnecessary but understandable), 4
(undesirable), 5 (void). We target the **Preferred** column:

| Indicator | Required | **Preferred (our floor)** |
|---|---|---|
| Category 1 encounters alerted | ≥95% | **100%** |
| Category 2 encounters alerted | ≥80% | **≥90%** |
| Alerts that are category 3, 4 or 5 | ≤75% | **≤50%** |
| Category 3 encounters alerted | — | **≤30%** |
| Category 4 encounters alerted | — | **≤1%** |
| Cat 1/2 with adequate time available but less warning given | ≤45% | **≤35%** |
| Mean warning time where adequate time exists | ≥90% of adequate | **≥95%** |
| Mean warning time where it does not | ≥70% of objective | **≥75%** |

Parameters and test scenarios (GUID-159/160 Part III): STCA warning time up to 2 min; converging
in both planes, alert at least 80 s before collision (ideally 90 s); same-level crossing at least
50 s (ideally over 60 s). MSAW with digital terrain: prediction 40–60 s, warning 30–50 s,
imminent 20–30 s; obstacles prediction 50–60 s, imminent 20–30 s; minimum terrain clearance
300–1,000 ft; test scenarios alert at least 10–20 s before infringement.

FAA orders (JO 7110.65BB, 7210.3EE, changes to 9 July 2026) give **no** numeric conflict alert or
MSAW parameters; they refer to the ERAM Site Adaptation Manual and the NAS Configuration
Management Document, which are not public. Facility managers may inhibit alerts and must
"minimize" nuisance alarms (7210.3 §12-6-7). So the legacy alerting parameters are an outreach
item; legacy alert *behaviour* we can only observe where it is reported (LEGACY_SHORTCOMINGS).

**Best published (collision-avoidance analogue):** ACAS X vs TCAS II over 5×10⁵ encounters from
the US encounter model (MIT LL, Asmar and Kochenderfer 2013, Table 5; P): risk ratio 3.26×10⁻³ vs
8.81×10⁻³ with alert probability 0.284 vs 0.532 — 63% less risk with 47% fewer alerts. That is
the shape of result we want for ground safety nets: lower risk *and* fewer alerts, both measured.

**Formal proof:** DAIDALUS (DO-365 reference) algorithms proved in PVS, and its floating-point C
code verified free of overflow (NASA/TU Wien 2023; P). None published for ERAM or STARS.

Encounter models: MIT LL em-model-manned-bayes v2.1.0, BSD-2-Clause, "Distribution Statement A,
approved for public release" (fetched 2026-11-14).

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
| ERAM and STARS conflict alert and MSAW parameters | ERAM Site Adaptation Manual, NAS Configuration Management Document (not public) | Outreach |
| ERAM conflict probe trajectory error by look-ahead | Tech Center reports; data in figures only (ICAS 2002 R45) | Measure legacy estimates ourselves in M2 |
| TBFM ETA error by look-ahead | NASA ATD-2 material | Measure ourselves in M2 |
| OpenAP validation numbers | Sun, Hoekstra, Ellerbroek, Aerospace 2020 | Retry the publisher page |
| ERAM and STARS failover time | Not public | Outreach |
| Interfacility message set (ERAM to adjacent centres) | NAM Common Coordination ICD Rev E (NAS-IC-21009205); the public ERAM–non-US ICD covers only transport: TCP over VPN, 4-byte hex length header, ≤1,800 bytes, 95th-percentile latency ≤225 ms, recovery within 60 s, availability goal 0.9999 | Look for a public copy of Rev E |

## 5. Synthetic ADS-B dropout continuity run

`oa-perf one --adsb-dropout S` is an opt-in synthetic stress run. It writes a steady-feed control
and an outage run with the same seed, radar schedule, and confirmed address-bearing cutover cohort.
The outage withholds ADS-B in measured seconds `[warmup, warmup + S)`; radar plots continue, and
ADS-B resumes after the interval. Both runs disable early stopping and must complete the full
measured duration or the comparison is rejected. Output-overflow seconds have an uncertain
cohort count. The count covers only the confirmed, known-address ADS-B cohort live at cutover;
it counts non-coasting tracker rows and is a continuity proxy, not a sensor-freshness score.
Anonymous radar tracks and reacquisition timing are not scored by this mode.

For example, `oa-perf one --n 8192 --warmup 60 --duration 15 --truth-score` enables the fusion
assignment log (bounded at 4,194,304 rows) and
scores identity only for confirmed tracks live at the final run time. It reuses the scorer's
assignment-label majority rule, with cumulative assignment evidence from warm-up and measurement
filtered to those final keys. The denominator is distinct known truth aircraft offered during
measurement; duplicate, false-track, false-assignment and swap counts are reported separately.
The first confirmed appearance distribution samples the tracker once per simulated second from
run start and covers the primary keys matched at the final snapshot.
The log is opt-in because it adds memory and timing overhead. Truncated runs, unknown offered
labels, incomplete assignment columns or log overflow produce an explicit unavailable result.
This is a final-snapshot identity diagnostic, not a trajectory-accuracy score; it cannot report
anonymous tracks that ended before the snapshot. It currently cannot be combined with
`--adsb-dropout`.

Add `--initiation-diagnostics` to see how each radar used the shared initiation budget during
measurement. Each radar reports the work available at the start of its batches, charged work,
requested and denied retirement reservations, exact pair tests, capped batches, and refused plots.
Add `--association-diagnostics` to compare the anonymous plots each radar received, how many
completed candidate gating, and the coarse candidate visits and exact checks they required.

Run this radar-only cold start on both revisions with identical traffic settings. The three radars
cover the same stress area. The score measures identities at 75 seconds of simulated traffic.

```sh
cargo +1.89.0 run --release --locked -p oa-perf -- one \
  --n 8192 --layout stress --radars e,t,t --adsb-frac 0 --seed 20261123 \
  --warmup 60 --duration 15 --no-early-stop --max-tracks 16384 --max-wall 120 \
  --truth-score --initiation-diagnostics --out /tmp/oa-radar-cold-start
```
