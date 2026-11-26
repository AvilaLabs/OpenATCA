# Demo fidelity reference

**Status:** engineering reference for a prototype. It is not an ERAM or STARS interface
specification, a controller-approved replica, or evidence of operational suitability. No ERAM
controller manual or STARS TCW/TDW Operator Manual was available for this review. Do not turn an
unverified detail below into controller-facing syntax, fields, colors, or response text.

This table traces the requested early demo scope to source evidence. `PRIMARY-FAA` means FAA policy,
order, or Technical Center report; it does not mean that the source is a system specification.
`COMMUNITY` identifies emulator documentation and is not authoritative for deployed ERAM/STARS.
`INFERRED` is a design hypothesis and must not appear as a claimed real-system behavior. Demo status
describes the current OpenATCA capability, not a judgment about real-system fidelity.

## ERAM command-frequency baseline: top 30

The source is DOT/FAA/TC-TN06/04, *En Route Air Traffic Controller Commands: Frequency of Use
During Routine Operations* (May 2006). Its Table 3 contains the ten most frequent HCS/DSR/URET
functions; Appendix A, Table A-1 lists the top thirty. The sample is an 11.4-hour ZDC recording
from March 2005, not an ERAM command census or a current-use ranking. The report says the top thirty
account for 95.0% of its observed entries and the top ten for 70.3%. This is a prioritization
baseline only. The report does not establish current ERAM command grammar or exact response text.

For rows `ERAM-CMD-01` through `ERAM-CMD-30`, the source/confidence is PRIMARY-FAA, legacy HCS,
DSR, and URET task-frequency evidence. Exact deployed ERAM syntax remains unverified. The current
prototype has no claim of implementing these exact commands.

| Row | Legacy task, as reported | Share | Source / confidence | Demo status |
|---|---|---:|---|---|
| ERAM-CMD-01 | Offset data block | 19.8% | TC-TN06/04 Table 3; PRIMARY-FAA, legacy | Frequency priority only; exact ERAM action/grammar unverified |
| ERAM-CMD-02 | Implied aircraft selection (accept handoff / force data block) | 16.4% | Table 3; PRIMARY-FAA, legacy | Frequency priority only; selection and handoff semantics need current manual/controller review |
| ERAM-CMD-03 | Initiate handoff | 7.6% | Table 3; PRIMARY-FAA, legacy | Priority; current ERAM grammar and response unverified |
| ERAM-CMD-04 | Assign interim altitude | 5.5% | Table 3; PRIMARY-FAA, legacy | Priority; JO procedure supports the task, not input syntax |
| ERAM-CMD-05 | Adjust vector line | 4.8% | Table 3; PRIMARY-FAA, legacy | Priority; exact controls/steps unverified |
| ERAM-CMD-06 | Route display | 3.7% | Table 3; PRIMARY-FAA, legacy | Priority; current ERAM display behavior unverified |
| ERAM-CMD-07 | Delete aircraft | 3.5% | Table 3; PRIMARY-FAA, legacy | Priority; authority, safeguards, and syntax unverified |
| ERAM-CMD-08 | Toggle bookkeeping checkmark | 3.4% | Table 3; PRIMARY-FAA, legacy | Priority; current ERAM mapping unverified |
| ERAM-CMD-09 | Quicklook | 2.9% | Table 3; PRIMARY-FAA, legacy | Priority; current ERAM meaning/display unverified |
| ERAM-CMD-10 | Flight-plan readout request | 2.8% | Table 3; PRIMARY-FAA, legacy | Priority; report separately names QF only through community documentation |
| ERAM-CMD-11 | Clean up display or list | 2.3% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; control and side effects unverified |
| ERAM-CMD-12 | Show/hide view, window, toolbar, or area | 2.3% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; exact items and behavior unverified |
| ERAM-CMD-13 | Display/inhibit halo | 1.9% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; exact meaning and scope unverified |
| ERAM-CMD-14 | Adjust range | 1.8% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; controls unverified |
| ERAM-CMD-15 | Request/suppress data block | 1.7% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; distinguish display request from alert suppression; mapping unverified |
| ERAM-CMD-16 | Remove interim altitude | 1.5% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; current grammar unverified |
| ERAM-CMD-17 | Assigned altitude | 1.4% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; task only; exact command unverified |
| ERAM-CMD-18 | Full data block speed, heading, or free-text amendment | 1.4% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; field semantics and syntax unverified |
| ERAM-CMD-19 | Point out | 1.4% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; JO defines coordination task, not UI workflow |
| ERAM-CMD-20 | Undetermined entry (typically errors) | 1.3% | Appendix A, Table A-1 and note; PRIMARY-FAA, legacy | Do not emulate as a command; source class is mostly rejected/uncategorized input |
| ERAM-CMD-21 | Cursor home | 1.2% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; key/control mapping unverified |
| ERAM-CMD-22 | AM amendment | 1.1% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; expansion and current ERAM behavior unverified |
| ERAM-CMD-23 | Combined toggle filter | 0.9% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; exact filters and combined action unverified |
| ERAM-CMD-24 | Adjust console attribute | 0.8% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; attribute set and controls unverified |
| ERAM-CMD-25 | Toggle aircraft from special-attention area | 0.7% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; area semantics and UI unverified |
| ERAM-CMD-26 | Toggle special coding | 0.7% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; coding definitions/UI unverified |
| ERAM-CMD-27 | Continuous range readout | 0.6% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; control and readout format unverified |
| ERAM-CMD-28 | Adjust background color | 0.6% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; available settings and interaction unverified |
| ERAM-CMD-29 | Set background color | 0.5% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; exact settings and interaction unverified |
| ERAM-CMD-30 | Track reroute | 0.5% | Appendix A, Table A-1; PRIMARY-FAA, legacy | Priority; current ERAM grammar and resulting route view unverified |

The report's Table 4 is also useful for interaction planning: in that legacy sample, offset data
block, implied aircraft selection, and initiate handoff used CID 60.8%, 54.8%, and 70.0% of the
time, respectively, and trackball 38.9%, 44.2%, and 29.6%. It describes CID as a three-digit code
in that system and provides historical syntax examples. Those values do not establish current
ERAM CID length or behavior. A separate vNAS/CRC community reference describes CID as three
characters, a broader FLID set, and left/middle mouse target selection; vice labels the buttons
TBP/TBE. These are COMMUNITY observations about those clients, not FAA evidence. No CID,
beacon-code, callsign, or trackball input form should be presented as ERAM-exact until checked
against the ERAM manual and working controllers.

The following crosswalk is useful to the prototype team, but every cited form is **COMMUNITY**:
vNAS CRC documentation retrieved 25 Nov 2026, with no release version stated. It describes CRC as
a close simulation with explicit “VATSIMisms”; this is not a substitute for current ERAM manuals.

| Row | vNAS/CRC documented example or mapping | Confidence / demo status |
|---|---|---|
| ERAM-CMD-01 | Implied `<1-9> <FLID>` positions a block and `/<0-3> <FLID>` changes leader length. The source does not prove these equal the legacy “Offset Datablock” category. | COMMUNITY; related interaction only, equivalence unverified |
| ERAM-CMD-02 | `<FLID>` accepts or recalls a pending handoff; `/OK <FLID>` forces/overrides selected logic checks in CRC. | COMMUNITY; not an FAA grammar or complete legacy mapping |
| ERAM-CMD-03 | Implied `<sector ID> <FLID>` initiates a handoff. | COMMUNITY; exact deployed ERAM scope unknown |
| ERAM-CMD-04 / 16 / 17 | `QQ [R|P|L]<altitude> <FLID>` sets interim/local-interim/procedure altitude; `QZ <altitude> <FLID>` sets assigned altitude. Altitude-menu picks can clear altitude states. | COMMUNITY; examples only; facility/manual validation required |
| ERAM-CMD-05 | CRC vector length is controlled by the Master Toolbar; Page Up/Page Down changes vector-line length. This is not proven identical to the study's legacy vector-line entry class. | COMMUNITY; controls not suitable as a real-system claim |
| ERAM-CMD-06 / 30 | `QU <minutes> <FLID>` displays a route; `QU <FLID>` clears a route display; `QU <fix> <FLID>` amends a route direct to a fix. | COMMUNITY; exact deployed semantics and safeguards require manual review |
| ERAM-CMD-07 | `QX <FLID>` drops a track in CRC. | COMMUNITY; do not equate this automatically with the legacy “Delete Aircraft” class |
| ERAM-CMD-08 | No confidently equivalent checkmark action found in the reviewed command reference. | No supported mapping; keep out pending primary/manual evidence |
| ERAM-CMD-09 | `QL <sector IDs...>` enables quick look; `QL` with no sectors disables it. | COMMUNITY; local CRC behavior only |
| ERAM-CMD-10 | `QF <FLID>` displays flight-plan contents in the Response Area. | COMMUNITY; not evidence of current FAA formatting |
| ERAM-CMD-11 / 12 | CRC supports view/toolbars and repositioning/suppression controls. No exact equivalence to each broad legacy frequency category is established. | COMMUNITY; broad task mapping remains unverified |
| ERAM-CMD-13 | `QP J <FLID>` toggles a standard DRI; `QP T <FLID>` toggles a reduced-separation DRI for eligible tracks. | COMMUNITY; do not infer equivalence to every legacy halo action |
| ERAM-CMD-14 / 24 / 28 / 29 | CRC range, brightness, and display attributes have toolbar/menu controls. The documentation does not establish exact equivalence to the sampled legacy interactions. | COMMUNITY; facility/monitor/UI details remain unverified |
| ERAM-CMD-15 | Implied `<FLID>` cycles data-block type in a described case; `QP <FLID>` changes an FDB to LDB after point-out. This is not proof of the full legacy request/suppress task. | COMMUNITY; exact semantics limited to CRC documentation |
| ERAM-CMD-18 | `QS` edits fourth-line heading/speed/free-text data; exact subforms are documented by CRC. | COMMUNITY; current ERAM grammar not established by FAA source here |
| ERAM-CMD-19 | `QP <sector> <FLID>` initiates a point out; `QP A <FLID>` acknowledges a point out. | COMMUNITY; JO 7110.65 defines the procedure, not these input forms |
| ERAM-CMD-20 | This is a source category for ambiguous/rejected legacy entries, not a command. | Do not implement as a command |
| ERAM-CMD-21 | No primary or reviewed community command mapping established. | Leave unclaimed |
| ERAM-CMD-22 | `AM` edits fields of a flight plan; CRC docs include examples. | COMMUNITY; not current FAA syntax proof |
| ERAM-CMD-23 / 25 / 26 | No exact mapping to the legacy “combined filter,” special-attention, or special-coding categories established. | Leave unclaimed pending source evidence |
| ERAM-CMD-27 | CRC uses `LF` for creating/managing a CRR group; equivalence to the study's “continuous range readout” category has not been established. | COMMUNITY; do not equate from acronym similarity |

## ERAM R-side: display, command entry, data, and responses

| Row | Element | Evidence-backed behavior | Source / confidence | Demo status |
|---|---|---|---|---|
| ERAM-R-01 | R-side role | En-route controller uses the radar position as part of a two-person R/D sector team. | DOT/FAA/TC-12/62, *SepMan 2*; PRIMARY-FAA, evaluation context | Role exists in demo plan; task division and position-specific behavior need controller review |
| ERAM-R-02 | Surveillance picture and identity | ERAM procedures cover radar identification, tracking, data blocks, handoff, and controller responsibility for correct identity. | JO 7110.65BB §§5-3, 5-4, 5-13; PRIMARY-FAA | OpenATCA has a PVD-style drawing core, not a verified ERAM display |
| ERAM-R-03 | Full data block (FDB) | JO requires FDB when the controller has separation responsibility and a paired track exists. | JO 7110.65BB §5-3-8; PRIMARY-FAA | Requirement-level only; exact field order and layout unavailable |
| ERAM-R-04 | Limited/partial data blocks | JO describes display/use of data blocks and minimum information for specified cases; exact full/limited line layouts are not publicly established here. | JO 7110.65BB §§5-3, 5-4; PRIMARY-FAA | Exact layout not implemented as a verified ERAM replica |
| ERAM-R-05 | Data block altitude and assigned state | Altitude field(s) must reflect current aircraft status; assigned/interim/procedure/LIA distinctions are described by JO. | JO 7110.65BB §§5-13-3–5; PRIMARY-FAA | Workstation field codes, arrows, and line placement are unverified |
| ERAM-R-06 | Fourth line | Fourth line must be displayed; control-information use is limited to authorized messages/directives/LOAs. Current JO includes designated speed formats with character limits. | JO 7110.65BB §5-4-10; PRIMARY-FAA | Exact local format/adaptation must not be invented |
| ERAM-R-07 | CID / beacon / callsign / target selection | Legacy study records these as FLID methods; current ERAM selector details are unavailable in public primary sources. | TC-TN06/04 Table 4; PRIMARY-FAA, legacy; TBP/TBE labels in vice; COMMUNITY | Input primitives are not an exact deployed interaction model |
| ERAM-R-08 | Message Composition Area (MCA) | Community description shows a command area with preview/feedback; FAA report documents legacy command entry and selected error cases, not the current ERAM MCA layout. | vice ERAM documentation; COMMUNITY; TC-TN06/04, PRIMARY-FAA legacy | Do not claim current ERAM layout or exact preview behavior |
| ERAM-R-09 | Response Area | Community source describes a response area and acceptance/rejection display; FAA legacy report contains examples such as rejected handoff errors. No public primary source establishes ERAM wording, color, or sound. | vice; COMMUNITY; TC-TN06/04 Table 5, PRIMARY-FAA legacy | Exact response messages and sounds not implemented as verified ERAM |
| ERAM-R-10 | Toolbar and time view | Reported in community ERAM interface documentation; no public primary source found for exact toolbar arrangement or time view. | vice; COMMUNITY | Not a verified ERAM layout |
| ERAM-R-11 | Data block lines and leader | Community ERAM description gives a multi-line layout and special indicators. JO corroborates that some fields/functions and fourth-line uses exist, but not the full layout. | vice; COMMUNITY; JO 7110.65BB §§5-3/5-4; PRIMARY-FAA for limited points | Treat line-by-line labels, abbreviations, blink, and leader behavior as unverified |
| ERAM-R-12 | Handoff and point-out indication | JO defines transfer, receiving-controller, and point-out procedures; does not specify every visual code or exact UI transition. | JO 7110.65BB §§5-4-5–7; PRIMARY-FAA | Functional placeholder only pending manual/controller validation |
| ERAM-R-13 | Conflict Alert (CA) and Mode C Intruder (MCI) | Evaluate without delay; coordinate when another controller is involved. Pair suppression and specialized group suppression have distinct limits; suppressing is acknowledgment. | JO 7110.65BB §5-13-1; PRIMARY-FAA | Alert semantics must be tied to approved core; do not copy alert colors/visuals from community sources as FAA-confirmed |
| ERAM-R-14 | En-route MSAW (E-MSAW) | Evaluate alert without delay; suppression/inhibition conditions and acknowledgment are described in JO. | JO 7110.65BB §5-13-2; PRIMARY-FAA | OpenATCA can render supplied MSAW states; that does not establish ERAM trigger or presentation fidelity |
| ERAM-R-15 | Hold / EFC | For applicable ATC-initiated holds, enter hold information, maintain paired track, and use hold message, Hold Data Menu, or Hold View for EFC; entry constitutes coordination. | JO 7110.65BB §5-13-9; PRIMARY-FAA | Exact message syntax and view layouts unverified |
| ERAM-R-16 | Airspace status / SAA | Situation display Airspace Status View/SAA Filter View should reflect adapted SAA; if `SAA DOWN`, changes stop updating and visual indicators must be created manually. | JO 7110.65BB §5-13-10; PRIMARY-FAA | Not represented as a verified ERAM screen |
| ERAM-R-17 | QP/QZ/QQ/QS command letters | These appear in the 2006 legacy command-report examples and terminology, not as a published current ERAM grammar. | TC-TN06/04; PRIMARY-FAA, legacy | Do not label QP/QZ/QQ/QS forms as ERAM-exact |
| ERAM-R-18 | QF/QU commands | QF/QU labels are described in community ERAM material; the public FAA sources reviewed do not establish exact ERAM syntax or response. | vice; COMMUNITY | Prototype aliases remain unverified; no exactness claim |
| ERAM-R-19 | Route / flight-plan readout | JO treats flight-plan route and altitude updates as operational data that must be managed; exact readout presentation and field ordering remain unavailable. | JO 7110.65BB §5-13; PRIMARY-FAA | Data model operations are not an ERAM readout replica |
| ERAM-R-20 | Color and display coding | FAA-HF-STD-010A defines a palette and color-use constraints for ATC primary displays. JO 7210.3EE Change 3 provides terminal-specific color policy: black background; yellow point-out identifier and geographic restrictions; white owned blocks; green unowned limited/partial blocks and beacon extent; blue search/fusion targets and history trails; dim-gray compass/range rings; white predicted track and minimum-separation line. Critical information needs a redundant cue, pure blue is disallowed for small text/detail, colors need sufficient contrast, and use should be consistent across displays. This is a standard/policy mapping, not proof of exact installed ERAM or STARS presentation. | FAA-HF-STD-010A (2020); JO 7210.3EE Change 3 §3-9-1; PRIMARY-FAA | OpenATCA palette is not established as an ERAM/STARS palette match |
| ERAM-R-21 | Historical HCS response examples | TC-TN06/04 Table 5 classifies legacy handoff refusals such as inactive sector, missing track selection, missing flight plan, already-in-handoff, and non-adapted sector. It includes selected historical response text. | TC-TN06/04 Table 5; PRIMARY-FAA, legacy | Use only as legacy examples; do not reuse the text as current ERAM responses |
| ERAM-R-22 | Sign-in / preference set | TC-TN06/04 shows historical HCS/DSR entry `QP SI RU 1`; in the sample, sign-in triggers a preference set that changes display settings. This does not establish a current ERAM sign-in command or preference workflow. | TC-TN06/04 Figure 3; PRIMARY-FAA, legacy | Current sign-in grammar and screen behavior unverified |

These additional rows capture the **community field-level claims** so they can be checked without
confusing them with primary FAA interface evidence. All are vNAS CRC documentation retrieved
25 Nov 2026 (release version not stated), confidence COMMUNITY, and remain unverified for current
ERAM. The community source explicitly identifies simulator-specific VATSIM behavior.

| Row | CRC-described field or response | Community-described behavior | Demo status |
|---|---|---|---|
| ERAM-R-23 | FDB column 0 | VCI marks a check-in; `R` marks ownership by another sector. | Not a verified ERAM field specification |
| ERAM-R-24 | FDB line 0 | `P` marks initiated point-out; `A` marks acknowledged point-out. | Not a verified ERAM display code |
| ERAM-R-25 | FDB line 1 / Field A | Aircraft ID; the community tool describes click actions on this field. | Exact click behavior unverified for real ERAM |
| ERAM-R-26 | FDB line 2 / Fields B and C | B gives assigned altitude and a type/state character (C/T/L/P, arrows, and other CRC-specific markers); C gives reported altitude or a placeholder and may be hidden in a stated condition. | The meanings are useful hypotheses; exact field syntax/presentation requires manual confirmation |
| ERAM-R-27 | FDB line 3 / Fields D and E | D is CID. E may show destination ID and ground speed or special/handoff/coast/beacon status strings. | Do not use CRC status strings as FAA-authoritative values |
| ERAM-R-28 | FDB line 4 / Field F | Destination, aircraft type, assigned heading, assigned speed, or free text; CRC describes an HSF indicator and toggle. | JO establishes fourth-line rules, but CRC's full field mapping is COMMUNITY only |
| ERAM-R-29 | Unpaired/paired LDB and E-LDB | CRC describes beacon code/altitude or aircraft ID/altitude, with E-LDB converting certain fields to an enhanced format. | No real-system line format established from public primary source |
| ERAM-R-30 | MCA preview and feedback | CRC describes an upper Preview Area for commands and lower Feedback Area for processing success/errors; `Enter` executes commands, while certain middle-click selections execute immediately. | Layout/input behavior is COMMUNITY only |
| ERAM-R-31 | Response Area and QF output | CRC says QF sends flight-plan details to the RA, including time, CID, aircraft ID, owner sector, aircraft/equipment, beacon code, filed speed, assigned altitude, route, and remarks. | Exact real-system ordering and text are unverified |
| ERAM-R-32 | Time View | CRC describes current Zulu time in HHMM SS, configurable/repositionable and not suppressible. | COMMUNITY only |
| ERAM-R-33 | Toolbars and view controls | CRC documents master toolbar, per-view controls, and display menus. | Exact real ERAM layout unverified |
| ERAM-R-34 | Leader/vector/DRI | CRC documents FDB leader lines, velocity vector intervals, and standard/reduced DRI controls. | Do not claim geometry, control, or eligibility behavior without primary/manual confirmation |

## STARS TCW/TDW: terminal display and procedures

| Row | Element | Evidence-backed behavior | Source / confidence | Demo status |
|---|---|---|---|---|
| STARS-01 | TCW / TDW positions and training | FAA training order names STARS keyboard and Tower Display Workstation lessons; exact operator workflow is not contained in that order. | JO 3120.4S App. D; PRIMARY-FAA | Role labels may be used; keyboard layout/workflow not verified |
| STARS-02 | Tracking, tagging, handoff, altitude, coordination, ground speed, identity | These are named terminal-automation functions; controller remains responsible for identity, correct target/data association, and separation. | JO 7110.65BB §§5-14-1–3; PRIMARY-FAA | Function-level only; not a TCW/TDW interface specification |
| STARS-03 | Information shown | Displayed information must follow national orders and local directives. | JO 7110.65BB §5-14-5; PRIMARY-FAA | Facility-specific layouts cannot be claimed from national procedure alone |
| STARS-04 | CA/MCI | Evaluate without delay and coordinate where needed. A suppress function is for a specific alert; inhibit has a narrower use; suppression is acknowledgment. | JO 7110.65BB §5-14-6; PRIMARY-FAA | Underlying OpenATCA alert behavior is not proof of STARS presentation fidelity |
| STARS-05 | MSAW inhibit | JO specifies limited inhibit conditions and alert handling; details must be interpreted with local directives. | JO 7110.65BB §5-14-7; PRIMARY-FAA | No exact STARS input/response strings claimed |
| STARS-06 | TDW Fused Display Mode and ADS-B indicator | JO says ADS-B indicator displays only when TDW operates in Fused Display Mode, and specifies an outage use case. | JO 7110.65BB §5-14-5; PRIMARY-FAA | Mode indicator/UI detail still needs operator-manual confirmation |
| STARS-07 | Keyboard and trackball scheme | FAA training order names STARS keyboard and TDW training; it does not publish the key map. vNAS/CRC documents a simulator mapping such as F3 INIT CNTL and F4 TERM CNTL. | JO 3120.4S App. D; PRIMARY-FAA for training topic; vNAS/CRC; COMMUNITY for mapping | Do not use simulator key mappings as deployed STARS claims |
| STARS-08 | Full, limited, and enhanced-limited data blocks | JO requires information to conform with national and local direction but does not provide the full TCW/TDW block specification. vNAS/CRC describes LDB, PDB, and FDB differences and example fields; PDB is not assumed equivalent to enhanced-limited. | JO 7110.65BB §5-14-5; PRIMARY-FAA for requirement; vNAS/CRC; COMMUNITY for fields | Layout not implemented as verified STARS |
| STARS-09 | Display control bar (DCB) | No public primary source reviewed here establishes exact controls. vNAS/CRC describes Main/Aux DCB, toggle/spinner buttons, and submenus; its menu states contain simulation-specific disabled or simplified functions. | No primary UI spec found; vNAS/CRC; COMMUNITY | Do not claim the CRC menu is the installed STARS DCB |
| STARS-10 | Full Service Level (FSL) | JO 3120.4S names terminal training that includes the equipment; vNAS/CRC says its own STARS supports only FSL. Neither establishes the deployed screen details. | JO 3120.4S App. D; PRIMARY-FAA for training topic; vNAS/CRC; COMMUNITY for its scope | FSL display and commands remain unverified |
| STARS-11 | Emergency Full Service Level (EFSL) | JO 3120.4S includes transition to emergency service level as a training topic. vNAS/CRC documents only FSL support, so it does not supply EFSL behavior. | JO 3120.4S App. D, PRIMARY-FAA for training topic; EFSL interface unavailable | Do not invent the EFSL screen or claim failover fidelity |
| STARS-12 | Fused targets / identity and altitude | JO permits use of terminal automation for target identity and requires current altitude display in specified modes; responsibility remains with controller. | JO 7110.65BB §§5-14-1–5; PRIMARY-FAA | Functional statements only; mode-specific symbol/tag design unverified |
| STARS-13 | Color | JO 7210.3EE Change 3 §3-9-1 governs terminal color use; FAA-HF-STD-010A supplies the ATC display color standard. | PRIMARY-FAA | Palette application and precise STARS field mapping need manual and display review |

The following STARS field/response rows are **community comparisons only** (vNAS CRC
documentation, retrieved 25 Nov 2026; release version not stated). The source notes CRC-specific
simulation behavior and should not be treated as the TCW/TDW operator manual.

| Row | CRC-described field or response | Community-described behavior | Demo status |
|---|---|---|---|
| STARS-14 | LDB | Unassociated track; beacon code and altitude by default, optional temporary ground speed. | Community format only; facility settings can change display |
| STARS-15 | PDB | Associated track owned by another position; mirrors much of FDB line 2, with configurable ground-speed display. | Do not treat CRC PDB as identical to an FAA enhanced-limited block |
| STARS-16 | FDB line 1 | Aircraft callsign/ID. | Community field layout only |
| STARS-17 | FDB line 2 | Time-shares altitude/scratchpads/sector status and ground-speed/type/requested-altitude values; a character denotes handoff recipient. | Exact real STARS content/order is unverified |
| STARS-18 | FDB line 3 | Can show received/assigned beacon code in a specified mismatch case. | Community only |
| STARS-19 | Handoff/point-out response | CRC describes blinking data blocks around handoff/point-out state changes and simulator-specific accept/reject interactions. | JO gives procedure but not these visual/text transitions |
| STARS-20 | Conflict alert | CRC describes a blinking CA marker and recurring tone until a click acknowledges it; its conflict logic is a simulator implementation. | Never present CRC threshold, alert rendering, or acknowledgment mechanics as FAA-exact |
| STARS-21 | MSAW | CRC documentation says MSAW is not implemented in that simulator. | Do not imply STARS MSAW behavior based on CRC; primary order governs procedure only |

The vNAS/CRC STARS manual is a detailed COMMUNITY comparison but explicitly describes a simulator
with VATSIM-specific differences. It documents a DCB and System Status Area; LDB/PDB/FDB examples;
example fields and time-sharing; and that CRC's MSAW is not implemented. None of those details
establishes the deployed STARS TCW/TDW presentation. Use them only as labeled comparison material.

## Responses and syntax: limits of available evidence

The FAA Technical Center report gives **historical HCS** examples of handoff syntax and error
messages, including rejected handoff cases. These examples are useful for understanding how a
legacy command study classified errors; they are not reusable ERAM response strings. The ERAM and
STARS operator manuals were not available in the public sources reviewed. Exact command strings,
field order, abbreviations, key/button mappings, menu labels, response text, colors for each field,
tones, and error recovery behavior therefore remain unverified. Do not fill those gaps with
community emulator behavior or a reasoned guess and then present it as real-system fidelity.

In particular, `CID` and target-click selection are evidenced in the legacy report. The `TBP` and
`TBE` labels come only from community ERAM documentation reviewed here. The FAA report provides
neither those button names nor the current ERAM control mapping. The current display core's typed
intent queue and status labels (`CMD PENDING`, `CMD ACCEPTED`, `CMD REFUSED`, `CMD STALE`) are
OpenATCA outcomes, not ERAM response messages.

## Source register

- [DOT/FAA/TC-TN06/04, *En Route Air Traffic Controller Commands: Frequency of Use During Routine
  Operations* (May 2006)](https://hf.tc.faa.gov/publications/2006-en-route-air-traffic-controller-commands/full_text.pdf),
  Table 3, Table 4, Table 5, and Appendix A Table A-1. Legacy HCS/DSR/URET sample; not current
  ERAM command guidance.
- [FAA Order JO 7110.65BB](https://www.faa.gov/documentLibrary/media/Order/7110.65BB_Bsc_w_Chg_1_dtd_8-7-25.pdf),
  20 Feb 2025, Change 1 dated 7 Aug 2025: §§5-3, 5-4, 5-13, 5-14.
- [FAA Order JO 7210.3EE Change 3](https://www.faa.gov/documentLibrary/media/Order/7210.3EE_Chg_3_dtd_7-9-26_Final.pdf),
  effective 9 Jul 2026, §3-9-1 (terminal display color policy; facility directives and the STARS
  Site Rules Document also apply).
- [FAA-HF-STD-010A, *Standard Practice: Color Use in Air Traffic Control System Displays*](https://rosap.ntl.bts.gov/view/dot/92012),
  24 Mar 2020; standard palette and color-use requirements.
- [FAA Order JO 3120.4S](https://www.faa.gov/documentLibrary/media/Order/FAA_Order_JO_3120.4S.pdf),
  28 Aug 2024, App. D (TDW and STARS keyboard training topics; not interface specification).
- [vNAS CRC ERAM documentation](https://docs.virtualnas.net/crc/eram/) and [STARS
  documentation](https://docs.virtualnas.net/crc/stars/), retrieved 25 Nov 2026; release version not
  stated. These are COMMUNITY simulation manuals and explicitly include VATSIM-specific scope or
  behavior. Their detail is useful for comparison but not evidence of installed FAA systems.
- [vice documentation](https://pharr.org/vice/), COMMUNITY source for the TBP/TBE labels; not an FAA
  operator manual.
- [DOT/FAA/TC-TN06/14, *Methods for Examining Possible Effects of ERAM on Controller
  Performance* (May 2006)](https://rosap.ntl.bts.gov/view/dot/71692), a historical test-methods
  report describing legacy/ERAM UI comparison questions; not an ERAM operator manual.
- [vice ERAM documentation](https://pharr.org/vice), COMMUNITY source for detailed interface
  descriptions. It is not an FAA source and cannot settle real-system fidelity.
- Current OpenATCA distinction: [`oa-display` capability description](../crates/oa-display/README.md)
  and [capability status](CAPABILITIES.md). The drawing core and typed interaction primitives do
  not constitute a complete ERAM or STARS workstation.

## Required next evidence

Before calling any command, response, data block, or workstation screen “ERAM-faithful” or
“STARS-faithful,” compare the row against the applicable system/operator manual and facility
adaptation, then review it with working controllers. Record exact manual edition, section/page,
facility-specific scope, and controller review for each resolved row. Until then, preserve the
row's explicit gap and show the corresponding behavior as an OpenATCA difference or leave it out.

| F2-VIEW-01 | Center, range and pan | Scenario-pinned KORD center; initial60NM; steps20/40/60/90/120/250NM; minus/equals range, arrows pan, Home recenters. | OpenATCA preview behavior, project demo requirement 2026-11-26; no ERAM key mapping claimed | Active engine state, shown in differences list |

| F2-MAP-01 | ORD public procedure map | CIFP_261029.zip, exact archive SHA in generated ord_map.rs; four selected STAR/approach fix sequences, dashed synthetic DCTs, KORD reference point and reciprocal runway threshold centerlines. | PRIMARY-FAA coordinates; OpenATCA rendering | Each fix/name/line and runway has an in-app trace ID; no SWIM or inferred sector data |

| F2-LABEL-01 | Label placement and leader | Eight-position deterministic greedy overlap minimization, stable target order, default-position ties. | OpenATCA behavior, project demo requirement 2026-11-26 | Active; shared native/WASM renderer, no current ERAM placement claim |

| F2-SAFETY-01 | Configured safety nets | Real oa-ca on tracked pairs every scan; explicit development adaptation 3NM/1000ft,60s lookahead,5s hysteresis. MSAW terrain/obstacle/altimeter adaptation, approach-path and area proximity unconfigured. | OpenATCA implementation; task-level FAA alert evidence only | Real CA state/count; grey preview limits; public C90 MVA pin alone is not a terrain warning floor |
