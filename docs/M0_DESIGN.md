# M0 — Airspace foundation: design

Status: approved to build 2026-11-13.

## Goal

Turn the FAA's public aeronautical data for one 28-day cycle into a validated, deterministic
in-memory model of an ARTCC's airspace, starting with Chicago Center (ZAU). Every later
milestone (tracking, alerts, shadow mode) reads its adaptation data from this model.

This is the "site adaptation generated from source data" row of SCOPE.md section 3a: what
legacy systems maintain by hand per facility, we generate, check, hash and diff every cycle.

## Inputs (cycle 2026-10-29)

| Source | File | What we take |
|---|---|---|
| NASR CSV | `ARB_BASE.csv`, `ARB_SEG.csv` | ARTCC HIGH and LOW boundaries, FIR/CTA/UTA |
| NASR CSV | `FIX_BASE.csv`, `FIX_NAV.csv` | Named fixes and their navaid references |
| NASR CSV | `NAV_BASE.csv` | VOR, VORTAC, NDB, TACAN, DME |
| NASR CSV | `APT_BASE.csv`, `APT_RWY.csv`, `APT_RWY_END.csv` | Airports, runways, thresholds, elevations |
| NASR CSV | `AWY_BASE.csv`, `AWY_SEG_ALT.csv` | Airways with MEAs and segment data |
| NASR CSV | `STAR_*.csv`, `DP_*.csv` | Arrival and departure route structure |
| NASR CSV | `CLS_ARSP.csv` | Which airports have class B/C/D/E |
| NASR CSV | `*_CSV_DATA_STRUCTURE.csv` | The FAA's own schema: max lengths, types, nullability |
| CIFP | `FAACIFP18` (ARINC 424-18, FAA exceptions in the CIFP Readme) | Airports, runways, navaids, waypoints, airways, SIDs/STARs/approaches as coded legs, class B/C/D shapes (UC), special use airspace (UR), Grid MORA (AS) |

Raw downloads live outside the repo in `../openatca-data/raw/<source>-<cycle>/`, with SHA-256
recorded in `DATA.md`. Tests that need full files read the directory named by the
`OPENATCA_NASR` environment variable (renamed 2026-11-14; `OPENATCA_DATA` now names the whole `openatca-data` directory) and skip with a clear message when it is unset. Unit tests
use small excerpts of the real files committed as fixtures (FAA aeronautical data is public).

## Crates

All crates: Rust 2021 edition, `#![forbid(unsafe_code)]`, **no dependencies of any kind**
(including dev-dependencies). Clippy clean with `-D warnings`.

| Crate | Role |
|---|---|
| `oa-csv` | RFC 4180 reader. Streaming, header-indexed access, hard limits on record and field size, errors carry line numbers. |
| `oa-geo` | WGS-84 geodesy: validated `LatLon`, DMS parsing, ellipsoidal inverse and direct problems (Karney's method or Vincenty with a convergence guard), great-circle and rhumb segments, point-in-polygon on the ellipsoid surface. |
| `oa-nasr` | Typed NASR CSV records for the files above. Columns are resolved by header name, never position. Every file is validated against the FAA's `*_CSV_DATA_STRUCTURE.csv`. |
| `oa-arinc424` | CIFP parser. Fixed-width 132-column records, continuation records, header records, the record-number and cycle trailer. Field positions are tested against real records. |
| `oa-airspace` | The model: facilities, points, airways, procedures, class airspace, SUA, MORA grid. Builder, cross-source validation, deterministic serialization, hash and cycle diff. |
| `oa-trace` | Requirements traceability checker (tool, not part of the runtime). |

Binary: `oa-adapt` in `oa-airspace`:

```
oa-adapt build --data <dir> --cycle 2026-10-29 --artcc ZAU --out <file>   # snapshot + report
oa-adapt diff  <snapshot-a> <snapshot-b>                                  # what changed
```

## Model rules

- **Coordinates are computed from degrees/minutes/seconds ourselves**, then compared with the
  FAA's published decimal value. A disagreement beyond rounding is a data defect and is reported.
- **Every point has one identity** across NASR and CIFP (identifier plus ICAO region plus type).
  Where both sources carry a point, their positions are compared and any difference over 1 m is
  reported, with the distance.
- **Selection:** everything whose position lies inside the ARTCC's HIGH or LOW boundary, plus a
  configurable buffer (default 50 NM) so handoff and boundary-crossing traffic has context, plus
  every procedure for every airport inside.
- **Boundary segment geometry:** NASR does not say whether boundary points are joined by
  great-circle or rhumb lines. Until confirmed (FAA Order JO 7400 / the ARB data layout PDF),
  the model stores both and reports the maximum separation between them per boundary. This is a
  recorded Unknown, not a silent choice.
- **Limits of public boundary data.** The FAA's Aeronautical Charting Meeting 24-02 briefing
  ("ARTCC Boundary Data") says the NASR boundaries serve three purposes: NOTAMs, flight filing
  and charting. Operational boundaries the FAA uses for other purposes are not in NASR, some
  boundaries rest on letters of agreement and cannot be captured there, and the FAA plans to
  redefine the NASR LOW boundary as the ground boundary (a shift users will see), with a
  long-term aim of aligning everything to ERAM data. So: NASR gives us the published ARTCC
  outline, not ERAM's operational boundaries or sector boundaries. Sectors stay on the gap list
  (SCOPE.md section 6) and the model keeps the source of every boundary explicit, so operational
  boundaries can be loaded beside the published ones when we obtain them. Each cycle's report
  flags any change in the LOW boundary so the planned redefinition is caught when it lands.
  Source: https://www.faa.gov/air_traffic/flight_info/aeronav/acf/media/Briefings/ARTCC_Boundary_Data.pdf
- **Determinism:** the snapshot serializes in a fixed order with no hash-map iteration order
  leaking in. The same inputs always give a byte-identical snapshot and the same SHA-256.
- **Input bounds:** every record, field, count and string has a stated maximum. Exceeding one is
  an error naming the file, line and limit, never a crash or unbounded allocation.

## Model layout (RULESET A3, A4, A7, A9)

The model is the first core data structure. It is built by shell code (loading, matching,
reporting) but its in-memory form is held to the full ruleset, because the tracker and alerts
will read it every update.

**Units (A9).** Latitude and longitude are `i32` in units of 1e-7 degree (about 1.1 cm at the
equator; conversion from the parsed f64 rounds to nearest and the rounding error is reported).
Altitudes are `i32` feet. Distances are `u32` metres. Bearings are `u16` in units of 1/100
degree true. Speeds are `u16` knots.

**Strings.** One string table per model: a byte buffer plus offsets. Identifiers, names and
region codes are `StrIx(u32)`. Identical strings are interned once.

**Typed indexes (A4).** `PointIx`, `BoundaryIx`, `AirwayIx`, `ProcIx`, `LegIx`, `StrIx`, each a
`u32` newtype. A sentinel `NONE` value (`u32::MAX`) replaces `Option` in hot arrays.

**Tables, each struct-of-arrays (A3):**

| Table | Columns |
|---|---|
| Points | kind (fix, VOR, VORTAC, VOR/DME, DME, TACAN, NDB, airport, runway threshold, terminal waypoint, localizer), lat, lon, elevation (airports, thresholds), ident, ICAO region, name, sources (bitset: NASR, CIFP) |
| Boundary vertices | lat, lon (shared vertex array) |
| Boundaries | facility ident, stratum (HIGH, LOW, FIR, CTA, UTA, class B/C/D, SUA), floor and ceiling, vertex span (start, length), edge interpretation, source |
| Airways | designator, leg span |
| Airway legs | from point, to point, MEA, MEA opposite, GPS MEA, flags (gap, signal gap, dogleg) as bitsets (A6) |
| Procedures | kind (SID, STAR, approach), airport point, name, route span, source, coded (bool as bitset) |
| Routes | procedure, transition name, route type, leg span |
| Legs | path terminator, fix point, turn direction, course, distance or time, altitude description and two altitudes, speed limit, arc centre point |
| Airspace boundary segments | via code (circle, geodesic, rhumb, clockwise arc, counter-clockwise arc), lat, lon, arc centre, radius, bearing |
| MORA grid | `i16` hundreds of feet per 1° cell, with an explicit unknown value |

Any struct used as an array element (for example a leg, if stored array-of-structs for
locality) carries an exact `size_of` guard (A7).

**Source of every object.** Every row records which source(s) it came from. Where NASR and CIFP
both supply an object, the model takes position from CIFP if the two agree within 1 m, and
otherwise takes CIFP and reports the disagreement (CIFP is the coded navigation data aircraft
fly; NASR is the registry). The rule is stated in the report so it can be changed deliberately.

**Snapshot format.** Little-endian binary: magic `OATCA-ADAPT`, format version, cycle, ARTCC,
selection parameters, the SHA-256 of each input archive, then each table in fixed order with
its row count. The snapshot's identity is the SHA-256 of the whole file (A8). No table is
written in hash-map order.

## Report

`build` writes a human-readable report beside the snapshot: counts per object type, every
cross-source disagreement, every schema violation, every unresolved reference (an airway or
procedure that names a point we do not have), and the boundary-geometry Unknown.

## Obligations from the data

The CIFP customer agreement requires that anyone redistributing CIFP-derived data notify their
users of reported errors, and that discovered CIFP errors be reported to FAA Aeronautical
Information Services. Defects our validation finds are collected in the report and handled under
the Avila Labs defect reporting policy (held and batched, Connor approves anything sent).
