# CORE 0.1 vector coverage matrix

This matrix records actual checked-in vector execution coverage. “Not covered” means no vector is
present and no decoder/adapter result was checked; it is not a conformance claim. The runner is a
Rust-only checker today, so this matrix does not claim C or Python parity. Matrix scope follows
`CORE_LIBRARY_SPEC.md` §10; moved or unsupported areas remain explicit.

| 0.1 module/profile | Current case IDs | Positive coverage | Refusal coverage | Status / gap |
|---|---|---|---|---|
| `oa-sha256`, SHA-256 / FIPS 180-4 | `sha256.fips180_4.empty`, `sha256.fips180_4.abc` | Published zero-length and `abc` known answers | Not applicable to digest operation | Partial: no multi-block/boundary vectors or C/Python adapters |
| `oa-geo`, WGS-84 inverse, direct and rhumb inverse / Karney 2013 | `geo.wgs84.inverse.equatorial_one_degree`, `geo.wgs84.inverse.coincident_origin`, `geo.wgs84.direct.zero_distance_eastbound`, `geo.wgs84.rhumb_inverse.equatorial_eastward` | Equatorial one-degree inverse; exact coincident inverse; zero-distance direct endpoint/azimuth; equatorial eastward rhumb length/course | Out-of-range/non-finite inverse coordinate; direct non-finite distance; rhumb inverse pole endpoint | Partial: nonzero direct path, separation and polygon operations, additional antimeridian/pole/interior boundaries, and C/Python adapters remain uncovered |
| `oa-math`, `oa-ieee` | — | Not covered | Not covered | Not covered: reference/vector provenance and exact bit expectations required |
| `oa-deflate` | — | Not covered | Not covered | Not covered: no independently sourced synthetic member yet |
| `oa-zip` | — | Not covered | Not covered | Not covered: no CC0 container input and expected metadata/refusal yet |
| `oa-tar` | — | Not covered | Not covered | Not covered: no standard-derived member/header corpus yet |
| `oa-xml` | — | Not covered | Not covered | Not covered: namespace, malformed XML and refusal taxonomy absent |
| `oa-json` | — | Not covered | Not covered | Not covered: RFC-derived values and stable duplicate-key/offset outcomes absent |
| `oa-csv` | — | Not covered | Not covered | Not covered: dialect/quoting/record-bound vector profile not yet selected |
| ASTERIX CAT021 Ed 2.7 | `asterix.cat021.ed2_7.minimal_typed`, `asterix.cat021.ed2_7.refuse_truncated_header` | One manually derived block with one typed record; projection checks data source, descriptor, high-resolution raw coordinates, address, quality, Mode 3/A, flight level and raw-item count | Truncated two-byte header; exact stable kind/category/record/item/subitem/offset | Partial: one selected-field projection; additional items, encode, broader bounds and C/Python parity not covered |
| ASTERIX CAT021 Ed 2.4 | `asterix.cat021.ed2_4.minimal_typed`, `asterix.cat021.ed2_4.refuse_mops_version_3` | Same minimal typed record layout decoded under Ed 2.4 | I021/210 MOPS version 3 refused under Ed 2.4 with exact category, record, item, subitem and offset | Partial: selected-field projection and one edition boundary; other Ed 2.4-specific limits and C/Python parity not covered |
| ASTERIX CAT048 Ed 1.32 | `asterix.cat048.ed1_32.minimal_typed`, `asterix.cat048.ed1_32.refuse_zero_repetition` | One manually derived typed record; projection covers source, time, descriptor, polar/cartesian position, Mode 3/A, flight level, plot characteristics, address, identification and track status | Zero I048/250 repetition factor; exact category/record/item/subitem/offset | Partial: one selected-field projection and one item-level refusal; more category-level boundaries, encoding and C/Python parity not covered |
| ASTERIX CAT062 Ed 1.21 | `asterix.cat062.ed1_21.minimal_typed`, `asterix.cat062.ed1_21.refuse_latitude_over_90` | One manually derived typed record; projection checks source/time, position, Cartesian position/velocity, Mode 3/A, derived address/identification/selected altitude, track status, update ages and altitude/rate fields | I062/105 latitude one raw count beyond +90°; exact category/record/item/subitem/offset | Partial: one selected-field projection and one coordinate bound refusal; other compound boundaries, encoding and C/Python parity not covered |
| ARINC 424-18 FAA CIFP profile | — | Not covered | Not covered | Not covered: must be newly authored 132-column synthetic records; no FAA CIFP record reuse |
| NASR | — | Not covered | Not covered | Not covered: must be schema-derived synthetic rows; no FAA NASR excerpts |
| AIXM 5.1 FNS NOTAM profile | — | Not covered | Not covered | Not covered: synthetic namespaces/IDs/times and exact refusal cases required |
| FIXM/SFDPS | — | Not covered | Not covered | Not covered: only schema-authored synthetic messages; release gate awaits sealed cut |
| GRIB2 winds decoder subset | — | Not covered | Not covered | Not covered: minimal message must be authored from public templates; no real weather file excerpts |
| ASTERIX CAT034 | — | Out of 0.1; moved to 0.2 | Out of 0.1 | Excluded per current spec; unsupported-category rejection is not CAT034 conformance |
| Geodesic intersections | — | Out of 0.1; moved to 0.2 | Out of 0.1 | Excluded per current spec |

Every row above is currently Rust-checker scope only. Expected decode values must be independently
authored from a published layout/schema or a correctly licensed reference; product round-trips are
not oracle data. The checked-in case JSON documents carry per-case CC0 and derivation metadata;
the corpus manifest digest binds their exact bytes.
