# OpenATCA requirements register

Every requirement has a stable ID. Every requirement must be verified by at least one test that
carries the tag `// REQ: <ID>` on the line above the test function. `oa-trace` fails the build if
a requirement has no test, or a test names an ID that does not exist.

IDs are never reused. A withdrawn requirement stays in the register marked WITHDRAWN.

## M0 — Airspace foundation

### General

- **OA-GEN-001** All crates forbid `unsafe` code.
- **OA-GEN-002** No crate depends on any external crate, including dev-dependencies.
- **OA-GEN-003** Every parser rejects an input that exceeds a stated limit (record length, field
  length, record count, string length) with an error naming the source, line and limit.
- **OA-GEN-004** No parser panics on any input. Arbitrary byte input produces either a value or
  an error.

### CSV (oa-csv)

- **OA-CSV-001** Parses RFC 4180: quoted fields, doubled quotes inside quoted fields, CRLF and
  LF line endings, embedded newlines in quoted fields.
- **OA-CSV-002** Fields are accessed by header name; a missing column is an error naming it.
- **OA-CSV-003** Errors report the 1-based physical line number of the offending record.
- **OA-CSV-004** Rejects a record whose field count differs from the header's.

### Geodesy (oa-geo)

- **OA-GEO-001** `LatLon` rejects latitude outside [-90, 90], longitude outside [-180, 180],
  and non-finite values.
- **OA-GEO-002** Converts degrees/minutes/seconds/hemisphere to decimal degrees, rejecting
  minutes or seconds outside [0, 60).
- **OA-GEO-003** Solves the WGS-84 inverse problem (distance, initial and final azimuth) with
  error below 1 mm against published reference values (GeographicLib test set).
- **OA-GEO-004** Solves the WGS-84 direct problem with error below 1 mm against the same
  reference values.
- **OA-GEO-005** Handles nearly antipodal and coincident points without failing to converge.
- **OA-GEO-006** Point-in-polygon for polygons whose edges are geodesics, correct for points on
  either side of every edge of the ZAU boundary.
- **OA-GEO-007** Computes the maximum separation between a geodesic and a rhumb line joining the
  same two points.

### NASR (oa-nasr)

- **OA-NASR-001** Validates each NASR CSV file against the FAA's `*_CSV_DATA_STRUCTURE.csv`:
  every column present, maximum lengths, numeric types, nullability. Violations are collected,
  not fatal, and reported with file, line and column.
- **OA-NASR-002** Parses ARTCC boundaries (ARB) into ordered point sequences per facility and
  stratum (HIGH, LOW, and FIR/CTA/UTA types).
- **OA-NASR-003** Parses fixes, navaids, airports, runways and runway ends with positions
  computed from DMS and checked against the published decimal value.
- **OA-NASR-004** Parses airways with ordered segments, minimum enroute altitudes and gap flags.
- **OA-NASR-005** Parses STAR and DP route structure with transitions and airport/runway
  associations.
- **OA-NASR-006** Parses class airspace designations per airport.

### CIFP (oa-arinc424)

- **OA-424-001** Parses the CIFP header record and the cycle of every data record. Records carry
  the cycle of their last change, so earlier cycles are expected; a record whose cycle is later
  than the header's is rejected. (Amended 2026-11-14: the original text, "rejecting a file that
  mixes cycles", contradicted how the FAA codes CIFP.)
- **OA-424-002** Parses airport (PA), runway (PG), VHF navaid (D), NDB (DB), terminal NDB (PN),
  enroute waypoint (EA), terminal waypoint (PC) and localizer/glide slope (PI) records.
  (Amended 2026-11-14: PN added; approach legs name terminal NDBs.)
- **OA-424-003** Parses airway (ER) records into ordered fixes with route type and altitudes.
- **OA-424-004** Parses SID (PD), STAR (PE) and approach (PF) records into ordered legs with
  path-terminator, fix, altitude and speed constraints, keeping continuation records attached
  to their primary record.
- **OA-424-005** Parses class B/C/D airspace (UC) and special use airspace (UR) into boundary
  sequences including arcs and their centres and radii.
- **OA-424-006** Parses Grid MORA (AS) records.
- **OA-424-007** Every field position used is verified by a test against a real CIFP record.

### Airspace model (oa-airspace)

- **OA-ASP-001** Builds the model for one ARTCC and cycle from NASR and CIFP.
- **OA-ASP-002** Selects every object inside the ARTCC's HIGH or LOW boundary plus a
  configurable buffer, and every procedure of every airport inside.
- **OA-ASP-003** Reports every point present in both sources whose positions differ by more
  than 1 m, with the distance.
- **OA-ASP-004** Reports every reference to a point that cannot be resolved.
- **OA-ASP-005** Reports, per boundary, the maximum separation between geodesic and rhumb-line
  interpretations of its edges.
- **OA-ASP-006** Serializes the model deterministically: identical inputs produce a
  byte-identical snapshot and identical SHA-256.
- **OA-ASP-007** Diffs two snapshots, listing added, removed and changed objects with the
  changed fields.

### Traceability (oa-trace)

- **OA-TRC-001** Lists every requirement without a test and every test tag naming an unknown
  requirement, and exits non-zero if either list is non-empty.

### Hashing (oa-sha256)

- **OA-SHA-001** Computes SHA-256 per FIPS 180-4, matching every NIST CAVP SHA-256 byte-oriented
  test vector (short, long and Monte Carlo messages).
- **OA-SHA-002** Hashes streamed input incrementally with the same result as one-shot hashing,
  for every split point of the test messages.

### Archives (oa-zip)

- **OA-ZIP-001** Reads ZIP archives (central directory, local headers, stored and deflate
  entries), rejecting encrypted, multi-disk and ZIP64-beyond-limits archives with a clear error.
- **OA-ZIP-002** Inflates DEFLATE streams per RFC 1951 (stored, fixed and dynamic Huffman
  blocks), verifying each entry's CRC-32 and uncompressed size.
- **OA-ZIP-003** Enforces limits on entry count, uncompressed size per entry and total, and
  compression ratio, so a crafted archive cannot exhaust memory or disk.
