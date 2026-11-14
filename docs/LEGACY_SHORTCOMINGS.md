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

## What controllers report: NASA ASRS

Searched 2026-11-14 in ASRS Database Online: narrative or synopsis containing "ERAM" returns 167
reports (2008–2026), 139 of them from en route controllers. Theme counts below are reports whose
narrative matches the theme; one report can count under several. **Limits:** ASRS reports are
voluntary, so counts show what controllers chose to report, not how often things happen. 120 of
the 167 date from 2010–2014, the ERAM transition, so many describe builds that have since been
fixed; later reports often spell out "En Route Automation Modernization" and are partly missed
by this search. The themes are still the clearest public record of how ERAM fails the people
using it.

| ID | Theme | Reports | Representative report (ACN, year-month) | Becomes |
|---|---|---|---|---|
| LS-601 | Handoffs and point-outs that silently fail or go to the wrong sector; no auto handoff to approach when an interim altitude is set | 70 | 933380 (2011-02): "ERAM does not allow auto hand offs to approach controls when using Interim altitudes"; 1434518 (2017-03): controller expected ERAM to route the handoff by interim altitude; 1224571 (2014-12): receiving controller did not see the handoff | Handoff state machine with every transition acknowledged and visible to both sides; handoff target derived from the trajectory, not the current altitude alone (M4); missed handoff alerted before the boundary |
| LS-602 | Flight plans dropped, duplicated or stale; route amendments not reaching the next facility or the tower | 79 | 940920 (2011-03): "ERAM removed the flight plan from the system; even though everything appeared normal from our end"; 878728 (2010-03): two plans stored, one from the previous day; 1288294 (2015-08): amended plan never reached the destination tower, aircraft declared overdue | No silent removal: every plan removal has a cause and a log entry; duplicate and stale plans detected; amendment delivery confirmed to every affected position (M4) |
| LS-603 | Data blocks: wrong aircraft correlated, two positions able to change the same block, block removed while handoff showed pending | 63 | 974724 (2011-10): incorrect data block correlations; 979518 (2011-11): dual control of data blocks; 1085727 (2013-05): adjacent controller amended a block not under their control | One owner per data block field at any moment, enforced and shown (M4, M8) |
| LS-604 | Track quality: tracks jumping to other targets, false targets, a target in conflict with itself, coast tracks in the backup while primary tracked | 32 | 874931 (2010-02): "extra targets and track jumping to other beacon targets"; 974713 (2011-10): "ERAM incorrectly thinks the aircraft should be in a different position"; 878377 (2010-03): target conflicted with itself; 980792 (2011-11): backup showed coast while both primary channels tracked | Track swap, false track and coast metrics (M3); backup tracker validated against primary continuously (M6) |
| LS-605 | Conflict alert: nuisance alerts in restricted airspace that cannot be suppressed; alerts after the aircraft had already passed; reported as worse than the Host it replaced | 17 | 874942 (2010-02): unable to suppress alerts for fighters in special use airspace; 1015566 (2012-06): alert "after the two aircraft were all ready passed"; 1116751 (2013-09): alert "activates all too often when separation has been established"; 1160384 (2014-03) | Nuisance, late and missed alerts all measured (M5); airspace-aware suppression with the suppression itself visible |
| LS-606 | Low-altitude warning: false alert from trajectory-based logic; missed alert below the minimum vectoring altitude; altimeter defaulting to 29.92 shifting altitudes by 500 ft | 10 | 965462 (2011-08): false MSAW, reporter surmised it used trajectory not position; 919684 (2010-11): E/MSAW "never activated an alert"; 946886 (2011-05): altimeter default | Terrain warning logic published and tested on every recorded approach; altimeter source always shown (M5) |
| LS-607 | Special use airspace status wrong or missing: shown hot without altitudes, not shown until active, not updated in the probe | 22 | 1040449 (2012-10): "showing Jackal hot but there was no displayed altitude"; 1019165 (2012-06): restricted airspace should show ten minutes before activation; 1256321 (2015-04): notified it had been hot for 15 minutes | Airspace status from NOTAMs and schedules with advance display and alerting (M4, M5, M8) |
| LS-608 | Conflict probe (EDST/URET) not giving alerts controllers expected; restricted areas not updated in it; coordination one item at a time | 30 | 965470 (2011-08): "presented as though it will give these alerts and it doesn't"; 919434 (2010-11) | Probe and alerts share one trajectory and one airspace state (SCOPE 2a, M4) |
| LS-609 | Beacon codes: wrong aircraft tagged by code; codes reassigned between facilities; duplicates tagging up hours later | 18 | 1322299 (2016-01): system tagged a flight 200 miles away by code, instructions issued to the wrong aircraft; 1222104 (2014-11): duplicate codes tag up later in the day | Plan–track correlation never by code alone; code conflicts flagged (M3) |
| LS-610 | Interfacility mismatch (ERAM vs Host, ERAM vs STARS, Canada): routes, codes and plans disagreeing across the boundary | 41 | 1282432 (2015-07): ERAM and STARS "not interacting correctly"; 1089579 (2013-05): no data on an aircraft arriving from Canadian airspace | Interface comparator in shadow mode (M7); IF-P2 emulators |
| LS-611 | Testing and software updates on live traffic: anomalies resurfacing after rebuilds; controllers asking for ERAM to be turned off until fixed | 19 | 1068180 (2013-02): problems "resurfaced after recent software re-build"; 1059605 (2013-01), 879026 (2010-03), 942409 (2011-04): take ERAM off line until fixed | Updates proven in replay and shadow before reaching a position (M6, M7); regression tests from every reported anomaly |
| LS-612 | Backup modes: degraded backup display showing a target the primary did not; controllers forced to switch between systems; complex traffic under backup | 14 | 909444 (2010-09): backup "superior to the primary" for one target; 1028280 (2012-08): busy traffic during backup operations | as LS-005 |
| LS-613 | Workload and entries: unfamiliar entries, responses lost in clutter, too many updates pushed to one position | 46 | 877665 (2010-03): response "cluttered around various other text boxes"; 1188425 (2014-07): system "continually sends out updated information that burdens one Controller" | Entry responses at a fixed place; update rate to each position bounded and prioritised (M8) |

Search method for repeating this: ASRS Database Online, Text contains `ERAM`, narrative and
synopsis, exported as CSV; each theme is a case-insensitive keyword match over the narrative
only (for example LS-601: "hand off", "handoff", "auto-pop", "point out"), and every count was
checked by reading sample narratives.

## Not yet collected

- **STARS reports in ASRS.** The STARS search returns 920 reports, many of them about STAR
  arrival procedures rather than the STARS system; export and filtering in progress.
- **Controller forums and NATCA ATSAP quarterly reports.** Not readable by the research tools;
  ATSAP reports can be read by hand.
- **NTSB investigations naming ERAM or STARS functions.** None found yet. Next step: NTSB CAROL
  search for "conflict alert", "MSAW", "data block".
- **DOT OIG 2017 ERAM audit and the FAA 2024 safety review team report.** Not read yet.
