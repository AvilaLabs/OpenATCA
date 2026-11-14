# Documented shortcomings of ERAM and STARS

Status: first pass, 2026-11-14. Every entry comes from a public source and becomes a requirement
or a test scenario. Entries are written as what went wrong, not as blame. Our own findings in
FAA data are kept separately and are not part of this file.

Source quality is marked: **P** primary (the report or order itself was read), **S** secondary
(press or a summary of the primary), **U** unverified (search summary only; confirm before citing
outside this repository).

## Outages and resilience

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-001 | ERAM | Los Angeles Center, 2014-04-30. A U-2 flight plan with no filed altitude (controller entered about 60,000 ft) made conflict processing consider all altitudes over a route crossing the centre several times. Memory ran out and the system entered a reboot loop; ground stop up to about 2.5 h. Fix was a mandatory altitude and more memory | S: IEEE Spectrum; The Register 2014-05-12 | Incident replay IR-01 (M6). Every input has a stated work bound (INTERFACES IF-R1) |
| LS-002 | ERAM | East Coast, August 2015. Data deleted by controllers stayed in memory until storage filled; tied to a new custom data-window function, which was suspended. About 492 delays and 476 cancellations | S: Computerworld | IR-02. No allocation after init in the core (RULESET C); every store has a fixed capacity and a test that fills it |
| LS-003 | ERAM | Dallas–Fort Worth, 2025-09-17, about 7 h. A software update failed, the backup also failed, and a network switch failure slowed recovery; facilities went to manual procedures | S: VisaVerge, citing FAA | IR-03. Updates are staged on one replica with automatic rollback; the backup must not share the update until it has run clean (IF-R3) |
| LS-004 | STARS / telecom | Philadelphia TRACON (Newark sectors), 2025. Radar and frequencies lost; a high-latency telecom line carried the remote radar feed with no independent second path. Controllers: "We have no faith in our equipment" | S: AviationPros; The Air Current (paywalled, U) | IR-04. Feed latency is measured and alarmed (IF-R4, IF-R6); each facility can run on surveillance independent of the backbone (SCOPE 2a) |
| LS-005 | ERAM | When all three ERAM channels fail, the fallback is DARC, and controllers reported they were not current on DARC or EBUS | P: NATCA ATSAP report Oct–Dec 2015 | Degraded modes use the same display and commands as normal mode, and are exercised in training scenarios |
| LS-006 | ERAM | Post-deployment failures "raised questions about its reliability and security" | U: DOT OIG 2017 summary | Availability target and fault injection (M6) |
| LS-007 | NATS (UK, not FAA) | 2023-08-28. A flight plan with two waypoints sharing a name failed flight plan processing; the backup received the same plan and failed the same way | S: CAA review | IR-05. Poison-message quarantine (IF-R2); duplicate-name resolution tested in route conversion (M4) |
| LS-008 | FAA NAS | 2026-11-13 nationwide ground stop after communications failure across several en route centres. Officials' account: new equipment pushed more data into old routers | S: press, root cause not published | IR-06 (M6) |

## Surveillance and tracking

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-101 | STARS | Detroit, 2004: departures got wrong data tags or none, tags drifted from the aircraft, some aircraft not tracked until several miles out; small targets hard to see | S: AvWeb (old, before later STARS builds) | Track initiation delay and tag-association metrics (M3) |
| LS-102 | STARS | "Track jump anomalies, where tags sometimes jumped to other aircraft" (El Paso, 2002) | U: Route Fifty | Track swap rate, measured and reported (M3) |
| LS-103 | STARS | 2016: a tool deployed to analyse STARS facilities "where radar jumping and ghost targets have been problematic" | U: NATCA update 2016-02-19 | False-track density near true tracks (ESASSP I-18) (M3) |
| LS-104 | ERAM | DOT OIG AV-2013-119: "some key capabilities, such as ERAM's tracking software, are experiencing issues" | U: OIG snippet | ERAM track error measured against ADS-B truth on our recording (M2) |

## Flight data

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-201 | ERAM | Aircraft flying routes other than the one ERAM showed: embedded route text and route action notifications did not reach the right sectors, and there was "no national direction" on handling | P: NATCA ATSAP report Oct–Dec 2015 | Route conformance monitoring and route-change delivery to every affected sector (M4) |
| LS-202 | ERAM | Pre-departure reroute function disabled after a flight-plan amendment anomaly at Washington Center | U: ATO alert 2018-02-22 | Amendment property tests: any amendment sequence leaves a valid plan (M4) |
| LS-203 | ERAM | Extensive software problems delayed deployment and added hundreds of millions of dollars; known problems not resolved before features were added | S: DOT OIG 2012 via summary | Defects block new features (process rule) |
| LS-204 | Adjacent | Detroit pre-departure clearance software could not flag multiple flight plans filed for the same flight | S: PBS NewsHour | Duplicate-plan detection (M4) |

## Safety alerts

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-301 | Not stated | NTSB recommendation to "redesign minimum safe altitude warning and conflict-alert systems for greater reliability in capturing controller attention and reducing false alarms" | S: AvWeb; NTSB recommendation number to be found | Missed and nuisance alert rates both reported (M5); alert presentation evaluated with controllers (M8) |
| LS-302 | ERAM | FAA human-factors study: controllers selected probe notifications from the data block more often than from the alert list | P: FAA 2012, ROSA P dot/71726 | Alerts shown on the data block first (M8) |

## Display and workflow

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-401 | STARS | JO 7110.65 allows track suspend when "data block overlap in holding patterns or in proximity of the final approach create an unworkable situation" | P: JO 7110.65 ch. 5 §14 | Automatic data block placement with no overlap in holding and on final, tested on recorded traffic (M8) |
| LS-402 | STARS | Weather display settings returned to defaults after daily maintenance; the fix was a checklist | P: NATCA ATSAP report Oct–Dec 2015 | Display settings persist per position and are never reset by maintenance (M8) |
| LS-403 | ERAM | Fallback modes untrained (see LS-005) | P | as LS-005 |

## Sustainment

| ID | System | What happened | Source | Becomes |
|---|---|---|---|---|
| LS-501 | ERAM | Started 2004, delivered about 10 years later "with outdated technology that needed a major technology refreshment within 5 years" | P: GAO-24-107001 | Hardware-independent core on ordinary machines; minimal dependencies (SCOPE 6a) |
| LS-502 | NAS | 51 of 138 FAA systems (37%) unsustainable and 54 (39%) potentially unsustainable: parts unavailable, technicians retiring | P: GAO-24-107001 | Same |
| LS-503 | NAS | Funding leaves the FAA in "fix-on-fail" | S: NATCA via MeriTalk | Same |

## Not yet collected

- **Controllers' everyday complaints.** Forums and the NASA ASRS database could not be read by
  the research tools, so display lag, clutter, keyboard burden, nuisance conflict alerts, ADS-B
  and radar mismatch and ERAM freezes are not documented here yet. Next step: search ASRS
  directly (public web form) for narratives naming ERAM and STARS, and read NATCA's ATSAP
  quarterly reports in full. Until then the M8 display work cannot claim to fix those
  complaints.
- **NTSB investigations naming ERAM or STARS functions.** None found yet. Next step: NTSB CAROL
  search for "conflict alert", "MSAW", "data block".
- **DOT OIG 2017 ERAM audit and the FAA 2024 safety review team report.** Not read yet.
