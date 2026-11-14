# MVA / MIA chart naming forms (oa-mva)

Surveyed 2026-11-14 from the `gml:description` of the 55 downloaded messages
(`raw/mva`: 35, `raw/mva-zjx`: 20) and from the file names linked by the FAA page snapshots
(`raw/mva/mva_page_snapshot.html`, `mia_page_snapshot.html`; 360 distinct names, of which about 315 are
the plain forms). Only the downloaded messages' descriptions were read; for the rest the file
name is the evidence, and a description may differ.

| Form | Example (description or file name) | Seen in | oa-mva |
|---|---|---|---|
| `<FAC>_MVA_FUS3\|FUS5_<year>` | `ALO_MVA_FUS3_2024` | 47 downloaded | accepted, set Fus3/Fus5 |
| `<FAC>_MVA_FUS3\|FUS5_<year>_v1` | `C90_MVA_FUS5_2024_v1` | 5 downloaded | accepted |
| `<FAC>_TAV_<year>` | `ZAU_TAV_2025`, `ZJX_TAV_2025` | 2 downloaded | accepted, set Tav |
| `<FAC>_<SUB>_MVA_<year>` | `SAV_LHW_MVA_2022` | 1 downloaded | accepted, set Sub |
| `<FAC>_<SUB>_MVA` (file names) | `FHU_FHU_MVA`, `ZLC_GTF_MVA`, `L30_LAS_MVA`, `ZHN_OGG_MVA`, `JCF_EDW_MVA`, `ZUA_GUAM_MVA` | snapshot only (about 22) | accepted if the description matches |
| `<FAC>_<SUB>_MVA_FUS3\|FUS5` | `BOI_BZN_MVA_FUS3`, `GEG_MSO_MVA_FUS5`, `ZLC_GTF_MVA_FUS5`, `ZHN_OGG_MVA_FUS3` | snapshot only (11) | refused (UnknownChartName) |
| `<FAC>_<SUB1>_<SUB2>_MVA_FUS5` | `D01_DEN_PUB_GJT_MVA_FUS5`, `D01_DEN_PUB_MVA_FUS3` | snapshot only (2) | refused |
| `<FAC>_<SUB>_VOR_MVA` | `TWF_BYI_VOR_MVA`, `TWF_TWF_VOR_MVA` | snapshot only (2) | refused |
| `<FAC>_<SUB>_<NAME>_MVA` with a digit/space in the name | `ZHN_HNL_ASR9_MVA`, `ZHN_ITO%20ASR8_MVA`, `ZHN_HNL_ARSR4_MVA` | snapshot only (3) | refused (ASR9 would parse as a sub-area, but only if its description says so) |
| `<FAC>_MVA_MULTI`, `<FAC>_MVA_VOR`, `<FAC>_MVA_CENRAP` | `HLN_MVA_MULTI`, `POE_MVA_MULTI`, `HLN_MVA_VOR`, `ZUA_MVA_CENRAP` | snapshot only (4) | refused |

Edition labels seen: `_2022`, `_2024`, `_2025`, optionally `_v1`.

## Sub-area charts and queries

`SAV_LHW_MVA_2022` is Savannah TRACON's own MVA chart for Wright Army Airfield (LHW). Its PDF
(`SDAT MVA PROJECT NAME: SAV_LHW_MVA_2022`, sectors A to Q, 1,500 to 2,700 ft) states no
separation standard (no 3 or 5 nm note), and its AIXM outline covers the same circle as SAV's
FUS3/FUS5 charts (about 11,260 nm2), so it overlaps them everywhere.

Decision: a sub-area chart has set kind `SetKind::Sub` and keeps `LHW` in `Model::sub` /
`Model::chart_sub`. It is not part of `min_altitude_at` for FUS3, FUS5 or TAV (those answer only
from their own charts, as before). `min_altitude_at(.., SetKind::Sub)` answers from the sub-area
charts of every facility, and `min_altitude_with_sub(m, lat, lon, set, "SAV")` returns the higher
of the set's answer and SAV's own sub-area charts: for a warning the higher minimum is the
conservative one where charts overlap, and no separation standard is assumed for the sub chart.
