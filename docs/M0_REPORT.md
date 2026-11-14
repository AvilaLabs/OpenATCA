# M0 — Airspace foundation: closing report

Status: closed 2026-11-14. Design: [M0_DESIGN.md](M0_DESIGN.md).

## What M0 delivers

From the FAA's public aeronautical data for one 28-day cycle, `oa-adapt build` produces a
validated, deterministic model of an ARTCC's airspace. It writes a binary snapshot identified
by its SHA-256, plus a report of everything the build found. `oa-adapt diff` compares two
snapshots and lists every change.

Legacy systems maintain site adaptation by hand for each facility. Here it is generated from
source data every cycle, checked against two independent FAA sources, hashed, and diffed.

## Result for Chicago Center (ZAU), cycle 2026-10-29

Inputs: NASR CSV `NASR_CSV_29_Oct_2026.zip` (sha256 `60b07245…`) and CIFP `CIFP_261029.zip`
(sha256 `e3ff2b31…`). Full hashes are in [DATA.md](../DATA.md).

| Table | Rows |
|---|---|
| Points (fixes, navaids, airports, thresholds, terminal waypoints, localizers) | 11,640 |
| Boundaries (ARTCC HIGH/LOW, class B/C/D, restricted areas, MOAs) | 137 |
| Airways / airway legs | 153 / 2,738 |
| Procedures (55 SIDs, 39 STARs, 628 approaches) | 722 |
| Routes / legs | 2,545 / 9,649 |
| MORA cells | 70 |

Snapshot: 925,547 bytes, SHA-256 `41ae4460fdf8f587db80b8253ae54f2a487c6dc1674fda7cdd9dbb8ddaeb98d6`.

### Checks

- **Cross-source agreement.** Of the points carried by both NASR and CIFP, 0 inside the ZAU
  model differ by more than 1 m. Across the whole national cycle, 101,878 pairs were compared and
  7 differ by more than 1 m (largest 37.1 m). All 7 are outside ZAU.
- **References.** Every airway and procedure reference resolves except 2. Both trace to coding in
  the FAA source data and are held as candidate defects. The 36 airway "points" at the Canadian
  and Mexican borders are border crossings, not missing points, and are reported separately.
- **Schema.** Every NASR file is validated against the FAA's own `*_CSV_DATA_STRUCTURE`
  description. All 397,304 CIFP records decode with 0 errors.
- **Fixed point.** 59,068 coordinates are stored as 1e-7 degree integers. The largest rounding
  error is 6.8 mm.
- **Determinism.** The same inputs give a byte-identical snapshot and the same hash on every run.
- **Boundary geometry.** NASR does not say whether boundary points are joined by geodesic or rhumb
  lines (UNKNOWNS UK-001). For ZAU the two readings differ by at most 592 m, well inside the 50 NM
  selection buffer. Both are reported for every boundary.

### Performance

The full ZAU build, from the two zip archives, takes 2.5 s on a laptop with 0.55 GB peak memory.
That covers decompression, parsing, validation, cross-checking, selection, snapshot and report.
An early build took 17.6 s.

### Cycle-to-cycle change

`oa-adapt diff` between the 2026-10-01 and 2026-10-29 cycles found 109 changes in the ZAU model.
Each one is listed by object, with old and new values.

## Defects found

In the FAA data, held privately under the Avila Labs defect reporting policy and not yet sent:

- 1 confirmed defect: a navaid whose published decimal position is the DME antenna's, about 105 m
  from the VOR. It is outside ZAU.
- 4 candidates: runway ends coded with no displaced threshold while NASR has one; small
  NASR/CIFP navaid position differences; and the 2 unresolved procedure references above.

In our own model, found and fixed during M0:

- **Same-identifier navaids collided.** Ten NASR beacons shared a derived identity with another
  station and were dropped. Through that collision, two Oshkosh approaches (ILS and LOC
  runway 36) pointed at the identically named beacon near Columbus, Ohio, about 600 km away. The
  model now keys NASR navaids by NASR's own unique key and pairs them with CIFP only within 1 NM.
  Of 24,774 references, exactly those two changed target, both to the correct station.
- **Review fixes.** An adversarial review found geometry bounds wrong across the antimeridian and
  under geodesic bulge, airway legs joined across a break, missing radii read as zero, and diff
  keys that could collide. All are fixed, each with a test that pins the behaviour.

## Assurance state

- 72 numbered requirements. Every one is traced to at least one test by `oa-trace`, which CI
  runs on every push.
- 268 tests, including real-data tests over the full national NASR and CIFP cycle, Karney's
  geodesic test set and the NIST SHA-256 vectors.
- Rust standard library only. No external crates. `unsafe` forbidden. Lints deny `unwrap`,
  `expect`, `panic` and unchecked indexing. Release builds check integer overflow.

## What M0 does not deliver

- **Sectors.** SCOPE.md lists sectors under M0, but the public data does not contain them. NASR
  boundaries serve NOTAMs, flight filing and charting only. ERAM's operational and sector
  boundaries are not published, and some rest on letters of agreement (FAA Aeronautical Charting
  Meeting 24-02). The model keeps the source of every boundary explicit so operational
  boundaries can be loaded beside the published ones when we obtain them. Sectors stay on the gap
  list (SCOPE.md section 6).
- **Magnetic variation.** Procedure legs carry the magnetic courses CIFP publishes. No variation
  model is applied yet.
- **Undecoded CIFP fields.** Some fields are not yet consumed or verified (UK-002): RNP values,
  some altitude description codes (399 legs), and the southern-hemisphere MORA convention.
- **Radar-vector SIDs.** Four ZAU SIDs exist in NASR only, with no coded legs (UK-003).

## Next

M1 (record and replay) is under way. A bounded XML parser for the FAA SWIM feeds parses every
recorded message at about 500 MB/s, and the feed adapter, SFDPS decoder, compressed recording
and replay are in progress.
