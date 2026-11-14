# Open unknowns

Every entry says why it is unknown and what settles it.

| ID | Area | Unknown | Why | Next step |
|---|---|---|---|---|
| UK-001 | Boundaries | Whether NASR ARTCC boundary points are joined by geodesics or rhumb lines | No FAA document found states it; JO 7900.1F says only "straight lines" | Model stores both and reports the separation (OA-ASP-005); ask FAA AIS during outreach |
| UK-002 | CIFP | Fields parsed but not verified against a second source: header CRC; ER col 44; PD/PE/PF cols 26, 45-47 (RNP), 117, 119, 120; localizer cols 75-83; PG cols 91-95; PA cols 62-70, 82-85; RF arc radius scale; southern-hemisphere MORA convention; altitude description codes; TCH indicator values D/R; UR continuation types other than C | No public second source checked yet for these fields | Verify each against CIFP Readme exceptions and charted values before any later milestone uses the field; until then the model does not consume them |
| UK-003 | Procedures | Radar-vector SIDs (for example O'HARE NINE at KORD, computer code NOT ASSIGNED in NASR) exist in NASR but are not coded in CIFP | CIFP codes only procedures with a defined path | Model takes such procedures from NASR, marked as having no coded legs |
