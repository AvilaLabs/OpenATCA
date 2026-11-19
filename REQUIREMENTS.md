# OpenATCA requirements register

Every requirement has a stable ID. Every requirement must be linked to at least one test or Kani
harness that carries the tag `// REQ: <ID>` on the line above it. `oa-trace` fails the build if a
requirement has nothing linked, or a tag names an ID that does not exist. It checks the links
only; whether the tests pass and the proofs hold comes from running them.

IDs are never reused. A withdrawn requirement stays in the register marked WITHDRAWN.

## M0 — Airspace foundation

### General

- **OA-GEN-001** All crates forbid `unsafe` code.
- **OA-GEN-002** No crate depends on any external crate, including dev-dependencies.
- **OA-GEN-003** Every parser rejects an input that exceeds a stated limit (record length, field
  length, record count, string length) with an error naming the source, line and limit.
- **OA-GEN-004** No parser panics on any input. Arbitrary byte input produces either a value or
  an error.
- **OA-GEN-005** Every struct held in quantity as an array element asserts its exact size at
  compile time, and every index type is four bytes with one shared `NONE` sentinel (RULESET A4,
  A7).
- **OA-GEN-006** The named-proof runner records its pinned tool versions and rejects mismatched
  versions, source bindings or scoped loop diagnostics. Scoped loop presets accept only a
  registered complete identity family at the declared bounds. In fail-fast mode, the suite stops
  launching proofs after a non-passing result, reports its diagnostics, records remaining selected
  proofs as `NOT_RUN`, and cannot report an overall pass.

### CSV (oa-csv)

- **OA-CSV-001** Parses RFC 4180: quoted fields, doubled quotes inside quoted fields, CRLF and
  LF line endings, embedded newlines in quoted fields.
- **OA-CSV-002** Fields are accessed by header name; a missing column is an error naming it.
- **OA-CSV-003** Errors report the 1-based physical line number of the offending record.
- **OA-CSV-004** Rejects a record whose field count differs from the header's.
- **OA-CSV-005** A UTF-8 byte order mark is skipped however the reader splits it across reads, and bytes that
  start like one but are not one are read as data.

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
- **OA-GEO-008** In the local east-north frame (`aeqd`) a true velocity at the frame origin converts to
  the frame by the analytic differential (east times sphere radius over prime-vertical radius, north
  times it over the meridian radius) and back to 1e-6 m/s, at origin latitudes 45, 89, 89.99 and 89.9999
  degrees in both hemispheres; no finite displacement is used.
- **OA-GEO-009** The local differential of the projection at a plane point (plane step to true east and
  north metres) equals a fine central difference of the inverse projection at 45, 89, 89.99 and 89.9999
  degrees and in the south, and the velocity conversions apply that matrix and its inverse.
- **OA-GEO-010** At exactly a pole east and north are those of the limit along the frame origin's own
  meridian: east is the origin's east, north continues along that meridian through the pole. The
  differential there is finite, for the pole as origin and as a point, and mirrors in the south.
- **OA-GEO-011** The differential of a change of frame at one physical point needs no east or north; it
  agrees with the true-axes differentials of the two frames and its round trip is the identity, at
  midlatitude and near a pole.
- **OA-GEO-012** A covariance is carried through a linear map as `M S M^T`.
- **OA-GEO-013** The inverse projection refuses (returns no point for) a distance from the origin that
  is not finite or is within `ANTIPODE_MARGIN_RAD` (OA-GEO-017) of half the sphere's circumference; a huge
  finite east or north does not overflow into a NaN position.
- **OA-GEO-014** The crate's production sources call no platform maths function (`sin`, `atan2`,
  `hypot`, `powf`, `mul_add` and the rest of a fixed list); the elementary functions are
  `oa-math`'s, so a result is the same bits on every IEEE-754 machine. The check is itself tested
  against a planted call.
- **OA-GEO-015** The small-angle kernels (sine and cosine to 0.25 rad, arctangent to 0.2, the angle
  of a sine and cosine pair) agree with `oa-math` to a few units in the last place over their
  range, and hand over to `oa-math` outside it.
- **OA-GEO-016** The local frame's forward and inverse projection agree with the library-function
  form to 2e-12 degrees and 2 micrometres at every origin latitude and out to 12,000 km; a frame
  recentred a hundred thousand times from the sine and cosine of its latitude carried with it keeps
  them within 1e-13 of those of its origin, and a point that cannot carry them (near a pole, across the
  antimeridian, far from the origin) takes the frame `frame_new` makes.
- **OA-GEO-017** The chart's domain is the angular distance `c < PI - ANTIPODE_MARGIN_RAD` (1e-5 rad, 64 m) about the
  origin, where the tangential scale `sin c / c` is 3.2e-6 or more and the 1e-6 floor an earlier version
  put under it is never reached. `unproject` and the differentials (`try_frame_to_true`, `try_frame_from_true`,
  `try_frame_change`) refuse past it with `None`; `frame_to_true`, `frame_from_true`, `frame_change` and the
  velocity conversions then return NaN. Inside it the differential is exact, with no floor.

- **OA-GEO-018** Ellipsoid flattening accepts both endpoints of [0, 0.01] and rejects the next
  representable value above the upper endpoint.
- **OA-GEO-019** The direct problem rejects a non-finite distance or azimuth with an error naming
  the invalid quantity.
- **OA-GEO-020** Degree sine and cosine return exact quadrant values with positive zero at axes.
- **OA-GEO-021** Degree arctangent returns exact axis and diagonal angles and preserves the sign
  of zero when it determines the half-turn direction.
- **OA-GEO-022** Angle normalization uses (-180, 180], while the compensated remainder retains
  the sign of an exact half-turn.
- **OA-GEO-023** Compensated angle differences preserve the direction of exact half-turns.
- **OA-GEO-024** The compensated sum retains a unit lost when adding it to a much larger value.
- **OA-GEO-025** Sub-grid angular cleanup preserves signed zero and angles outside its grid.
- **OA-GEO-026** A displayed geodesy error retains the failed quantity or supplied refusal reason.
- **OA-GEO-027** Oblique WGS-84 rhumb lengths and courses agree with independent meridian and
  isometric-latitude integrals, including near a pole and across the antimeridian; the direct
  problem reaches the reference endpoint within 1e-8 degrees.

- **OA-GEO-028** The closest-point fallback scan agrees within one micrometre with analytic
  spherical distances for an on-segment point, an interior perpendicular foot and a nearest endpoint.

### Elementary functions (oa-math)

- **OA-MATH-001** `sin`, `cos` and `tan` are faithfully rounded (error below 1 ulp, measured
  against a correctly rounded mpmath reference) for every finite argument tested, including
  arguments near multiples of pi/2, the hardest known cases for argument reduction, subnormals and
  magnitudes up to the largest finite double.
- **OA-MATH-002** `atan`, `asin`, `acos` and `atan2` are faithfully rounded over their whole
  domains against the same kind of reference, including arguments near 1, subnormals and ratios
  beyond 2^60.
- **OA-MATH-003** `exp` and `ln` are faithfully rounded over their whole domains, including the
  subnormal results of `exp`, the overflow and underflow limits, subnormal arguments of `ln` and
  arguments near 1.
- **OA-MATH-004** `pow` is faithfully rounded wherever the result is finite or subnormal, including
  where |y ln x| is near the overflow limit, huge exponents with x near 1, negative x with integer
  y, and subnormal results.
- **OA-MATH-005** `hypot` and `cbrt` are faithfully rounded over their whole domains: `hypot` with
  no intermediate overflow or underflow and the result overflowing only when it does not fit,
  `cbrt` for both signs and subnormals.
- **OA-MATH-006** `sin`, `cos`, `sin_cos` and `tan` return the IEEE special values: signed zeros
  are preserved by `sin` and `tan`, an infinite argument gives NaN, a NaN argument gives NaN,
  subnormal arguments return `x` (`sin`, `tan`) or 1 (`cos`), and the largest finite argument is
  reduced rather than rejected.
- **OA-MATH-007** `atan`, `asin` and `acos` return the IEEE special values: `atan(+-inf) = +-pi/2`,
  signed zeros preserved by `atan` and `asin`, `acos(1) = +0`, `acos(-1) = pi`, an argument
  outside [-1, 1] or NaN gives NaN.
- **OA-MATH-008** `atan2` follows C99 Annex F for every combination of zeros (of either sign),
  finite values, infinities and NaN, and returns the correctly signed result for ratios that
  underflow or overflow.
- **OA-MATH-009** `exp` and `ln` return the IEEE special values: `exp(+-0) = 1`, `exp(+inf) = +inf`,
  `exp(-inf) = +0`, overflow to `+inf` above 709.782712893384, underflow to `+0` below
  -745.1332191019411; `ln(+-0) = -inf`, `ln(x<0) = NaN`, `ln(+inf) = +inf`, `ln(1) = +0`.
- **OA-MATH-010** `pow` follows C99 Annex F for every combination of zeros, ones, negative bases,
  odd and even integer exponents, non-integer exponents, infinities and NaN, including
  `pow(x, +-0) = 1` and `pow(1, y) = 1` for NaN operands.
- **OA-MATH-011** `hypot` returns `+inf` when either argument is infinite even if the other is NaN,
  and `cbrt` and `powi` return the IEEE special values for zeros, infinities, NaN and the extreme
  exponents `i32::MIN` and `i32::MAX`.
- **OA-MATH-012** Over 200,000 inputs of every magnitude, `sin(x)^2 + cos(x)^2` is within 4.5e-16 of 1.
- **OA-MATH-013** `sin_cos(x)` equals `(sin(x), cos(x))` bit for bit for every input tested,
  including NaN and infinity bit patterns.
- **OA-MATH-014** `atan2` returns a result in the half-plane of the sign of y and in the correct
  half of it for the sign of x, is odd in y bit for bit, and satisfies atan2(y, -x) = pi - atan2(y, x)
  and agreement with `atan(y/x)`.
- **OA-MATH-015** `exp(ln(x))` returns `x` to within the conditioning of the round trip, and
  `asin(sin t)`, `acos(cos t)`, `atan(tan t)`, `cbrt(x)^3`, `hypot` and `pow(x, y) * pow(x, -y)`
  agree with their identities to within a few ulps.
- **OA-MATH-016** `powi(x, n)` gives exact results where the exact result is representable: every
  power of two from 2^-1074 to 2^1023 (subnormals included) from either sign of `n`, and small
  integer powers.
- **OA-MATH-017** `tan(x)` agrees with `sin(x) / cos(x)`, and `pow` agrees with `sqrt` and with
  `x * x` for the exponents 1/2 and 2, to within a few ulps.
- **OA-MATH-018** A hash of the output bits of one million evaluations across all functions on a
  fixed pseudo-random input sequence equals a pinned value; the same value is produced on every
  target the crate is built for (x86-64 and wasm32 checked).
- **OA-MATH-019** The crate sources contain no call to a platform maths function (`sin`, `exp`,
  `powf`, `floor`, `mul_add` and the rest of a fixed list, as methods, `f64::` calls or bare
  `f64::` paths used as values), no float `%` (only the listed integer shifts and the exact `fmod` line of `rem_euclid`), no `unsafe`, and the
  manifest lists no dependency; the check is itself tested against planted calls.
- **OA-MATH-020** Every NaN returned has one fixed bit pattern (positive quiet NaN), whatever the
  NaN bits of the input.
- **OA-MATH-021** Every function has a fast path in plain double arithmetic whose error bound is
  derived in the source; it answers only when the rounding of the result is certain, and otherwise
  the double-double path answers. Over at least one million random inputs per function (typical
  ranges, the full range of exponents, near 1, exact results) and the edge values, the public
  function and the fast path (when it answers) equal the double-double path bit for bit.
- **OA-MATH-022** The fast path answers for at least a stated fraction of the committed reference
  inputs and of 300,000 random inputs of a typical range, per function (sin/cos 0.97, `sin_cos`
  and `tan` 0.94, atan, atan2, asin, acos 0.97, exp 0.95, ln and hypot and cbrt 0.99, pow 0.91
  on the random inputs; 0.38 to 0.97 on the committed inputs, which are mostly extreme).
- **OA-MATH-023** On the committed reference data the maximum error of every function is at most
  0.5 ulp plus 1e-5 and no result differs from the correctly rounded double (`pow`: at most one
  case, a result within 2^-70 of a midpoint); the check fails when one result is moved by one ulp;
  the 600 arguments below 2^19 closest to a multiple of pi/2 give correctly rounded sin, cos and
  tan.
- **OA-MATH-024** `powi(x, n)` never forms the power of opposite sign first: it is `m^|n|` in
  double-double (reciprocal for negative `n`) times `2^(e n)` with `x = m 2^e`, rounded once, so
  `powi(2, -1024)` is the subnormal 2^-1024 and `powi(2, -1075)` the tie 2^-1075, which rounds to zero.
- **OA-MATH-025** `powi(NaN, 0) = 1`; every other NaN result of `powi` is the canonical NaN, whatever
  the payload or sign of the input.
- **OA-MATH-026** `scale_round` decides a subnormal half-way case from both parts of the
  double-double: a low term below (above) the tie rounds down (up) even when the sum of both rounds
  to the tie.
- **OA-MATH-027** `hypot(x, x)` for `x = from_bits(A*B)`, `A = 54,608,393`, `B = 38,613,965`
  returns `A*A` as bits, the exact value being just below a half-way case.
- **OA-MATH-028** `powi(x, n)` agrees with `pow(x, n)` to within two ulps over random bases and
  exponents up to 10,000 in magnitude.
- **OA-MATH-029** The helpers the fast paths build on are exact where they say so: the square, the
  cut product and the product with a multiple of 1/16 equal the exact double-double product (the
  cut product up to 2^-77), the multipliers of the `ln` table have at most 12 significant bits, the
  quadrant sign bits of `sin` and `cos` follow the quadrant table, and the case tables built at
  compile time equal the same double-double sums made at run time.
- **OA-MATH-030** Proved (Kani, `quadrant_signs_follow_the_quadrant_table`): for every quadrant 0 to 3 and
  every sign bit of y and of x, `sin_sign` and `cos_sign` equal the quadrant table (sin negative for y,
  never, not y, always; cos the sine of the next quadrant).
- **OA-MATH-031** Proved (Kani, `magic_number_rounds_half_even`): for every double of magnitude below 2^51,
  the magic-number sum with 1.5 * 2^52 leaves the value rounded to an integer, ties to even, in the low
  bits (`magic_int`) and as the double `t - ROUND_MAGIC`.
- **OA-MATH-032** Proved (Kani, `cut_split_halves_are_exact_and_short`): the split of `two_prod_cut` gives
  a high half of at most 26 bits and a low half of at most 27 bits of the mantissa, each exact, whose sum
  is the input, for every normal double whose low half is normal.
- **OA-MATH-033** `rem_euclid(a, b)` equals the standard library's `f64::rem_euclid` bit for bit on two
  million inputs (bearing wraps, mixed signs, arbitrary bit patterns, wide exponent range) and on the
  edge values, a NaN result excepted, which has the crate's one fixed bit pattern.
- **OA-MATH-034** `canon_nan(x)` returns the one fixed NaN (`0x7ff8000000000000`) for every NaN, including those
  an invalid operation makes at run time (`0.0 / 0.0`, `inf - inf`, `inf * 0.0`, `oa_ieee::sqrt(-1.0)`, tried
  under `black_box` so the compiler cannot fold them), negative and payload NaNs, and every other value
  (signed zeros, subnormals, infinities) unchanged bit for bit.
- **OA-MATH-035** `cmp_canon_f64`/`_f32` order two floats by the IEEE-754 total order after both went through
  `canon_nan` (`-inf < negatives < -0.0 < +0.0 < positives < +inf < NaN`, every NaN equal to every other, a
  negative or payload NaN included); `is_sign_negative_canon_f64`/`_f32` read the sign bit after `canon_nan`
  (true for `-0.0`, false for every NaN); `copysign_canon_f64`/`_f32` give the magnitude of the first
  argument with the sign of the second after `canon_nan` (a NaN magnitude gives the canonical NaN). Core
  code calls these in place of `total_cmp`, `is_sign_negative`, `is_sign_positive` and `copysign`, whose
  results depend on the sign and payload of a NaN (see OA-TRC-247).
- **OA-MATH-036** `min_f64`/`max_f64`/`clamp_f64` and the `f32` forms ignore a NaN argument (the other is
  returned; two NaNs, or a NaN `x` in `clamp`, give the canonical NaN), return `-0.0` from `min` and `0.0`
  from `max` for the two zeros in either argument order, and never panic (`clamp` with `lo > hi` gives `hi`).
  Tested for every combination of sign, zero, infinity and NaN. Core code calls these in place of the
  float `min`, `max` and `clamp` (see OA-TRC-251).

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
- **OA-424-008** A field outside its character class or range (bearings and courses over 360
  degrees, magnetic variation over 180, section and subsection codes, fix identifiers, the
  IFR flag, the header production flag) is an error naming the field.

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

- **OA-ASP-008** The selection area's bounding filter never excludes a point that is inside the
  area or its buffer: it is correct for areas and volumes crossing the antimeridian and includes
  the poleward bulge of every geodesic edge. A boundary that cannot be turned into an outline is
  an error, never read as "does not touch".

- **OA-ASP-009** A source record that is incomplete or out of range in a way that would change
  its meaning is a build error naming the record, never a stored default: an airspace circle or
  arc without a usable radius, a procedure leg course outside 0 to 360 degrees.
- **OA-ASP-010** An airway leg ends at the next row only when that row belongs to the same
  airway, lies further along it and starts at the point the leg names as its end; otherwise the
  leg has no end point and the report lists it. A gap flag is stored with its leg.
- **OA-ASP-011** A NASR airport and a CIFP airport with the same identifier are merged only
  within 1 NM (`MAX_AIRPORT_PAIR_M`); a farther pair is two objects, listed in the report as a
  disagreement, and the report states the cap.
- **OA-ASP-012** The snapshot writer enforces the limits the reader enforces (ARTCC length,
  file size), so every file it writes can be read back; a model whose column values are out of
  range (coordinates, altitudes, bearings, codes) fails validation.
- **OA-ASP-013** The `oa-adapt` command line rejects unknown, duplicate and valueless flags,
  empty values and malformed identifiers, and parses SHA-256 values as exactly 64 hexadecimal
  digits.
- **OA-ASP-014** A NASR navaid is unique by its own key (id, type, city, state) and is never
  dropped because its derived (ident, region) equals another's: where identities would
  collide, the NASR navaids among them take a scope (the airport of a terminal NDB, else
  state and city). A NASR navaid pairs with a CIFP navaid (D/V, D/B or P/N) only when it is
  the same ident and class and the nearest, within 1 NM (`MAX_PAIR_M`); two CIFP candidates in
  the cap, or one CIFP navaid wanted by two NASR navaids, pair nobody and are reported; a
  farther same-ident navaid is reported with its distance. A CIFP reference resolves by its
  own ident, region, section and airport, never to another station's point.
- **OA-ASP-064** A special-use boundary keeps its source time code and level: `time_code` is
  continuous, part-time (blank in the source) or unknown, and `level` is high, low, both or
  unknown; a boundary from NASR or a controlled-airspace volume, and any other source letter,
  is unknown, never a default. The columns are in the snapshot (format version 2 and later); a version 1
  snapshot is refused with a typed version error.

- **OA-ASP-065** A CIFP or NASR boundary touches the selection area exactly when a point of its
  outline is in the area (inside a polygon or within the buffer of an edge), an outline edge crosses
  an edge of the area, a vertex of the area is within the buffer of an outline edge, or a vertex of
  the area is inside the outline. The outline is the stored boundary as a dense polyline: geodesic
  and rhumb edges as what they are, within 5 km of a point, arcs within 0.2 m of the arc. Two
  boundaries that overlap with no vertex of either inside the other are selected.
- **OA-ASP-066** A MORA cell is selected exactly when it overlaps the area: a polygon vertex lies
  in the cell, or the cell's outline (meridians and parallels) has a point in the area, crosses an
  edge of the area, or comes within the buffer of a vertex of the area; a strip thinner than the
  spacing of any sample grid is not missed.
- **OA-ASP-067** A NASR runway end with a displaced threshold (length above zero) and no
  displaced-threshold coordinate makes no THRESHOLD point (the physical end is not the threshold);
  the report lists its airport and runway as "threshold unknown: displaced, no coordinate". A CIFP
  threshold for the same runway takes precedence and the end is not listed; a displacement of zero
  uses the physical end.
- **OA-ASP-068** Selection uses an arc as stored: the coded radius around the stored centre. The
  report lists, per selected volume, every arc whose coded radius differs from the distance between
  its centre and either end point by more than 93.6 m (0.05 NM, half the 0.1 NM coding unit, plus
  1 m for whole-metre storage and 1e-7 degree positions).
- **OA-ASP-069** A CIFP volume whose records end without the boundary-end marker and whose first
  and last points differ is stored marked incomplete (`Boundaries::incomplete`; the source does
  not say how it closes, so the outline is not precise) and is still selected; a volume that ends
  with the marker, or returns to its first point, or is a single circle, and every NASR boundary,
  is complete. `boundary_is_complete` returns the mark, the diff lists it, and the report lists
  every selected incomplete volume.
- **OA-ASP-070** The snapshot format version is 3 (per-volume completeness added); a snapshot of an
  earlier version is refused with an error that names the version and says to rebuild, never read
  with a default completeness.
- **OA-ASP-071** A procedure resolves a CIFP terminal waypoint (P/C) first to the terminal waypoint
  of its own airport, then to an enroute fix; a terminal waypoint of another airport is never used,
  and a reference that matches neither is unresolved and counted. Two airports' terminal waypoints
  with one identifier and region at different positions are two points (the second scoped to its
  airport) and are not reported as duplicates; a terminal waypoint repeated at one airport with
  different positions is. Runways, localizers and terminal NDBs are looked up by airport the same way.
- **OA-ASP-072** The build computes the AIRAC cycle (28-day, epoch 2026-01-22 is cycle 2601) of the
  NASR effective date and requires it to equal the CIFP header cycle; otherwise it refuses with
  `BuildError::CycleMismatch` naming both cycles. With `allow_cycle_mismatch` (`oa-adapt build
  --allow-cycle-mismatch`) it builds and the model carries the mismatch text (`cycle_override`),
  which is written in the snapshot (format version 4; a snapshot without a mismatch stays version
  3 and byte-identical), shown in the report and listed by the diff.
- **OA-ASP-073** An airport is its identifier plus its ICAO region everywhere in the build: the airport
  registry, CIFP runway, localizer, terminal waypoint, terminal NDB and procedure joins, airport references in
  procedure legs, the order of procedures and `find_proc`. Two airports with one identifier in two regions keep
  their own runways and procedures.
- **OA-ASP-074** Boundary sampling and testing are bounded before any point is made: a boundary needing more
  than `MAX_BOUNDARY_SAMPLES` points, or a sample set whose edge tests would pass `MAX_TOUCH_WORK`, is refused
  with an error.
- **OA-ASP-075** Boundary records sharing a key (source, stratum, facility, multiple, name) merge only when
  identical. Records that differ are a conflict: all are refused, counted (`boundary_conflicts`) and named in the
  boundary notes, whatever the input order.
- **OA-ASP-076** A coordinate whose round-trip error could not be measured is counted (`pos_unverified`) and
  reported, not taken as an error of zero.
- **OA-ASP-077** NASR procedures and class airspace rows whose airport is not in the sources are counted in the
  skipped list.
- **OA-ASP-078** The calendar helper refuses impossible dates: year 1 to 9999, month 1 to 12, a day that exists
  in the month; `airac_cycle` returns `None` for them.


### Traceability (oa-trace)

- **OA-TRC-001** Lists every requirement without a test or proof and every test or proof tag naming an
  unknown requirement, and exits non-zero if either list is non-empty. A Kani harness (`// REQ:` above
  `#[kani::proof]`, with an optional `#[kani::unwind(n)]`) counts as a proof harness and the report marks it
  as one, apart from tests.
- **OA-TRC-100** A `// REQ:` tag above `#[kani::proof]` (or an unwind bound and then the proof, in either
  order) is accepted as a proof harness and listed as one; a tag above an unwind bound with no proof is malformed.
- **OA-TRC-101** The matrix status of a requirement says only what was checked: "test linked" for a tagged
  test, "proof harness linked" for a tagged Kani harness, "test and proof harness linked" for both. It never
  says "verified" or "proved", and a harness whose body asserts false is still "proof harness linked".
- **OA-TRC-102** The last line of the text report says it is a traceability result (every requirement linked,
  ids unique, no test or proof run) and ends in `PASS` or `FAIL`.
- **OA-TRC-103** No source file under `crates/*/src` builds a float from text or bytes (`parse::<f64>`, `from_bits`,
  `from_le_bytes`, an untyped `parse` into a float and the `f32` forms) except where a per-line allowlist gives the
  reason the value cannot reach the program as NaN, infinity or an out-of-range number; an allowlist entry that
  matches no line is itself a failure.
- **OA-TRC-104** For Python source, `oa-trace` discovers only `test_*.py` modules and links a `# REQ:` tag directly above a
  Python `def test_*` as a test (not a proof), and reports malformed IDs and tags attached to non-test
  definitions.
- **OA-TRC-200** `cargo run -p oa-trace` reads the class of every workspace member and passes a workspace
  whose crates are all classified and whose core code is clean (RULESET B7 to B9). Core code excludes
  top-level `src/bin/`, `src/main.rs` and test-only items.
- **OA-TRC-201** A member with no `[package.metadata.openatca] class`, or one other than `core`, `shell`, `split`
  or `unsplit`, fails the check and the failure names the crate.
- **OA-TRC-202** Core code (all of `src/` for a core crate, `src/core/` and `src/core.rs` for a split crate) may
  name from `std`, `core` and `alloc` only the modules RULESET B8 allows; any other (`fs`, `io`, `net`, `env`,
  `process`, `thread`, `sync`, `cell`, `os`, `path`, `std::time`, the hash-map family, `alloc::sync`) fails, each hit
  is reported as `file:line: token` with the allowed alternative, and `static mut`, `thread_local!`, the print
  and `dbg!` macros, `HashMap`, `HashSet` and `RandomState` fail. A path split over lines or inside a
  `use std::{..}` group is still found.
- **OA-TRC-203** Core code that names a path into a `shell` module (`crate::shell`, `super::shell`,
  `<crate>::shell`, or `shell` in a `use` group) fails.
- **OA-TRC-204** Comments (line, block, nested), string literals (plain and raw), and character literals are
  blanked before the scan, so a forbidden name inside them is not a hit; lifetimes are not mistaken for
  character literals; line numbers are kept.
- **OA-TRC-205** A core crate lists no shell or unsplit crate in any dependency table; core code names a
  workspace crate (under its own name or a rename) only if it is core, or by its `::core::` path if it is split.
- **OA-TRC-206** A split crate has `src/core` (or `core.rs`) and `src/shell` (or `shell.rs`), nothing else but
  `lib.rs`, `main.rs` and `bin/` in `src/`, and a `lib.rs` that declares exactly `mod core;` and `mod shell;`
  plus re-exports and attributes.
- **OA-TRC-207** Every `unsplit` crate is in the fixed list of twelve in `classcheck.rs`; the count is printed as
  migration debt; the allowlist file may hold only crates of that list that are still unsplit, so the debt
  only shrinks (RULESET B9).
- **OA-TRC-208** `oa-trace` reads the package name, class, every dependency table (including `[target.*]`, with
  `package` renames and `path`), `[lib]`/`[[bin]]` paths, a `build` key and the root `members` and `exclude`
  lists with its own small reader, without a dependency.
- **OA-TRC-209** `members` is read in either quote style across lines; zero members, an entry that is not a plain
  quoted path (a glob included) and a `crates/`, `tools/` or `validation/` directory with a Cargo.toml that is
  neither a member nor excluded all fail.
- **OA-TRC-210** An unreadable directory or file, a symlink under a scanned source tree, a core or split crate
  that scans no file, a custom `[lib]` or `[[bin]]` path, and a `build.rs` or `build` key in a core or split
  crate fail.
- **OA-TRC-211** `#[cfg(test)]`, `cfg(all(test, ..))` and an inner `#![cfg(test)]` strip an item only when an item
  keyword follows; on a field, variant, match arm, parameter or statement nothing is stripped, and
  `cfg(any(test, ..))` and `cfg(not(test))` strip nothing.
- **OA-TRC-212** In core code, a `use` that renames (`as`) or globs (`*`) a std, core, alloc or workspace-crate
  path, a bare `use std;` or `use <crate>;`, a leading `::` path, `extern crate`, `#[path]` and the macros
  `include!`, `include_str!`, `include_bytes!`, `env!`, `option_env!` and `file!` fail.
- **OA-TRC-213** In a split crate, a third module, a `crate::` path outside `crate::core::` and a `super::` that
  reaches the crate root fail from core code; `src/core.rs` is scanned like `src/core/`.
- **OA-TRC-214** No file is exempt by name; a test-only `mod x;` resolves the way rustc resolves it, together with
  the files below it; only the top-level `src/bin` and `src/main.rs` are skipped.
- **OA-TRC-215** A core crate depending on a shell or unsplit crate fails whichever dependency table, rename or
  path form declares it; a path dependency that is not a workspace member fails.
- **OA-TRC-216** The check passes on the real workspace, and the unsplit crates are exactly those still listed
  in the allowlist file.
- **OA-TRC-217** In core code the only `extern crate` is `extern crate alloc;`.
- **OA-TRC-218** In core code a float method `core` lacks fails, with the alternative named (`oa_ieee::x` for sqrt,
  floor, ceil, trunc and round; `oa_math::pow` for `powf`; `oa_math::x` for the rest; for `rem_euclid` and
  `div_euclid` the integer path form `i64::rem_euclid(a, b)` or `oa_math` for floats) and the note that a
  user-defined method of that name is disallowed too. Only the crate `oa-ieee` is exempt. See OA-TRC-221 for
  the list and the call forms.
- **OA-TRC-219** Every crate of class core except `oa-ieee` has `#![no_std]` or `#![cfg_attr(not(test), no_std)]`
  in `src/lib.rs` (RULESET B10).
- **OA-TRC-220** The public API of `oa-ieee` is exactly `pub fn` sqrt, floor, ceil, trunc and round: any other
  public item, module, re-export or macro, or a missing function, fails.
- **OA-TRC-221** The float list in `classcheck.rs` equals the set of `f32`/`f64` inherent methods absent from `core` on
  the pinned toolchain, derived from a `no_std` compile probe of std's documented methods. A listed name after
  `.` (called) or after `::` (whatever precedes it: `f64::sin`, `<f64>::sqrt`, `Self::sin`, `R::sin`, called or not)
  fails; `oa_ieee::x`, `oa_math::x` and `i64::rem_euclid(a, b)` pass.
- **OA-TRC-222** In core code `macro_rules!`, the `$` token, a raw identifier, `cfg!(target_..)`, `#[cfg(target_..)]`,
  `cfg_attr(target_..` (and `unix`, `windows`), a raw pointer type and `{:p}` in a string literal fail.
- **OA-TRC-223** The `src/lib.rs` of a core crate declares no module named `main` or `bin`, in any visibility, inline or
  in a file.
- **OA-TRC-224** A `#[cfg(test)] mod x;` inside an inline module skips nothing: the file is scanned.
- **OA-TRC-225** Every core-crate dependency, in any table, must resolve to a core workspace crate: `workspace = true`
  entries are resolved through the root `[workspace.dependencies]`, a `[lib] name` is the name source uses, and a
  dependency that cannot be resolved (an outside crate, a missing root entry) fails; a split crate may depend on
  shell crates but not on outside code.
- **OA-TRC-226** In split core code `self::super::` and any `super` after `self::` counts as `super`.
- **OA-TRC-227** The `no_std` attribute of a core crate is an inner attribute at the top of `src/lib.rs`, before any
  item and outside any module or cfg block; the failure says the crate "must have" it.
- **OA-TRC-228** Each public function of `oa-ieee` has the body `v.<same name>()` exactly.
- **OA-TRC-229** A workspace `exclude` entry that does not normalise fails.
- **OA-TRC-230** `lib.rs` of a split crate carries only `#![forbid(unsafe_code)]` and `doc` attributes: an
  attribute on `mod shell` or `mod core` (`#[macro_use]`, `#[allow]`, `cfg_attr`) and an inner `allow` fail; a
  `#[macro_export]` in shell code fails; core code has no `#[macro_use]` or `#[macro_export]`.
- **OA-TRC-231** Shell code of a split crate may not `impl` a type or trait that its core module defines,
  spelled by name, by path (`crate::core::`, `super::core::`), through a `use .. as` rename or through a type
  alias; its own types, std traits on them and a core type as a trait argument are allowed.
- **OA-TRC-232** Every core and split crate declares `[lints] workspace = true` and has `#![forbid(unsafe_code)]`
  as an inner attribute at the top of `src/lib.rs` (`oa-ieee` included); core code has none of the tokens
  `unsafe`, `extern`, `#[link`, `no_mangle`, `export_name`, `link_section`.
- **OA-TRC-233** Core code may not silence the type-name lint (`non_camel_case_types`, `nonstandard_style`,
  `bad_style`) or define a type alias that is not CamelCase; `X::<float method>` with a qualifier `X` that is
  not `oa_math`, `oa_ieee`, a core crate, `self`/`super`/`crate`, a module the crate declares or (for the
  Euclidean pair) an integer type fails.
- **OA-TRC-234** Core code may not call `as_ptr`, `as_mut_ptr`, `.addr(`, `expose_provenance`, `ptr::eq`,
  `ptr_eq`, name `usize`/`isize` `MAX`, `MIN` or `BITS`, use `size_of`/`align_of` outside a `const _: () =
  assert!(..)` size guard, use `concat!`, `stringify!` or `format_args!`, or cast to a fn pointer.
- **OA-TRC-235** A core `pub fn` (`pub(crate)` included) whose signature names `Fn`, `FnMut`, `FnOnce` or `dyn`,
  takes `impl Trait` as a parameter or has a bound or `where` clause fails unless
  `tools/oa-trace/core-callbacks-allowlist.txt` lists it as `crate file.rs::fn reason`; an entry that matches no
  signature, has no reason or is malformed fails.
- **OA-TRC-236** `.to_bits()` in core code (tests excepted) has the receiver `canon_nan(..)`; `oa-math` is exempt.
- **OA-TRC-237** The platform float-method calls in non-test code of an unsplit crate may not exceed the number
  recorded for it in `tools/oa-trace/unsplit-float-budget.txt`; a missing, stale, doubled or malformed entry
  fails; the counts are printed.
- **OA-TRC-238** A dependency inline table over several lines is read as one entry of its own key.
- **OA-TRC-239** `rust-toolchain.toml` pins 1.89.0 with the components `clippy` and `rustfmt`, as the CI workflow
  installs it; `fuzz/rust-toolchain.toml` pins the fuzz nightly.
- **OA-TRC-240** A turbofish (`::<..>`, any spacing) between a method name and its parentheses changes no rule:
  float methods, `.addr(`, `to_bits` and the unsplit float count all still match.
- **OA-TRC-241** A core `pub fn` inside an `impl` whose generics or `where` clause carry a bound, with a bare
  `fn(..)` pointer type in its signature, and a `pub struct`, `enum`, `union` or `type` holding an `Fn*`, `dyn`
  or `fn(..)` type need an allowlist line like any other callback.
- **OA-TRC-242** `canon_nan` and `canon_nan_f32` in core are `oa_math`'s: qualified through it or imported from
  it; a fn, binding, method or import of that name from anywhere else fails. `f64::to_bits(x)`, `f32::to_bits(x)`
  and `to_le_bytes`/`to_be_bytes`/`to_ne_bytes` on a float (a literal, an `as f64` cast, a name the same fn
  declares `f32`/`f64`, a struct field declared so, or the UFCS form) fail unless the value passed `canon_nan`.
- **OA-TRC-243** `&raw const`/`&raw mut`, the identifier `Pointer`, `type X = usize;` and `isize`, `<usize>::MAX`
  and a `cfg`, `cfg!` or `cfg_attr` naming `debug_assertions` fail in core code; `debug_assert!`, `alloc::format`
  and `format!` pass; `core::hash`, `core::cell` and `alloc::rc` still fail.
- **OA-TRC-244** Every `pub trait` (unrestricted `pub`, `unsafe` included) in core code is sealed, meaning one of
  its supertraits is reached through a module the crate declares non-public (`mod sealed { pub trait Sealed {} }`
  and `pub trait T: sealed::Sealed`, or `Sealed` imported through that module), or is listed in
  `tools/oa-trace/core-callbacks-allowlist.txt` as `crate file.rs::TraitName reason`. A `pub use` of the sealing
  trait and a blanket `impl<X> sealed::Sealed for X` un-seal it and fail. The sealing trait itself, `pub(crate)`
  and private traits and test-only ones are not listed.
- **OA-TRC-245** In an item the shell can reach (plain `pub`; a method of a `pub trait`; a `pub` field), every
  generic bound (angle brackets, `where`, the `impl` block around a method) and every `impl Trait` argument is
  `Copy`, `Sized`, `?Sized`, a lifetime or a sealed trait of a core crate; `Clone`, `PartialEq`, `Eq`,
  `PartialOrd`, `Ord`, `Default`, `Debug`, `Hash`, `Iterator`, `Into` and every other trait fail unless the
  allowlist names the item. Private and `pub(crate)` items may carry any bound, a type parameter with no bound
  is not checked, and an `impl` of a trait for a type is not checked.
- **OA-TRC-246** `Fn`, `FnMut`, `FnOnce`, `dyn` and a `fn(..)` pointer in the signature of a reachable item (OA-TRC-245),
  in a `pub` field of a `pub` struct or union, in the body of a `pub enum` or `pub type`, or by naming a type
  alias of the same crate (private or not) whose body holds one, fail unless the allowlist names the item.
  Private and `pub(crate)` items, and bodies, may use closures, `dyn` and fn pointers.
- **OA-TRC-247** `total_cmp`, `is_sign_negative`, `is_sign_positive` and `copysign` are banned in core code (`oa-math`
  excepted), method or path, and so is `signum` on a float (an integer's `signum` is not seen); the failure names
  `oa_math::cmp_canon_f64`/`_f32`, `is_sign_negative_canon_f64`/`_f32` and `copysign_canon_f64`/`_f32`, which
  take the sign and order after `canon_nan`. Every use in the real core crates is migrated.
- **OA-TRC-248** `to_bits` and `to_le_bytes`/`to_be_bytes`/`to_ne_bytes` on a float are found also as a path used
  as a value (`.map(f64::to_bits)`), as `<f64>::to_bits(..)` and `<f32>::..`, on a tuple-struct float field
  (`struct P(f64)`, `self.0`), on a name bound by a tuple pattern with float types (`let (a, b): (f64, f64)`) or
  a tuple type, on a name bound by an untyped `let` whose initializer mentions a float literal, a float-typed
  name, a float field, a float method or function or an `as f64` cast (an integer cast at the end of the
  initializer, or a later `let` of the same name, makes it not a float), and on a field, tuple field, function
  return type or constant that any other core crate declares as a float. Whatever is left is stated in the
  RULESET's known lexical limits.
- **OA-TRC-249** `into_raw`, `into_raw_parts`, `from_raw`, `leak`, `as_ptr`, `as_mut_ptr`, `as_ptr_range`,
  `as_mut_ptr_range`, `addr_of`, `addr_of_mut`, `.addr(`, `expose_provenance` and `with_exposed_provenance` fail
  in core code.
- **OA-TRC-250** The `src/lib.rs` of every crate of class core (`oa-ieee` included) has, at the top level and outside
  test code, `#[cfg(not(target_pointer_width = "64"))]` followed by `compile_error!(..);`, so the crate does not
  build for a target whose `usize` is not 64 bits. That is the one target cfg core code may have; any other
  still fails. The check reads the string, so `"32"`, a missing `compile_error!` or a guard inside a module fails.
- **OA-TRC-251** A float `min`, `max` or `clamp` (a float receiver or float argument by the evidence of
  OA-TRC-248, or the path `f64::max`, `<f32>::min` and so on, also as a value in `fold(.., f64::max)`) fails in
  core code (`oa-math` excepted), with `oa_math::min_f64`/`max_f64`/`clamp_f64` (and the `f32` forms) named, whose
  results for the zeros and the NaN are defined (OA-MATH-036). Integer and iterator `min`/`max`/`clamp` pass.

### Hashing (oa-sha256)

- **OA-SHA-001** Computes SHA-256 per FIPS 180-4, matching every NIST CAVP SHA-256 byte-oriented
  test vector (short, long and Monte Carlo messages).
- **OA-SHA-002** Hashes streamed input incrementally with the same result as one-shot hashing,
  for every split point of the test messages.
- **OA-SHA-003** Matches the straightforward reference implementation on 10,000 deterministic
  pseudo-random messages of 0 to 2,000 bytes, hashed one-shot and in chunks at random split points.
- **OA-SHA-004** Reports throughput in MB/s at 1 KiB, 64 KiB and 8 MiB (release builds only,
  informational, no assertion on speed) alongside the system `sha256sum` when present.

### Archives (oa-zip)

- **OA-ZIP-001** Reads ZIP archives (central directory, local headers, stored and deflate
  entries), rejecting encrypted, multi-disk and ZIP64-beyond-limits archives with a clear error.
- **OA-ZIP-002** Inflates DEFLATE streams per RFC 1951 (stored, fixed and dynamic Huffman
  blocks), verifying each entry's CRC-32 and uncompressed size.
- **OA-ZIP-003** Enforces limits on entry count, uncompressed size per entry and total, and
  compression ratio, so a crafted archive cannot exhaust memory or disk.
- **OA-ZIP-004** A ZIP64 extra field must hold exactly the values whose central-directory field
  is 0xFFFFFFFF, once; any other combination is rejected. An archive entry cannot be built
  outside the parser.

- **OA-ZIP-005** The streaming inflater (`Inflater`) takes input in pieces of any size and gives
  exactly the output of the one-shot `inflate` for every chunking from 1 byte up, for stored,
  fixed and dynamic blocks; it reports the bytes it consumed and the end of the stream, so bytes
  after the stream are left to the caller; and `inflate` is built on it. Output reaches the sink
  in runs of at most one window, and an error from the sink is returned unchanged.
- **OA-ZIP-006** The streaming inflater and the gzip reader keep hard limits: a total output cap
  (across all members for gzip) and a compression-ratio cap above a small exempt size, each its
  own error; a gzip bomb stops at the cap with an error and the good prefix delivered.
- **OA-ZIP-007** The streaming inflater and gzip reader allocate only when created: a fixed
  32 KiB window, three Huffman tables and fixed-size arrays, and no growable collection. The
  window and the tables keep their address and size through pushes, resets and members. (A
  counting allocator needs an `unsafe` impl, which RULESET A0 forbids in this crate, so the check
  is on the buffers themselves.)
- **OA-ZIP-008** The gzip reader parses every header form (FEXTRA, FNAME, FCOMMENT and FHCRC in
  any combination), verifies CRC-32 and ISIZE, and reads concatenated members, identically for
  every chunking from 1 byte up and in agreement with the system `gzip` (skipped if missing).
- **OA-ZIP-009** Each kind of gzip damage has its own error: bad magic (first and second byte,
  and at a later member), unsupported method, reserved flags, over-long name, header CRC-16,
  invalid DEFLATE data, CRC-32 and ISIZE; after an error the reader refuses further input.
- **OA-ZIP-010** A gzip stream cut short (a file a crash left after a sync flush) is read up to the
  cut: `finish` reports it truncated, with the stage (header, data, trailer), the bytes recovered
  from the unfinished member and how many of them lie before its last sync flush; all of it was
  delivered; the cut is never reported as success and never an error that loses the good data;
  a cut after a flush recovers exactly the flushed data, at every cut point.
- **OA-ZIP-011** Never panics: every byte flip, deletion, insertion and truncation of valid
  streams, and seeded noise in several chunkings, give output and errors for the streaming
  inflater and the gzip reader; the streaming inflater gives the same output and error for every
  chunking of the same input.
- **OA-ZIP-061** A repeated entry name is refused with `DuplicateName` (the first repeat in
  directory order), exactly and at any length; the check costs time proportional to the total
  name bytes, not to the square of the entry count: ten thousand 4,096-byte names sharing a
  4,093-byte prefix parse in seconds.
- **OA-ZIP-062** Offsets read from the archive (the zip64 locator and record, a local header) are added to constants with checked
  arithmetic: a value that would wrap is a typed truncation error, never a panic or a wrapped offset.


## M1 — SWIM input adapters

### XML (oa-xml)

- **OA-XML-001** Accepts the XML declaration with version 1.0 and encoding UTF-8 (any case) or
  none, and an optional standalone pseudo-attribute and UTF-8 byte order mark; rejects any other
  version, any other encoding, and a malformed declaration, each with its own error.
- **OA-XML-002** Accepts elements, attributes in either quote style, namespace declarations,
  comments, CDATA sections and whitespace, and produces a flat struct-of-arrays document in which
  every name, value and text is a span into the input; whitespace-only text between markup is
  not stored. `Parser::new` returns a `Result` (OA-XML-027).
- **OA-XML-003** Accepts character references (decimal and hexadecimal, with any number of digits
  including leading zeros: the body is scanned to its `;`, bounded only by the input) and the
  five predefined entities, and flags the attribute values and text that contain them. A value
  above U+10FFFF is `BadChar` however it is written.
- **OA-XML-004** Rejects any DOCTYPE, wherever it appears, with a dedicated error, so that
  entity-expansion (billion laughs) and external-entity (XXE) documents are refused.
- **OA-XML-005** Rejects processing instructions other than the XML declaration.
- **OA-XML-006** Rejects undefined entities and malformed or out-of-range references, with the
  offset of the reference.
- **OA-XML-007** Rejects input that is not valid UTF-8, with the offset of the first bad byte.
- **OA-XML-008** Rejects characters outside the XML 1.0 `Char` production, whether raw (in text,
  attribute values, comments, CDATA and names) or written as a character reference; U+FFFE
  and U+FFFF are `BadChar` everywhere.
- **OA-XML-009** Rejects mismatched end tags, unclosed elements, input that ends inside markup,
  and malformed tags, names and attributes. Every truncation of a valid message is reported
  at the end offset: `UnexpectedEof` when the input stops inside markup (a tag, attribute, end
  tag, comment, CDATA section, reference, the XML declaration or the opening of one),
  `Unclosed` when it stops between complete markup with elements open, `NoRoot` before any
  root, `Utf8` inside a multi-byte character.
- **OA-XML-010** Rejects duplicate attributes on one element, by qualified name and by expanded
  name (namespace URI and local name, Namespaces in XML 1.0 section 6.3: `a:x` and `b:x`
  bound to one URI are duplicates, wherever on the element the declarations sit). The cost
  per element is O(a^2) in its attributes, `a` at most `max_attrs_per_elem` (64 by default).
- **OA-XML-011** Rejects text, elements and other markup before or after the root element;
  whitespace and comments are allowed there.
- **OA-XML-012** Rejects an undeclared namespace prefix on an element or attribute, honouring
  declaration scope; `xml` is predefined.
- **OA-XML-013** Resolves an element's or attribute's prefix to its namespace URI span through
  the in-scope declarations, with a free function, including default namespaces, `xmlns=""` and
  unprefixed attributes. Each element carries the head of a chain of its in-scope
  declarations, so a call follows at most `max_ns_bindings` links (OA-XML-022).
- **OA-XML-014** Decodes a span's references into a caller-supplied buffer and fails, without
  writing past it, when the buffer is too small. All or nothing: on any error (too small, bad
  reference, span out of range) the buffer is unchanged. It validates and measures in a first
  pass and writes in a second, because restoring would need a saved copy of the buffer, which
  cannot be had without allocating.
- **OA-XML-015** Compares a span to a string, decoding references on the fly without allocating,
  and returns the error `decode_into` would return for the same span: it does not stop at the
  first mismatch, so the rest of the span is validated either way.
- **OA-XML-016** Enforces every limit (input bytes, depth, elements, attributes, attributes per
  element, name length, text nodes, namespace bindings in scope): a message at the limit parses and one unit over is refused
  with an error naming the limit and the byte offset.
- **OA-XML-017** Never panics: every truncation, every single-byte replacement, deletion and
  duplication of valid messages, and arbitrary byte strings, give a document or an error.
- **OA-XML-018** Every error carries the byte offset of the problem and a readable message.
- **OA-XML-019** A parser is reusable: tables keep their construction-time capacity across
  parses (no allocation after initialisation), and no state leaks from one message to the next,
  including after a failed parse.
- **OA-XML-020** Every real SWIM message in the local sample (`OPENATCA_SCDS`) parses; the test
  reports file count, maximum depth, maximum element count and throughput, and skips when the
  variable is unset.
- **OA-XML-021** The element table is a consistent tree: parents, first children and next
  siblings agree, and a walk from the root visits every element once.
- **OA-XML-022** The namespace bindings in scope at any point (the sum of the declarations over
  the open elements) are limited by `max_ns_bindings`, default 64, and the binding table has
  exactly that capacity, so every prefix lookup, while parsing and in `resolve_elem_ns` and
  `resolve_attr_ns`, is at most that many comparisons. The real corpus (19,025 messages)
  peaks at 15 bindings in scope; 64 is 4x that, rounded up to a power of two. In the worst
  case the default admits (64 live, the prefix found last) parsing costs under 10x the
  time per byte of the same message without the prefix.
- **OA-XML-023** Names follow the XML 1.0 fifth-edition `NameStartChar` and `NameChar`
  productions exactly, decoding UTF-8 code points, with an ASCII fast path; a colon only
  separates one prefix from a local name, and neither part may be empty.
- **OA-XML-024** Rejects the reserved namespace bindings: `xmlns:xml` bound to anything but the
  XML namespace, any other prefix or the default namespace bound to the XML namespace, and
  any binding of `http://www.w3.org/2000/xmlns/`.
- **OA-XML-025** Normalises on decode, not in the spans: `decode_into` and `span_eq_str` take a
  `SpanKind`. Text: `\r\n` and a lone `\r` become `\n` (XML 1.0 section 2.11). Attribute
  value: a literal tab, `\n`, `\r` or `\r\n` each become one space (section 3.3.3, CDATA
  type); character references are never normalised. A value that needs decoding for either
  reason is flagged; the fast path allocates nothing. CDATA sections are not flagged and
  keep their line ends raw (a recorded deviation).
- **OA-XML-026** A span that does not lie inside the input is `SpanOutOfRange` in every public
  function that takes a span, never an empty value.
- **OA-XML-027** `Parser::new` returns a `Result`. Every limit has a hard ceiling
  (`Limits::CEILING`: input bytes `i32::MAX`; elements, attributes and text nodes 16,777,216;
  depth and name length 65,536; attributes per element and namespace bindings 1,024), and
  input bytes, depth, elements and name length must be at least 1; a violation is
  `LimitsInvalid` naming the limit. Tables are reserved with `try_reserve_exact`, so a limit
  too large to allocate is `LimitsInvalid` too, never an abort.
- **OA-XML-028** The pieces of an element's text that a comment or CDATA section splits are several
  text nodes of one owner, and `decode_text_run` joins them as XML 1.0 defines the element's
  character data (`12<!--x-->34` and `<![CDATA[12]]>34` are `1234`), decoding each as text and
  bounded by the output buffer (`OutputTooSmall` past it). Whitespace-only text is still layout
  and not stored, except next to text or a CDATA section of the same element (`a<!--c--> <!--d-->b`),
  where it is data.
- **OA-XML-029** A namespace declaration's value is an attribute value: references (and attribute
  normalisation) are expanded before it is stored, into a buffer reserved in `Parser::new`
  (2,048 bytes per binding slot; overflow is `Limit` on namespace bindings). Every namespace
  comparison uses the expanded URI: duplicate expanded attribute names, the reserved `xml` and
  `xmlns` bindings, and what `resolve_*_ns` returns, which a decoder reads with `ns_uri_bytes`.

### SCDS feed frames (oa-feed)

Format: `docs/FEED_FRAMES.md`. The bridge itself (`adapters/scds-bridge`, Java) is outside code
under RULESET F6 and is verified by its live run, not by the cargo tests.

- **OA-FEED-001** A frame of any kind round-trips every field exactly (kind, flags, seq, receive
  time, JMS timestamp, payload, raw bytes, stream offset), including empty and 300-byte payloads
  and extreme integer values.
- **OA-FEED-002** The reader decodes the golden vector produced by the bridge's Java encoder,
  and the Rust encoder produces the same bytes.
- **OA-FEED-003** Frames are identical however the stream is chunked: every two-way split point
  at the start and end of a mixed stream, and chunk sizes from 1 byte up.
- **OA-FEED-004** A CRC mismatch is reported as a CRC error at the frame's byte offset; the
  reader resynchronises to the next good frame and counts the skipped bytes exactly, at every
  chunk split.
- **OA-FEED-005** Bytes that are not a magic are reported as a bad-magic error at their offset
  and skipped exactly up to the next frame.
- **OA-FEED-006** A length above 4 MiB is refused as an oversize error as soon as the header is
  present, without waiting for the payload, and the reader resynchronises.
- **OA-FEED-007** An unknown kind (including 0) is refused with its own error and the frame is
  skipped.
- **OA-FEED-008** A stream that ends inside a frame gives a truncated-tail error with the bytes
  present; leftover bytes during resynchronisation are counted as skipped and not reported
  twice.
- **OA-FEED-009** Never panics: every byte flip, truncation, insertion and deletion of a valid
  stream, and 200 000 bytes of seeded noise in several chunkings, give frames and errors; every
  input byte is either in a delivered frame or counted as skipped.
- **OA-FEED-010** The reader runs in a fixed caller-supplied buffer of one maximum frame, refuses
  a smaller one, accepts a frame with a 4 MiB payload, takes only what fits on a push and never
  stalls; a payload above 4 MiB cannot be encoded.
- **OA-FEED-011** Health counts frames, messages, payload bytes, minimum and maximum payload size,
  the payload-size histogram, redelivered messages and rejects.
- **OA-FEED-012** Health reports message rates over the last complete second and the last 60
  seconds, from fixed ring buffers, with injected times; a stale ring reads as empty.
- **OA-FEED-013** Health counts sequence gaps (and the frames missing), duplicates and
  regressions.
- **OA-FEED-014** Health puts latency (receive time minus JMS timestamp) in a fixed-bin
  histogram, counts negative values as clock skew apart from the histogram, counts messages with
  no timestamp, and reports p50 and p95 (bin upper edge, ms) and the exact maximum.
- **OA-FEED-015** STATUS frames set the connected or disconnected state and count reconnect
  attempts with their announced delay.
- **OA-FEED-016** Time since the last frame is computed from the injected clock and never
  negative.
- **OA-FEED-017** The hot structs (frame view, rate slot, health record) keep their budgeted
  sizes with no hidden padding (RULESET A7).
- **OA-FEED-018** Replaying recorded files gives an identical health record and summary every
  time, equal to reading the same bytes as one stream, from frame fields only (never the clock).
- **OA-FEED-019** Recording rotates at the size limit on frame boundaries, names files
  `<feed>-<UTC start>-<NNNN>.oaf` (raw; `.oaf.gz` when compressed, OA-FEED-026) with a
  zero-padded counter from 0001 within one second, so name order is creation order; counts
  existing recordings toward the total cap, stops once with a warning at the cap and never
  deletes a file.
- **OA-FEED-020** UTC file names and timestamps are correct calendar times, including leap days
  and times before 1970.
- **OA-FEED-021** Every frame error carries and prints the byte offset of the problem.
- **OA-FEED-022** The command line accepts the documented forms and refuses missing or malformed
  arguments, exclusive options and unknown flags.
- **OA-FEED-023** The `oa-feed` binary in `--stdin` mode reads frames, prints a health line,
  records the input byte for byte (`--record-raw`), and `oa-feed replay` of the recording prints the same figures
  on every run.
- **OA-FEED-024** The latency histogram has 10 ms bins to 100 ms, 100 ms bins to 1 s, 1 s bins to
  60 s, 10 s bins to 600 s and one overflow bin; each bin's upper edge is the next bin's start,
  and a p50, p95 or maximum in the overflow bin prints as `>=600000`.
- **OA-FEED-025** Latencies of 7 to 10.5 s (as measured on the live TBFM feed) fall in distinct
  1 s bins, and p50, p95 and the maximum come out as numbers in that range; a printed quantile never exceeds the maximum.
- **OA-FEED-026** `--record` writes `<feed>-<UTC start>.oaf.gz` (gzip, level 6) and
  `--record-raw` keeps the plain `.oaf`; the two are exclusive; the recorded frames are the input
  frames exactly.
- **OA-FEED-027** A compressed recording is sync-flushed once a frame's receive time is a second
  past the last flush, by an idle flush when the feed is quiet, and on rotation and exit (each
  file is finished with its trailer): a file read while recording, or after a crash, holds every
  frame up to the last flush.
- **OA-FEED-028** Rotation and the total cap count the compressed bytes on disk, including
  existing `.oaf` and `.oaf.gz` files, rotate on frame boundaries, stop once at the cap and never
  delete a file.
- **OA-FEED-029** Replay chooses gzip or raw from the file's first bytes, not its name, even when
  the source returns one byte per read.
- **OA-FEED-030** Replaying a compressed recording gives a health record, summary and byte count
  identical to replaying the same frames uncompressed; a file cut by a crash replays every frame
  up to the cut and is reported truncated with the bytes recovered; damaged gzip data is
  reported and the frames before it still replay.
- **OA-FEED-031** The `oa-feed` binary in `--stdin` mode records a compressed file that
  `gzip -t` accepts and that decodes to the input, and `oa-feed replay` of it prints exactly what
  replay of the raw recording prints.
- **OA-FEED-032** The real SCDS recordings under `OPENATCA_SCDS_REC` (`*.oaf`) converted to
  `.oaf.gz` replay to identical summaries; the test reports the compressed sizes and ratios and
  skips when the variable is unset.
- **OA-FEED-033** Flush timing and recording file names follow the local clock, not frame
  stamps: a frame stamped far in the future does not stop later flushes or name the file, and frames
  whose stamp is over an hour from local time are counted and reported.
- **OA-FEED-034** The sequence-gap total saturates instead of overflowing when sequence numbers
  jump by more than the counter can hold.
- **OA-FEED-050** A frame whose reserved header bytes (offsets 6 and 7) are not zero is refused
  with its own error kind (`Reserved`) even when its CRC matches, counted by `oa-feed` health and by
  `oa-replay`, and the reader resynchronises after it; `docs/FEED_FRAMES.md` states the rule, and
  every frame the encoder or the bridge writes has zero there (the real recordings replay without
  one).
- **OA-FEED-051** A recording that fails while writing (a write, an idle flush or the final
  finish) is a persistent failed state of the recorder: the failure is kept with its first error,
  every later frame is counted as lost rather than silently dropped, the final summary prints the
  failure and the lost count, and `oa-feed` exits non-zero when a requested recording failed
  (feed consumption may continue, and the log says so). Tested with a writer that fails on demand.
- **OA-FEED-100** (proof) For every sequence number and every reachable health counter, `observe_seq` never
  panics or overflows; a forward jump adds exactly one gap event and the saturated number of missing frames,
  a late frame inside the last gap takes exactly one off `missing` and counts as reordered, and no other frame
  changes `gaps` or `missing`; the open gap stays inside `missing`, and `missing` never exceeds the sum of the
  gap lengths added.
- **OA-FEED-150** Every refused frame (each error kind) adds one to the health `frame_errors`
  figure and moves nothing else; the skipped-byte figure is set from the reader's running total;
  both show as `errors=` and `skipped=` in the summary line.
- **OA-FEED-151** Every latency bin edge is exact: the first value of each bin, one above it and
  one below it (the previous bin) land where documented, negatives land in bin 0 and anything from
  600000 ms in the overflow bin.
- **OA-FEED-152** A frame repeating the last `seq` is a duplicate (not a regression, also for the
  first frame) and leaves the expectation unchanged; the expected `seq` is never a gap; one ahead
  is one gap of one missing frame; one below the last is a regression unless it fills the
  most recent open gap (OA-FEED-172).
- **OA-FEED-153** Latency is the receive time in whole milliseconds minus the JMS time: exact
  values land in the maximum and the histogram, equal times are zero (not skew), a stamp ahead by
  one millisecond is skew.
- **OA-FEED-154** The latency quantile is `None` without samples and with a zero denominator
  (either alone, no division by zero), otherwise the upper edge of the bin holding it.
- **OA-FEED-155** The summary prints `state=disconnected` after a disconnect STATUS, and
  `lat_p50_ms`, `lat_p95_ms` and `lat_max_ms` all as `-` while there are no latency samples
  (none yet, no JMS stamp, or only clock skew).
- **OA-FEED-156** `push` takes exactly what fits (a full buffer, then one byte over takes 0), an
  emptied buffer takes a full buffer again, and when consumed bytes at the front are in the way it
  compacts so all that then fits is taken and no byte is lost.
- **OA-FEED-157** A frame is delivered exactly when all its bytes are present: one byte short
  waits, exactly one frame and one frame plus one byte or the next magic deliver it; four magic
  bytes and a full header are judged, one byte fewer waits.
- **OA-FEED-158** `offset` is the absolute stream position of the next unconsumed byte after
  several frames, after a resync over garbage (including garbage alone), after a compaction and
  after a truncated end.
- **OA-FEED-159** The recording defaults are a 2 GiB total cap and 256 MiB rotation size, kept
  when only the other option is given.
- **OA-FEED-170** When a flush or the finish of a compressed recording fails, the frames written since the last
  successful flush, which the compressor still held, are counted as lost with the frame that hit the error
  (`lost_frames`); a raw recording holds none back. Tested with a writer that fails on demand.
- **OA-FEED-171** A recording that stopped at its size cap makes the run exit non-zero with the reason printed
  (`Recorder::exit_reason`; a failure takes precedence over the cap); a healthy recording has no reason. Tested on
  the mapping function.
- **OA-FEED-172** A frame whose `seq` is below the next expected but inside the most recent open gap is counted
  as reordered (`reordered`) and takes one off that gap's missing count, without moving the expectation: 3, 5, 4
  gives missing 0, reordered 1 and regressions 0, and 3, 6, 4 gives missing 1. Only the last gap is kept (start
  and length, fixed size). Any other lower value is still a regression and forgets the gap.
- **OA-FEED-173** Reading a whole stream (`read_frames`, the loop the ADS-B clip loaders share) ends with
  `FrameReader::finish`: the frames before a partial frame at the end are all delivered, and the partial frame,
  whether it stops inside the header or inside the body, is counted once with the damaged frames.

### Compression (oa-deflate)

- **OA-FEED-174** A frame below the next expected `seq` but not below where the last gap began (as opened,
  before any late frame shrank it), and outside the part of that gap still open, is a repeat of a late frame or
  a late frame of a part the gap record no longer covers: it counts as a duplicate and moves neither the
  expectation nor `missing`, so a redelivered late frame (3, 5, 4, 4, 6) makes no false gap. A frame below that
  point is a regression as before.
- **OA-FEED-175** An `OAF2` frame (magic `OAF2`, a u64 `mono_ns` between `jms_timestamp_ms` and
  `payload_len`) round-trips every field exactly, `Frame::mono_ns` gives the monotonic time, and an `OAF1`
  frame in the same stream gives `None`.
- **OA-FEED-176** `OAF2` frames decode identically however the stream is chunked, damage in one is a
  CRC error at its offset with resynchronisation on to the next frame of either version, and every byte
  is delivered or counted skipped.
- **OA-FEED-177** `oa-feed --record` and `--record-raw` store each frame as an `OAF2` frame with the
  recorder's monotonic time since start (not going back), taken when the chunk of input that completed the
  frame was read, before it is queued; a frame that already has one keeps it; the
  other fields and the payload are the input's; a gzip recording of such frames replays.
- **OA-FEED-178** `--duration` is refused above 10 years (`MAX_DURATION_SECS`, 315 360 000 s) and
  `--summary-every` above one day (`MAX_SUMMARY_SECS`, 86 400 s), including `u64::MAX`, at parse time; at the
  limits a run does not panic. Every `Instant` offset in the binary is checked.
- **OA-FEED-179** A feed label is one safe filename component: 1 to 64 ASCII letters, digits, `-`, `_` and `.`,
  not starting with `.`. `--feed` and a label derived from the env file name are refused at parse time, and
  `Recorder::open` refuses one again before touching the directory.
- **OA-FEED-180** `utc_stamp` and `utc_iso` accept Unix times from 0001-01-01T00:00:00Z to
  9999-12-31T23:59:59Z and return `None` outside them; a recording is not named from a clock outside that range.
- **OA-DEF-001** Compresses to a raw RFC 1951 DEFLATE stream that the oa-zip inflater decodes to
  exactly the input, at levels 1 to 9 (others are clamped), for empty input, one byte, all-same
  bytes, random bytes and data at 32 KiB, 64 KiB and buffer-slide boundaries (each +-1).
- **OA-DEF-002** Uses maximum-length (258) matches and the maximum distance (32768); data that
  repeats a 32 KiB block compresses to about one copy, and a repeat just outside the window is
  still decoded correctly.
- **OA-DEF-003** The gzip writer emits an RFC 1952 member (no timestamp, no name) whose CRC-32
  and ISIZE are correct, writes the header even for empty input, and is reusable for further
  members with identical output.
- **OA-DEF-004** The system `gzip -t` and `gzip -dc` accept our output, including concatenated
  members, for all levels and for chunked writes.
- **OA-DEF-005** The oa-zip inflater decodes the system `gzip` output (levels 1, 6, 9) of the
  same inputs; the test skips when `gzip` is missing.
- **OA-DEF-006** Huffman codes are complete prefix codes of at most 15 bits (7 for the
  code-length code) for any frequencies, including those whose unlimited tree is deeper, and for
  zero or one used symbol.
- **OA-DEF-007** Incompressible input falls back to stored blocks, so output exceeds input by
  framing only (about 5 bytes per 64 KiB).
- **OA-DEF-008** Sync flush ends on a byte boundary with an empty stored block; the output cut
  there, closed with an empty final block, decodes (also in system `gzip`) to exactly the data
  written before the flush. A flush with no new input emits nothing.
- **OA-DEF-009** Every feed directory of the real SWIM sample (`OPENATCA_SCDS`, `*.msg`
  concatenated) round-trips at levels 1, 6 and 9; the test reports ratio and MB/s per level
  against `gzip -6` and `xz -6`, requires level 6 to be within 10% of `gzip -6` (release builds),
  reports level 6 speed against a 30 MB/s target with a warning below it, and skips when the variable is unset.
- **OA-DEF-010** Never panics and always round-trips for chunk sizes from 1 byte upward
  (1, 7, 258, 4097, ...), irregular chunking and interspersed flushes, empty writes, and finish
  or flush with nothing written.
- **OA-DEF-011** Output is a pure function of input, level and write boundaries (no clock, no
  randomness); a compressor is reusable after `finish` or `reset` with identical output, and
  allocates only in `new`.
- **OA-DEF-012** An error returned by the output closure is returned unchanged by `write`,
  `flush` and `finish`; the compressor works again after `reset`.

### SFDPS flight messages (oa-fixm)

- **OA-FIXM-001** Decodes a FIXM 3.0 + FAA NAS 3.0 `MessageCollection` of flight messages into one
  row per `message`, in document order, with source, centre, message time (Unix ms), GUFI,
  callsign, computer id, flight plan id, departure and arrival points, status, flight type,
  assigned and reported altitude, position and position time, ground speed, track velocity,
  aircraft type, route text, operator, beacon code, controlling and handoff units, runway
  times and sequence numbers; verified on hand-written messages of source types TH, HZ, FH, AH,
  OH, DH, CL, BA and LH.
- **OA-FIXM-002** Parses timestamps as strict ISO-8601 UTC (`Z` only, real calendar dates, no
  leap second, 1970 to 9999) to Unix milliseconds, GUFIs as strict UUIDs and decimals without
  rounding surprises; anything else is refused.
- **OA-FIXM-003** A field that is present but malformed is a counted field error: the row is kept,
  the field is absent and its bit is set in the row's error word, nothing is defaulted.
- **OA-FIXM-004** Altitudes are range-checked (-2,000 to 100,000 ft) and converted from their
  `uom`; an unknown or missing `uom` is a field error.
- **OA-FIXM-005** Positions are range-checked (latitude +-90, longitude +-180) and read as
  EPSG:4326 latitude then longitude only; any other reference system is a field error.
- **OA-FIXM-006** Ground speed and track velocity are converted from their `uom` to 0.1 knot and
  range-checked; the ground track is derived from the velocity components by a free function.
- **OA-FIXM-007** A message with a missing or invalid GUFI, a missing or invalid timestamp, a
  type other than `FlightMessageType`, or a second `flight` element is refused whole, counted
  by reason, and appends no row; neighbouring messages are unaffected.
- **OA-FIXM-008** `Events` has a stated row capacity and string-table capacity; a document that
  does not fit is refused with the rows and string table exactly as before, never partly
  appended (IF-R1).
- **OA-FIXM-009** Elements and attributes outside the structure seen in real data are counted per
  path in a bounded table (overflow is counted too); they are never errors, and only the
  topmost unknown element of a subtree is counted.
- **OA-FIXM-010** A document whose root is not a `MessageCollection` in the FAA NAS 3.0 namespace,
  one over the element limit, or an input that is not the parsed bytes, is refused with an
  error.
- **OA-FIXM-011** A single-valued field given twice in one message is a field error, not a silent
  first-wins.
- **OA-FIXM-012** The decoder never panics: every deletion, duplication, truncation and
  replacement of a byte of valid fixtures, and random multi-byte damage, give rows or an error,
  with every present column inside its model range.
- **OA-FIXM-013** A decoder is reusable: tables keep their construction-time capacity, no state
  leaks from one document to the next, including after a failed decode.
- **OA-FIXM-014** Every real SFDPS message in the local sample (`OPENATCA_SCDS`) decodes; the test
  reports messages, rows, rejections with reasons, field errors by field, top unknown paths,
  distinct GUFIs and callsigns, rows with a position and throughput (at least 20,000
  messages/s in a release build), and skips when the variable is unset.
- **OA-FIXM-015** Route text is kept in the bounded string table, entity-decoded; a route longer
  than the limit, or empty, is a field error and takes no table space.
- **OA-FIXM-016** Field names, bits and display tables agree, and short codes read back as text.
- **OA-FIXM-017** A document with several messages gives one row per message, in order, and
  counts each message.
- **OA-FIXM-018** A decimal too large for the scaler (twelve integer digits at 1e-7, or a sum past `i64`) is
  refused, not wrapped: the position is a field error, counted, and a following message still decodes.
- **OA-FIXM-019** A message's `xsi:type` is read as a QName: its prefix is resolved with the namespaces
  in scope and the (namespace, local name) pair must be the NAS namespace and `FlightMessageType`
  (an undeclared prefix, or a foreign namespace, is `NotFlightMessage`). Namespace URIs compare
  by their expanded text, so an escaped spelling of a FIXM, NAS or XSI URI decodes like the plain one.
- **OA-FIXM-020** The row capacity needed and the undecoded total count only the children of the
  root that the decoder recognises as `message` elements; a foreign sibling counts for neither.
- **OA-FIXM-021** The calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days that do not exist in the
  month; hour 24 is refused.

### TBFM metering publication (oa-tbfm)

Built on the survey in `docs/TBFM_SURVEY.md` (12,760 real messages, 20 centres).

- **OA-TBFM-001** Times are parsed from strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS`, an optional
  fraction of one to three digits, `Z`) to exact Unix milliseconds, including leap days, the
  century rules and the year range 1970 to 9999.
- **OA-TBFM-002** A timestamp that is not exactly that form is refused with its own error:
  wrong shape (offsets, lower case, spaces, date only, compact form, trailing bytes), a
  non-digit, a fraction of more than three digits, or a value out of range (month, day of
  month, hour, minute, second `:60`, year before 1970).
- **OA-TBFM-003** The output is a caller-owned struct-of-arrays with a stated capacity, one
  `Vec` per column sharing an index, reserved once; rows round-trip through the columns, a push
  at capacity writes nothing and says so, and the staged row keeps its budgeted size
  (RULESET A3, A7, C2).
- **OA-TBFM-004** A `flt` section becomes one row with the flight identity (`aid`, `apt`,
  `dap`, `gufi`, `cid`, `tmaId`), the envelope and message fields, and the flight-plan fields
  (status, type, engine, beacon code, speed, altitudes, runways, coordination fix and times);
  the repeats of `aid`, `apt` and `dap` inside `flt` must agree with the `air` attributes or
  fill an attribute `air` lacks, and a disagreement is refused. The route and crossing text
  fields are known and not stored.
- **OA-TBFM-005** An `mrp` section becomes one row with its own `mrpType` and the meter route
  names: TRACON, meter fix, delay and scheduling fixes, the four arcs, gate, runway,
  configuration, category and scenario.
- **OA-TBFM-006** Each `eta` and `sta` section becomes one row, and each time is filed under its
  place (delay fix, scheduling fix, meter fix, the four arcs, runway) in the `eta_ms` or
  `sta_ms` column for that place; several `eta` or `mrp` sections in one `air` are several
  rows.
- **OA-TBFM-007** The `sch` flags (`man`, `rfz`, `sfz`, `sus`) are stored as two bits each:
  absent, `NO`, `YES`; any other word is refused.
- **OA-TBFM-008** Every message is classified: `air` (flight data), adaptation, control, other,
  empty, and the `mis` heartbeat and sync markers; non-flight kinds produce no rows and are
  counted, and the centre and envelope time are reported for all of them.
- **OA-TBFM-009** A flight (see OA-TBFM-019) is accepted whole or not at all; a wrong root or
  namespace, an XML error or a capacity overrun leaves the caller's rows exactly as they were,
  and a refused field carries the byte offset of its element or attribute value.
- **OA-TBFM-010** Every field has its own refusal: a missing required attribute (`envSrce`,
  `envTime`, `msgTime`, `msgId`, `tmaId`, `aid`, `airType`), a name longer than its column, a
  character outside printable ASCII, a malformed number, transponder code, centre or time, a
  word outside the vocabulary, a field twice in one element, a value element with children or
  with text in several nodes; each is reported with the field and the reason. An element sent
  empty is not an error: it is recorded in the row's `cleared` mask.
- **OA-TBFM-011** Reaching the row capacity is the reject reason `capacity`, never a
  reallocation or a partial message, and rows already held count against it.
- **OA-TBFM-012** Unknown elements (a whole subtree counts once), unknown attributes and
  elements outside the TBFM namespace are counted and do not affect the rows or the decision to
  accept; namespace declarations and `targetNamespace` are not counted; known elements that
  are deliberately not stored are counted separately; statistics add both up over accepted
  messages and count rejects by reason and by field.
- **OA-TBFM-013** The decoder never panics: every truncation, deletion, insertion and
  replacement of each byte of a message with every section kind, and seeded random
  corruption, give either rows or an error, and a refused message leaves every column the same
  length as before.
- **OA-TBFM-014** Character references and CDATA are decoded before a value is validated, an
  attribute value is normalised as XML requires, and a decoded value longer than the 64-byte
  working buffer is refused as too long.
- **OA-TBFM-015** After construction decoding does not grow any column or any parser table,
  across accepted messages, refused messages and capacity overruns.
- **OA-TBFM-016** Every message of the real TBFM sample (`OPENATCA_SCDS`, directories named
  `tbfm*`) decodes with no reject and no unknown path; the test reports messages, rows and
  rejects by reason, field errors by field, distinct flights, GUFIs, airports and meter fixes,
  and throughput, which must be at least 20,000 messages per second (parse and decode, one
  thread, release builds); it skips when the variable is unset.
- **OA-TBFM-017** Against an airspace snapshot of one ARTCC (`OPENATCA_SNAPSHOT`) the real rows'
  meter fixes and airports are resolved to model points, and the distinct identifiers and rows
  that resolve are reported for all centres and for the model's own centre; the test skips
  when either variable is unset.
- **OA-TBFM-018** The cross-check resolves a meter fix to a model fix, navaid or terminal
  waypoint and an airport to a model airport by its identifier or alternate identifier (never
  to a runway threshold), counts distinct identifiers and rows per role and per scope (all
  centres, the model's own centre) and lists the identifiers that do not resolve.
- **OA-TBFM-019** A flight is decoded all or nothing, not the message: an `air` with a refused field
  is rolled back (rows and counters) and counted with its field and reason (`rejected`,
  `first_reject`, per field and per reason), its sibling flights are kept, and a `tma` with bad
  attributes goes the same way; a bad envelope, a wrong root, an XML error and a capacity overrun
  still refuse the whole message. Statistics and the replay pipeline count the refused flights.
- **OA-TBFM-020** `spd` carries its unit with the value (`spd_unit`: none, knots, Mach, unknown), read
  from the value's form because the feed has no marker: `ddd.d` is knots, `d.dd` is Mach (all
  jets in the recorded sample), any other form is stored with the unknown unit and counted
  (`unknown_units`); units are never mixed in a column without it.
- **OA-TBFM-021** A schedule flag with a word other than `YES` or `NO` keeps the flight, stores the flag
  as unknown and is counted (`unknown_words`); an unknown change type still refuses the flight.
  `msgId` (ten digits) is stored as `u64`.
- **OA-TBFM-022** Values are trimmed of outer whitespace; the site-adapted labels (`gat`, `cfg`, `scn`,
  `cat`) may hold interior spaces (printable ASCII), identifiers and fixes may not. The TBFM schema
  is not in the repository, so that labels allow spaces is a decision, not a confirmed rule.
- **OA-TBFM-023** Text split by a comment or CDATA section is joined before validation, and the 64-byte
  working bound applies to the total.
- **OA-TBFM-024** A namespace URI written with a character reference compares as its expanded text, so the message is
  not refused as foreign.
- **OA-TBFM-025** The calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days that do not exist in the
  month; hour 24 is refused.

### TFMS data service (oa-tfms)

Built on the survey in `docs/TFMS_SURVEY.md` (4,237 real documents, flight data, flow
information and feed status). Decision: the decoder follows the oa-fixm and oa-tbfm pattern; flow
messages are a separate row type; the TFMS `gufi` is linked to the SFDPS plan identifier, not to
the SFDPS `gufi` (they are different kinds of value, survey).

- **OA-TFMS-001** Times are parsed from strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS`, an optional
  fraction of one to nine digits, `Z`) into Unix milliseconds (`i64`); any other shape, digit,
  fraction or range is refused with its own error.
- **OA-TFMS-002** Scalar fields have strict parsers: names (bounded, upper-case letters, digits
  and a few separators), unsigned integers, altitudes (hundreds of feet with an optional letter),
  decimal and degrees-minutes-seconds angles to 1e-7 degree, booleans, vocabularies and whole
  words; each refuses anything else.
- **OA-TFMS-003** Rows are caller-owned structs of arrays with a stated capacity and exact size
  guards; every value has an explicit absent form (`NO_TIME`, `NO_NUM`, `NO_COORD`, `NO_STR`, a
  zero code or a zero name).
- **OA-TFMS-004** Every flight message becomes one row whose identity (message kind, trigger,
  aircraft identification, flight reference, GUFI, departure and arrival, source facility and
  time, computer id, categories) comes from the message attributes and the qualified aircraft
  identifier, which must agree; an unknown message type is a row of kind `other`.
- **OA-TFMS-005** Positions (degrees-minutes-seconds and decimal), time at position, altitudes
  in feet, speeds and the next position are stored as fixed point in 1e-7 degree, feet and knots.
- **OA-TFMS-006** Estimated, actual, scheduled, proposed and metered times with their types,
  fixes with times, coordination, boundary crossing, departure and arrival, the airline's
  out/off/on/in, runway and gate times and the original times are stored as Unix ms.
- **OA-TFMS-007** Route text is kept in the bounded string table, entity-decoded, with the legacy
  format preferred over the element text in either order, and with remarks and the departure and
  arrival procedure names.
- **OA-TFMS-008** A traffic management initiative flight list becomes one flight row per entry,
  with the entry's status, update type and time, FCA identifier and the number of `fxaFlight`
  elements; the message's own time and facility go to every entry.
- **OA-TFMS-009** Restrictions, advisories, FCA/FEA definitions and airport configurations become
  flow rows with program type, scope, element, start, end, rates, identifiers and text; the
  program of an advisory is read from whole words of its title and a cancellation is flagged.
- **OA-TFMS-010** The identity attributes of a message and the identity in its body must agree
  (refused as inconsistent otherwise); the aircraft identification, flight reference, source
  time and message type are required.
- **OA-TFMS-011** Every field has its own refusal naming the field and the kind (missing, too
  long, bad character, format, time, vocabulary, range, duplicate, inconsistent, not a leaf,
  text), with the offset of the value.
- **OA-TFMS-012** A message is accepted whole or not at all (OA-TFMS-023); a capacity
  overrun or a text-table overrun leaves the caller's rows and text table exactly
  as they were before the document.
- **OA-TFMS-013** A document that is not well-formed, whose root is not `tfmDataService` in the
  service namespace, or whose content is refused is rejected with a reason; status documents,
  empty documents and mixed documents are classified.
- **OA-TFMS-014** Capacity of flight rows, flow rows and text bytes is stated and never exceeded;
  reaching it is a reject reason, never a growth; after construction no column or table grows.
- **OA-TFMS-015** Unknown elements (a whole subtree counts once), unknown attributes and unknown
  namespaces are counted, not errors, and elements are matched by namespace and local name
  whatever the prefix; known elements that are deliberately not stored are counted separately.
- **OA-TFMS-016** The decoder never panics: every truncation, byte replacement, deletion and
  insertion of flight and flow documents is decoded or refused, and the columns stay
  consistent.
- **OA-TFMS-017** Every message of the real TFMS sample (`OPENATCA_SCDS`, directories named
  `tfms*`) decodes with no reject and no unknown path; the test reports messages, rows by
  type, rejects by reason, field errors by field, distinct flights and throughput, which must be
  at least 20,000 messages per second (parse and decode, one thread, release builds); it skips
  when the variable is unset.
- **OA-TFMS-018** Against the real SFDPS sample, TFMS flights are linked by GUFI and the overlap
  is reported as found; no TFMS GUFI equals an SFDPS GUFI and the two matching routes never
  name different flights; the test skips when the variable is unset.
- **OA-TFMS-019** Counters over many documents (accepted, messages, rows by kind, rejects by
  reason, field errors by field, unknown and ignored paths) are folded by a free function.
- **OA-TFMS-020** The linkage matches a TFMS flight to the SFDPS flights by its GUFI (equal to
  the SFDPS flight plan identifier) and, when it has no GUFI, by aircraft identification plus
  departure plus arrival, and cross-checks flights found both ways.
- **OA-TFMS-021** The linkage report counts, as found, the flights with and without a GUFI,
  UUID-shaped GUFIs, keys that name more than one SFDPS flight and empty inputs.
- **OA-TFMS-022** The default capacities and the vocabulary tables agree with the codes.
- **OA-TFMS-023** A refused field refuses its message, not the document: the message (or TMI flight list
  entry) is rolled back with the text table and counters, counted with its field and reason, and
  its siblings are kept; only an unreadable document or a capacity overrun is refused whole.
- **OA-TFMS-024** A word outside a vocabulary (trigger, status, categories, time types, program, scope,
  true/false flags) keeps the message with the column marked unknown (`UNKNOWN_WORD`,
  `flag::UNKNOWN`) and counted; the direction of a position still refuses the message.
- **OA-TFMS-025** Ten-digit identifiers (`flightRef`, restriction id, reference id, number) are `u64`; eleven
  digits are refused. The replay tables key on them and hash ordinary values as before.
- **OA-TFMS-026** Text split by a comment or CDATA section is joined, for values and for the text table,
  with the size bounds applied to the total.
- **OA-TFMS-027** A speed is stored in thousandths with its unit (`speed_unit`, `tas_unit`: none, knots, Mach,
  unknown): a whole number, `ddd.d` and `Nddddd` are knots, `d.dd` and `Mddd` are Mach; any other shape of
  digits, `.`, `M` and `N` is kept with unit unknown (the value when it reads as a number) and counted
  (`unknown_units`), and the message is kept; a byte that is none of those refuses the message as before.
- **OA-TFMS-028** Whether a message carried a flight reference is its own flag (`flag::FLIGHT_REF`), so a reference
  of 0 ("0000000000") is told from none; the linkage counts a row without one in `tfms_rows_without_ref` and
  treats it as a flight of its own, never as part of the flight whose reference is 0.
- **OA-TFMS-029** A namespace URI written with a character reference compares as its expanded text, so the message is
  not refused as foreign.
- **OA-TFMS-030** The calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days that do not exist in the
  month; hour 24 is refused.

## M1 — Controller display

### Display list, projection, font, rasterizer (oa-display)

- **OA-DSP-001** The display list is a flat struct-of-arrays list of line, polyline (a span of a
  shared vertex array), filled rectangle, circle outline and text primitives. Every primitive
  carries a palette index and a layer, coordinates are `i32` in 1/16 pixel, and a colour index,
  layer, text size or circle radius out of range is refused.
- **OA-DSP-002** Capacity is fixed at creation. A push to a full table returns an error naming the
  table and its capacity and marks the list faulted; a faulted list is never drawn.
- **OA-DSP-003** Clearing a list for the next frame keeps every capacity, so building frames does
  not allocate in the list.
- **OA-DSP-004** The projection puts the centre at the middle of the screen with north up and
  east right, and projecting then unprojecting a screen position, or the reverse, returns it to
  within 1/16 pixel (and the position to within 1e-7 degree plus that quantum).
- **OA-DSP-005** The distance from the centre drawn on screen is within 0.15 % of the WGS-84
  geodesic (oa-geo) out to 400 NM at centres from the equator to 65 N, and bearings are true to
  0.3 degree.
- **OA-DSP-006** The projection refuses a centre off the globe, a range outside 0.05 to 20000 NM,
  a zero-size screen, the absent-position sentinel and positions off the globe, and saturates a
  far projected point instead of overflowing; ring radii are true ground distances.
- **OA-DSP-007** The stroke font has every character of A to Z, 0 to 9 and ` .,/-+:()*#` with the
  up and down arrows and `<` `>`, sorted for lookup, with every stroke on the 5 by 8 grid; lowercase folds to
  capitals and an unknown character draws a crossed box.
- **OA-DSP-008** The easily confused characters 0 and O, 1 and I, 5 and S, 8 and B, and 2 and Z
  differ in at least 4 cells of the glyph grid, other close neighbours in at least 3, and no two
  glyphs are identical.
- **OA-DSP-009** The rasterizer clips every primitive to the buffer exactly: a clipped line, circle
  and rectangle equal the part inside the buffer of the unclipped drawing; lines entirely outside,
  crossing each edge and on each corner behave correctly; nothing is written outside the visible
  pixels, however extreme the coordinates; and a line is the same whichever end comes first.
- **OA-DSP-010** The rasterizer is integer only and deterministic: the frame depends only on the
  list and the frame geometry, not on what the buffer held before or on the stride.
- **OA-DSP-011** A deterministic sweep of random primitives, including `i32` extremes, every size
  and alignment, odd strings and many buffer geometries, never panics, writes only palette colours
  to visible pixels, writes nothing else, and gives the same hashes on a second run.
- **OA-DSP-012** Small fixed scenes (a line fan with rectangle, circle and polyline; every glyph
  class at several sizes and alignments; layers with clipping; a hand-built airspace map) render
  to pinned SHA-256 hashes.
- **OA-DSP-013** The frame hash is the SHA-256 of the visible pixels in row order, independent of
  stride, and changes with any pixel; an unusable frame geometry or short buffer is refused.
- **OA-DSP-014** A faulted list, or a list whose columns, indexes, spans or text were edited into
  an inconsistent state, is refused before any pixel is written.
- **OA-DSP-015** Text is anchored left, centre or right at the top of its capitals, and its
  extent is the glyph grid scaled by the size.
- **OA-DSP-016** Every foreground palette colour has at least 4.5:1 contrast on the background,
  and no two data colours come within 55 (of 255) of each other in plain RGB or as seen with
  protanopia, deuteranopia or tritanopia.
- **OA-DSP-017** The airspace map draws ARTCC HIGH and LOW boundaries (closed, in distinct
  colours and layers), airway legs, fixes, navaids and airports as distinct symbols, labels below
  configurable range thresholds, range rings at their true radius with labels, and a legend naming
  the facility, cycles, range, ring spacing and both strata.
- **OA-DSP-018** A scene that overflows any table fails with an error naming it, leaves the list
  faulted and is never drawn, and the capacities computed from the model hold at every range and
  centre including the poles and the antimeridian.
- **OA-DSP-019** The same model and view give the same list and the same bytes however many
  frames come between, and a frame never changes the list's capacity.
- **OA-DSP-020** The PPM writer emits P6 with RGB order, no row padding, and an error, not a
  panic, for a short buffer.
- **OA-DSP-021** A real ZAU snapshot (`OPENATCA_SNAPSHOT`) renders, every frame has the same hash,
  and the time per frame at 1920 by 1080 is reported; the test skips when the variable is unset.
- **OA-DSP-022** `oa-scope render` draws a snapshot to a PPM and reports the frame's SHA-256, and
  a malformed command line is a usage error naming the problem.
- **OA-DSP-023** Points and legs the map cannot place are counted and said on the screen in the
  alert colour, never dropped silently. A leg that ends at a published border crossing is
  expected data, not an unplaced one (see OA-DSP-027).
- **OA-DSP-024** Lines and polylines are 1 or 2 pixels wide (another width is refused). A 2 pixel
  line is the Bresenham line plus a copy one pixel along its minor axis, deterministic, the same
  whichever end is first, clipped like any line, and pinned by a golden hash.
- **OA-DSP-025** The map hides detail by default and says so. Every layer (own boundary, neighbour
  boundaries, high airways J and Q, low airways V and T, VOR-class navaids, airports, fixes,
  labels, rings) can be switched off. Every range threshold keys on the display range (the zoom,
  nautical miles from the centre to the nearest edge of the map area), never on a symbol's
  distance from the centre, and each is configurable. Defaults: fixes and labels of fixes, minor
  navaids and other airports at a range of 40 NM or less; terminal waypoints 20 NM or less;
  runway thresholds and localizers 15 NM or less; airports without a coded instrument procedure
  and NDB, DME and TACAN 40 NM or less; VOR, VORTAC and VOR-DME and airports with a coded
  instrument procedure always, with their labels out to 100 NM. A `HIDDEN:` legend line names
  everything left out by a layer switch or a threshold at the current range.
- **OA-DSP-026** The own facility's boundary is drawn 2 pixels wide in its stratum colour;
  neighbouring facilities' boundaries are 1 pixel wide in a dimmer colour under it.
- **OA-DSP-027** The alert colour is used only for conditions that need attention, never for
  expected data. Legs ending at border crossings (unresolved role BORDER_CROSSING) are reported in
  the normal text colour as a count; the alert colour appears only when points, legs or edges are
  genuinely unplaced.
- **OA-DSP-028** The legend sits in a fixed strip at the top of the screen that the projection
  excludes from its map area, so the legend never hides map data and the range circle fits below
  it; legend rows stay inside the strip and do not overlap.
- **OA-DSP-029** Each range ring is labelled where it crosses the 45 degree north-east diagonal
  from the centre, and no two ring labels overlap at any ring spacing or screen size.
- **OA-DSP-030** Positions outside a latitude and longitude box around the view are rejected
  before projection, and the box never changes the picture: the list is identical with and
  without it, including near the antimeridian.
- **OA-DSP-031** The frame hash is computed only on request (`--hash`, or in tests), not on every
  frame; the command line takes `--hide` and the range thresholds, and a bad layer name is a usage
  error.
- **OA-DSP-032** Labels are placed deterministically with collision culling. Candidates are sorted
  by priority (VOR-class navaids, airports with instrument procedures, other airports, fixes and
  minor navaids, terminal waypoints), then ident, then position, then table row. Each takes the
  first of four fixed offsets (north-east, south-east, north-west, south-west of its symbol) that
  overlaps no placed label, ring label, the legend strip or the screen edge; otherwise it is
  dropped and counted, and the legend shows `LABELS DROPPED: N` in the normal colour. Placement
  uses a fixed-capacity grid of cells with a bounded list each, allocated once; a full cell drops
  the label and counts it. No two drawn labels overlap, a label is dropped only where every offset
  is taken by a label of equal or higher priority, and the result does not depend on point order.
- **OA-DSP-033** A view whose projection constants are not finite (a centre at either pole) is refused
  with the typed `BadView`; a view one count of 1e-7 degree from a pole is accepted, with finite constants,
  and projects its centre and nearby points on the screen.
- **OA-DSP-034** A legend text that runs past the right edge of a narrow screen (down to the 64-pixel minimum)
  is marked there with a two-pixel bar in the text colour and counted (`legend_cut`), and the command line prints the whole
  legend, so the cycles, range and HIDDEN list are never cut without notice. A wide enough screen prints
  nothing extra (the default HIDDEN line already runs past 800 pixels). The legend stays in its fixed strip (it is marked, not wrapped).
- **OA-DSP-035** A command-line number of `oa-scope render` (centre, range, thresholds) that is NaN, infinite,
  or for the range outside 0.05 to 20000 NM is a usage error when it is parsed, not a value passed on to the view.
- **OA-DSP-036** A list, scene or scratch whose size is derived from the model or the screen is checked in
  128-bit arithmetic against a named display limit before anything is allocated: `MAX_LIST_ROWS` (2^24 rows per
  table), `MAX_LIST_TEXT_BYTES` (2^26) and `MAX_GRID_BOXES` (2^22 label boxes). A request above it is
  `DisplayError::TooLarge`, not an allocation.
- **OA-DSP-037** `build_scene` refuses a view whose range thresholds are NaN, infinite or negative, or whose
  projection has a non-finite constant or an empty screen, with `DisplayError::BadView`.
- **OA-DSP-038** Live traffic enters the display as plain data: one displayed target (position, ground track and
  speed, altitude if known, address, callsign, coasting and uncertain flags, the age of the last reacquisition,
  the 95% radius in metres, a bounded history) and a fixed-capacity set of them per frame. The display crate does
  not depend on the tracker. A style with a leader outside 0 to 30 minutes, a trail above 16 positions, or a
  capacity above 2^20 targets is refused, and nothing is allocated for it.
- **OA-DSP-039** Each target state has its own drawing, never colour alone: a tracking target is an 8 px square with a
  centre dot; coasting adds an X through the symbol and the data block tag `CST`; uncertain is a 12 px diamond with
  its 95% radius drawn as a circle of true scale and the tag `UNC`; a track reacquired within the configured seconds
  has a 9 px circle in the alert colour and the tag `REACQ`. Flags map to symbols exactly as the table in the
  `traffic` module says.
- **OA-DSP-040** Each target has a velocity leader along its ground track as long as it flies in the configured
  minutes (default 1; none without a velocity), a history trail of the last N positions (default 5) as small dots
  in the dim colour, and a data block at a fixed offset: callsign (the hex address without one), altitude in hundreds
  of feet and ground speed in knots over ten, each rounded to the nearest, `---` or `--` when absent, then the state
  tags. Data blocks do not yet avoid one another (LS-401, M8).
- **OA-DSP-041** A target set at capacity refuses the target, counts it and returns a typed error; it never truncates
  silently, and the frame shows `TARGETS NOT DRAWN: N` in the alert colour. Targets off the map area or under the
  legend strip are counted as off screen, not as refused. The traffic tables are sized for the set's capacity and a
  frame allocates nothing in the list.
- **OA-DSP-042** The traffic of a frame is a pure function of the set, the projection and the style: the same
  input gives the same display list and the same frame hash, and a synthetic scene has a golden hash.
- **OA-DSP-043** The shell (`oa-scope-replay`, which links the tracker; the display crate does not) converts tracker
  rows into targets: the newest row at or before the frame's time, none from a track silent for more than 3 s, ground
  speed and track from the velocity (no track under 1 knot), the altitude only with the altitude flag, the
  coasting and uncertain flags, the radius from the covariance, history rows a fixed step apart, the reacquired age
  from the newest flagged row inside the window, the newest callsign heard at or before the time (never a later one).
  Targets off the map are kept out of the set so they cannot use its capacity.
- **OA-DSP-044** `oa-scope-replay` runs the tracker over an ADS-B clip from a warm-up before a start time and writes one
  PPM per step with a `hashes.txt` of `frame_index time sha256` lines. The same inputs give the same hashes, with
  or without the images; a frame does not depend on earlier frames. A bad number, time or step is a usage error and
  a window without reports is a plain failure.
- **OA-DSP-045** Traffic layers draw above the map and below the legend strip's backdrop, so the legend is never covered
  by a target.
- **OA-DSP-046** The traffic target set cannot hold an inconsistent or invalid target: its columns are private and the
  only way in is the push, which refuses a target with a position or a history position off the globe, a trail longer
  than 16, or unknown flag bits (counted, and `TARGETS MALFORMED: N` on the frame, as `TARGETS NOT DRAWN` is for a full
  set). Nothing is defaulted for a missing column. Every part of a target that cannot be drawn is counted by reason
  (a track outside [0, 3600), a leader or 95% circle the geodesic or projection could not place, a history dot that does
  not project, a text that did not fit), an uncertain target with no usable 95% radius shows the tag `R95?`, and the
  loss total names what was lost. A target carries `CS?` and its hex address when two callsigns were heard for the
  address at one instant. A position is validated before it can count as off the map: a position off the globe
  and one that does not project are loss, counted apart from off-map, which is not. An altitude beyond what the data
  block holds is written `999+` or `-99-` and counted, never shown as a flight level. `Target` has a size guard of 164
  bytes.
- **OA-DSP-047** Tracker rows are stored as plain columns per address and shown without substitution: columns of
  different lengths are an error before any row is read, a row with a column missing is refused and counted, rows before
  the window are counted as before the window (not as loss), and a row past a named capacity (addresses 2^17, rows per
  address 2^18, rows 2^24) is refused and counted, except that the total-rows cap is checked from the sizes the input declares
  before any row is stored and aborts the run with an error before the frame loop. The store and the callsigns are built by the shell, once per drain, with the capacities a parameter; the core reads
  them without allocating. The work of one frame is a fixed number of binary searches per address (the newest row, the
  callsign, the newest reacquired row, the history dots), stated beside the capacities. Rows of one address at one
  time keep the order the tracker wrote them and the later one wins. A velocity that is not a number is counted and
  shown unknown; a history dot whose position is off the globe is dropped and counted, and the target stays. An altitude beyond `i32` feet is clamped to one above the absent value
  and counted, an altitude flagged present but not a number is counted and shown absent, a speed beyond the field is the
  largest it holds, a 95% radius that is not a number or does not fit `f32` is unknown (counted) and a reacquired window
  that could equal the absent age is refused. Time differences that do not fit an `i64` are older than any window and
  an overflow in the history lookup is counted. The stored `Row` has a size guard of 32 bytes.
- **OA-DSP-048** Callsigns are the newest heard at or before the frame's time, never a later one. Two different
  callsigns for one address at one instant are a conflict, reported as such and the same whatever order the reports
  came in; the conflict stands until the next report with a callsign. Callsign inputs of different lengths are an
  error.
- **OA-DSP-049** `--start` is parsed with integer arithmetic only: Unix seconds (digits, and a point with up to nine
  more) or `YYYY-MM-DDTHH:MM:SS[.fffffffff]Z` with fields of fixed width, a year from 1970 to 2261 (the last whole year
  an `i64` of nanoseconds holds), a day the month has (leap years included), no leap second. A time that does not fit
  is refused with a usage error and never saturated; fractional nanoseconds are kept exactly. Durations and steps use
  the same integer parser.
- **OA-DSP-050** `oa-scope-replay` refuses an `--out-dir` that is not empty unless `--overwrite` is given, which deletes
  only the files the tool writes (`frame-NNNN.ppm` and `hashes.txt`) before the run writes its own.
- **OA-DSP-051** `oa-scope-replay` ends with a report of what it excluded or lost, each reason named on its own and
  with its denominator (clip frames by kind, decode, quality-gate removals by reason, tracker refusals by reason, rows
  by reason, callsigns, every part of a target by reason), and exits with code 3 after writing its output when work
  was lost, unless `--allow-loss` is given. Exit codes: 0 success, 1 a failure, 2 a usage error (including a duration
  that is not a multiple of the step), 3 work was lost.
  **Loss** is work the run was asked to do and did not: damaged clip frames, payloads that could not be decoded,
  reject frames (messages the bridge refused to frame, so input that never arrived), tracker output rows that
  overflowed, valid measurements the tracker could not use (no track slot, a numerical failure), rows the store
  refused (address table, rows per address, a missing column), addresses that could not be put in time order, targets
  the display refused (full, malformed, columns unreadable), positions off the globe or that do not project, a leader,
  a circle (or one drawn smaller than true), a history dot or a text that could not be drawn, a stamp clipped, and
  history dots dropped for a position off the globe.
  **Not loss**, because the system did what it is for or the screen says so: reports removed by the ADS-B quality gates
  and records the decoder dropped as invalid, and measurements the tracker refused as invalid or rejected by the jump
  rule (input that is wrong is refused and counted by reason, not work lost), status frames, clip frames and tracker
  rows outside the window, tracks that ended, targets off the map, empty callsigns, callsign conflicts (shown as
  `CS?`), values clamped or shown unknown (velocity or altitude not a number, a 95% radius unknown), and altitudes
  beyond the data block, written `999+` or `-99-`. There is no limit on a fraction removed by the quality gates.
- **OA-DSP-052** A full traffic frame allocates nothing after the tables are built: the rows to targets conversion, the
  traffic drawing over a full set (every target state, refusals at capacity, a malformed target) and the whole frame
  chain (a non-empty airspace map, traffic, stamp, pixels and the frame hash) run under the counting allocator of `validation/oa-alloc-check`
  (window `display_window`) with zero allocations (the set is refilled, with its refusals and a malformed target, inside the measured window), a positive control for each allocation call and a negative control that injects one.

### Replay and flight state (oa-replay)

Milestone M1, "record and replay; bit-exact replay harness". Recorded feeds go in, one
deterministic flight state comes out.

- **OA-RPL-001** The flight table is a struct-of-arrays of fixed capacity (default 32,768
  flights) allocated once at construction; bad settings (capacity 0 or above 2^24, a non-positive
  age limit or sweep spacing) are errors. Its key indexes are sized to at least twice the capacity,
  so the load never exceeds 1/2.
- **OA-RPL-002** An SFDPS row, applied with the replay time as an argument, fills the flight's
  identity (callsign, computer id, departure, arrival, aircraft type, flight-plan id), last
  position (latitude, longitude, altitude, ground speed, track velocity), position time,
  last-update time, last source and controlling unit, and counts the update.
- **OA-RPL-003** A position older than the flight's last position time never overwrites it: the
  row's position fields are dropped and the row is counted as out of order (per flight and in
  total), while its other fields still apply, with the exceptions of OA-RPL-201. An equal or newer
  position applies. A row without a usable position (absent or malformed) is never out of order,
  and leaves the position time as it was (OA-RPL-200).
- **OA-RPL-004** Identity fields fill when empty and confirm when equal; a different non-empty
  callsign, departure, arrival or aircraft type is a counted field conflict and the held value
  is kept. The computer id is not identity (each centre assigns its own) and takes the latest.
- **OA-RPL-005** A TBFM row links to the flight whose flight-plan identifier equals the row's
  `gufi` attribute (the TBFM `gufi` is the SFDPS `flightPlan@identifier`, not the SFDPS GUFI). A
  `gufi` matching no flight creates a TBFM-first flight, which takes its real GUFI when SFDPS
  reports the same flight plan; linked, created and promoted rows are counted.
- **OA-RPL-006** A TBFM row with no `gufi` links through the `(centre, tmaId)` pair, held in a
  second fixed-size index; a row that matches nothing is counted as unlinked and applies
  nothing. The same `tmaId` from another centre is another key.
- **OA-RPL-007** TBFM rows set the metering columns: meter fix, ETA at the meter fix, STA (the
  scheduling-fix time where the feed has no meter-fix time), runway and the frozen flag (`sfz`).
  A TBFM row older than the flight's last TBFM message time is refused whole and counted as out
  of order.
- **OA-RPL-008** A TBFM `DEL` change type, on the `air` or on the `mrp` section, clears the
  metering columns and the frozen flag. It does not delete the flight, and metering can start
  again.
- **OA-RPL-009** A flight is evicted only when its last update is more than its age limit
  (default 30 minutes, configurable, for a flight with no status seen; see OA-RPL-067 for the others)
  older than the replay time, at a sweep that runs on a fixed
  cadence of replay time (multiples of the sweep spacing, default one minute). Exactly the limit
  is not older than it. Nothing is evicted between sweeps.
- **OA-RPL-010** A sweep visits slots in slot order and returns the evicted slots to a free stack,
  so slot reuse is deterministic; eviction removes the flight from every index.
- **OA-RPL-011** A full table refuses a new flight (from either feed) and counts it; it never
  evicts early to make room. Existing flights keep updating, and the refused flight is admitted
  once a sweep has freed a slot (INTERFACES IF-R1).
- **OA-RPL-012** The state hash is SHA-256 over a canonical serialisation iterating live flights
  in GUFI order, so it does not depend on insertion history: shuffled application of commuting
  updates gives the same hash.
- **OA-RPL-013** The state hash does not depend on slot assignment or eviction history, and
  changes when any column changes; an empty table's hash does not depend on capacity.
- **OA-RPL-014** The open-addressing index (linear probing, backward-shift deletion) agrees with
  a model under random inserts and removes, including with a degenerate hash that forces long
  probe runs; it refuses past load 1/2; its hash functions are fixed, unkeyed, deterministic
  functions of the key (not adversarial-robust, and bounded by the index length per operation).
- **OA-RPL-015** The flight table allocates nothing after construction: every column and index
  keeps its address and capacity through applying, refusing, sweeping and evicting.
- **OA-RPL-016** The pipeline takes replay time from each frame's `recv_unix_ns` and records a
  checkpoint (replay time, frames, rows applied, flights live, state hash) at the first frame at
  or after each multiple of the checkpoint spacing (default 10 s).
- **OA-RPL-017** Every parse, decode and apply failure is counted under its own kind (XML,
  unsupported root, bad GUFI, bad timestamp, malformed field, TBFM field, and the rest); REJECT
  and STATUS frames are counted and not decoded. Nothing is silently skipped.
- **OA-RPL-018** A frame earlier than one already seen, by no more than the clock's step tolerance
  (10 minutes), does not move replay time back and is counted as a time regression; a larger
  difference follows OA-RPL-061 to OA-RPL-063.
- **OA-RPL-019** Replaying the same recordings twice, or with the input read in any chunking
  (1 byte to 16 MiB), or with the streams named in another order, gives identical checkpoints,
  counters and final state hash.
- **OA-RPL-020** A `.oaf.gz` recording replays identically to its raw `.oaf` (chosen by the
  file's first bytes); a gzip file cut short is counted and its frames up to the cut replay.
- **OA-RPL-021** Recordings are read as streams and merged with a fixed-capacity min-heap, one
  head frame per stream, ordered by `(recv_unix_ns, feed kind, seq, stream index, byte offset)`: a
  total order, equal to a global sort of the same frames whenever each stream is in receive order.
  Memory is bounded by the number of streams times a fixed per-stream buffer, not by the input
  length (peak resident memory on the real recordings is reported by the binary's `timing` line).
- **OA-RPL-022** Corrupted recordings (every single-byte flip, truncations, random garbage and
  damage) and corrupted XML payloads never panic; a damaged frame leaves a count, and a cut inside
  a frame is a counted truncation.
- **OA-RPL-023** The `oa-replay` command line gives each file the feed kind of the flag that
  names it, sets the checkpoint spacing, age limit, sweep spacing and capacity, and rejects
  missing, zero, non-numeric and unknown arguments.
- **OA-RPL-024** The `oa-replay` binary prints the checkpoints and a final summary (frames and
  rows per feed, TBFM linking, table totals, errors by kind, flights live and the final state
  hash); checkpoints are printed as they are recorded; a progress line (replay time, frames per
  second, replay-time to wall-time ratio, resident memory) goes to stderr every N minutes of
  replay time (default 10); every stdout line except the `timing` line is identical from run to
  run, whatever the flag order or read size; exit status is 2 for usage errors and 1 for an unreadable file.
- **OA-RPL-025** Replaying the real SCDS recordings under `OPENATCA_REC` (SFDPS and TBFM) twice
  and with another read size gives identical checkpoints and the same final state hash, with no
  frame, gzip or clock error and TBFM rows linked to SFDPS flights (skipped if unset). The final
  hash equals that of the earlier all-in-memory replay.
- **OA-RPL-026** The canonical serialisation of a fixed table state has a pinned golden hash, so
  a change of the serialisation or of what the state holds is a deliberate, visible edit.

### FNS NOTAM messages (oa-notam)

Built on the survey in `docs/FNS_SURVEY.md` (112 real messages, AIXM 5.1 `AIXMBasicMessage` with
the FAA event extension).

- **OA-NOTAM-001** Decodes an AIXM 5.1 basic message with one `event:Event` into one row, in a
  caller-owned struct-of-arrays: event identifier (UUID), NOTAM series, number, year, type (new,
  replace, cancel), the referenced NOTAM (series, number, year, all three
  or none), location designator, affected FIR, ICAO location, cross-reference id, issue time and
  cancellation time; verified on hand-written messages.
- **OA-NOTAM-002** Times are strict: ISO-8601 UTC (`Z` only, real calendar dates, no leap second,
  1970 to 9999, a fraction of at most three digits) and the compact `YYYYMMDDhhmm` form read to
  exact Unix milliseconds; every other form is refused.
- **OA-NOTAM-003** The output has a stated capacity for rows, text bytes, polygons and vertices,
  reserved once; rows round-trip through the columns, a push at capacity writes nothing and says
  so, and the staged row keeps its budgeted size (RULESET A3, A7, C2).
- **OA-NOTAM-004** The features of a message (airport, runway, runway direction, runway element,
  taxiway, taxiway element, navaid, unit, obstacle area, vertical structure, radar system,
  primary surveillance radar) are recorded as a kind mask and the most specific kind, with the
  airport designator and ICAO indicator, the runway (or runway direction) designator, the navaid
  type and the unit designator; member order does not matter.
- **OA-NOTAM-005** The classification codes are read: Q code (`selectionCode`), scenario,
  classification (INTL, DOM, MIL) and the traffic, purpose and scope letters as bit sets; short
  codes are trimmed of surrounding white space; a letter outside the vocabulary or repeated is a
  field error.
- **OA-NOTAM-006** The start and end of the effective period come from both the `validTime` and
  the compact `effectiveStart` and `effectiveEnd`; either alone stands, and two that disagree
  are a field error on that field. An end written `PERM` or ending in `EST` sets an explicit
  permanent or estimated flag.
- **OA-NOTAM-007** The NOTAM text and the schedule are kept in a bounded string table,
  entity-decoded with white space kept exactly; a text longer than the limit, empty, or in
  several text nodes is a field error and takes no table space.
- **OA-NOTAM-008** The altitude floor and ceiling are in feet with a reference (MSL, AGL, flight
  level): `lowerLimit` and `upperLimit` (`SFC`, `GND`, `UNL`, `FLnnn`, feet and metres above
  ground or mean sea level) win over the `minimumFL` and `maximumFL` pair; an unknown form, a
  value outside -2,000 to 100,000 ft, or a floor above the ceiling in the same reference is a
  field error.
- **OA-NOTAM-009** The circle is the centre of `coordinates` (`DDMM[NS]DDDMM[EW]`, whole minutes,
  range checked) in 1e-7 degree and the `radius` (1 to 999 nautical miles) in metres; a centre
  or radius alone is kept but is not a circle.
- **OA-NOTAM-010** Polygons (`posList` of runway and taxiway element surfaces) are spans of a
  shared vertex array of 1e-7 degree coordinates, closing vertex not repeated, each with its
  feature kind; a coordinate out of range or not a number, an open ring, fewer than four points,
  an odd number of values, a reference system other than EPSG:4326 as a URN, or a ring over the
  vertex limit is a field error on the row, none of the message's polygons is kept, and nothing is
  dropped silently.
- **OA-NOTAM-011** Every field has its own refusal: a malformed value is a counted field error,
  the row is kept, the field is absent and its bit is set in the row's error word; a
  single-valued field given twice, or an empty value element, is an error rather than first-wins.
- **OA-NOTAM-012** A document with no event, several events, no or an invalid event identifier, no
  or several NOTAM elements, a wrong root or namespace, or that is not well-formed XML is refused
  whole, appends nothing, is counted by reason and does not affect its neighbours.
- **OA-NOTAM-013** Reaching a row, text, polygon or vertex capacity is a refusal naming the
  table, with the rows, string table, polygons and vertices exactly as before (IF-R1); a
  document over the element limit is refused likewise.
- **OA-NOTAM-014** Elements and attributes outside the structure seen in real data, including a
  known name in another namespace and feature kinds not in the survey, are counted per path in a
  bounded table (overflow counted too) and never errors; only the topmost unknown element of a
  subtree is counted and nothing inside it is read.
- **OA-NOTAM-015** The decoder never panics: every truncation, deletion, duplication and
  replacement of each byte of messages with every feature kind and field, and seeded random
  damage, give a row or an error, with every present column inside its model range and the
  tables unchanged after a refusal.
- **OA-NOTAM-016** After construction decoding does not grow any column or table, across accepted
  and refused documents; no state leaks from one document to the next or between decoders.
- **OA-NOTAM-017** Every real FNS message of the local sample (`OPENATCA_SCDS`, directories named
  `fns*`) decodes with no rejection, no field error and no unknown path; the test reports
  messages, rows, rejections by reason, field errors by field, feature kinds, rows with geometry
  and throughput, which must be at least 20,000 messages per second (parse and decode, one
  thread, release builds); it skips when the variable is unset.
- **OA-NOTAM-018** Against an airspace snapshot of one ARTCC (`OPENATCA_SNAPSHOT`) the real rows'
  locations, airports, runways and navaids are resolved to model entries, and the polygons are
  tested against the model's ARTCC boundary; the distinct identifiers and rows that resolve are
  reported for all rows and for the model's own FIR; the test skips when either variable is
  unset.
- **OA-NOTAM-019** Field names, bits and display tables agree.
- **OA-NOTAM-020** The cross-check resolves a location to a model airport (identifier or
  alternate), fix, navaid, terminal waypoint or ARTCC; an airport to a model airport; a runway
  when the airport resolves and every end named is a threshold of that airport; and an ILS when
  the airport has a localizer; it counts distinct identifiers and rows per role and scope and
  lists those that do not resolve.
- **OA-NOTAM-021** The cross-check reports which polygons touch the model's own ARTCC boundary
  (high and low) by geodesic containment of vertices and boundary samples, and how many circle
  centres are inside it; polygons oa-geo refuses and boundaries oa-geo refuses are counted, not
  errors.
- **OA-NOTAM-022** A ring coordinate too large for the scaler (twelve integer digits at 1e-7, or a sum past
  `i64`) is refused, not wrapped: the polygons field is a counted error with nothing kept, and a following
  message still decodes.
- **OA-NOTAM-023** A namespace URI written with a character reference compares as its expanded text, so its elements
  are not counted as another namespace.
- **OA-NOTAM-024** A referred number and year without a referred series is a valid reference (the local format
  names the cancelled or replaced NOTAM that way) and leaves the referred series zero; a referred series alone, or
  a year alone, is still a malformed reference.
- **OA-NOTAM-025** The calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days that do not exist in the
  month; hour 24 is refused.

- **OA-RPL-027** A frame whose receive time goes backwards inside one stream is not reordered
  (that would need unbounded lookahead): it is emitted in file order and the pipeline counts a time
  regression.
- **OA-RPL-028** Rotated recordings of one feed chain in name order (`--sfdps-dir`, `--tbfm-dir`,
  `--tfms-dir`, `--fns-dir`, or several files in one stream, raw or gzip, any mix) and replay
  identically to the unsplit recording, with the same checkpoints and final state hash.
- **OA-RPL-029** TFMS and FNS payloads are decoded (OA-RPL-041 to OA-RPL-059); a payload of the wrong
  kind of document is counted under its typed error, and nothing is left in a "not applied" bucket
  (the `not_applied` count stays in the summary line and is always 0).

### ADS-B in the replay (oa-replay)

ADS-B reports (`docs/ADSB_PAYLOAD.md`) merge with the SWIM recordings in one time-ordered stream
(input path of the M2 benchmark, `docs/M2_DESIGN.md`).

- **OA-RPL-030** The ADS-B aircraft table keeps, per address (the 24-bit address, with the
  non-ICAO bit making a separate key), the latest report (time, latitude, longitude, barometric
  altitude, ground speed, track, flags, source type) and counts; callsign, squawk, category and the
  quality fields (NIC, NACp, NACv, SIL, SIL type, version, GVA, SDA) are carried forward from
  reports that have them.
- **OA-RPL-031** The ADS-B table is a struct-of-arrays of fixed capacity (65,536 aircraft) allocated
  once; a new address when it is full is refused and counted (per table and per feed), known
  aircraft keep updating, and no column moves or grows. Aircraft not heard for a while are freed
  (OA-RPL-065).
- **OA-RPL-032** An ADS-B report older than the aircraft's held report is refused whole and counted
  (per aircraft, in total and per feed); an equal time applies.
- **OA-RPL-033** The ADS-B table is part of the replay state hash (`replay_hash`, used by every
  checkpoint and the final hash) in address order, so the hash does not depend on the order
  aircraft were first seen, and changes with any column or count.
- **OA-RPL-034** Until an ADS-B frame has been fed the replay hash equals the flight-table hash, so
  the checkpoints and final hash of every replay without ADS-B are unchanged (pinned to the value the
  code gave before ADS-B existed).
- **OA-RPL-035** An ADS-B payload whose header or length `oa_adsb::decode_payload_lenient` refuses
  (length, magic, layout, too many records) is counted under its own error kind and applies none of
  its reports; no corruption of a payload panics.
- **OA-RPL-036** ADS-B is a feed kind of its own that sorts after TFMS and FNS in the merge key, so
  at an equal receive time SWIM frames come first and ADS-B last, whatever order the streams were
  named in; the merged order is still the global sort of the total key (OA-RPL-021).
- **OA-RPL-037** A mixed SFDPS, TBFM and ADS-B replay of hand-made recordings gives the same
  checkpoints, counters and final hash in every stream order and read size, with the right frame,
  row, aircraft and error counts.
- **OA-RPL-038** `--adsb <file>` and `--adsb-dir <dir>` give a file or a directory of rotated
  recordings the ADS-B feed kind; the binary prints the ADS-B feed line, an `adsb` table line and
  the errors, identical run to run and across read sizes and rotation.
- **OA-RPL-039** Replaying a real ADS-B clip written by `oa-adsb-clip` (`OPENATCA_ADSB_CLIP`) twice
  and with another read size gives identical checkpoints and final hash, with no error and no refused
  aircraft (skipped if unset).
- **OA-RPL-040** An `oa-replay` merge of a one-second-frame ADS-B clip and a SWIM feed gives the
  frames in receive-time order, interleaved at about one-second granularity.

### TFMS and FNS in the replay (oa-replay)

TFMS flight and flow messages (`oa-tfms`) and FNS NOTAMs (`oa-notam`) are applied to tables of their
own beside the flight table. Both are bounded, struct-of-arrays, and left out of the state hash until a
frame of their feed has been fed (as the ADS-B table is).

- **OA-RPL-041** The TFMS flight table keeps, per flight reference, the latest value of each estimate and
  time the messages carried (estimated and actual departure and arrival, original, runway, gate, fix and
  coordination times, estimate flags and types, status), leaving a value alone when a later row lacks it;
  identity (acid, departure, arrival, identifier) fills an empty column and a disagreement is counted.
- **OA-RPL-042** A TFMS flight row older (by `sourceTimeStamp`) than the flight's latest is refused whole
  and counted; an equal time applies.
- **OA-RPL-043** A TFMS flight links to the SFDPS flight whose flight-plan identifier equals its
  identifier, or, with no identifier, to the single SFDPS flight with its acid, departure and arrival;
  an ambiguous key and an identifier that matches nothing leave it unlinked; the link is the SFDPS GUFI,
  retried while unlinked (the identifier every row, the acid key on the 1st, 2nd, 4th, 8th ... row) and counted by method.
- **OA-RPL-044** TFMS programme rows (restrictions, advisories, FCA/FEA, airport configurations) keep one
  row per programme key with the latest row, count rows by program type, refuse an older row, and leave
  the table at the first sweep after the programme's end time.
- **OA-RPL-045** A TMI flight list entry sets the flight's TMI update type, flight-list entry count, first
  FCA identifier, TMI update time and status.
- **OA-RPL-046** The TFMS flight table has a fixed capacity: a new flight with no free slot is refused and
  counted, a held flight still updates, a flight silent for six hours of replay time is evicted at a
  sweep and its slot is reused.
- **OA-RPL-047** A TFMS payload that is not well-formed XML, not a TFMS document, has a refused field or
  is counted under its own typed error and applies nothing; the next good payload applies. A payload
  with more rows than the row tables hold applies the rows that fit (OA-RPL-073).
- **OA-RPL-048** A TFMS flight row with no flight reference is not applied and is counted.
- **OA-RPL-049** FNS new, replacement and cancellation NOTAMs are keyed by series, number and year (a
  local-format NOTAM, which has no series, by its location in the series' place): a
  replacement removes the NOTAM it names and is stored, a cancellation removes the NOTAM it names and is
  not stored, and a replacement or cancellation naming nothing held is counted.
- **OA-RPL-050** A NOTAM re-issued under a held identity replaces it whole, unless it was issued earlier
  than the held one, which is refused whole and counted.
- **OA-RPL-051** A NOTAM leaves the table at the first sweep after its end or cancelled time in replay
  time; one with no end time leaves after thirty days without an update; a reader can ask how many are in
  force at a time.
- **OA-RPL-052** A NOTAM keeps its circle, altitude limits and references, affected feature, polygon and
  vertex counts and the bounding box of its polygon vertices: the latitude range and the smallest
  longitude interval covering them, with the west end larger than the east end when the interval
  runs through 180 degrees.
- **OA-RPL-053** An FNS payload that is not well-formed XML or is refused by the decoder, and a row with
  no usable identity, are counted under typed errors and applied to nothing; a malformed
  field is counted and treated as absent.
- **OA-RPL-054** The NOTAM table has a fixed capacity and refuses and counts a new NOTAM when full.
- **OA-RPL-055** Until a TFMS or an FNS frame has been fed the replay hash is the one it was before they
  were applied, so SFDPS and TBFM replays (with or without ADS-B) hash unchanged.
- **OA-RPL-056** Once fed, the TFMS flight, TFMS programme and FNS tables are in the replay hash, each
  table's change moves it, and the same frames give the same hash.
- **OA-RPL-057** A mixed SFDPS, TFMS, FNS replay from hand-made recordings gives the same checkpoints,
  errors and final hash whatever the order the files are named in and however they are chunked.
- **OA-RPL-058** TFMS and FNS STATUS and REJECT frames are counted as other frames and apply nothing.
- **OA-RPL-059** Replaying the real 2026-11-14 SCDS TFMS and FNS samples (`OPENATCA_DATA`, `.msg` files
  given synthetic one-second receive times by the test) twice gives identical checkpoints, errors and
  final hash (skipped if absent).
- **OA-RPL-060** The TFMS, programme and NOTAM tables allocate nothing after construction: every buffer
  keeps its address and capacity through applying, sweeping and reusing slots.

### Clock, ages and oversized documents (oa-replay)

- **OA-RPL-061** One frame whose receive time is more than 2 minutes ahead of the replay clock and of its
  feed's last frame does not move the clock, is processed at the old time and is counted as held, and nothing is evicted
  because of it.
- **OA-RPL-062** A forward step of the receive times confirmed by three consecutive frames (each no
  earlier than the one before, all more than 2 minutes ahead) moves the clock to the third frame and
  shifts the last-update times in the flight, TFMS, programme and NOTAM tables by the same amount, so
  no flight is evicted by the step and eviction goes on from the new time.
- **OA-RPL-063** The same for a step back: three consecutive frames more than 2 minutes behind the
  clock move it back (the earlier ones are counted as time regressions as before), the tables' times
  shift with it, and time is not frozen.
- **OA-RPL-064** A replay whose frames are never more than 2 minutes from the clock (small
  regressions included) hashes exactly as it did before the step rules, so the existing hashes hold.
- **OA-RPL-065** The ADS-B table frees an aircraft whose held report is more than 30 minutes older than
  the replay clock, at a sweep each minute of replay time, and reuses the slot; a long run or a flood
  of addresses therefore fills the table only while it lasts, and the hash does not depend on slot
  assignment.
- **OA-RPL-066** An ADS-B report whose time is more than 2 minutes after the replay clock is refused
  and counted before it takes a slot or becomes the held report, so it cannot make the real reports of
  that aircraft look out of order.
- **OA-RPL-067** A flight stays as long as its latest SFDPS flight status allows without a message:
  active 2 hours, proposed 4 hours, completed, cancelled or dropped 10 minutes, no status seen 30
  minutes (each configurable), judged at the sweep.
- **OA-RPL-068** An SFDPS document with more messages than the event table has rows applies the first
  messages that fit; the others are counted, one each, as row-capacity errors.
- **OA-RPL-069** An SFDPS document whose route text does not fit the string table applies the messages
  before the first that does not fit; the others are counted, one each, as string-capacity errors.
- **OA-RPL-070** A TBFM document with more rows than the row table applies the rows that fit; the
  others are counted, one each, as capacity errors.
- **OA-RPL-071** Wild frames that are scattered, or that disagree in direction or order, never confirm
  a step and each is counted as held; confirmed forward and backward steps are counted separately.
- **OA-RPL-072** ADS-B evictions and refusals for time are counted, appear in the replay hash from the
  first of either (under a new tag; a replay with neither hashes as before), and the same history
  gives the same hash.
- **OA-RPL-073** A TFMS document with more flight or programme rows than the row tables applies the
  rows that fit; the others are counted, one each, as capacity errors.
- **OA-RPL-074** The three state ages are settings that must be positive and have command-line flags
  (`--active-age-secs`, `--proposed-age-secs`, `--terminal-age-secs`).
- **OA-RPL-075** A flight's age limit follows its latest status: a message with no status leaves it, a
  new status replaces it.
- **OA-RPL-076** Ten-digit TFMS flight references and restriction identifiers that differ by a multiple of 2^32
  are different flights and programmes; the state hash writes an identifier below `u32::MAX` as four
  bytes as before and a larger one as an escape plus eight bytes.
- **OA-RPL-077** An ADS-B record whose position or flags are out of range is dropped and counted under
  the bad-record error kind, one for each; the other records of its payload are applied, and a record
  with only an unknown source, a bad measurement or a set reserved byte is kept.
- **OA-RPL-078** A frame more than 2 minutes ahead of the clock and of its own feed's last frame is
  held, not accepted: 9.9 minutes ahead, repeated among honest frames, never moves the clock and the
  honest frames are never counted as regressions.
- **OA-RPL-079** Each feed's frames are compared with that feed's own last frame: feeds whose receive
  times differ by a constant under the tolerance count no regressions against each other, a frame
  before its own feed's last is still a regression, and a feed steadily more than the tolerance behind
  the others is held and never steps the clock.
- **OA-RPL-080** A confirmed step marks every flight, programme and NOTAM row as stale until it is
  updated; the marked rows are counted (`stale_marked`) and the rows still stale can be counted at any
  time (`pipeline_stale_count`); eviction ages are as before.
- **OA-RPL-081** A frame time equal to `i64::MIN`, the clock's "no frame yet" value, is held and counted,
  never taken as the clock or as a candidate: five such frames neither step the clock nor let a later
  frame a day ahead be taken as a first frame, and before the first frame it is held as well.
- **OA-RPL-100** (proof) `clock_observe` never panics or overflows for any clock state, feed index (any
  `usize`) and frame time (any `i64`, including the extremes) from any reachable candidate state (every
  candidate count at most `CLOCK_CONFIRM_FRAMES`); a step is returned only on the frame that completes a run of
  `CLOCK_CONFIRM_FRAMES` consecutive candidates of that frame's feed, with the saturated difference as its
  delta, every candidate count is then cleared, no step is returned from an empty candidate state, and a frame
  never adds candidates to another feed.
- **OA-RPL-101** (proof) From any state the pipeline can hold (every feed with an accepted frame has it at or
  before the clock), one frame that is accepted moves a running clock forward by at most `CLOCK_TOLERANCE_NS`
  (2 minutes), or by at most `FEED_GAP_MAX_NS` (30 minutes) when no other feed has ever had an accepted frame,
  and the state stays consistent with the clock that results.
- **OA-RPL-150** A feed's candidate frames behind the clock continue one run only when each is within the
  tolerance of the one before, in either direction: a genuine step back whose three frames arrive up to a
  second out of order confirms on the third; candidates ahead of the clock continue a run when each is not
  more than the tolerance before the one before and not more than 30 minutes after it. Candidates a day apart
  each start a new run and never confirm.
- **OA-RPL-170** A confirmed step shifts every ADS-B report time by the step (when the report times follow the recorder, see
  OA-RPL-250) as it does the other tables' ages, and marks each aircraft stale (counted in `stale_marked`, counted by `adsb_stale_count`): a forward
  step of an hour keeps every aircraft that was live, and a backward step of ten minutes accepts the next
  genuine reports.
- **OA-RPL-250** A confirmed step is held by the ADS-B table as pending, without moving any report time, until
  the next report settles whether the report times follow the recorder's clock or keep their own: the report's
  lag behind the clock is read both ways, and a reading fits when the report is at most 2 minutes ahead and at
  most 30 minutes behind. If only the first fits, or both and its lag is no larger in size, the held times shift
  by the step; otherwise if the second fits the table's `offset_ns` takes the step and the held times stay. A
  report stamped by a clock that did not step, after a forward or backward step, is applied, not refused out of
  order or too far ahead. Each resolution is counted (`domain_follows_clock`, `domain_kept_own`). The
  offset and the pending step are hashed only when either is non-zero.
- **OA-RPL-251** Ages are measured against the clock minus the offset and the pending step, so a sweep while a
  step is pending evicts nobody the step did not age, however long no ADS-B frame comes.
- **OA-RPL-252** A report that fits neither reading while a step is pending is refused as too far ahead and
  counted, and the step stays pending until a report that fits.
- **OA-RPL-253** Steps before the first report add; the resolution uses their sum.
- **OA-RPL-254** The progress line's replayed span is computed with saturating arithmetic: a stream whose
  clock steps are each representable but whose total span is not does not stop the run.
- **OA-RPL-255** A file is read in pieces of at most `MAX_CHUNK` bytes; its size never sets an allocation.
- **OA-RPL-171** A stale mark is cleared only by an accepted update that carries what the row shows: an ADS-B
  report that is accepted, an SFDPS row whose position was not refused as older and that carries a position or
  a flight status. A refused older position, or an SFDPS amendment with neither, leaves the row stale.
- **OA-RPL-172** A TBFM row clears the stale mark only when it carries metering state (a delete, a meter fix,
  a runway, a time or a freeze flag); a row of identity alone does not.
- **OA-RPL-173** The stale marks are part of the state hash, in every table, written only when some row is
  stale so a replay with none hashes as it did.
- **OA-RPL-174** A confirmed step whose size does not fit in 64 bits is not applied: the clock and every age
  are kept, the candidate state is kept as it was, and the step is counted under `clock-step-overflow`.
- **OA-RPL-200** The position time is written only together with a position: a row whose position is absent or
  malformed leaves the flight's position time as it was, whatever `positionTime` it carries, so a later valid
  position older than that row's time (but not older than the held one) is applied.
- **OA-RPL-201** Each flight keeps the timestamp of the newest SFDPS message applied to it. A message older than
  that does not change the flight's status, controlling unit or computer id and is counted as a stale amendment
  (`stale_amendments`). A message with no usable timestamp is
  applied as before and counted (`no_source_time`). The source times and the counters are part of the state
  hash, written only when some flight has a source time or a counter is not zero.
- **OA-RPL-202** A claim on a flight-plan identifier held by another flight succeeds only if that flight's
  newest SFDPS message is older than the claiming message; otherwise the old message does not take the
  identifier, and is counted (`plan_refused_old`). A TBFM update for the identifier links to the owner. A claim
  with no source time (TBFM, or an SFDPS message with no timestamp) takes the identifier as before.
- **OA-RPL-203** A cancelled or replaced NOTAM leaves a tombstone (identity, and the `issued` time of the
  cancelling or replacing notice). A new or replacing notice whose identity has a tombstone and whose `issued`
  time is not later than the tombstone's is refused and counted (`tomb_refused`, an out-of-order row), so a
  redelivery cannot bring it back; a cancellation or replacement of a NOTAM not held still leaves a tombstone,
  so a cancellation that arrives first holds.
- **OA-RPL-204** A tombstone is kept until the cancelled NOTAM's end (or cancelled) time if it was held and had
  one, else for seven days of replay time from the cancellation (a redelivery arrives within hours; a week is far
  past any seen), then dropped at the next sweep (`tomb_expired`). A confirmed step of the replay clock moves a
  tombstone held for replay time with the clock (OA-RPL-263).
- **OA-RPL-263** A tombstone's deadline records its basis: the notice's own end or cancelled time (feed time) is
  never moved by a clock step; the seven-day span of replay time is moved by it, forward or back, so a step
  never shortens a tombstone's protection (a redelivered old NEW is still refused after a forward correction
  past the seven days).
- **OA-RPL-264** When a merge or an after-frame hook ends the run with an error, the frame and gzip error
  counts found so far are already in the pipeline's own counts, and the error is still returned.
- **OA-RPL-265** `replay_state_digest` covers the basis of each live NOTAM tombstone's time (a span of replay
  time, or the notice's own end), which decides whether a step of the replay clock moves it: two states
  that differ only in that flag have different digests. Nothing is added while no tombstone is held, and
  `replay_hash` is unchanged.
- **OA-RPL-266** A recording with a monotonic time per frame and a genuine gap (the wall time elapsed is the
  monotonic time elapsed, up to a difference within the clock tolerance of 2 minutes, the frame ahead of the
  clock) is accepted at once at its own time (`Tick::RealGap`): no age is shifted, no row is marked stale,
  and a flight silent for the gap expires under its age limit (an ACTIVE flight after 3 hours), also when
  the first frame after the gap waited seconds in the recorder before it was stamped (OA-RPL-272).
- **OA-RPL-267** A frame whose wall time is far from the clock while the monotonic time elapsed is short
  (a step of +3 hours in 1 second, or a step back) is a step of the recorder's clock: handled by rules
  4 to 6 as before, and counted in `Clock::mono_steps`.
- **OA-RPL-268** A recording with no monotonic time (`OAF1` frames, or `process_frame`) replays exactly as
  before the field existed: the same clock, tables, counters and `replay_state_digest`.
- **OA-RPL-269** `replay_state_digest` covers every time-domain decision (counts, a running hash of each
  decision's kind and elapsed times, and each feed's reference), written only once there is a monotonic
  reference or a decision, so two replays that classify a gap differently differ.
- **OA-RPL-270** The classification at its tolerance: elapsed times differing by exactly
  `CLOCK_TOLERANCE_NS` (2 minutes) are time that passed, one nanosecond more is a step; a monotonic time
  that went back, a frame with none, and a frame behind the clock are not classified as a real gap.
- **OA-RPL-271** Monotonic times are compared only between frames of the same stream: two recordings of the
  same feed from different recorder runs, whose monotonic origins happen to make a gap look genuine, are
  not classified, and `process_frame_mono` and `clock_observe_mono` take the stream. A feed's monotonic
  reference is dropped when a step confirmed by another feed forgets or re-bases its record.
- **OA-RPL-272** The wall time elapsed since a feed's last frame is split exactly into the monotonic time
  elapsed (time that passed) and a step (the rest). A step within the clock tolerance is noise; a larger one
  goes through the candidate run, the vote and the confirmation of rules 4 to 6, and when it confirms the
  ages shift by the step alone (never more than the clock's move), so a gap that is partly a step ages
  every row by the time that passed and translates only the step.
  `MONO_AGREE_TOL_NS` (10 seconds) agree, one nanosecond more is a step; a monotonic time that went back, a
  frame with none, and a frame behind the clock are not classified as a real gap.
- **OA-RPL-273** A cancellation removes the held NOTAM it names only when that version was issued at or before
  the cancellation; a version issued after it is kept and the cancellation is counted (`cancel_stale`, hashed
  only when not zero). Either way the cancellation leaves its tombstone, so the NOTAM held and the tombstone
  issued time are the same in every arrival order of a notice, its cancellation and its reissue.
- **OA-RPL-274** The longitude cover of a wide polygon set (more than half a turn) takes the largest gap between
  neighbouring vertex longitudes from one sort, not a scan of every vertex against every other: the work grows as
  n log n, and the cover equals the pairwise method's.
- **OA-RPL-205** The tombstone table is of fixed size (set when the NOTAM table is made, by default the size of
  the NOTAM table); when it is full the oldest tombstone is evicted and counted (`tomb_evicted`). Tombstones are
  part of the state hash, after the NOTAMs, only when there are some or a tombstone counter is not zero.
- **OA-RPL-206** Programmes whose title or element differ after byte 24 are different programmes: a longer
  identity is kept whole in a fixed pool of cells made at init and compared whole on lookup. A new programme whose
  title or element is over 512 bytes, or for which no cell is free, is refused and counted (`refused_long`,
  reported as a full table); two different identities are never merged.
- **OA-RPL-275** A NOTAM is its series (or `!` and its location when it has none), number and year. A local-format
  row that cancels or replaces names the referred NOTAM by number and year; the referred series is `!` and the
  row's own location, so the cancellation removes that office's NOTAM and not another office's of the same number.

- **OA-RPL-180** Frames reach the clock in merged receive order, so a stream whose times run ahead cannot move
  the clock ahead of another stream that still has frames: a stream walking 119 s per frame beside a stream
  talking every second for 30 minutes leaves the clock never more than the tolerance past the second stream's
  latest frame and never evicts its flight.
- **OA-RPL-181** A forward run of `CLOCK_CONFIRM_FRAMES` candidates is a step with no vote (in merged order
  the other feeds are silent over the gap); a backward run is a step only if the feeds backing the new time
  (the stepping feed and every other feed with an open candidate on that side within the tolerance of the
  frame) are at least as many as those backing the old clock (every other feed with an accepted frame, no
  candidate and a last frame within the tolerance of the clock). A refused backward run is counted in
  `steps_outvoted` and asks again on the feed's next candidate.
- **OA-RPL-182** A feed whose times jump ten minutes back while three others agree is outvoted: no backward
  step, the refusals are counted, and the clock ends with the three.
- **OA-RPL-183** A feed ten minutes behind that speaks first in merge order starts the clock; three feeds that
  agree ten minutes later take it with one forward step.
- **OA-RPL-184** A step back of an hour on every feed at once is one backward step, however the frames
  interleave, with every age shifted (nothing evicted) and the rows marked stale.
- **OA-RPL-185** In a single-feed recording a frame later than the feed's last by at most 30 minutes is
  accepted and the clock follows it, so frames 5 minutes apart move the clock each time with no step, and rows
  age out as time passes.
- **OA-RPL-186** In a single-feed recording a gap over 30 minutes is a confirmed step (the ages carry over the
  pause), as for any run of three candidates.
- **OA-RPL-187** Two interleaved feeds that are both sparse (gaps over the tolerance) do not freeze the
  clock: the feed that speaks first runs alone while the other has no accepted frame, then the clock moves by
  forward steps (11 over 30 frames of each feed, 2.5 minutes apart, trailing the newest frame by under 15
  minutes) with no step back.
- **OA-RPL-188** (proof) A step is returned only for three candidates; a backward step also needs the vote
  (at least as many backers of the new time as of the old clock); a refused backward step is held and
  counted with the run kept at the limit; three forward candidates in a row are always a step; a forward step,
  a backward step, a counted refusal and a lone-forward advance are each reachable.
- **OA-RPL-189** The limit of the single-feed rule: in a single-feed recording one wild frame less than 30
  minutes after the feed's last is accepted (a gap and a glitch cannot be told apart), one more than 30
  minutes after it is held as a candidate.
- **OA-RPL-190** A feed that resumes alone after a pause, with another feed's last frame just before the pause
  (merged order), takes a forward step after three frames: the ages of both feeds' rows are shifted and the
  rows marked stale, not refused.
- **OA-RPL-230** The pipeline's checkpoints are held in a ring allocated at construction (`checkpoint_ring`,
  default 8,640, 24 hours at the default 10 s); when it is full the oldest undrained checkpoint is dropped
  and counted (`checkpoints_dropped` in the summary) and the ring never grows, however many checkpoints
  are recorded.
- **OA-RPL-231** Consumers take checkpoints out with `pipeline_drain_checkpoints`: a run that drains as it
  goes (the `oa-replay` binary, the benchmark loader) drops none, even with a ring of one.
- **OA-RPL-232** `checkpoint_ring` of 0 and a checkpoint spacing under 1 second are refused when the
  pipeline is built (a checkpoint hashes every table, so a shorter spacing would hash on every frame).
- **OA-RPL-233** `replay_hash` is the hash of the table contents; `replay_state_digest` adds the replay
  time, every field of the clock and the sweep and checkpoint cadence state: two pipelines with equal
  content hashes but different clocks (one holding a candidate step) have different digests, and the
  same further frames then step one and not the other. `replay_hash` is unchanged.
- **OA-RPL-234** A failed write of a checkpoint line, the summary or the timing line stops `oa-replay`,
  prints one line on stderr (a closed pipe is named) and exits with status 1; the checkpoint whose line
  failed is not counted as printed.
- **OA-RPL-235** A failing after-frame hook stops `run_streams_with` at that frame and its error is
  returned; no later frame is read.

## M5 — Terrain and obstacles

### GeoTIFF elevation (oa-tiff)

- **OA-TIFF-001** Reads classic TIFF 6.0 in little and big endian; a bad byte-order mark or magic
  number is refused, BigTIFF (magic 43) is refused with a typed error, and truncated headers are
  errors.
- **OA-TIFF-002** Reads strips (any rows per strip, including a short last strip) and tiles
  (including tiles that overhang the right and bottom edges), uncompressed, giving the samples
  that were written.
- **OA-TIFF-003** Decompresses Deflate (tags 8 and 32946) through the zlib wrapper, checking the
  header, refusing a preset dictionary and verifying the Adler-32 and the exact decompressed size.
- **OA-TIFF-004** Decodes TIFF 6.0 LZW (MSB-first, early change, clear codes, table full) to
  exactly the size the segment needs, and refuses bad codes, streams that end early and streams
  that run long.
- **OA-TIFF-005** Applies predictor 2 (horizontal differencing, 16-bit integers) and predictor 3
  (floating point byte planes), in both byte orders, with every compression and in both layouts,
  restoring float bit patterns exactly; a predictor that does not fit the sample type is refused.
- **OA-TIFF-006** Reads one sample per pixel as 32-bit float or 16-bit signed integer; other
  layouts, other sample counts, planar data and other compressions are refused with a typed error.
- **OA-TIFF-007** Places samples by the GeoTIFF raster-type convention: for `PixelIsArea` the
  tiepoint is the outer corner of the pixel and samples are half a pixel inside it, for
  `PixelIsPoint` the tiepoint is the sample; a tiepoint away from the raster origin is honoured.
- **OA-TIFF-008** Accepts only geographic rasters in NAD83 (4269) or WGS 84 (4326) with axis-aligned
  pixels, one tiepoint and a positive finite pixel scale inside the valid latitude and longitude
  range; missing or inconsistent georeferencing and `ModelTransformation` are refused.
- **OA-TIFF-009** Turns the `GDAL_NODATA` value and any non-finite sample (NaN, infinity) into
  NaN samples and counts them; an unparsable nodata text is refused.
- **OA-TIFF-010** Enforces stated limits (file, dimension, pixel count, segment, tag value count,
  IFD entries, ASCII length) and never panics: every truncation, byte inversion and seeded random
  input gives a grid or an error; a repeated tag is refused.
- **OA-TIFF-011** `grid_elevation_at` interpolates bilinearly, is exact at samples, reproduces a
  plane exactly, reports points beyond the outermost samples as outside, and reports a cell with
  any nodata corner as no data rather than averaging the rest.
- **OA-TIFF-012** `grid_max_in_radius` returns the highest valid sample within a great-circle
  radius with its position, counts valid and nodata samples, reports whether the circle's edge lies
  inside the data, and refuses a bad centre or a radius that is not positive, finite and within the
  limit.
- **OA-TIFF-013** `grid_max_in_polygon` does the same inside a polygon (even-odd in the
  latitude/longitude plane, concave polygons included), and refuses polygons with fewer than three
  or too many vertices, non-finite vertices or a span over 180 degrees of longitude.
- **OA-TIFF-014** A tile set answers elevation, radius and polygon queries across tile edges for
  both overlapping tiles and abutting tiles whose lattices line up, agrees with the same query on
  the undivided grid, does not invent a value across tiles that do not line up, and prefers a tile
  with data over one with nodata.
- **OA-TIFF-015** Tile sets over the tile limit are refused, empty sets answer outside, and
  polygons over the vertex limit are refused.
- **OA-TIFF-016** On the real USGS 3DEP 1 arc-second tiles (`OPENATCA_DATA`) every tile is 3612 by
  3612, pixel-is-area, its nominal degree begins on an integer degree at a pixel corner, shared
  borders with the east and north neighbours coincide in position, agree in value to within a metre
  for all but a fraction of a percent of samples, and match far better than the same overlap shifted
  by one pixel; skipped when the variable is unset.
- **OA-TIFF-017** For every NASR airport inside the loaded tiles the 3DEP elevation at the airport
  reference point is compared with the surveyed field elevation; the test reports count, median,
  95th percentile and the ten worst per airport kind, and the same comparison with the sampling point
  moved by half and whole pixels; it requires a median under 15 ft and a 95th percentile under 60 ft
  for land airports; skipped when the data is absent.
- **OA-TIFF-018** For every DOF obstacle inside the loaded tiles `AMSL - AGL` is compared with the
  3DEP elevation at its position, verified and unverified separately; the test reports median,
  95th percentile and the ten worst, and requires a median under 25 ft for verified obstacles;
  skipped when the data is absent.
- **OA-TIFF-019** On two adjacent real tiles a tile set answers elevation points on both sides of
  the shared edge to within a metre of the owning tile, and a radius across the seam returns the
  highest sample of both tiles; skipped when the data is absent.

- **OA-TIFF-020** The `*_conservative` radius queries count a sample when any quad it is a corner
  of touches the circle (the closed square of one full step around its centre, a superset of the
  `PixelIsArea` pixel and valid for `PixelIsPoint`), so no interpolation between samples can exceed
  the result; a high sample clipped by the circle edge is found, a query smaller than one cell
  returns the corners of the cell it lies in, and a sample two steps out is not counted.
- **OA-TIFF-021** The conservative radius is a WGS 84 geodesic distance bounded below with the
  smallest meridian and prime-vertical radii over the latitude band the radius can reach, so a
  sample within the true distance always counts and over-inclusion is bounded (documented, tested
  against Vincenty's inverse formula at 0.5, 20, 41 and 60 degrees); a circle reaching a pole or
  crossing the antimeridian is refused with a typed error.
- **OA-TIFF-022** The conservative polygon queries count a sample when its footprint touches the
  polygon (edge contact, containment either way, concave polygons, polygons smaller than a cell).
- **OA-TIFF-023** The conservative queries across a tile set (abutting and overlapping tiles)
  return what the undivided grid returns; for overlapping tiles a valid sample beats nodata.
- **OA-TIFF-024** A conservative result is never a bare number: it is complete or incomplete, and
  an incomplete one carries the count of nodata samples, the count of missing samples (the area
  leaves the loaded data, including past the outermost samples), the number of whole-degree squares
  holding them, and the highest valid sample as a lower bound only.
- **OA-TIFF-025** Conservative queries refuse an empty tile set, tiles that do not share one
  lattice, too many tiles, too large a region, a bad radius, a bad centre and bad polygons.
- **OA-TIFF-026** On random small grids the conservative radius maximum is never below a
  brute-force reference (bilinear and nearest-sample surfaces sampled at 1/16 of a step, inside the
  circle by WGS 84 distance) and never above the best sample within the documented over-inclusion
  margin.
- **OA-TIFF-027** The same property for polygons against a brute-force reference.
- **OA-TIFF-028** On a 3 by 3 block of real 3DEP tiles around Chicago (`OPENATCA_DATA`), for 500
  random points and radii of 1, 3 and 5 nm, every conservative answer is complete and not below
  the centre-only maximum except for the sphere-versus-ellipsoid radius difference (under 1% of
  queries); the test reports how often and by how much (in feet) it exceeds it and the query time;
  skipped when the data is absent.
- **OA-TIFF-029** Before the elevation array is sized from the declared dimensions, every strip or
  tile must lie inside the file and be able to hold what the declaration needs (at least its
  uncompressed size, or its size over 1032 when compressed, the most DEFLATE can reach); a file of
  a few hundred bytes declaring 8192 by 8192 is refused with a typed error and nothing allocated.
- **OA-TIFF-030** The Orientation tag is honoured by refusal: absent or 1 (top-left) is read, any
  other value is refused with a typed `Unsupported` error rather than placing terrain wrongly.
- **OA-TIFF-080** A point at exactly +180 or -180 degrees longitude (the same meridian) gets the
  same elevation answer from a tile that holds either alias.
- **OA-TIFF-081** A Float32 `GDAL_NODATA` sentinel is parsed to f32 and compared with the samples
  in that type, so a sample equal to the sentinel is nodata (`-123.456`, `-3.4028235e38`).
- **OA-TIFF-082** A non-finite sample in a file (+/-infinity) is nodata and counted, and a
  bilinear query next to it answers `NoData`, never a known elevation.
- **OA-TIFF-083** A non-finite sample in a `Grid` built by a caller reads as NaN through
  `grid_sample`, and the point and polygon queries treat it as nodata; no query returns
  `Metres(NaN)` or an infinite peak.
- **OA-TIFF-084** A conservative maximum over samples that are non-finite is `Incomplete`
  with those samples counted as nodata, never `Complete`.
- **OA-TIFF-085** A NaN or infinite double in a tag (pixel scale, tie points, transformation) is a bad
  value at the point it is read, not a coordinate.
- **OA-TIFF-086** A `GDAL_NODATA` text that parses to an infinity (`inf`, `1e999`) is refused; only a
  finite number or the text `nan` is a nodata value.
- **OA-TIFF-087** A conservative query whose work (lattice cells times the larger of 1 and the polygon
  vertices, plus the tiles) is over `MAX_QUERY_WORK` is refused with `TooMuchWork` before any cell is
  visited.
- **OA-TIFF-088** The work estimate of a conservative query includes one lookup in each candidate tile
  for every lattice cell, so overlapping tiles raise it; the estimate is never below the work the
  query counts as it runs, and an over-budget query is refused before any cell is visited.
- **OA-TIFF-089** The GeoTIFF vertical units key (4099) is read: metre or absent leaves samples as they
  are, international foot (9002) and US survey foot (9003) are converted to metres with the exact
  factors 0.3048 and 1200/3937, nodata is matched before conversion, and any other unit is refused
  with `GeoRef`.
- **OA-TIFF-090** A plain (centre-in-area) polygon query whose cell tests times the polygon vertices are
  over `MAX_QUERY_WORK` is refused with `TooMuchWork` before any cell is visited.
- **OA-TIFF-091** The plain radius query charges its cell tests to `MAX_QUERY_WORK` and adds the
  cells of every tile it scans, so overlapping tiles cannot multiply the work; over the limit it is
  refused with `TooMuchWork` before the cells of the tile that crosses it are visited.
- **OA-TIFF-092** A scan whose bounding box has a bound that is NaN or infinite is refused with a
  typed error; it never scans a widened box.
- **OA-TIFF-093** A grid with fewer than two samples a side, or a lattice that is not finite and
  positive, is refused by the plain queries with `BadGrid` and never panics a point lookup.
- **OA-TIFF-094** A conservative query whose box sits at a lattice index too large to hold is
  refused with `TooManySamples`, not clamped to a box that finds nothing.
- **OA-TIFF-095** The file the fuzzer found overflowing an offset product (committed as a seed) is read
  and queried without a panic.

### Digital Obstacle File (oa-dof)

- **OA-DOF-001** Parses every field of the FAA DOF line format (OAS code and number, verification,
  country, state, city, latitude and longitude with hemisphere, type, quantity, AGL and AMSL feet
  including negative AMSL, lighting, horizontal and vertical accuracy, marking, FAA study number,
  action, Julian date) into struct-of-arrays with the currency date of the file.
- **OA-DOF-002** Positions are exact in 1e-7 degrees for seconds to 0.01, signed by hemisphere;
  degrees, minutes, seconds and hemisphere are range checked, the poles and the antimeridian are
  accepted and anything beyond them is refused.
- **OA-DOF-003** Accuracy codes decode per the README tables to feet and metres (horizontal 1 to 8,
  vertical A to H; unknown, blank and invalid codes decode to none), and every lighting and marking
  letter of the README has a name.
- **OA-DOF-004** Each field that fails validation, including separator columns, rejects the line
  with a typed error naming the field and the 1-based line; short and over-long lines are rejected
  with their length; Julian dates are checked against the calendar including leap years.
- **OA-DOF-005** Counts physical lines and rejected lines (the kept list is capped, the total is
  not), accepts CRLF and LF and a missing final newline, skips nothing silently, and refuses files
  with a missing or malformed currency line or header as a whole.
- **OA-DOF-006** The bounding-box query returns the obstacles inside a latitude/longitude box with
  inclusive edges and supports boxes that cross the antimeridian.
- **OA-DOF-007** The parser never panics: every truncation, byte replacement and seeded random
  input gives a file or an error, and the accepted columns stay the same length with
  lines = accepted + rejected + header.
- **OA-DOF-008** The real `DOF.DAT` (`OPENATCA_DATA`) parses with no rejected line, the line and
  obstacle counts agree, the per-state and per-country files are subsets whose total falls short
  by under 100 obstacles, and positions are inside the world; skipped when the data is absent.
- **OA-DOF-009** A bounding-box query whose bound is NaN or infinite is refused with a typed error;
  it is never read as 0 degrees or as the whole world.
- **OA-DOF-010** The seconds field is read with checked arithmetic and held under 60.00, at its edges
  (59.99 accepted, 60.00 and 99.99 refused).

## M1 — ADS-B history input

### JSON (oa-json)

- **OA-JSON-001** Parses exactly RFC 8259: every JSONTestSuite `y_` case is accepted and every
  `n_` case refused (gated on `OPENATCA_DATA/raw/jsontestsuite`, 95 and 188 cases), and a
  hand-written set of valid and invalid texts (leading zeros, `+`, `NaN`, trailing commas,
  single quotes, comments, vertical tab as whitespace, unterminated values) agrees.
- **OA-JSON-002** Every bound is a constant in `Limits` and exceeding it is a typed error with the
  byte offset: input bytes, nesting depth, string bytes (counted on the raw text between the quotes, escapes
  as written, the same on the plain and the escaped path),
  and node count (values plus object keys); a text exactly at a bound is accepted.
- **OA-JSON-003** Numbers are kept as their source text; `as_f64` returns `None` when the value is
  not finite, and `as_i64` returns `None` for a fraction, an exponent or a value outside `i64`
  (`i64::MIN` and `i64::MAX` are accepted).
- **OA-JSON-004** Strings decode the eight short escapes and `\u` including surrogate pairs; a lone
  or mis-ordered surrogate, a bad escape and an unescaped control character are refused with
  their own errors.
- **OA-JSON-005** Invalid UTF-8 anywhere in the input, a byte-order mark, and any non-whitespace
  after the top-level value are refused, the last with the offset of the first extra byte.
- **OA-JSON-006** Duplicate object keys are accepted and all kept in document order; `get`
  returns the last.
- **OA-JSON-007** Nothing recurses: a million nested arrays parse (with the depth limit raised)
  and are walked without overflowing the stack, and the default depth limit refuses them.
- **OA-JSON-008** For each of the 35 JSONTestSuite `i_` cases the outcome is the documented choice
  in the test table (out-of-range numbers accepted as text with `as_f64` finite or not; lone
  surrogates, invalid UTF-8, a byte-order mark and 500-deep nesting refused); a case added to
  the suite without a documented choice fails the test.
- **OA-JSON-009** The document is a flat arena in document order; `elements`, `members`, `get`,
  `skip` and the kind accessors give the right nodes for nested arrays and objects, and queries
  of the wrong kind or an out-of-range index return empty or `None`.
- **OA-JSON-010** No input panics: every prefix of a sample and 20 000 seeded random byte
  mutations give a document or an error.

### Tar archives (oa-tar)

- **OA-TAR-001** Reads ustar entries with prefix names, directories, empty files and data padded
  to 512-byte blocks, and finds the end-of-archive marker.
- **OA-TAR-002** A sequence of readers is one stream: splitting the archive at every byte offset
  into two parts, and into parts of one byte and zero bytes, gives the same entries.
- **OA-TAR-003** If the stream ends early, at any byte offset, the reader returns a typed
  `Truncated` (offset, stage, entry, declared size, data present) after the complete entries,
  never an error and never a panic; a stream ending between entries without the end marker, or
  inside the marker, is also `Truncated`; the reader stays finished.
- **OA-TAR-004** GNU long names (type `L`), PAX `path` overrides and ignored keys, skipped PAX
  global headers, and GNU base-256 sizes are read.
- **OA-TAR-005** A size over the entry limit, a name over the name limit, a long-name or PAX record
  over the metadata limit, a bad checksum, a non-octal size, a non-UTF-8 name, a negative
  base-256 size and data after a lone zero block are each refused with a typed error carrying
  the offset, and the reader then refuses further calls.
- **OA-TAR-006** Entry data the caller did not read is skipped; link, device and directory entries
  carry no data whatever the size field says.
- **OA-TAR-007** A reader error is reported with the stream offset and an interrupted read is
  retried.
- **OA-TAR-008** Seeded random byte damage and garbage of several lengths never panic.
- **OA-TAR-058** One `next_entry` call consumes at most `max_meta_records` long-name, long-link
  and PAX header records (zero-length ones included) and `max_meta_total_bytes` of their
  content; a chain longer than either is refused with a typed `TooManyMeta` error carrying the
  offset and the reader then refuses further calls, so a caller never waits on an unbounded run
  of extension headers. The default ceilings are 64 records and 1 MiB.
- **OA-TAR-059** A pending PAX `size` override applies only to the next header that is not an extension
  record: a long-name, long-link or PAX record that follows keeps the size in its own header.

### ADS-B history (oa-adsb)

- **OA-ADSB-001** A trace row with every column and an aircraft object decodes to the right time
  (base plus offset, to the millisecond), position (1e-7 degree), barometric and geometric
  altitude, ground speed (0.1 kt), track (0.1 degree), vertical rate, source type, callsign,
  squawk, category, `nic`, `nac_p`, `nac_v`, `sil`, `sil_type`, `version`, `gva` and `sda`;
  a row without an object has those fields absent.
- **OA-ADSB-002** Each flag bit (stale, new leg, rate is geometric, altitude is geometric) is read
  as readsb writes it; a geometric altitude is reported as geometric and not as barometric;
  `"ground"` sets the ground flag; null altitude, speed, track and rate are absent.
- **OA-ADSB-003** All 14 readsb source types are recognised by name in readsb's order; rows from
  files before 2022 (7 to 9 columns) decode with the source absent; extra columns are ignored.
- **OA-ADSB-004** Aircraft-object fields are range checked (bit-field widths) and trailing spaces
  are removed from callsigns; unknown keys are ignored; duplicate keys keep the last; a malformed
  or null known field is left absent and counted (OA-ADSB-025), the row is kept.
- **OA-ADSB-005** Only the rules the position's meaning needs (position range, flags, row shape,
  time) refuse a row, alone, counted under their own reason; the other rows of the trace are kept.
- **OA-ADSB-006** The box (edges inclusive) and the time window (`[from, to)`) are applied while
  decoding; rows outside are counted separately and not delivered; the ZAU box is
  38.5 to 46.5 N, 83.5 to 93.5 W.
- **OA-ADSB-007** A structural error (not an object, missing or malformed `icao`, `timestamp` or
  `trace`, an out-of-range timestamp, a JSON error) refuses the whole trace and leaves the output
  and the counts untouched; a `~` address sets the non-ICAO bit.
- **OA-ADSB-008** A trace over the row limit is refused whole; a row over the column limit and an
  object over the member limit are refused alone.
- **OA-ADSB-009** No trace input panics: every prefix of a sample and 5 000 seeded random byte
  mutations give reports or an error, and every kept report is in range.
- **OA-ADSB-010** The OAF1 payload (`docs/ADSB_PAYLOAD.md`) round-trips and has the documented
  layout: header, 80-byte little-endian records, reserved byte zero.
- **OA-ADSB-011** A payload with a wrong length, magic, record length, reserved field, count or a
  record value out of range is refused and appends nothing; the encoder refuses an empty or
  out-of-range row range and more than the per-frame maximum.
- **OA-ADSB-012** The summary counts distinct aircraft, reports with `nac_p`, and those with
  `nac_p` of 8 or more, and finds position jumps (a pair the speed rule of OA-ADSB-028 calls
  impossible and at least 0.5 NM apart) between consecutive reports of one aircraft in time order,
  exempting a report
  that starts a new leg and counting closer pairs separately as noise; input order does not
  matter.
- **OA-ADSB-013** Scanning a day archive decodes gzip and plain trace files, skips other entries,
  counts and names a corrupt gzip and a JSON error without stopping, delivers one batch per
  trace with reports, and applies the time window.
- **OA-ADSB-014** An archive split into parts gives the same result as the whole; the first part
  alone ends in a typed truncation after delivering every complete trace before the cut; a sink
  error stops the scan.
- **OA-ADSB-015** `oa-adsb-clip` writes OAF1 MESSAGE frames sorted by time whose receive time is
  the first report's time, honours `--box`, `--from` and `--to` (Unix seconds or ISO UTC),
  reports truncation and exits 3 unless `--allow-truncated`, writes only frames to standard
  output with `--out -`, and exits 2 on bad arguments and 1 on an unreadable part.
- **OA-ADSB-016** On the real first part of an adsb.lol day archive (`OPENATCA_DATA/raw/adsblol`;
  the first 8 MiB, or all of it with `OPENATCA_ADSB_FULL`) no trace is refused and no row is
  rejected, reports are kept in the ZAU box, and under 0.1 % of consecutive pairs are position
  jumps; the test prints the counts and skips when the data is absent.
- **OA-ADSB-017** `oa-adsb-clip` starts a new frame when the next report is at least
  `--frame-ms` (default 1000) after the frame's first report, stamps each frame with its first
  report's time, and writes every report exactly once and in time order whatever the span.
- **OA-ADSB-018** `--frame-ms` outside 1 to 60000, or not a whole number, is refused with exit 2
  and the usage line; 1 and 60000 are accepted.
- **OA-ADSB-019** The frame splitter ends a frame at the span (a report exactly one span later
  starts the next) or at 60 000 reports, clamps the span to 1 to 60000 ms, and its ranges are
  contiguous, non-empty, in order and cover every report once.
- **OA-ADSB-020** A clip frames reports with any grouping, including the earlier one of up to
  60 000 reports per frame, and `decode_payload` gives the same reports in the same order.
- **OA-ADSB-021** The `ADS2` payload carries the measurements (wind speed and direction, true and
  indicated airspeed, Mach, magnetic and true heading, outer and total air temperature, roll, track
  rate) in 80-byte records at the documented offsets, round-trips them, and refuses a record whose
  direction, heading or roll is out of range.
- **OA-ADSB-022** An `ADS1` payload as the first clips wrote it still decodes, with the measurements
  absent and its own rules (64-byte records, zero bytes after offset 57, up to 60 000 records).
- **OA-ADSB-023** The measurements are read from the aircraft object (and indicated airspeed and roll
  from the row's own columns, the object overriding), are per observation and never carried to a row
  without them.
- **OA-ADSB-024** A malformed or out-of-range measurement is left absent and counted once per row in
  `Stats::bad_measurements`; the row is still kept; 360 degrees wraps to 0.
- **OA-ADSB-025** A row keeps its position when a field other than time, position, shape or flags is
  unknown, malformed, out of range or null: an altitude, speed, track, vertical rate, an object
  field (callsign, squawk, category, NIC, NACp, NACv, SIL, GVA, SDA, version, a non-object or
  oversized object) is left absent, an unknown source type becomes `unknown` and an unknown
  `sil_type` name becomes `unknown`, flag bits above the four readsb defines are cleared. Each
  occurrence is counted per field and per value (`Stats::unknown_fields`, `Stats::unknown_values`
  for the first 32 distinct pairs, the rest in `unknown_overflow`); counts add across traces. Flags
  that are null, not an integer or above 255 still refuse the row, because they say whether the
  position is current and continuous.
- **OA-ADSB-026** Rows of a trace whose address is 0x000000 or 0xFFFFFF (misconfigured
  transponders, shared by unrelated aircraft) are counted in `Stats::reserved_address` and not
  output; the same digits with the non-ICAO mark are an ordinary identifier.
- **OA-ADSB-027** `decode_payload_lenient` keeps the readable records of a payload and counts the
  rest (position or flags out of range dropped; unknown source kept as `unknown`; bad measurement
  kept absent; reserved byte ignored) and a wrong header or length still refuses the whole payload;
  `decode_payload` stays strict and gives the same reports on a clean payload. The `ADS1` and
  `ADS2` layouts are unchanged.
- **OA-ADSB-028** `jump_verdict` decides whether a displacement is possible from the distance, the
  time gap, the emitter category and the reported ground speeds: not over the category's hard cap
  (1,000 kt ordinary and reserved codes, 2,000 kt high-performance, unmanned and no-information,
  4,088 kt space), consistent with a reported ground speed (at most 1.2 times the larger plus 40 kt plus 2 kt
  per second of gap), and under the class ceiling when no ground speed is given. A 900 kt aircraft
  reporting 900 kt is plausible; a 450 kt aircraft that jumps 2,782 kt is not; a displacement
  within the position allowance is always plausible.
- **OA-ADSB-029** The emitter category the jump rule uses comes from `category_observe`: an ordinary
  class is taken at once, a cap-raising one (`A0`, `A6`, `B0`, `B6`, `B7`) only after three
  consecutive printed categories agree, and a reserved code is ignored and never raises the cap above
  the ordinary one. The summary applies it, and a report that is itself a jump does not count.

- **OA-ADSB-150** The hard cap of an emitter category is 2,000 kt for `A0`, `A6`, `B0`, `B6` and no category
  (never below a configured ordinary cap), 4,088 kt for `B7`, and the ordinary cap for every other code, reserved
  codes included; the no-speed ceiling is 600 kt for `A1` and `A7`, 450 kt for `B1` to `B4`, 800 kt for `A2` to
  `A5` and `B6`, 200 kt for `C1` to `C3`, 4,088 kt for `B7` and 2,000 kt for every other code.
- **OA-ADSB-151** A pair beyond the position allowance whose time gap is not a positive number (NaN, zero,
  negative) or whose distance is not a number is a jump of the cap kind; within the allowance it is plausible
  whatever the gap; the allowance widens the cap and the ceiling alike.
- **OA-ADSB-152** With a ground speed on either or both reports, a displacement is plausible up to 1.2 times the
  larger speed plus 40 kt plus 2 kt per second of the gap and not beyond it, whatever the class ceiling; the hard
  cap still applies first, and with no ground speed the class ceiling applies.
- **OA-ADSB-153** `is_jump` is true exactly when `jump_verdict` is not `Plausible`.
- **OA-ADSB-154** Every field of an `ADS2` record (geometric altitude, vertical rate, category, NIC, NACp, NACv,
  SIL, SIL type, version, GVA, SDA and the rest) is encoded at its documented offset and decoded back unchanged.
- **OA-ADSB-155** Every field of an `ADS1` record that the layout has decodes unchanged, the measurements of
  the later layout are absent, and a non-zero reserved byte is counted and ignored.
- **OA-ADSB-156** A payload of exactly the most records of its layout is accepted and one more is refused; the
  length must be exactly header plus count records; `encode_payload` writes `to - from` records and refuses
  an empty, reversed or past-the-end range and one over the most; `frame_ranges` starts a frame at the first
  report that is a full span after the frame's first, or after the most rows, and clamps the span to 1 to 60000 ms.
- **OA-ADSB-157** A record with an unknown source, a bad measurement and a set reserved byte at once is kept,
  and each of the three counts shows it; any two of them show as two.
- **OA-ADSB-158** Unknown values are counted per field and per value, cut at 24 bytes on a character boundary,
  kept for 32 distinct pairs with the rest in the overflow count; adding two `Stats` sums every count and merges
  their value tables (a pair both have is summed, one the table cannot hold goes to the overflow count).
- **OA-ADSB-159** The archive scan counts trace and other entries (a cut entry is not a trace entry), keeps
  at most 16 refusal messages while counting every refusal, refuses an entry over the trace size limit unread,
  and applies the gzip ratio limit only above 1 MiB of output.
- **OA-ADSB-160** `parse_iso` reads `YYYY-MM-DDTHH:MM:SSZ` to the exact Unix second and refuses an out-of-range
  month, day, hour, minute or second (a leap second 60 is the next minute) and any other shape.
- **OA-ADSB-161** The clip command line reads every flag (`--out`, `--box`, `--from`, `--to`, `--frame-ms`,
  `--allow-truncated`, `--help`, `-h`, parts), with times as Unix seconds or ISO, and refuses a missing value, an
  unknown option, a bad box, a frame span outside 1 to 60000 ms, a missing output or no parts.

- **OA-ADSB-162** A command-line date is refused unless its day exists in its month (28 or 29 days in
  February by the Gregorian leap rule, 30 in April, June, September and November) and every field is
  plain digits, with no sign.
- **OA-ADSB-170** The supported speed envelope `SPEED_ENVELOPE_KT` is the highest hard cap of any category
  (4,088 kt) and `SPEED_ENVELOPE_MPS` is the same in metres per second.
- **OA-ADSB-171** A payload record whose report time is before 2000-01-01T00:00Z or at or after
  2100-01-01T00:00Z (`TIME_MIN_NS`, `TIME_MAX_NS`) is dropped and counted (`dropped`, `bad_time`); the strict
  decoder refuses the payload.
- **OA-ADSB-172** The trace decoder refuses and counts (`REJ_TIME`) a row whose absolute time (base plus offset) is
  outside `TIME_MIN_NS..TIME_MAX_NS`, so every row it produces is one the payload reader accepts.
- **OA-ADSB-100** (proof) The record decoder, for every `ADS2` record (80 octets) and every `ADS1` record
  (64 octets), never panics or overflows; a record it keeps has a position inside +-90 and +-180 degrees,
  a source type that is absent, known or `unknown`, and anomaly bits among the three that exist.
- **OA-ADSB-101** (proof) For every byte string of up to 176 octets the payload header check never panics, and
  a header it accepts has a length of exactly header plus count records; for a payload with a valid header and 4
  records of either layout (the record decoder and the column append taken as their stated contracts),
  `decode_payload_lenient` and `decode_payload` never panic or overflow, kept plus dropped equals the
  records, each anomaly count is at most the kept records, and the strict decoder reports only a record
  below the count.
## Weather aloft (oa-grib2)

GRIB Edition 2 decoder for NOAA RAP and HRRR winds and temperatures aloft, written from the WMO
Manual on Codes (FM 92 GRIB) and NCEP's public GRIB2 documentation. Pure core; not yet wired into
`oa-replay`.

- **OA-GRIB-001** Reads sections 0 to 8 of GRIB Edition 2: several messages back to back in one
  file, several fields in one message (sections 4 to 7 repeated, section 3 optionally omitted so
  the previous grid holds), and the local-use section is skipped.
- **OA-GRIB-002** Every input is bounded and inconsistent input is refused with a typed error:
  message length (`MAX_MESSAGE_BYTES`), grid points (`MAX_GRID_POINTS`, `MAX_MESSAGE_POINTS`),
  fields per message (`MAX_FIELDS_PER_MESSAGE`), section lengths, section order, end marker
  `7777`, truncation, a point count that disagrees with the grid, and group lengths that do not
  add up. Damage to the message framing stops the file walk and is reported without losing the
  messages before it.
- **OA-GRIB-003** Grid template 3.30 (Lambert conformal) is read on a spherical earth. The
  projection maps latitude and longitude to fractional grid indexes and back (round trip under
  1e-6 of a cell, both hemispheres, tangent and secant cones), the first grid point maps to
  index (0, 0), and grid length at LaD is carried to the map plane.
- **OA-GRIB-004** Grid template 3.0 (regular latitude/longitude) is read, with the stated far
  corner checked against Ni, Nj and the increments; longitude wraps on a global grid.
- **OA-GRIB-005** Anything else is refused with its own typed error: scanning modes other than
  0x00 and 0x40, earth shape codes other than the spherical 0, 1, 6 and 8, and grid templates
  other than 3.0 and 3.30.
- **OA-GRIB-006** Product templates 4.0, 4.1, 4.8 and 4.11 give parameter (named for TMP, RH,
  UGRD, VGRD, VVEL, HGT and PRES; the raw discipline, category and number are always kept), fixed
  surface type and scaled value (isobaric in Pa, height above ground in m), reference time and
  forecast lead; other templates are refused.
- **OA-GRIB-007** Data representation template 5.0 (simple packing) decodes exactly
  `(R + X * 2^E) / 10^D` for every bit width from 0 to 32, and a field of zero bits per value is
  constant.
- **OA-GRIB-008** Template 5.2 (complex packing) decodes groups of any width and length from
  the group tables (NG entries in each table, the last length taken from the template), and
  honours missing-value management 1 and 2 for single points and for whole groups.
- **OA-GRIB-009** Template 5.3 (complex packing with spatial differencing) decodes orders 1 and 2
  with descriptors of any width and a negative minimum; with missing-value management the
  differencing runs over the non-missing points only.
- **OA-GRIB-010** Section 6 indicators 0 (bitmap follows), 254 (previous bitmap) and 255 (none)
  are honoured; the number of set bits must equal the number of data values; missing points are
  NaN and counted.
- **OA-GRIB-011** Data representation templates other than 5.0, 5.2, 5.3 and 5.40 (5.41 PNG,
  and the unassigned numbers) are refused with `UnsupportedPacking(template)`; the message is
  recorded as skipped and the next message of the file is still decoded.
- **OA-GRIB-012** Bilinear interpolation at a latitude and longitude reproduces a bilinear
  field exactly, returns nothing outside the grid or when any of the four surrounding points is
  missing, and wraps across the seam of a global grid.
- **OA-GRIB-013** Earth-relative wind at a point: when the resolution and component flags say
  u and v follow the grid axes, they are rotated by the Lambert convergence angle
  `n (lon - LoV)`; the rotation is checked against true east and north found independently by
  projecting small steps, and against the sign convention on both sides of the orientation
  longitude. A grid whose components are already east and north is left alone.
- **OA-GRIB-014** The `.idx` inventory parser reads `msg:offset:d=YYYYMMDDHH:VAR:level:forecast:`
  lines including `N.M` message numbers for several fields in one message, refuses malformed or
  non-increasing lines and over-long input, finds rows by variable and level, and gives the
  inclusive byte range of a message.
- **OA-GRIB-015** No input makes the decoder panic: every truncation of a message and thousands
  of randomly mutated messages produce a value or a typed error.
- **OA-GRIB-016** Every RAP and HRRR file under `OPENATCA_DATA/raw` has its inventory read
  without unpacking (`inspect_stream`): grid first point equals the stated one, parameter and
  level names and order agree with the file's `.idx`; the test reports which packing templates
  the files use and skips when the data is absent.
- **OA-GRIB-017** On every decodable real field: TMP lies in 180 to 330 K, wind speed is below
  120 m/s, there are no missing points, and geopotential height rises as pressure falls at every
  grid point; skips when the data is absent.
- **OA-GRIB-018** RAP and HRRR 250 mb winds of the same analysis time, sampled at 1,000 points
  inside the HRRR domain, agree as earth-relative vectors with median below 4 m/s and 95th
  percentile below 12 m/s, and agree better than the same comparison of unrotated grid-relative
  components; skips when the data is absent.
- **OA-GRIB-019** Public ECMWF ecCodes test files (`OPENATCA_DATA/raw/grib2-testdata`) decode
  to physical fields: 5.0 temperature, 5.3 order 1 with a bitmap, 5.3 order 2 with a bitmap, 5.3
  order 2 with missing-value management, with exact missing counts and smooth neighbours;
  refusals (rotated grid) are typed; skips when the data is absent.
- **OA-GRIB-020** `inspect_stream` reads sections 1 to 6 of every field, including fields
  whose packing is not supported, and names the packing template number.
- **OA-GRIB-021** Template 5.40 (JPEG 2000), decoded by `oa-j2k`, gives exactly the packed
  integers of the independent oracle: every field of every `OPENATCA_DATA/raw/rap` file (2,516
  fields, 382,399,292 points) is compared sample by sample, before the `R`, `E`, `D` scaling, with
  the same field re-encoded as simple packing after decoding with OpenJPEG outside this
  repository (`derived/grib2-repack/rap`, whose scaling parameters must also be identical); no
  mismatch is allowed, and the test reports the decode time per field; skips when the data is
  absent.
- **OA-GRIB-022** The public NCEP GEFS file (`raw/grib2-testdata/gep10.t12z.pgrb2af78.grib2`, 43
  messages, 51 fields, all template 5.40) decodes with no field skipped, and its fields are
  physical: temperature 180 to 330 K, relative humidity 0 to 100, wind below 120 m/s, geopotential
  height rising as pressure falls at every point; skips when the data is absent.
- **OA-GRIB-023** Template 5.40 decodes a hand-built message: the codestream's samples are the
  packed integers and are scaled by `(R + X * 2^E) / 10^D`; a bitmap spreads the values over the
  grid; a field of 0 bits per value is constant and has no codestream.
- **OA-GRIB-024** Template 5.40 refuses what does not hold together: a compression type other than
  lossless, a sample depth or point count that differs from the section, a section that is not 23
  octets, and a damaged codestream (typed `Jpeg2000(error)`); the next message of a file is
  still decoded.
- **OA-GRIB-025** The value count of section 5 is checked against the grid's point count (or the
  bitmap's set bits) before anything is unpacked or allocated, for templates 5.0, 5.2, 5.3 and
  5.40, and the unpackers refuse a count beyond `MAX_GRID_POINTS` when called directly; a message
  claiming 4,294,967,295 values with zero bits each, or one group of that length, is refused with
  a typed error and nothing is sized from the claim.
- **OA-GRIB-050** A decoded value that is not finite after scaling (R, E and D can push a packed
  integer past the f32 range) is refused with a typed error for every packing; only a point the
  field marks missing comes out as NaN. `oa-wind` refuses an infinite cell when building an hour.
- **OA-GRIB-051** A complex-packed field's scratch memory (group tables, packed integers and the
  decoded values) is computed exactly and checked against a cap before anything is allocated:
  one length-one group per point with zero-bit tables and no data is refused when it exceeds the
  cap, and the limits are small enough to test with modelled values. The real RAP, HRRR and GEFS
  fields pass.
- **OA-GRIB-052** The cap is `Limits::scratch_bytes`, default 128 MiB, and does not depend on the
  size of the data section. A 16 Mi-point grid of length-one groups needs 448 MiB and is refused;
  a test builds the same construction at 300,000 points under a 1 MiB cap and sees
  `ScratchBudget` with the exact byte counts.
- **OA-GRIB-053** Scratch is 28 bytes per point-and-group (16 per group, 12 per point): the RAP 130
  grid (151,987 points) needs under 5 MB even with a group per point and HRRR CONUS (1,905,141
  points) 53 MB, so an ordinary field, including a constant one, decodes under the default.
- **OA-GRIB-054** A complex field that declares more groups than points is refused with a typed
  error before its tables are read.
- **OA-GRIB-055** A group width is the width reference plus a 32-bit table entry, added in 64 bits and
  refused as `BadPacking` above 32 bits; no header field is added to or shifted before it is checked.
  The descriptor that overflowed 32 bits (reference 32, delta 0xFFFFFFFF) is the committed fuzz seed
  `fuzz/seeds/grib2/width-overflow.grib2`.
- **OA-GRIB-056** A reference time (and the end of a statistical interval) must be on the Gregorian
  calendar: day within the month (leap years by the 4, 100, 400 rule), hour 0 to 23, minute 0 to 59,
  second 0 to 60; 2023-02-29, 1900-02-29, February 30 and 31 and April 31 are refused, 2000-02-29 and
  2024-02-29 are read.
- **OA-GRIB-057** Product templates 4.8 and 4.11 are decoded and their interval is kept: the end of the
  interval, the number of time ranges and the statistical process (`Field::interval`, `None` for
  templates 4.0 and 4.1). The interval starts at the reference time plus the forecast time.
- **OA-GRIB-058** `peek_info` reads the first field's description from the first bytes of a message
  (through section 5), so an inventory can be checked against a file without reading its data.
- **OA-GRIB-059** Section and message lengths that would wrap an offset are added with checked arithmetic: a damaged length at the
  top of the address space ends the scan or is refused with a typed error, never a panic.
- **OA-GRIB-060** Decoding is bounded by the input's size: one call of `decode_stream` (all its messages together)
  or `decode_message` may decode `work_budget(input bytes)` grid points, 4 Mi plus 64 per input byte. A field
  that does not fit is refused with `Grib2Error::WorkBudget` before any of it is unpacked, so a message that
  declares 16 Mi points in a few hundred bytes (a constant field, a flat codestream) costs nothing. The densest
  real input has 12.9 points per byte; reading the inventory (`inspect_stream`) is not limited.

## JPEG 2000 decoder (oa-j2k)

JPEG 2000 Part 1 codestream decoder for exactly the profile of NOAA's GRIB2 template 5.40, written
from ITU-T T.800 (08/2002) (`raw/standards/itu-t800`) with no outside code: a raw codestream with
one tile, one component, LRCP, the reversible 5-3 wavelet, no quantization and code-block style 0.
Everything else is refused with a typed error. Pure core. The real-data check against another
decoder is OA-GRIB-021.

- **OA-J2K-001** The MQ arithmetic decoder (T.800 Annex C) reproduces the 256-bit test sequence of
  the identical coder in JBIG2 (T.88 H.2) from its published encoding, and decodes what an
  independently written MQ encoder produces over all 19 contexts, with the initial context states
  of Table D.7. Past the end of a segment it is fed 1-bits and never fails.
- **OA-J2K-002** Tag trees (B.10.2) decode a hand-written bit string, and round-trip random trees
  from 1 x 1 to 9 x 9 leaves queried in random order at rising thresholds. Decoding is iterative
  with a bounded depth.
- **OA-J2K-003** Packet headers are read with the bit stuffing of B.10.1: after 0xFF the next byte
  carries seven bits and its top bit must be 0, and a header ending on 0xFF is followed by one
  stuffed byte.
- **OA-J2K-004** The inverse reversible 5-3 transform (Annex F) is integer exact: known vectors
  (including the floor of a negative sum), a single sample of either parity, every length 1 to 40
  at both parities, and 2-D reconstruction with odd sizes and odd image origins over up to 4
  levels, horizontal then vertical as F.3.2 requires.
- **OA-J2K-005** The EBCOT block decoder (Annex D) reproduces what an independently written block
  encoder codes, for all four sub-band orientations (zero-coding contexts of Table D.1), blocks of
  1 x 1 to 64 x 64 and sparse, dense and signed data; more coding passes than bit-planes allow are
  refused.
- **OA-J2K-006** Whole codestreams round-trip exactly: 1 x 1 to 451 x 33 images of 1 to 16 bits
  with 0 to 6 levels, code blocks of 4 x 4 to 1024 x 4, signed samples, odd image offsets,
  one to eleven quality layers, constant and extreme images.
- **OA-J2K-007** Two frozen byte-for-byte codestreams decode to their known samples, and the test
  encoder still produces the first of them.
- **OA-J2K-008** Everything outside the profile is refused with its own typed error: a missing SOC,
  the markers POC, PPM, PPT and RGN (informative TLM, PLM, PLT, CRG and COM are skipped), other
  progression orders, a multiple component transform, code-block styles other than 0, the 9-7
  wavelet, precincts, SOP and EPH markers, quantization other than none, subsampling, several
  components, tiles or tile-parts, bit depth above 16, a missing EOC or bytes after it, and a
  Psot beyond the stream.
- **OA-J2K-009** Packet damage is refused: a packet running past the data, bytes after the last
  packet, and exponents that leave a band no magnitude bit-planes.
- **OA-J2K-010** No input makes the decoder panic: every truncation of a codestream, four thousand
  randomly mutated ones and garbage after valid headers produce an image or a typed error (release
  builds check overflow as well, RULESET E3).
- **OA-J2K-011** Bounds are enforced before allocation: image sides (`MAX_DIM`), samples
  (`MAX_SAMPLES`), code blocks (`MAX_CODE_BLOCKS`), levels, layers and coding passes.
- **OA-J2K-012** COC and QCC override COD and QCD for component 0 in either order; a COC, QCC or
  second COD that names another component or repeats is refused.
- **OA-J2K-013** Tier-1 work (samples visited over all coding passes of all included blocks) is
  counted from the packet headers and refused with `WorkBudget` before any block is decoded when
  it exceeds `work_budget`: at most `3 * (bits + 8)` visits per sample and at most `WORK_FLOOR`
  plus `WORK_PER_BYTE` per byte of tile body, which every real RAP, HRRR and GEFS field passes with
  margin.
- **OA-J2K-050** Magnitude bit-planes are bounded by the SIZ precision at header parse (each
  quantization exponent is at most the sample bits plus the sub-band gain: 0 for LL, 1 for HL and
  LH, 2 for HH), and the inverse lifting uses checked arithmetic, so no coefficient can overflow
  32 bits: an inconsistent stream or an overflowing line is refused with a typed error.
- **OA-J2K-051** A code-block size or a resolution rectangle that would overflow `w * h`, the flag-array size or a row offset, or that
  runs backwards, is refused with a typed error; a coefficient array too short for a row is an error, not a silent stop.
- **OA-J2K-014** The codeword segments a stream could create (code blocks times layers, known from the
  headers) are capped at `MAX_SEGMENTS` (65,536) and the working tables at `MAX_SCRATCH_BYTES` (72 MiB),
  refused before any table is built. Largest real input over 6,068 template 5.40 messages: 58 blocks, 1
  layer, 58 segments, 151,987 samples, 57,485 body bytes. A 32768 x 512 claim with 8 x 8 blocks and 64
  layers is refused as `TooManySegments`.

## M5 — Minimum vectoring and IFR altitudes (oa-mva)

- **OA-MVA-001** Loads one FAA AIXM 5.1 chart message (MVA FUS3, MVA FUS5 or the center MIA
  "TAV") into struct-of-arrays sectors: facility and set taken from the message name, sector id,
  minimum altitude in feet MSL, and vertices stored as exact 1e-7 degrees.
- **OA-MVA-002** A sector holds its exterior ring first and its holes after it; rings are ranges
  of one shared vertex table (closing vertex kept as given) and a hole flag is a bitset.
- **OA-MVA-003** Anything outside the documented accepted structure is refused with a typed
  error naming the element (an upper limit, a curve segment, a second patch or time slice) or
  attribute, and a refused message leaves the model unchanged.
- **OA-MVA-004** A value outside what is accepted is refused naming the field: a unit other
  than FT, a limit reference other than MSL, a coordinate reference other than CRS84, non-planar
  interpolation, a time period that ends, an interpretation other than BASELINE, a sector name
  that is not 1 to 8 letters and digits, an altitude outside 0 to 18,000 ft, a longitude or
  latitude out of range, an odd or short coordinate list, three-dimensional coordinates, and a
  ring that does not close.
- **OA-MVA-005** Every count is bounded by a named constant: file bytes, sectors per chart,
  rings per sector, vertices per ring, sector extent, charts and vertices in the model; a
  duplicate sector in a chart and a duplicate chart are refused.
- **OA-MVA-006** Decimal degrees are converted to 1e-7 degrees with integer arithmetic only;
  more than 7 decimals, signs other than a leading minus, exponents and empty parts are refused.
- **OA-MVA-007** `min_altitude_at` returns the highest minimum altitude of the sectors of the
  requested set that contain the point, with how many contain it, and `None` where no sector
  does; sets are never mixed.
- **OA-MVA-008** Point containment is exact (128-bit integer arithmetic): a point on a sector's
  edge or vertex is inside it, a point strictly inside a hole is outside it, a point on a hole's
  edge is inside it, and a point one unit (1e-7 degree) off an edge is outside.
- **OA-MVA-009** `sectors_within_radius` lists every sector of the set whose polygon (edge or
  interior, holes included) comes within the radius, not only those with a vertex within it,
  using a distance metric that never exceeds the true WGS-84 distance.
- **OA-MVA-010** Queries refuse a latitude or longitude that is out of range or not finite and a
  radius that is not positive, finite and at most 200 nm; an empty model answers `None`.
- **OA-MVA-011** Ring integrity is counted and never repaired: open rings, proper crossings,
  touching edge pairs, zero-width spikes, repeated vertices, zero-area rings, clockwise
  exteriors, and holes outside their exterior.
- **OA-MVA-012** Coverage of a chart is rasterised and reports gaps (uncovered pockets inside
  the union of its sectors) and overlaps (cells claimed by two sectors) with areas, with sectors
  that share an edge neither overlapping nor leaving a gap.
- **OA-MVA-013** Sector labels are read from `pdftotext -raw` text and compared with the XML:
  matched, mismatched, missing from the PDF and extra on the PDF are reported separately.
- **OA-MVA-014** Terrain maxima take only the samples whose centres lie inside the sector
  polygon, count no-data samples separately, and the margin is the minimum altitude less the
  higher of the terrain and obstacle maxima; it is an upper bound when the sector has a nodata
  sample or a missing tile (OA-MVA-075).
- **OA-MVA-015** On the real FAA files (`OPENATCA_DATA/raw/mva`; the test prints the counts and
  skips when the data is absent) all 35 messages load with no refusal: 423 sectors, 604 rings,
  altitudes 2,000 to 4,300 ft, and queries at known places answer.
- **OA-MVA-016** On the real files every ring closes, no polygon crosses itself, and there are
  no spikes, zero-area rings or holes outside their exterior.
- **OA-MVA-017** On the real files no gap between the sectors of a chart is 10 raster cells
  (about 0.1 nm2) or larger, and no overlap is 2 nm2 or larger.
- **OA-MVA-018** On the real files every sector altitude that appears on its PDF plot equals the
  XML value, except the two charts recorded as findings (CVG FUS3 and TOL FUS5); the test skips
  when `pdftotext` is absent.
- **OA-MVA-019** On the real Rockford FUS3 chart (every chart with `OPENATCA_MVA_FULL`) the
  3DEP terrain and DOF obstacle maxima are found for every sector and every margin is positive;
  the test skips when the terrain or DOF files are absent.
- **OA-MVA-020** `oa-mva-check` prints the load, ring, coverage, PDF and clearance sections,
  exits 0 when every file loads, 1 when a file is refused (naming it), and 2 on bad arguments.
- **OA-MVA-021** A message named `<FAC>_<SUB>_MVA[_<year>][_v<n>]` (`SAV_LHW_MVA_2022`) loads as
  a sub-area chart of facility FAC with set kind `Sub` and the sub-area kept per sector and per
  chart; the same facility and sub-area twice is a duplicate, another sub-area is not.
- **OA-MVA-022** A message name that is none of the accepted forms (`HLN_MVA_MULTI`,
  `ZLC_GTF_MVA_FUS5`, a malformed edition label, ...) is refused with an error naming it, and
  the model is left unchanged.
- **OA-MVA-023** Sub-area charts are not part of a FUS3, FUS5 or TAV query; `min_altitude_with_sub`
  returns the higher of the set's answer and the facility's own sub-area charts at the point.
- **OA-MVA-024** On the real `raw/mva-zjx` files (skipped when absent) all 20 messages load,
  including `SAV_LHW_MVA` as a sub-area chart with 17 sectors.
- **OA-MVA-074** The meridian at 180 degrees east and west is one place: containment, sector
  distance and radius queries give the same answer for a point at +180 and at -180, and a sector
  on either side of the seam is found from either alias (a sector whose own extent crosses the
  seam is refused at load by the extent limit).
- **OA-MVA-075** A sector whose terrain scan met any nodata sample or missing tile is incomplete
  (`terrain_complete` is false): its margin is kept as an upper bound, it is left out of the
  complete-terrain distribution, and `oa-mva-check` lists each such sector with its nodata count.
- **OA-MVA-076** A non-finite terrain sample is counted as nodata in the sector scan, never as a
  known elevation; a sector with only such samples has no terrain maximum.
- **OA-MVA-077** Coverage cell areas and overlap extents use the WGS-84 meridian and prime-vertical radii at the
  latitude, within 1e-4 of the exact authalic zone area for a 0.1 degree cell at 45 N.
- **OA-MVA-078** An attribute is matched by namespace and local name: `uom`, `srsName`, `srsDimension`,
  `interpolation` and `indeterminatePosition` are the unprefixed attributes, `id` is `gml:id`, `type` is
  `xlink:type` and `nil` is `xsi:nil`. Any other qualified attribute, such as `x:uom` beside `uom`, and
  any repeat of a qualified name, is refused in either order.
- **OA-MVA-079** A chart whose sectors span more than 180 degrees of longitude (one either side of the
  antimeridian) is refused by the coverage check with an error naming the antimeridian; the extent is
  computed without 32-bit overflow.
- **OA-MVA-080** Every unordered pair of sectors that share a cell is reported as an overlap, however many
  sectors cover the cell (three identical polygons give three pairs of equal area in any order); a cell
  with more than 16 owners, or more than 8 million overlap records, refuses the chart.
- **OA-MVA-081** Rasterising stops as soon as the overlap-record cap is reached (the remaining sectors are
  not painted), and a chart is also refused once its cell visits (one per cell of each sector, so
  overlap repeats count, one per row of a sector's row table and one per row an edge crosses) pass
  `MAX_CELL_VISITS` (40 million, about 6 times the largest real chart's 6.99 million), so the cost of a refusal, and of an accepted chart, is bounded by that budget.
- **OA-MVA-082** Namespace URIs are compared in their expanded form (read with `oa_xml::ns_uri_bytes`), so an
  AIXM, GML, XLink or XSI namespace written with character references loads like the plain one.
- **OA-MVA-083** The ring checks need a number of edge pairs known from the ring sizes (every pair of
  edges of a sector); a sector over `MAX_SECTOR_EDGE_PAIRS`, or a model over `MAX_MODEL_EDGE_PAIRS`,
  is refused with an error before any pair is tested. The real charts are under a quarter of both.
- **OA-MVA-084** The terrain and obstacle scan refuses, with an error, a sector that is not in the model,
  obstacle columns of different lengths, a terrain grid without a valid lattice or shorter than its
  dimensions, and work over `MAX_CLEARANCE_WORK` (rows, edge rows, cells, pair tests, counted before
  they are built); a NaN percentile is NaN.
- **OA-MVA-085** The overlaps of a chart come out in one fixed order: largest first, then by sector name, then
  (two pairs that tie on both) by the sector pair as the model orders the sectors, and the pairs are collected in
  an ordered map, never a hash map, so the report is the same on every run and whatever order the pairs were
  found in. Tested with two pairs that tie on cells and names, collected in every order.

## M2 — Weather inputs

### Aviation Weather Center data (oa-awc)

Built on the captured files in `raw/awc` (18 hourly METAR and PIREP files, snapshots of TAF, SIGMET,
US SIGMET and G-AIRMET); fields and null patterns are recorded in that directory's `SOURCE.md`.

- **OA-AWC-001** Timestamps are strict ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`, optional fraction of one
  to three digits, years 2000 to 2099); other forms, impossible dates and `:60` are refused.
  Station and aircraft identifiers are 3 to 4 (aircraft type 2 to 4) upper-case letters or digits.
- **OA-AWC-002** A METAR row carries station, observation time, position, altimeter (tenths of hPa),
  temperature and dew point (tenths of a degree), wind (`VRB` coded), gust, visibility in hundredths
  of a mile with the "or more" flag for `"10+"` (strings and numbers both read), flight category and
  the raw text; absent and null fields give the `NONE` sentinels.
- **OA-AWC-003** The altimeter group is read from `rawOb` (`A3014` as inHg times 100, `Q1021` as hPa,
  nothing after `RMK`) and compared with `altim`; compared, disagreeing (over 0.4 hPa), missing and
  no-group records are counted, and a disagreeing row keeps its fields but has its altimeter withheld.
- **OA-AWC-004** Duplicate METARs (same station, observation time and type, as in overlapping hourly
  files) are removed by `finalize_metar`, keeping the greatest receipt time then text; duplicates and
  keys whose copies differ are counted, and the table is identical whatever the order of the files.
- **OA-AWC-005** `altimeter_at(table, station, t, max_age)` returns the latest trusted altimeter at or
  before `t` with its age, never a later one, skipping withheld altimeters, and refuses with a typed
  error an unknown station, no observation yet, no trusted altimeter, or an age over `max_age`.
- **OA-AWC-006** `indicated_altitude_ft` converts pressure altitude and a setting in hPa to indicated
  altitude with the ISA relation (identity at 1013.25 hPa; about 270 ft at 1023.1 hPa), and returns
  nothing for a setting or altitude outside its bounds.
- **OA-AWC-007** A METAR record that is not an object, lacks a required field, has a bad identifier,
  time, type or code, or a value out of its bound is refused alone, appends nothing and is counted by
  reason; the other records of the file are kept.
- **OA-AWC-008** A file that is not valid JSON, not an array, or holds more records than the limit is
  refused whole with a typed error and leaves the table and counts unchanged; a file that would
  exceed the table row limit is refused before anything is appended.
- **OA-AWC-009** A PIREP row carries time, position, flight level (hundreds of feet; none for during
  descent or climb, OA-AWC-021), aircraft type, temperature, wind, and two icing and two turbulence layers with
  coded intensity, type and frequency; a reversed base and top, an unknown code or a flight level over
  600 is refused and counted; duplicates are removed and counted.
- **OA-AWC-010** International SIGMET records give a polygon (one ring, or several for `AREAS`) in
  degrees, base and top in feet with absent ends open, and validity times; a ring of under three
  points (other than the one-coordinate `AREA`, OA-AWC-018), an unknown geometry or a base above
  the top is refused and counted.
- **OA-AWC-011** US SIGMET/AIRMET records take the base as the smallest `altitudeLow*` and the top as
  the largest `altitudeHi*` given, and the creation time as the issue time.
- **OA-AWC-012** G-AIRMET records read text coordinates and hundreds-of-feet altitude text into feet,
  `SFC` as 0, a base of `FZL` as a flag, the freezing-level band and level, validity from `validTime`
  to `expireTime`, and refuse a bad altitude text or a base above the top.
- **OA-AWC-013** Duplicate hazard areas across snapshots are removed and counted independently of file
  order; `active_hazards_at` selects the rows valid at a time; `hazard_covers` tests a point and an
  optional altitude against the rings (open bounds, antimeridian longitudes, and a line covers nothing).
- **OA-AWC-014** TAF records give a header (station, issue, validity, position) and periods (change
  group, probability, wind, gust, visibility, lowest broken or overcast ceiling); a period with an
  end before its start refuses the whole forecast; duplicate issuances are removed and counted.
- **OA-AWC-015** No input panics: every prefix of a sample for each product and 400 seeded random byte
  mutations of each give tables or typed errors.
- **OA-AWC-016** On the real captured files (`OPENATCA_DATA/raw/awc`; skipped with a note when absent)
  every file parses, every METAR, PIREP and TAF record is accepted, every hazard record is accepted
  or refused for `reversed-band` only (any other refusal fails the test, and each refused record is
  printed), a point never disagrees with its `rawSigmet` position, `altim` and `rawOb` never disagree, de-duplication
  counts balance, and at least 90 % of the stations in the ZAU box have a trusted altimeter in at
  least 95 % of the hours; the test prints the counts.
- **OA-AWC-017** `oa-awc-summary [dir]` scans a directory tree, parses each recognised file by its
  name, reports records, refusals by reason, duplicates, the altimeter cross-check and station
  availability, lists files refused whole and skipped names, and exits 1 if any file was refused whole.
- **OA-AWC-018** An international SIGMET `AREA` with exactly one coordinate is accepted as a point
  hazard (`GEOM_POINT`, one vertex) that covers nothing; an `AREA` of two coordinates, and a
  one-coordinate area from any other product, is refused as `bad-geometry` (the AWC API schema
  documents no other meaning); `describe_hazard_refusals` lists each refused record of a file.
- **OA-AWC-019** A point's coordinate is compared with the position `rawSigmet` states after
  `OBS AT` or `FCST AT` (minutes or whole-degree form, optional time); checked and disagreeing
  points are counted, and a disagreement is never a refusal.
- **OA-AWC-020** `point_hazards_near` returns the point hazards within a caller-chosen great-circle
  distance of a position (and, optionally, whose altitude band contains an altitude), across the
  antimeridian; it returns nothing for a negative or non-finite distance and never returns areas.
- **OA-AWC-021** A PIREP with flight-level type `DURD` or `DURC` has no single flight level whatever
  `fltLvl` says: the level is stored as none and `pirep_alt_ft` gives none; its layers are kept.
- **OA-AWC-022** The scan reads each file with a length check and a capped read (`MAX_FILE_BYTES`, the
  `oa-json` input limit); a larger file is listed as refused whole without being read.
- **OA-AWC-023** `point_hazards_near` measures the WGS-84 geodesic distance (`oa-geo` inverse), not a spherical one.
- **OA-AWC-024** The ISO timestamp parser's calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days that
  do not exist in the month (29 February of a non-leap year, day 31 of a 30-day month); hour 24 is refused.

## M2 — Benchmark truth (oa-truth)

### ADS-B reference trajectories (oa-truth)

- **OA-TRUTH-001** Reports are selected on NACp >= 8 (and at most 11), NIC >= 7, SIL 3 and, where
  the version is present, ADS-B version >= 2; a row failing one of these, carrying readsb's stale
  flag, or with a source other than ADS-B (MLAT, TIS-B, ADS-R, ...) is removed and counted under
  one named reason, and rows in always equal rows kept plus rows removed.
- **OA-TRUTH-002** Quality fields are carried forward within an aircraft for at most 60 s; a row
  with no NACp, NIC or SIL value that young is not truth and is counted as such.
- **OA-TRUTH-003** A report that `oa_adsb::jump_verdict` calls impossible from the last kept one
  (hard cap of the emitter category, reported ground speed, time gap, plus a 200 m allowance) is
  removed and counted as a jump and does not affect the track; five jumps in a row start a new leg
  instead of rejecting the rest of the flight.
- **OA-TRUTH-004** An aircraft splits into legs at readsb's new-leg flag (also when the flagged
  row is itself removed) and at a gap of more than 120 s between kept reports, not at exactly
  120 s; leg start, end, report count and address are readable.
- **OA-TRUTH-005** On a straight track with known noise (5 m per axis, 10 minutes) the
  smoothed position is under 4 m RMS from the true path and the velocity under 1.5 m/s RMS, the
  velocity points the right way, and the reported 95 % radius is above the real error and under 50 m.
- **OA-TRUTH-006** On a constant-rate turn (8 km radius, 200 m/s, 5 m noise) the smoothed
  position is under 6 m RMS and 20 m worst, and the velocity within 5 m/s.
- **OA-TRUTH-007** A leg 400 km long stays within 4 m RMS of the true path: the local frame adds
  no position error, and velocity is true east and north.
- **OA-TRUTH-008** Barometric altitude is not smoothed: exact at a report time, the linear
  interpolation between the bracketing reports otherwise, absent if either lacks it; vertical
  rate is the reported rate interpolated, else the altitude difference over a bracket of 2 s or more.
- **OA-TRUTH-009** There is no extrapolation: no truth before a leg's first or after its last
  report or between legs (no bracket), none inside a leg unless a real report lies within 10 s on
  both sides (gap; exactly 10 s is allowed), none for an unknown address, and an address differing
  only in the non-ICAO bit is a different aircraft.
- **OA-TRUTH-010** Truth whose 95 % horizontal radius exceeds the gate (50 m) is refused as too
  uncertain; the same track with precise reports, or a looser gate, is answered.
- **OA-TRUTH-011** Leave-one-out hides every fifth report of each leg, predicts it from the rest
  under the bracket rule and publishes N, mean, RMS, median, p95, p99 and max of the horizontal,
  cross-track, along-track and altitude residuals, the share of predictions under the gate and the
  share of hidden reports inside their predicted 95 % radius; on known noise the residuals have
  the expected size and the radius is honest; the measurement can be switched off.
- **OA-TRUTH-012** The same input gives the same truth and statistics bit for bit, in any input
  order, and one changed report changes the SHA-256 digest of the model.
- **OA-TRUTH-013** Aircraft are keyed by the full address word, so non-ICAO addresses stay
  separate from ICAO ones with the same 24 bits.
- **OA-TRUTH-014** Columns of different lengths and out-of-range configuration values are refused
  with a typed error, out-of-range coordinates are removed and counted, empty input gives an empty
  truth, and the gates are the design's values (NACp 8, NIC 7, SIL 3, version 2, 60 s, 120 s,
  10 s, 50 m).
- **OA-TRUTH-015** On the real ZAU ADS-B clip (skipped when `OPENATCA_ADSB_CLIP` is unset) the
  accounting balances, the model's 95 % radius covers 85 to 99.5 % of hidden reports, the
  leave-one-out horizontal p95 is under the 50 m gate, nothing is answered before an aircraft's
  first report, and two builds are identical.
- **OA-TRUTH-016** `oa-truth-check <clip.oaf>` reads OAF1 ADS-B frames and prints the legs, rows
  kept and removed by reason, the leave-one-out table, the share of query times under the
  uncertainty gate, the build time and the model digest (equal to the library's); `--accel-q`,
  `--sigma-scale` (each up to 1e6) and `--no-loo` are honoured; usage errors and unreadable input exit 2.
- **OA-TRUTH-017** An aircraft flying 900 kt in a straight line and reporting 900 kt gives one
  continuous leg with no report removed as a jump, at 1 s and at 3 s between reports.
- **OA-TRUTH-018** A position jump of 2,782 kt in a 450 kt track is removed and counted, whether or
  not the reports carry a ground speed.
- **OA-TRUTH-019** Reports of the reserved addresses 0x000000 and 0xFFFFFF are removed as invalid
  and give no aircraft or leg; the same digits with the non-ICAO mark are kept.
- **OA-TRUTH-021** The quality values carried forward within an aircraft come only from rows that
  are in range, not stale and from an ADS-B source: a row from another source does not vouch for the
  ADS-B rows after it.
- **OA-TRUTH-022** Rows of one aircraft at one instant do not make the truth depend on input order. Two
  eligible reports (see OA-TRUTH-024) conflict when their squared horizontal distance exceeds 13.82
  (the 2-degree-of-freedom chi-square value at 99.9 %) times the sum of their squared per-axis
  sigmas, each the NACp 95 % radius over 2.4477; errors are assumed independent. A conflicting
  group is removed whole (counted as duplicate times); the rest are tried in the order of their
  content, so the same reports in any input order give the same truth and statistics. The 50 m
  limit gates the reference's output uncertainty only (OA-TRUTH-010), not this pair rule.
- **OA-TRUTH-020** The emitter category that sets the jump cap is taken only from reports that pass
  the quality rules and are kept, and a cap-raising one must persist (`oa_adsb::category_observe`):
  one report printing B7 or a reserved code does not let a 2,782 kt jump into truth, and a category
  printed by every report does raise the cap after three.
- **OA-TRUTH-023** `oa-truth-check` on a clip that ends inside a frame (a partial header or a partial body)
  keeps the valid frames and counts the partial frame in the frames it prints as refused.
- **OA-TRUTH-024** Whether a same-time row takes part in the conflict check is decided by the full
  selection (coordinates, NACp, NIC, SIL, version, stale flag, source) against the carried quality as
  it was before the instant; an ineligible row is removed for its own reason and cannot veto a
  usable one.
- **OA-TRUTH-025** The result for same-time rows does not depend on their order in the input,
  including when one of them is ineligible.
- **OA-TRUTH-026** Same-time rows identical in position and NACp are collapsed before the conflict
  check, so 1,000 identical rows cost no pair evaluations beyond the rows themselves.
- **OA-TRUTH-027** More than 16 distinct eligible same-time rows of one aircraft are refused and
  counted under `oversized_same_time_group`; 16 are tested pairwise.
- **OA-TRUTH-028** Two NACp 8 reports 70 m apart at one instant (each inside its 92.6 m 95 % radius)
  are kept as one position measured twice.
- **OA-TRUTH-029** Two same-time reports far beyond the bound of OA-TRUTH-022 are quarantined.
- **OA-TRUTH-030** A mixed-NACp same-time pair is judged at its own bound: NACp 8 with NACp 10 is
  kept at 139 m and quarantined at 144 m (bound 141.5 m).
- **OA-TRUTH-031** A row removed as a jump does not change the carried quality: a following report
  that prints none gets the same selection and weight with or without the jump row.
- **OA-TRUTH-032** A low-quality row that is also a jump does not remove later usable reports; a
  low-quality row on the track still does.
- **OA-TRUTH-033** A `sigma_scale` or `accel_q` above 1e6 is refused at configuration (`BadConfig`): the
  covariance squares them and would overflow to infinity and NaN.
- **OA-TRUTH-034** A variance that is NaN or infinite has no horizontal radius: the query refuses it as too
  uncertain, the uncertainty statistics count it as a miss, and a leave-one-out prediction with a
  non-finite state or variance counts as not predicted. It is never zero uncertainty.
- **OA-TRUTH-041** A query whose converted latitude, longitude or velocity is not finite (a state past the
  frame's chart) is refused as too uncertain, never returned as a sample.
- **OA-TRUTH-042** A `--accel-q` or `--sigma-scale` value of `oa-truth-check` that is not a finite number
  above zero is a usage error, before the clip is read.
- **OA-TRUTH-043** A difference of two report times that does not fit an `i64` never panics: the builder
  splits the leg there, queries refuse it as no nearby report, and interpolation and the leave-one-out
  bracket treat it as no interval.
- **OA-TRUTH-044** A time difference is never saturated: where it can exceed an `i64` (the quality carry
  age, the query bracket, interpolation) the overflow reads as "not within", and the steps of one leg the
  smoother subtracts without a check never exceed `leg_gap_ms`.
- **OA-TRUTH-045** `oa-truth-check` refuses a clip over 4 GiB before reading it, and a clip whose frame
  headers claim more than 50 million rows before decoding the frame that crosses the cap (typed
  `ClipError`). Ten times the largest real day (330 MB, 4.07 million rows). `oa-sensorsim` reads clips
  through the same reader (`oa_truth::clip::read_clip`), so it has the same caps.
- **OA-TRUTH-046** Proved (Kani, `clip_row_count_refuses_exactly_when_the_total_crosses_the_cap`,
  `clip_claim_rows_is_total`): the row count the clip reader accumulates from frame headers cannot wrap, and
  it refuses at the first frame whose exact running total is over the cap, and at no other.
- **OA-TRUTH-047** Of several accepted rows of one aircraft at one instant the builder keeps the one that
  carries the most fields, in the preference order barometric altitude, geometric altitude, vertical rate,
  ground speed, track (`preference` in `build.rs`; a row with a valid value earlier in the list wins), then
  by content. A row whose altitude is absent never wins over one whose altitude is valid, whatever the input
  order.
- **OA-TRUTH-048** Eligible rows of one instant with two valid barometric (or two valid geometric) altitudes
  more than `ALT_QUANTUM_FT` (25 ft) apart make the instant ambiguous: all its eligible rows are dropped,
  counted as `ambiguous_same_time_altitude`, and the instant is not truth; the pick is never arbitrary and
  does not depend on input order. Rows whose positions differ beyond their stated NACp are dropped and
  counted as `duplicate_time`.
- **OA-TRUTH-049** A state past the chart of the leg's frame (the antipode of its origin) is refused as too
  uncertain; no NaN velocity or position is returned as truth. The velocity conversion uses
  `oa_geo::aeqd::try_frame_velocity` and handles `None`.
- **OA-TRUTH-050** Every limit of `TruthConfig` is exact: NACp gate 1 to 11, NIC at most 11, SIL at most 3, version
  at most 7, carry time not negative and the other time gates positive and at most one day, floating settings
  finite and positive (the covariance ones capped), jump allowance zero or more, `jump_resync` at least 1,
  `min_leg_points` at least 3, `loo_stride` at least 2; each is accepted at the limit and refused one step beyond.
  `MAX_REPORTS` is 200 Mi reports, exactly that many are accepted and one more refused, and every `TruthError`
  has its message text.
- **OA-TRUTH-051** Rows of one instant: the kept row is the first in the order barometric altitude, geometric
  altitude, vertical rate, ground speed, track, whichever input order; geometric as well as barometric altitudes
  more than 25 ft apart make the instant ambiguous (25 ft is not), an absent one does not; every pair of distinct
  eligible positions is compared once and no pair twice.
- **OA-TRUTH-052** A report that fails on quality still sets the quality values the reports after it carry (a
  NACp of 7 is not forgotten, so the following reports without a NACp fail with it); one of several rows of an
  instant that fails does not.
- **OA-TRUTH-053** A leg whose reports pass 3,000 km (0.4709 rad of arc) from its first report is split there and
  counted under `frame_range`, at the first report past the limit and not before.
- **OA-TRUTH-054** The measurement sigma of a report is its NACp 95 % radius over 2.4477 times `sigma_scale`
  (checked on the forward covariance of the first report against the Kalman update); the shares of report times
  and interval midpoints under the gate count intervals up to and including twice the bracket (20 s), compare the
  radius at the midpoint (not at the earlier report), and are exact against the Kalman/RTS covariance.
- **OA-TRUTH-055** Queries: a radius exactly at the gate is truth, a velocity well past the speed envelope is
  not; the vertical rate is the interpolated reported rates when both are present, else the altitude difference
  over a bracket of at least 2 s, else absent; a leg's first whole millisecond rounds up and its last rounds down
  (before 1970 too); a leg with no reports has no start, end or count.
- **OA-TRUTH-056** Each quality gate passes at its limit and fails one step below (NACp 8 to 11, NIC 7, SIL 3,
  version 2); a row missing any one of NACp, NIC or SIL has no quality, a missing version is allowed; the NACp and
  NACv tables are the standard's values.
- **OA-TRUTH-057** Leave-one-out: positions 2, 7, 12 and so on of a leg are hidden; a leg with fewer than three
  reports left, a hidden report with no remaining report on a side, one more than 10 s away on either side, or a
  prediction with a non-finite state or variance is counted as not predicted and adds no residual; horizontal,
  along-track and cross-track residuals, the coverage test (residual against 2.4477 times the root of the predicted
  plus the report variance), the gate share and the altitude residual (time-weighted neighbours, all three
  present) equal values worked out from the filter equations; distributions are the signed mean, the RMS and the
  nearest-rank median, p95, p99 and maximum.
- **OA-TRUTH-058** The smoother's forward and smoothed states and full covariances, and the interpolated state and
  covariance between and at reports, equal the Kalman and Rauch-Tung-Striebel equations evaluated independently;
  the process noise is the white-jerk matrix element by element; the 3 x 3 inverse is exact and a singular or
  non-finite matrix has none.
- **OA-TRUTH-059** `oa-truth-check` prints each removal reason with its count, the removal total beside the kept
  and input counts, the shares as `percent (hit of n)` (`n/a` with no sample), the records dropped from decoded
  frames when there are any and only then, and refuses an unknown option, a missing value and a second path with
  the usage text.
- **OA-TRUTH-060** The clip reader's caps are 4 GiB and 50 million rows; a row total exactly at the cap is accepted
  and one past it refused; after the cap is crossed later frames change nothing; frames that are not messages are
  ignored; dropped records, undecodable payloads and a cut frame are each counted once; `ClipError` has its
  message texts.
- **OA-TRUTH-061** The truth digest hashes a float's IEEE bits through `oa_math::canon_nan`: two truths that differ
  only in the sign or payload of a NaN in any float column or setting have the same digest, and a truth that
  differs in a finite value does not.


## M2 — Benchmark harness (oa-bench)

Built on `docs/M2_DESIGN.md` sections 2 to 6. Truth comes through a plain query and leg list (`Truth`),
so `oa-truth` is wired in later without changing the metric code; until then the binary runs the
synthetic world only. Tests use hand-made fixtures; the real-sample test reads
`OPENATCA_DATA/scds/{sfdps,tbfm,tfms}-20261114T*` and skips when absent.

- **OA-BENCH-001** Every statistic reports N, the signed mean, the RMS, and the median, 95th and 99th
  percentile and maximum of the absolute values, by the nearest-rank method (rank `ceil(p*n/100)`, 1-based,
  no interpolation); the result does not depend on the order of the values, non-finite values are skipped,
  and an empty set gives N = 0.
- **OA-BENCH-002** Every paired-sample metric is split by flight phase (climb, cruise, descent, unknown
  from the truth vertical rate, 500 fpm), altitude band (breaks at 10,000, 18,000, 29,000 and 41,000 ft),
  coast flag and association method; each split partitions the samples.
- **OA-BENCH-003** Truth is consumed as `Truth { query(addr, t_ms) -> Result<TruthSample, NoTruth>, legs }`
  with `TruthSample` (lat, lon, baro altitude, east and north velocity, vertical rate, 95% horizontal
  uncertainty, leg) and `NoTruth` (unknown aircraft, no bracket, too uncertain, gap); a time with no truth
  is counted in the exclusions table by reason and never filled in, and a sample whose uncertainty exceeds
  the gate (50 m) is excluded as too uncertain.
- **OA-BENCH-004** SFDPS extraction reads ERAM track samples (flight key, aircraft identification,
  position time, receive time, position, reported altitude, track velocity, coast flag), the aircraft
  address and the address hint, handoff events with their event kind and both units, and expanded-route
  points (fix, time, coordinates where given) into struct-of-arrays tables, using the `oa-fixm` decoder.
- **OA-BENCH-005** TBFM extraction keeps ETA at the meter fix and runway and STA at the scheduling fix and
  runway with the meter fix, flight plan identifier and issue time; TFMS extraction keeps estimated
  arrival and departure times with the flight plan identifier, airports and issue time.
- **OA-BENCH-006** Every table has a stated capacity; a row past it is dropped and counted by table, and a
  document the parser or decoder refuses is counted by feed and not ingested.
- **OA-BENCH-007** Association takes `aircraftAddress` first, the address hint second and the callsign with
  the gate last; ambiguous flights (two aircraft pass the gate) are not associated and are counted; every
  flight has one outcome, the outcomes are counted, and the samples of unassociated flights are excluded
  by reason.
- **OA-BENCH-008** The callsign check compares the ERAM position with the truth: a flight with at least
  80% of the shared times within 3 NM horizontally and 1,000 ft vertically is associated as method
  callsign; one that fails the check is still associated, under its own method, so it never leaves the
  results; the address needs no such check; the method is stored per pair and every metric can be split
  by it.
- **OA-BENCH-009** Cross-track error (T1, positive to the right of the truth track) recovers an injected
  bias and noise; along-track error (T2) is reported raw and after fitting one constant time offset by
  least squares (`along = tau * speed`), the offset is published, and the corrected error recovers the
  injected noise.
- **OA-BENCH-010** The fitted offset drifts when a time bin (or, after trimming 5% either side, an
  aircraft group) of at least 30 samples differs from the overall offset by more than 1 s; a drifting
  offset withholds the corrected along-track numbers with the reason while the offset and the raw numbers
  stay published; a large steady offset is not drift.
- **OA-BENCH-011** Speed error (T3), heading error (T3, truth speed above 30 kt) and altitude error (T4)
  recover injected biases.
- **OA-BENCH-012** Publication interval (T5, consecutive position times of one flight, repeats and
  intervals over 600 s counted apart) and staleness (T6, receive time minus position time, withheld with
  the reason when the recording has no receive time) are measured from the stream.
- **OA-BENCH-013** Presence (T7: share of truth flight time in the zone with an ERAM position within
  30 s, legs with no associated flight counting as absent) and initiation delay (T8: first ERAM position
  minus zone entry or takeoff, signed) match the construction.
- **OA-BENCH-014** Association errors (T9: the ERAM position closer to another aircraft's truth than to its
  own for 3 or more consecutive tested samples) find the deliberate track swap, list it with both
  aircraft, and report none where there is none.
- **OA-BENCH-015** Meter-fix ETA error (F1) and scheduled-time error (F2) against the truth crossing of
  the fix (closest approach within 2 NM, not at the end of a truth leg) are reported by look-ahead in
  5-minute bins to 60 minutes; a fix never approached and an estimate issued after the crossing are
  excluded and counted.
- **OA-BENCH-016** TFMS arrival-estimate error against the truth landing (last truth point on the ground
  within 1 NM of a destination threshold) and departure-estimate error against the truth takeoff are
  reported by look-ahead. On the ground is judged in feet above mean sea level (OA-BENCH-110).
- **OA-BENCH-017** Route conformance (R1) reports the truth distance from ERAM's expanded route in force at
  each sample, the share of flight time beyond 2 NM, and lists each flight with time beyond it (bounded,
  the rest counted).
- **OA-BENCH-018** Handoff timing (H1) compares the initiation and acceptance event times with the truth
  crossing of the boundary between the transferring and receiving unit, signed, by event. A crossing time
  is taken only where the truth is continuous across it (OA-BENCH-100).
- **OA-BENCH-019** Samples and estimates that cannot be scored are counted in an exclusions table keyed by
  scope and reason (no truth by kind, not associated by outcome, no meter fix, never within 2 NM, no
  route, and others); nothing is dropped silently.
- **OA-BENCH-020** The report carries the input file hashes, the code version, the replay state hash, the
  extraction counts, the scope statement (published track, not internal), the statistics definitions, every
  metric with N and its withheld reason if any, and the exclusions table, in plain text and CSV.
- **OA-BENCH-021** The same inputs give byte-identical text and CSV reports.
- **OA-BENCH-022** A directory of raw `.msg` files (name order, no receive time, no replay hash) or of
  `.oaf` recordings (receive times, replay pipeline state hash, identical on repeat) loads; the input hash
  of one file is its SHA-256 and of several is the SHA-256 of the `name:sha256` lines.
- **OA-BENCH-023** The `oa-bench` binary refuses `--adsb` without `--out` and a non-positive truth gate
  override (exit 2), runs the synthetic world with `--synthetic --out <dir>` writing `report.txt` and `report.csv`
  with every self-test check passing and identical bytes on a repeat, and prints extraction counts with
  `--extract-only`.
- **OA-BENCH-024** On the real 2026-11-14 sample (skipped when absent) extraction yields the expected
  counts: 13,268 ERAM track samples, 2,807 handoff events, 265 expanded routes with 1,397 points, 165
  flights with `aircraftAddress`, no capacity drops, and no refused TBFM or TFMS document.
- **OA-BENCH-025** The synthetic self-test (closed-form truth; injected bias, noise, time offset,
  decimation, receive delay, coasting, dropout, start delay, estimate errors, handoff times, an off-route
  stretch and a track swap) is recovered by every metric within its stated tolerance (26 checks, the last two on the callsign-only aircraft with a 20 km error that stays in the headline).
- **OA-BENCH-026** A truth built from a plain lookup table and the same legs gives the same paired samples
  and metrics as the closed-form truth, so the interface needs nothing but the query and the legs.
- **OA-BENCH-027** The address hint associates a flight that has no aircraftAddress, is stored as its own
  method, and a hint with no truth aircraft is not retried by callsign.
- **OA-BENCH-028** `oa-truth` query answers map onto the benchmark's truth interface reason for reason:
  unknown aircraft, outside legs, no nearby report and too uncertain each map to the same-named
  benchmark reason with the same meaning; a sample keeps its position, velocity,
  uncertainty and leg, with altitude and vertical rate rounded to whole units.
- **OA-BENCH-029** Truth legs are `oa-truth`'s legs in index order with their first and last whole
  millisecond; the leg callsign is the most frequent callsign among the decoded reports of the address
  inside the leg's span, and `None` when no report carries one.
- **OA-BENCH-030** The truth gates come from `oa-truth`'s configuration (design: 10 s bracket, 50 m); the
  bracket and uncertainty overrides change it, and the report's truth section carries the gates, the
  statistics, the removals by reason, the leave-one-out table and the truth digest.
- **OA-BENCH-031** An OAF1 ADS-B clip is read through `oa-feed` and `oa-adsb` into reports, counting
  frames that did not decode, and its SHA-256 is the input hash.
- **OA-BENCH-032** The airspace lookups come from the M0 model: fixes and navaids by identifier (not
  airports), airports by ICAO and FAA identifier with elevation and thresholds, the model's ARTCC as the
  zone, and the containing ARTCC as the unit where exactly one contains the point (OA-BENCH-111).
- **OA-BENCH-033** `oa-bench --adsb <clip.oaf>` runs end to end: on a hand-made clip and SFDPS fixture
  whose track is 500 m left of the truth, T1 recovers -500 m (cross-track, positive right) within 2 m with
  every sample paired; the report holds the truth section, the clip hash and the gate values, a repeat gives
  identical bytes, and an override of the gates is recorded in the header with its reason.
- **OA-BENCH-034** On the real ADS-B clip and the SCDS sample (skipped when `OPENATCA_DATA` or
  `OPENATCA_ADSB_CLIP` is unset), whose times do not overlap, the run completes with no paired samples and
  an exclusions table.
- **OA-BENCH-035** Association never depends on the error being measured. A callsign flight is kept when
  truth overlaps it in time (at least 5 shared times) and lies within 50 NM of its positions for 80% of
  them; a flight that fails the 3 NM / 1,000 ft check is associated all the same under its own method
  (callsign, failed the check), counted in the headline rows, split out by the method split, and listed
  as a case. Two or more callsign candidates are ambiguous.
- **OA-BENCH-036** An address or address-hint association is checked without using ERAM's accuracy:
  callsigns that contradict (both known, none of the truth legs agrees) or ERAM positions beyond the 50 NM
  region for more than 20% of the shared times make it implausible; with too few shared times it is
  trusted and counted as unchecked. An implausible flight is not in the primary rows and its cross-track,
  raw along-track and altitude errors are published in separate flagged tables, each flight listed with
  its reason; an address with a believable error is trusted as before.
- **OA-BENCH-037** The statistics of an empty set have n = 0 and NaN in every other field, never 0.0; the
  text report prints such a row as "no data" and the CSV as `NA`; scalars of an empty set (presence, share
  of time beyond the route limit, largest per-flight share, offset deviations with nothing tested) are NaN
  and print the same way; no report contains the text `NaN`.
- **OA-BENCH-038** Raw along-track error is the primary along-track result and the offset-corrected one
  is secondary, and says that the fitted offset cannot be told apart from real ERAM lag. The offset is
  published with its sample standard error, its between-aircraft standard error and a 95% half-width
  (1.96 times the larger); the drift test keeps its 5% trim and also publishes the untrimmed largest
  group deviation and the five worst aircraft groups.
- **OA-BENCH-039** A tracker's output columns are scored by the same code path as ERAM's samples: the
  tracker entry point is the legacy-table adapter followed by the one benchmark function, gives a
  report identical to calling the two directly, and yields the same metrics, in the same order, with the
  same offset scalars and notes as an ERAM run; a tracker that lags the truth shows the lag in the raw
  along-track error.
- **OA-BENCH-040** A tracker's output scored in address ranges gives the report of the same rows scored
  in one piece, byte for byte in the text and the CSV, whatever order the flights are in the table and
  whatever order the ranges are added in: the case lists (association, T9) are sorted by severity, truth
  address and time before they are cut to 200, and every sum behind a statistic (means, RMS, the along-track
  offset fit) is exactly rounded, so the order of the values cannot change a bit. T5 is summed from whole
  milliseconds in both paths.
- **OA-BENCH-041** Scoring in address ranges refuses ranges that overlap, ranges that together leave any
  key from 0 to 0xFFFFFFFF uncovered (the radar-only tracks keyed from 0x40000000 included), and a chunk with
  a flight keyed or associated outside its range; no flight or truth leg is counted twice or dropped.
- **OA-BENCH-042** The headline tables T1, T2-raw and T4 carry the number of flagged flights and of their paired
  samples that they leave out (`flagged_flights_excluded`, `flagged_points_excluded`) and a last row, "all
  associated including flagged", over the headline and the flagged samples together; the other rows are
  unchanged, and the run in ranges gives the same.
- **OA-BENCH-043** A chunk scored in address ranges is accepted only when its plan-data results (F1 to F3, R1,
  H1) equal exactly what a table with no data gives, NaN included; a result that differs only in a NaN is
  refused, not read as empty.
- **OA-BENCH-044** The association of a run in address ranges is the real one, one entry per flight with the
  chunks' associations in range order, and its counts equal those of the run in one piece; it is never an
  empty stand-in.
- **OA-BENCH-045** Reading an ADS-B clip drops only a record that cannot be read (position or flags out of
  range), counts it apart from refused frames, and keeps the other records of its frame.
- **OA-BENCH-050** A route is never joined across a point that cannot be placed: an expanded route with an
  unplaced point between two others is excluded and counted (a dogleg whose middle fix is unknown is not
  scored as the straight line, whatever the truth does), while unplaced points at either end only shorten it.
- **OA-BENCH-051** A fix identifier that names more than one place is never resolved by row order: the airspace
  lookup lists every distinct place (rows at one place are one place), answers a single place only when
  there is one, and counts the ambiguous identifiers; a route chooses the place clearly nearest to the
  previous placed point and otherwise is excluded and counted; a meter fix is the one place inside the zone
  or the estimate is excluded and counted.
- **OA-BENCH-052** Meter-fix crossings, takeoffs and landings are taken from the one truth leg that overlaps the
  flight plan's active interval the most (the 60 minutes after issue when the plan has no ERAM sample) and
  cached per leg (a meter fix per passage, OA-BENCH-071): one aircraft flying two trips with a perfect estimate for each scores zero error on both.
- **OA-BENCH-053** A truth leg that begins or ends near a runway threshold with no barometric altitude is not a
  takeoff or landing: the estimate is excluded and counted with that reason.
- **OA-BENCH-054** The common-time comparison scores two systems at the same truth times: two systems with the
  same error at every instant but different publication rates have different native RMS and equal
  common-time RMS, a grid time counts only when both have a paired sample within the tolerance, and the
  grid times left out are counted (duplicate tracks: OA-BENCH-070).
- **OA-BENCH-055** The legacy extraction accepts exactly the documents the replay pipeline accepts: both build
  their parser and decoders from the replay's one definition of the limits (`doc_xml_limits`, `doc_fixm_limits`,
  the table sizes) and both decode as far as the tables fit, so a 2 MiB SFDPS document with ignored text, and
  SFDPS, TBFM and TFMS documents with more rows than a table holds, give the same accepted row counts in both,
  not a whole refused document in one and a kept prefix in the other. The rows past the table are counted
  (`rows_not_decoded`) and the report shows the documents the extraction refused over all feeds.
- **OA-BENCH-056** A flight plan identifier that more than one flight used never joins an estimate to the first
  flight: the plan index keeps every flight of an identifier (bounded; flights past the bound are counted), and
  an estimate resolves to the one flight whose active interval (the interval of OA-BENCH-052) contains its issue
  time and whose callsign does not contradict the aircraft identification the estimate carries. No such flight,
  or more than one, excludes the estimate with a named reason. The result does not depend on the order the
  flights were ingested in.
- **OA-BENCH-057** An ADS-B clip that ends inside a frame (a partial header or a partial body) keeps its valid
  frames and counts the partial frame with the frames it refused.
- **OA-BENCH-058** The digest of an input file is computed from the bytes that are ingested, in the same read:
  a file that changes between listing and reading, or while it is read, gives the digest of the bytes scored,
  never of another copy. `.msg` files are hashed as they are read, `.oaf` and `.oaf.gz` recordings as the replay
  merge reads them and the ADS-B clip as its frame loop reads it; a file whose read does not reach its end is an
  error, not a digest. The report notes this for the feed inputs and for the clip. A `.msg` file past the size
  limit is refused (OA-BENCH-092).
- **OA-BENCH-059** An estimate whose plan identifier names one recorded flight joins it unless both carry an
  aircraft identification and the two differ (the identifier was reused by a flight outside the recording): then
  it is excluded with a named reason. There is no time check for a lone flight, so an estimate issued before the
  flight's first ERAM sample still joins.
- **OA-BENCH-060** An estimate whose plan identifier was used by more flights than the plan index keeps is
  excluded with a named reason, never resolved against the flights kept; the overflow count is in the report.
- **OA-BENCH-061** Among several flights of one plan identifier, an estimate goes to the one flight whose callsign
  equals its aircraft identification (OA-BENCH-090) when exactly one does, even if its issue time is outside that flight's
  active interval (an estimate issued before the flight's first ERAM sample is not handed to the neighbouring
  flight or dropped); otherwise the active interval of OA-BENCH-052, with its 60-minute rule for a flight with
  no ERAM sample, decides.
- **OA-BENCH-062** The digest covers every byte of every read, including reads after a zero-length read; the
  digest of a file whose last read was not a zero-length read is an error, not a digest.
- **OA-BENCH-070** The common-time comparison never picks a row by its error: among rows of one system for one
  aircraft at one time (several flight records), it uses the row of the flight with the most paired samples
  for that aircraft over the whole run, the lowest flight index on a tie, whatever the grid phase; every other
  row at that time is counted as a duplicate-track sample and reported beside C1 for each system.
- **OA-BENCH-071** Each contiguous stretch of a truth leg inside the 2 NM radius of a meter fix is one passage,
  its time the closest approach within the stretch. An estimate is scored against the first passage that ends
  after its issue time (the passage it is issued in, when issued inside one); the estimated time never chooses
  it. A leg crossing a fix at 11:30 and again at 12:30 scores an estimate issued at 12:05 against 12:30.
  Passages are cached per (leg, fix, passage). F3 takes one takeoff (leg start) and one landing (leg end) per
  leg, so it has no second occurrence in a leg to choose between.
- **OA-BENCH-072** The report header carries metrics version 2 and a scoring-code digest: SHA-256, in hex, over
  the name, length and text of the metric modules (included at build, hashed at run time), plus the build's
  `OPENATCA_SOURCE_REV` when set. A change to those modules changes the digest.
- **OA-BENCH-090** Every join on an identifier compares one normal form: leading white space skipped, cut at the
  first zero or white space, ASCII upper case, zero padded. That covers callsign against aircraft identification
  (one flight or several), plan identifier in the plan index and in every lookup into it (TBFM, TFMS, the
  matching counts), and fix and airport names against the airspace model. Two accepted spellings of one
  identifier (case, padding) never score differently.
- **OA-BENCH-091** The extraction counts list what the decoders refused: SFDPS messages rejected by reason and
  field errors in kept rows by field, TBFM flights and TFMS messages refused for a field by field index and by
  reason, each with its total. A row or field that did not reach scoring is in one of the counts.
- **OA-BENCH-092** An input file is bounded. A `.msg` file longer than the replay parser's document limit plus
  64 KiB, by its length or because it grows while read, is not kept: it is read to its end through a fixed
  buffer and refused as a document (OA-BENCH-102). The airspace snapshot is read with a cap against the
  snapshot size limit and refused (the load fails, naming the file) when longer. No read allocates more than
  the limit.
- **OA-BENCH-100** A handoff crossing time is derived from truth only where the truth is continuous across the
  boundary: the last sample over the transferring unit and the first over the receiving unit are no further
  apart than the truth leg gap (`oa_truth::LEG_GAP_MS`) and the unit is known at every refinement probe.
  Otherwise the handoff is excluded as "crossing inside a truth gap" and counted. With
  several crossings in the window the one nearest the event is used.
- **OA-BENCH-101** Mean and RMS are summed on values scaled by a power of two near the largest magnitude, so
  finite input gives no overflow or underflow to a perfect value: `[1e200]` has RMS 1e200, values near
  `f64::MAX` and the smallest subnormal give their RMS, and one huge value among ordinary ones is not
  lost. A statistic still not finite is NaN (not computable), never 0.
- **OA-BENCH-102** A `.msg` file longer than a document can be is counted as a refused document of its feed
  with the reason "longer than a document can be" and the load continues; its bytes are all in the input
  digest.
- **OA-BENCH-103** A fix passage whose stretch inside the radius begins or ends at a hole in the truth has no
  closest-approach time. The sum of squared milliseconds behind a histogram RMS that leaves the range of a
  `u128` gives an RMS of NaN, not a wrapped or panicking one.
- **OA-BENCH-110** The takeoff and landing ground test never compares pressure altitude with field elevation
  (feet above mean sea level). The truth altitude becomes `pressure_alt + (altimeter inHg - 29.92) * 1000`
  with the field's METAR altimeter setting nearest in time and within 60 minutes (the earlier of two equally
  near; a value outside 27 to 32 inHg is no setting). A field with no such setting excludes the event as
  "no altimeter setting for the field" and counts it; nothing is assumed (`oa-bench --metar <dir>` supplies
  the settings, and without it every such event is excluded).
- **OA-BENCH-111** A point inside the areas of more than one ARTCC has no unit: facility-name order never
  chooses. A handoff whose truth window holds such a point is excluded from H1 as "truth position inside more
  than one ARTCC area, unit ambiguous" and counted, and the report lists each overlapping facility pair with
  the number of such positions examined.
- **OA-BENCH-112** Distances and east-north error components use the WGS-84 geodesic inverse and direct
  solutions of `oa-geo`, not a small-distance tangent plane: 89.99 N at 0 E and 180 E are 2233.9 m apart
  with the other point due north, 90 degrees along the equator is 10 018 754 m, and a mid-latitude pair
  agrees with the old tangent plane to a tenth of a percent. An invalid position gives NaN, which the pairing
  excludes and counts.
- **OA-BENCH-113** `dist_to_polyline` is the exact WGS-84 geodesic distance to the nearest geodesic segment
  (`oa_airspace::geom::exact_edge_m`), within 1 m of a dense-sampling brute force on a 600 NM segment.
- **OA-BENCH-114** The synthetic world's truth velocity (and the velocity of its ERAM stream) is true east and north at
  the aircraft's position, the plane velocity carried through the same geodesic map as the positions: its heading
  equals the heading of the position track to 0.01 degrees.
- **OA-BENCH-115** The bench publishes `T3-heading-grid`: the heading error regressed on `(lon - mean lon) * sin(lat)`,
  with slope, standard error, count and the reference longitude; slope 1 for an injected error equal to that term, 0
  for none.
- **OA-BENCH-116** A handoff crossing time is given only where the truth is available and definite at every probe of
  the search and on both sides of the bracket; a crossing that may lie inside a gap in the truth is excluded and
  counted as "crossing inside a truth gap". Without a gap it is located to 1 s.
- **OA-BENCH-117** T8 is the time of the first associated position of the leg minus the zone entry, or, for a leg
  that starts inside the zone, minus its observed takeoff (OA-BENCH-125). A value of zero or less is not a delay: it
  is counted as `established_before_entry` and left out of the distribution; a pop-up 20 s after entry gives 20 s.
- **OA-BENCH-118** `legacy_from_track_samples_by_leg` makes one flight per truth leg of an address (a row goes
  to the latest leg of its address starting at or before its time), so the publication interval (T5) is never
  taken between two trips of one aircraft.
- **OA-BENCH-119** `check_split` refuses a split of the ADS-B reports into the tracker's set A and the truth's
  set B when an (address, time) is in both sets or a set holds two reports of the same address, time and
  content: a typed violation, a hard error of the run and not a warning.
- **OA-BENCH-120** `score_track_samples` and the `oa-track` scorer build the legacy table by truth leg, so T5
  of a scored run is per trip, not pooled across the trips of one address.
- **OA-BENCH-121** `observation_key` is the one identity of an ADS-B report for the split and its check: address,
  time and every field scoring or quality handling reads (position, altitudes, vertical rate, speed, track,
  squawk, flags, source, category, NIC, NACp, NACv, SIL, SIL type, version). Reports of one address and time
  that differ in any of them are not copies: they stay together on one side, ordered by the key, so the
  retained evidence and the scores do not depend on input order.
- **OA-BENCH-122** `load_metar_dir` reads each METAR file once into memory and hashes and parses those bytes, so
  the reported digest describes the data scored even if the file changes between reads.
- **OA-BENCH-123** `T3-heading-grid` does not depend on the order of the pairs: longitudes are taken relative to
  the smallest longitude of the samples and the samples are summed in sorted order. A cohort spanning more than
  90 degrees of longitude is refused (scalar `refused_span` is 1, no slope), not fitted to an arbitrary anchor.
- **OA-BENCH-124** `stats_of` keeps in the mean what scaling by a power of two rounds away from values too small
  for the scaled range: `[1e308, -1e308, 1e-300]` has mean 3.33e-301, equal to the exact sum over the count.
- **OA-BENCH-125** A takeoff in T8 is an observed event: the first airborne ADS-B report after a ground report
  inside the truth leg (`LegStart::Takeoff`, with the gap back to the last ground report as the stated
  uncertainty, `takeoff_time_uncertainty_s_max`). A leg that begins inside the zone is measured from its takeoff;
  one whose first stated ground state is airborne is excluded and counted as `legs_first_seen_airborne_in_zone`,
  one never airborne as `legs_starting_inside_zone_never_airborne`, one with no ground indication as
  `legs_starting_inside_zone_no_ground_report`. No altitude cutoff is used.
- **OA-BENCH-126** T7 counts every grid point whose truth query failed, by reason (`truth_grid_points_failed_*`),
  with the total and the usable points, and says that presence is conditional on usable truth; a leg with no usable
  truth at any point is counted as `legs_no_usable_truth`, not as never in the zone.
- **OA-BENCH-127** A pair whose truth ground speed is below 30 kt is left out of the heading statistics with a
  counted reason, `T3-heading: truth speed below 30 kt`.
- **OA-BENCH-128** The airspace lookups keep airports of two ICAO regions that share an identifier apart:
  thresholds join the airport of their own region, and an identifier naming airports of more than one region is
  not answered and is counted (`ambiguous_airports`).
- **OA-BENCH-129** Rows, samples and values the benchmark leaves out are counted under a named reason and
  reported, not dropped: tracker rows of a flight the table refuses, SFDPS, TBFM and TFMS rows ingest cannot
  place, samples left out of the T2 offset fit and of the common-time comparison (non-finite or slow), and truth
  states with a non-finite or out-of-range altitude or vertical rate, which are refused as `NoTruth::NotFinite`.
- **OA-BENCH-130** T9 names the nearest other aircraft the same way whatever the order of the legs: of two at the
  same distance, the lower address, with such samples counted (`nearest_tied_samples`). The legs looked at over a
  call are bounded by `SwapConfig::max_leg_visits`; a sample past it is refused and counted
  (`samples_refused_work_budget`). T9 measures nearness, not identity.
- **OA-BENCH-131** T4 compares ERAM altitude with the truth on ERAM's own reference. A reported value at or
  above 18,000 ft is a flight level, compared with the truth pressure altitude as it is. A lower value is an
  altitude set with the local altimeter setting, so the truth is first brought to indicated altitude with
  `pressure altitude + (inHg - 29.92) * 1000` ft, using the setting of the nearest station (by position) that has
  one within 60 minutes of the sample (the setting nearest in time). A pair with no setting is not compared: it is
  counted under `T4` / `no altimeter setting`, never compared uncorrected. T4 reports `compared_as_flight_level`,
  `corrected_with_altimeter`, `excluded_no_altimeter_setting` and `mean_altimeter_correction_ft`; the flagged view
  applies the same correction.

## Winds aloft (oa-wind)

Wind and temperature at any (latitude, longitude, pressure altitude, time) from NOAA RAP analyses decoded
by `oa-grib2`: the trajectory predictor's weather input. Pure core, file shell in `archive`, and the
`oa-wind-check` validation tool. Nothing is extrapolated.

- **OA-WIND-001** Pressure altitude becomes pressure by the ICAO standard atmosphere (exact, since
  pressure altitude is defined on the ISA): 1013.25 hPa at 0 ft, 506.0 at FL180, 300.9 at FL300, 226.32 at
  the tropopause, continuous across it, and the inverse round-trips. Non-finite input and altitudes outside
  -5000 m to 20 000 m are refused.
- **OA-WIND-002** Horizontal interpolation is bilinear in the grid's own coordinates; a field linear in
  the grid indexes is reproduced exactly at any position inside the grid.
- **OA-WIND-003** Vertical interpolation is linear in log-pressure between the bracketing isobaric levels;
  a field linear in ln p is reproduced exactly between levels, and the aircraft's pressure altitude is
  converted through the ISA pressure.
- **OA-WIND-004** U and V stored grid-relative (the grid's component flag) are rotated to east and north
  by the convergence angle at the query longitude; a uniform earth-relative wind stored grid-relative is
  recovered, is wrong without the rotation, and a grid that is already earth-relative is not rotated.
- **OA-WIND-005** Time is linear between the two bracketing analysis hours; the answer reports the hours
  used and the weight; on an hour the weight is 0 (or 1 against the previous hour for the last).
- **OA-WIND-006** A query is refused with a typed `Refusal` outside the grid, above or below the level
  range (the limits themselves are answered), outside the time span by even one second, outside the ISA
  range, for non-finite input, and with no hours; never extrapolated.
- **OA-WIND-007** A missing value at a grid corner or level that the query uses is refused as
  `MissingValue`, never filled; a query that does not touch it is answered (OA-WIND-052).
- **OA-WIND-008** An hour is built only from complete, consistent fields: a level lacking U, V, T or
  height, a repeated field, differing grids, point counts or valid times, a fractional level, an unusable
  time unit, fewer than two levels and more than 64 levels are refused with typed errors; other
  parameters and non-isobaric levels are ignored; the arrays are level-major in ascending pressure.
- **OA-WIND-009** Bytes that are not a GRIB2 file with the needed fields are refused, and an hour is
  loaded by decoding only the isobaric UGRD, VGRD, TMP and HGT messages (the rest are only inspected).
- **OA-WIND-010** The `.idx` byte ranges of exactly the isobaric UGRD, VGRD, TMP and HGT messages are
  selected (layer labels such as `500-1000 mb` and other variables excluded) and adjacent ranges joined.
- **OA-WIND-011** The archive refuses two files with one valid time and too many files, sorts by time,
  refuses queries before reading any file where it can, reports an unreadable file as a typed error, and
  holds at most three hours.
- **OA-WIND-012** The validation statistics (median, 95th percentile, mean, maximum), the conversion of a
  reported "from" direction and speed in knots to east and north, the vector comparison with a reference
  field pair (a rotation-correct model agrees, an unrotated one does not) and the observation comparison
  (refused queries counted by reason, not compared) behave as stated.
- **OA-WIND-013** On the real RAP t12z hour (skipped when absent) the hour has 37 levels on the 451 x 337
  grid, 90.0 MB of arrays, equals the hour built by `load_hour` from the whole file, its stored values
  equal `oa-grib2`'s decoded values exactly, and the query at grid points agrees with `oa-grib2`'s
  `wind_at` and bilinear sampling to rounding.
- **OA-WIND-014** On the real data (skipped when absent) RAP agrees with the HRRR 250 hPa wind at t12z with
  a median vector difference under 2 m/s (and is worse without the rotation), and a half-hourly sweep over
  all hours never holds more than three and refuses a time after the last hour.
- **OA-WIND-015** ADS-B observations for the RAP comparison are the reports that have a barometric
  altitude (not on the ground) and a readsb wind (speed and direction) or outer air temperature, at most
  one per aircraft per gap, with position, time, roll and track rate carried over in SI-ready units.
- **OA-WIND-016** The ADS-B comparison gives, for RAP minus readsb, the count, median and 95th percentile
  vector difference, the speed bias and the u and v bias, and the temperature difference against `oat`,
  for all observations, by altitude band (4) and by turning state (straight, turning, unknown); a refused
  query is counted by reason and left out, and a failed file load stops it.
- **OA-WIND-017** `oa-wind-check adsb` refuses a bad `--gap-s` (exit 2), skips with a note and exit 0
  when there are no RAP files, does not scan an archive whose day does not overlap the RAP hours, and on
  the real data (skipped when absent) exits 0 with either the comparison or the no-overlap statement.

- **OA-WIND-050** No answer carries a non-finite number: `build_hour` refuses an infinite cell (a
  NaN is a missing value and stays), and the sampler refuses an infinite corner value, and any
  point or sample that would not be finite, instead of returning it.
- **OA-WIND-051** `bracket` refuses valid times that are not strictly increasing (a duplicate would
  give a NaN weight), as well as an empty list and a time outside the span.
- **OA-WIND-052** A query uses only the inputs with non-zero interpolation weight: at an exact hour,
  pressure level, grid node or edge, a missing value or unreadable file of a zero-weight neighbour does
  not refuse it; a missing value at a weighted input is still refused.
- **OA-WIND-053** Whole-file reads are bounded: a GRIB file by `MAX_FILE_BYTES` and an `.idx` sidecar by
  `MAX_IDX_BYTES` (1 MiB; the largest real one is 32 596 bytes), checked by length before reading
  and again by a read that stops one byte past the limit; an oversize `.idx` is ignored and the file
  read whole.
## M3 — ASTERIX codecs (oa-asterix)
- **OA-WIND-054** The hour model is instantaneous: a wanted field (isobaric UGRD, VGRD, TMP, HGT) from a
  statistical-interval product template is refused as `StatisticalInterval`, counting the fields and
  naming the first; a quartet of instantaneous fields loads as before, and an interval field the hour
  does not use is ignored.
- **OA-WIND-055** An `.idx` is checked against its file before its ranges are trusted: its message
  offsets are the file's message starts, each message's own header agrees with the index's first row for
  it, the decoded ranges are exactly the variables, levels and times the index claimed, and the hour
  holds exactly the levels the index names. Any difference is an `IndexMismatch`, counted
  (`IndexUse::Refused`, `Archive::index_fallbacks`), and the whole file is read: the hour is then the
  one a file without a sidecar gives, never a thinner one.
- **OA-WIND-056** On the real RAP hours (skipped when absent) the shipped `.idx` of the original file is
  refused for the local subset and the hour equals the whole-file hour; an index written from the
  subset's own headers is accepted and gives the same hour.
- **OA-WIND-057** Two wanted fields of one message (rows `N.1` and `N.2`) select one byte range.
- **OA-WIND-058** The day of an ADS-B archive part named `vYYYY.MM.DD` is refused unless the date exists and the year is 1970 to
  9999 (not a day 1 to 31 in any month).
- **OA-WIND-059** A percentile rank that is NaN gives no value; it is not read as rank 0.

Encoder and decoder for EUROCONTROL ASTERIX category 021 (ADS-B target reports, Ed 2.7), 048 (monoradar
target reports, Ed 1.32) and 062 (system track data, Ed 1.21), written from the specifications (Part 1
Ed 3.1 for the general structure). A generic core (data block, FSPEC, item kinds, driven by one constant
UAP table per category) and a typed layer for the items the tracker needs; every other item round-trips as
raw bytes. The decisions where the specification leaves room are listed in the `wire` module docs.

- **OA-AST-001** A data block is CAT, a two-octet LEN that counts the header and every record (at most
  65 535), and records that tile the block exactly; several blocks in one buffer decode one after another,
  and a stream whose last block is cut short is refused.
- **OA-AST-002** The FSPEC is read and written bit 8 first with the LSB as FX: FRN 1 to 7 in the first
  octet, 8 to 14 in the second, and so on; the encoder writes the shortest FSPEC and sets FX only between
  octets.
- **OA-AST-003** Every item kind (fixed, extended, repetitive, explicit-length SP and RE, compound with
  extended and repetitive subitems) is walked from its constant table entry, and the untyped items of a
  record come back as exactly the bytes that were put in.
- **OA-AST-004** A block that is shorter than its header, has a LEN below three or past the input, holds no
  record, or is of a category other than 021, 048 and 062 is refused with a typed error naming the
  category and byte offset; an error in the second record names record index 1 and a stream-absolute
  offset.
- **OA-AST-005** An FSPEC that is empty, ends with an all-zero octet that is not the only one, runs past
  the block or past the longest FSPEC the UAP can address, sets the bit of a slot the UAP leaves unused
  (CAT062 FRN 2 and 29 to 33, CAT021 FRN 43 to 47), or omits the data source identifier is refused.
- **OA-AST-006** An item whose FX chain does not end inside the record or within 32 octets, a repetitive
  item with a factor of zero or that runs past the block, an explicit-length field shorter than two octets
  or past the block, a compound item with no subitem, with a subitem the category does not define (naming
  the subitem), with FX on its last defined primary octet or (in an item decoded into fields) with a trailing all-zero
  primary octet, and a fixed item or subitem cut short are refused with the item number and byte offset.
- **OA-AST-007** A bit that the specification fixes to zero (a spare bit) is refused in every fixed and
  extended item and compound subitem whose table entry lists it, at the extension the specification puts it
  in (bits that are assigned in another extension of the same item are accepted); reserved enumeration
  codes are accepted.
- **OA-AST-008** A CAT021 record built by hand from the specification (data source, target report
  descriptor, high-resolution position, target address, quality indicators with an extension, Mode 3/A,
  flight level) decodes to the expected values and encodes to the expected bytes, the bytes derived from
  the LSBs in the test.
- **OA-AST-009** The same for a CAT048 record (time of day, descriptor, polar position, Mode 3/A, flight
  level, plot characteristics, aircraft address and identification, track number, Cartesian position, track
  status).
- **OA-AST-010** The same for a CAT062 record (time, WGS-84 and Cartesian position, velocity, Mode 3/A,
  aircraft derived data with address, identification and selected altitude, track number, track status,
  system track update ages, measured flight level, geometric altitude, rate of climb).
- **OA-AST-011** `encode(decode(bytes))` equals the bytes for every block the decoder accepts, and
  `decode(encode(record))` equals the record, for blocks with every typed item and every untyped item.
- **OA-AST-012** Untyped items, including SP and RE and compound, repetitive and extended items, are
  written back in FRN order to the same bytes whatever order they are given in.
- **OA-AST-013** The unit LSBs are named constants that cite the specification section; positions convert
  between raw counts and 1e-7 degree with at most half an LSB of error and the count survives the round trip
  through e-7 degree exactly; times convert between 1/128 s counts and milliseconds.
- **OA-AST-014** The encoder refuses a value that does not fit its element, a character outside the
  six-bit set, a latitude beyond 90 degrees, an item given twice, a raw item that is not exactly one
  well-formed item or whose slot does not exist, an empty block, too many records and a block over 65 535
  octets, with the record, item and kind.
- **OA-AST-015** Truncating every block of the corpus at every byte never panics: with LEN left alone every
  cut is refused, and with LEN made to match every cut is refused except those on a record boundary, which
  are valid blocks.
- **OA-AST-016** 4000 random mutations of the corpus (and 4000 arbitrary byte strings) never panic, and
  every mutation that is accepted re-encodes to exactly the bytes it was decoded from.
- **OA-AST-017** The bounds are named constants (block length 65 535, records per block, extension chain 32,
  repetitions 255, primary subfield 8) and hold at the edge: a block of exactly 65 535 octets and 255
  repetitions are accepted, one more is not.
- **OA-AST-018** Six-bit characters code A to Z, space and 0 to 9 and nothing else (six octets of zero in an
  identification are the one exception, see OA-AST-026); a latitude of exactly
  +-90 degrees is accepted and one count more is refused.
- **OA-AST-019** A CAT021 record built from an `oa-adsb` row (position, address, NACp, NIC, SIL, MOPS version,
  Mode 3/A, callsign, altitudes, ground vector, vertical rate) round-trips, with every quantised field within
  half its LSB: position 9 mm (1e-7 degree after conversion), flight level 12.5 ft, geometric height
  3.125 ft, speed 0.11 kt, track 0.0028 degree, vertical rate 3.1 ft/min.
- **OA-AST-020** On real adsb.lol rows (skipped when `OPENATCA_DATA` is absent) the same round trip holds
  for tens of thousands of rows within the same bounds.
- **OA-AST-021** A refusal renders as text naming the category, the record, the data item (and subitem) and
  the byte, and leaves out the parts that do not apply.

## M3 — Sensor simulation (oa-sensorsim)

Simulated secondary radar plots and injected ADS-B spoofs, generated from `oa-truth` (docs/M3_DESIGN.md
sections 1, 2 and 4). Error values are only those of docs/BENCHMARKS.md 3.1 "Simulated sensor inputs"; the
radar biases, split geometry and earth-radius factor have no sourced value and are parameters with no default.

- **OA-SIM-001** The generator is xoshiro256** seeded by SplitMix64, written from the published algorithms;
  both reproduce their published output sequences, streams for different (seed, aircraft, scan) words differ
  and repeat exactly, and uniform and normal draws have the right moments.
- **OA-SIM-002** Each named profile (`MSSR_EUROCONTROL_1997`, `MODE_S_ATC302`, `SSR_CD2_ATC302`) carries the
  tabulated values (range and azimuth sigma, quantum, altitude resolution, scan periods 4.8 s and 12 s,
  detection, Mode A, Mode C, false and split rates) and the simulated range and azimuth errors have the
  profile's sigma within 4% over more than 10 000 plots, with a mean consistent with zero.
- **OA-SIM-003** Range and azimuth bias are added exactly (with the random error set to zero, measured minus
  true equals the bias to 1e-9).
- **OA-SIM-004** A plot is made at the instant the beam azimuth equals the target azimuth: exactly the scan
  start plus the azimuth fraction of the period for a stationary target (to the nanosecond), and, for a moving
  target, at a time where the beam azimuth equals the azimuth of the truth position at that time (checked
  through the public truth query), inside the revolution, and not where a frozen-target solve puts it.
- **OA-SIM-005** Detection, false-plot and split-plot rates match the profile within 4.5 binomial standard
  deviations; false plots are uniform in area over the coverage disc and in azimuth, timed by the beam; split
  plots repeat the plot time and differ by the stated range and azimuth offsets.
- **OA-SIM-006** A target beyond the coverage range or below the 4/3-earth line of sight (effective-radius
  factor a site parameter) gets no plot and is counted by cause; the line-of-sight test matches the radio
  horizon formula 4.12 (sqrt h1 + sqrt h2) km.
- **OA-SIM-007** Altitude is quantised to 100 ft (Mode C) or 25 ft (Mode S) by the profile, and range to the
  1/8 NM grid for SSR CD-2.
- **OA-SIM-008** CAT048 output decodes with `oa-asterix` to the same plots within the element quantisation
  (range 1/512 NM, azimuth half of 360/2^16 degree, time 1/256 s plus the millisecond floor), with Mode A,
  flight level, address, SIM flag and target type, one block per scan.
- **OA-SIM-009** The same seed, truth and site give identical plots and identical CAT048 bytes; a different
  seed differs; an aircraft's plots do not depend on which other aircraft are in the truth.
- **OA-SIM-010** Each aircraft gets its own Mode A code (never 0000, 7500, 7600 or 7700); Mode S profiles put
  the aircraft address on the plot and non-Mode S profiles do not.
- **OA-SIM-011** An invalid site or profile (latitude, period, azimuth, coverage past the CAT048 range
  element, earth factor, non-finite bias, probability above one) is refused with a typed error, and a plot
  range past the CAT048 element is refused at encoding (see OA-SIM-024 and OA-SIM-025).
- **OA-SIM-012** The ghost transform adds an aircraft with a new address on a straight track at the stated
  speed and track, labels exactly those rows, and leaves every original row intact and in order.
- **OA-SIM-013** The drift transform moves the victim's reports in the window by the stated rate times the
  elapsed time along the stated bearing, labels exactly those rows, and changes nothing else.
- **OA-SIM-014** The replay transform copies the victim's reports of a window shifted later in time, under
  the victim's or another address, with the original positions, and labels them.
- **OA-SIM-015** The duplicate-address transform re-sends a donor aircraft's reports under the victim's
  address and labels them; the victim's and donor's own rows are unchanged.
- **OA-SIM-016** Every spoof transform refuses an address already present (ghost), an address past its
  width, a non-positive interval, shift or window, a victim or donor without reports and equal victim and
  donor, with a typed error.
- **OA-SIM-017** Transforms compose, and a detector's flags score against the labels as a confusion matrix
  (true and false positives and negatives, and per attack kind) whose cells sum to the row count.
- **OA-SIM-018** On a real ZAU clip (skipped when `OPENATCA_SENSORSIM_CLIP` is absent) a terminal and an
  en-route radar produce plots whose CAT048 output decodes, with a detection rate within one percentage point
  of the profile's.
- **OA-SIM-019** The `oa-sensorsim` binary writes the plots (CSV and CAT048) of an OAF1 clip, gives the same
  bytes for the same arguments, and refuses to run without the bias, coverage and split parameters.
- **OA-SIM-020** Reading a clip drops only a record that cannot be read (position or flags out of range) and
  counts it; the other records of its frame are kept.
- **OA-SIM-021** A ghost or drifted position is checked as a geographic position: latitude within 90 degrees
  (a count past it is refused, for the start and for every row of the track) and longitude within 180, as a
  typed `BadSpoof`; the pole and the antimeridian are accepted.
- **OA-SIM-022** A ghost whose duration (`(count - 1) * interval`, in milliseconds and in nanoseconds) or last
  timestamp does not fit in `i64` is refused with the typed `BadSpoof` before any row is made.
- **OA-SIM-023** Reading a clip that ends inside a frame (a partial header or a partial body) keeps the valid
  frames and counts the partial frame with the frames that did not decode.
- **OA-SIM-024** Encoding a plot whose azimuth is not finite or is outside 0 to 360 degrees, or whose range is
  not finite, is refused with a typed error; it is never encoded as north or zero. 360 degrees wraps to 0.
- **OA-SIM-025** Encoding a Mode A code above 0o7777 (other than the absent marker) is refused with a typed
  error; the code is never masked to its low 12 bits.
- **OA-SIM-026** A detection whose measured range or azimuth is not finite (an oversized profile sigma) gets
  no plot and is counted in `non_finite`.
- **OA-SIM-027** A numeric option of `oa-sensorsim` that is NaN, infinite or not a number is refused with an
  error naming it.
- **OA-SIM-028** Ghost and drift legs are WGS-84 geodesics from the leg start (initial azimuth, distance = speed or
  rate times elapsed), also near a pole. The flat offset helper `offset_deg` is for sub-millisecond advances only: it
  returns `None` for an offset above 500 m, a start within 0.1 degrees of a pole or a non-finite input.
- **OA-SIM-029** `oa-sensorsim` reads its clip with the reader of `oa-truth` (OA-TRUTH-045): a clip over 4 GiB or
  one whose frame headers claim more than 50 million rows is refused with a typed message.
- **OA-SIM-030** The truth position at a crossing time is kept at any latitude when the time is a whole
  number of milliseconds (no offset is applied); a sub-millisecond remainder is applied flat, or, where the flat
  step refuses (within 0.1 degrees of a pole), by the geodesic direct problem. An aircraft above 89.9 degrees is
  detected by a site at 89.5 degrees.
- **OA-SIM-031** A radar site's `epoch_ns` outside half the signed 64-bit nanosecond range is refused by
  `simulate_radar` with `SimError::BadSite("epoch")`, and the scan index, the scan start and the beam
  azimuth use checked or widened arithmetic, so no epoch or window overflows.
- **OA-SIM-032** A drift window across the whole time range does not overflow in the elapsed time.
- **OA-AST-022** The test-side pcap reader (`tests/common/pcap.rs`) returns the UDP payload of every packet of a
  libpcap file (little or big endian, microsecond or nanosecond magic) over Ethernet II and IPv4 without
  fragmentation, trims Ethernet padding by the IP length, and refuses with a typed error a bad magic or
  version, a link type other than Ethernet, a truncated file or packet, a packet cut by the snaplen, a captured
  length over 262 144, an Ethernet type other than IPv4, a bad IP header or length, a fragment, a protocol
  other than UDP and a bad UDP length.
- **OA-AST-023** On real captures from another organisation's system (skipped when `OPENATCA_DATA` is absent):
  every block of CAT021, CAT048 and CAT062 in the eight sample files decodes (none refused except the
  pre-standard file of OA-AST-028), re-encodes to the same bytes, and the blocks of categories 001, 002, 034
  and 065 are counted by category with their framing checked; the counts per file are pinned.
- **OA-AST-024** On the same files our decoded values agree with the upstream decoder's expected output: block
  category and length, record bytes, every item's bytes and identity, and the CAT048 and CAT062 field values
  (position, time, Mode 3/A, flight level, address, identification, track number, plot characteristics,
  track status, velocity, ages, altitudes), with zero differences; the container times of the GPS file
  equal the upstream time stamps and the CAT001/002 frames match its block list.
- **OA-AST-025** A compound item kept as raw bytes may have a primary subfield whose last octet sets no bit
  (real trackers write one in I062/390) and goes back as it came; a compound item with no subitem is still
  refused; in a compound item decoded into fields the trailing zero octet is refused, naming its byte.
- **OA-AST-026** An identification (I021/170, I048/240, I062/380 subitem 2) of six zero octets, which real
  radars send for "none", is kept raw and round-trips with the typed field empty; any other code outside the
  six-bit set is still refused.
- **OA-AST-027** The Reserved Expansion Field and the Special Purpose Field of CAT021, CAT048 and CAT062 are
  kept as raw bytes and round-trip, and a length past the record is refused.
- **OA-AST-028** A CAT062 record written in a pre-standard layout (the Ed 0.17 UAP, the 100 datagrams of the
  sample `asterix.pcap`) is refused by the Ed 1.21 decoder and not misread; on the real file all 100 are
  refused and all 100 tile their blocks exactly under the Ed 0.17 layout with a constant data source and a
  time of track that never runs backwards.
- **OA-AST-078** Each field of the CAT021 quality indicators (I021/090: NICBARO, SIL, NACp in the first
  extension; SILS, SDA, GVA in the second; PIC in the third) is read from its own bits, so that a value
  in one field never shows in its neighbour, and an extension that is absent gives no value.
- **OA-AST-079** Each flag and field of the CAT021 target report descriptor extension (I021/040: GBS, SIM,
  TST, CL) is read from its own bits, an extension that is absent gives no value, and the primary subfield
  gives ATP, ARC, RC and RAB at their own bits.
- **OA-AST-080** A descriptor and a quality item with every extension present decode from the wire to
  the values worked out by hand from the bit layout and encode back to the same bytes.
- **OA-AST-081** Every unit constant (the LSBs of CAT021, CAT048 and CAT062 and the bit counts of the
  positions) has its exact value.
- **OA-AST-082** A refusal in a block header (short input, LEN below the header, LEN past the input,
  category not carried, no record) names its byte offset, counted from the start of the block plus the
  offset the caller gave for it.
- **OA-AST-083** A refusal in the FSPEC of a record that is not the first (empty, cut short, not the
  shortest, too long, a slot the category leaves unused, no data source identifier) names the record and
  the byte offset of the FSPEC octet or of the record.
- **OA-AST-084** A refusal inside an item of a record that is not the first (fixed, extended, repetitive,
  explicit-length and compound items, spare bits) names the record, the item, the subitem of a compound
  item, and the offset where the item or the offending octet starts.
- **OA-AST-085** A refusal by the typed layer in a record that is not the first (a value out of range, a
  compound primary subfield that is not the shortest, which is named before a failure of the same
  item) names the record, the item and the offset.
- **OA-AST-086** In a stream of blocks, a refusal in the second block reports the offset from the start
  of the whole stream.
- **OA-AST-087** In the aircraft derived data of a CAT062 record (I062/380) an unparsed subitem that
  repeats one the record already gives typed, or that stands for the address or the selected altitude, is
  refused as a duplicate; the same subitem number with no typed counterpart is written; subitem
  number zero is refused, and so is a record with no subitem.
- **OA-AST-088** A position count converts to 1e-7 degree rounded to nearest with ties away from zero,
  for both signs and for each width, and the conversion back rounds the same way.
  A raw count that is past 180 degrees (or a width beyond 32 bits) gives no position instead of a wrapped
  one.
- **OA-AST-089** A compound primary subfield is refused as not the shortest only when an all-zero octet
  ends it after the first octet, naming that octet; a single zero octet at the start is left to the empty
  compound refusal, and an item that is not compound is never refused.
- **OA-AST-090** A value is packed into an unsigned element of a given width exactly when it fits: the
  largest value of the width is accepted, the next is refused, and a 32-bit element takes every value.
- **OA-AST-100** (proof) `rd` and `take` never panic, overflow or read out of bounds for any start and
  length over inputs of up to 6 octets, and advance the position only on success by exactly the octets read;
  `sext` keeps the low bits and stays in range, for every value and width of 1 to 32 bits.
- **OA-AST-101** (proof) The FSPEC parser returns, for every input, either a non-empty list of FRNs with a
  position inside the input, or an error positioned inside the input, without panic or overflow. CAT048:
  every input (the FSPEC is read or refused within the 4 octets the UAP can address, so 5 octets cover all).
  CAT021 and CAT062: inputs of up to 3 octets.
- **OA-AST-102** (proof) Walking any item of the CAT021, CAT048 and CAT062 UAPs (every slot, SP and RE
  included) over any bytes of up to 24 octets never panics, overflows or reads out of bounds; a walk that
  succeeds ends after its start and inside the input, and an error is positioned inside the input.
- **OA-AST-103** (proof) `parse_block` (the framing stage of `decode_block`) over any byte string of up to 3 octets, which holds the refusals
  of a short header, a LEN below three or past the input, no record and an unsupported category,
  never panics or overflows, and a block it accepts occupies more than zero and at most the input's octets.
  Whole blocks of 4 octets and more are beyond what the solver finishes (see `docs/VERIFICATION.md`).
- **OA-AST-104** (proof) Every six octets that decode as an identification encode back to the same six octets;
  a 24-bit time of day survives 1/128 s to milliseconds and back exactly, and neither conversion overflows for
  any `u32`.
- **OA-AST-105** A CAT062 position (I062/105) with a longitude beyond 180 degrees (2^25 counts) is refused by
  the decoder and by the encoder as an invalid value of that item, as a latitude beyond 90 degrees is; exactly
  +-180 degrees is accepted, and a raw longitude that would wrap in the unit conversion (0x40000000) is refused.
- **OA-AST-106** (proof) For every `i32`, the longitude check at 25 bits accepts exactly +-2^25 counts, and
  `raw_to_e7` at that width gives a position within 180 degrees or nothing, nothing only for a count the check
  refuses; neither panics.
- **OA-AST-107** A CAT021 high-resolution position (I021/131, LSB 180/2^30 degrees) with a longitude beyond
  180 degrees is refused by the decoder and by the encoder as an invalid value of that item; exactly +-180
  degrees is accepted. The three-octet I021/130 cannot hold a longitude beyond 180 degrees.

## M3 — Surveillance tracking (oa-track)

### ADS-B-only tracker and its scoring (oa-track, oa-bench adapter)

- **OA-TRK-001** A measurement (time of applicability, sensor id, position with covariance, optional
  velocity with covariance, optional barometric or geometric altitude with quantisation, optional vertical
  rate, address and identity; one 72-byte record) is validated before use: a position out of range, a
  covariance that is not finite and positive definite, a bad velocity, an altitude or rate out of range, and
  a missing address are refused with a typed reason and counted; no track is made from them. Bad
  configurations (zero capacity, M above N, a non-positive time, an unknown filter mode) are refused.
- **OA-TRK-002** On straight flight with noise taken from NACp 9, the tracker's position error is below
  7 m RMS at 1 Hz (raw reports: 17 m) and its speed and heading errors are below 1 m/s and 0.2 degree; at
  17 s input the one-second output stays under 25 m. The horizontal state lives in a local east-north plane
  (`oa_geo::aeqd`, moved from `oa-truth` so both share it) re-centred on every update.
- **OA-TRK-003** Position noise is the NACp 95 % bound over 2.448 per axis; with those values the mean
  normalised innovation squared of a position is 2.0 (within 1.7 to 2.4), and a report that claims three
  times the true accuracy pushes it above 3.
- **OA-TRK-004** Constant-rate turns at 1.5, 3 (standard rate) and 6 degrees per second at 1 s input, and
  3 degrees per second at 5 s input, are followed (position under 15 m RMS, heading under 3 degrees, scored
  on the one-second output); the interacting pair beats a single straight-model filter with the same
  process noise by 34 to 99 % in position and by 4 to 50 % in speed, and beats the best of three single
  filters (process noise 0.05 to 5 m^2/s^3) by 0 to 99 % in position (the test prints the table).
- **OA-TRK-005** On straight flight the pair costs nothing against one straight filter, and the model
  probabilities follow the aircraft: the turn probability is above 0.8 in the middle of a turn and below
  0.1 on a straight leg, and the turn rate estimate is within 0.5 degree per second of a 3 degree per
  second turn.
- **OA-TRK-006** An aircraft accelerating at 1 m/s^2 is tracked to under 10 m RMS position and 2 m/s RMS
  speed.
- **OA-TRK-007** The vertical filter (level and climb/descent models, quantisation as noise of variance
  step squared over 12) tracks a profile of level, 2,000 fpm climb, level and 1,500 fpm descent with 25 ft
  quantised altitudes to under 12 ft RMS and 120 fpm RMS, and its model probabilities exceed 0.9 in the
  middle of each phase.
- **OA-TRK-008** A track 600 km from the origin at 64 degrees north is as accurate as one at the origin
  (position under 8 m RMS, speed under 1 m/s), because the frame is re-centred on every update and the
  state (velocity and the whole covariance) carried through the change of the frame's axes.
- **OA-TRK-009** A measurement that arrives out of order inside the window is applied by re-filtering from
  stored state (decided again in time order, OA-TRK-230): with every fifth neighbour pair swapped, each swap is re-filtered and replayed exactly
  once and the output after the last swap is bit-identical to the in-order run.
- **OA-TRK-010** A measurement older than the clock by more than the window, one inside the window but
  older than every stored state of its track, and an exact repeat (same time and sensor) are each counted
  and dropped under their own counter; one inside the stored states is applied.
- **OA-TRK-011** A track is confirmed by 2 reports within 3 expected intervals, one expected interval after
  the first (the initiation delay is logged); a lone report expires, a second report after the window
  restarts the track, and a second report implying more than 700 kt restarts it (each counted).
- **OA-TRK-012** A silent track is flagged coasting from the first tick more than 3 expected intervals
  after its last update (104 s for a last update at 100 s) and dropped, logged and its slot freed at the
  first tick more than 60 s after it (no tick after 160 s).
- **OA-TRK-013** The fixed-rate output is a row per confirmed track at every whole second, one second
  apart; update rows carry the reports' own times.
- **OA-TRK-014** The same measurements give the same output bytes (the output digest and every column are
  equal across runs and across a different table capacity).
- **OA-TRK-015** Capacities are bounded and overflow is counted: reports of aircraft beyond the track
  capacity are refused and counted one by one, output rows past the table capacity are counted, and track
  log entries past the log capacity are counted.
- **OA-TRK-016** Two aircraft crossing 120 m apart keep their own addresses and each stays within 50 m
  of its own truth: identity is the address, never the kinematics.
- **OA-TRK-017** After the tracker is built no buffer it owns changes address or capacity through a run with
  confirmations, late measurements, drops, restarts and overflow of the output, the log and the track
  table (a counting allocator needs `unsafe`, which RULESET A0 forbids here, so the check is on the
  buffers).
- **OA-TRK-018** The ADS-B adapter derives the position covariance from NACp (bound over 2.448, per axis;
  NACp 8 to 11 checked against the DO-260B bounds) and the velocity covariance from NACv, with velocity
  from ground speed and track and 25 ft altitude quantisation.
- **OA-TRK-019** NACp, NIC, SIL, version and NACv are carried forward at most 60 s; a row with none in
  that age is removed like truth's `no_quality_in_age`; a velocity without NACv is used as NACv 1 and
  counted; a row with only a geometric altitude passes it flagged geometric.
- **OA-TRK-020** The adapter's gates are `oa_truth::quality_check`, the code truth selection uses: on a mixed
  batch the removals by reason (stale flag, source, no quality, NACp, NIC, SIL, version) equal
  `oa-truth`'s.
- **OA-TRK-021** A tracker's output rows are scored by `oa-bench`'s unchanged metric functions through
  `tracker_input`: a 100 m error in a known direction is recovered from T1 and T2, 2 m/s in speed from T3,
  50 ft from T4, and a copy of truth scores zero.
- **OA-TRK-022** Track sample columns of different lengths are refused with a typed error.
- **OA-TRK-023** The scoring split is exact: per aircraft, copies of one observation removed, then the
  distinct (time, input index) groups numbered from 0, even to set A (the tracker) and odd to set B
  (truth); the sets share no observation, together every distinct report, and independent of input order.
- **OA-TRK-024** The quality fields are carried forward over the whole aircraft sequence before the split
  (the same 60 s rule and code) and written into both sets, so splitting does not starve the tracker of
  NACp.
- **OA-TRK-025** The scoring run's configuration comes from the measured interval distribution of set A:
  the expected interval is the median, the initiation interval the 90th percentile, the drop time truth's
  leg gap (120 s).
- **OA-TRK-026** Straight and turning are separated as in ESASSP Vol 1 4.2.14: transversal acceleration
  (speed times heading rate over a 6 s span of truth) above 1.5 m/s^2 is a turn; heading wraps at north.
- **OA-TRK-027** On a synthetic clip with known truth (a straight, a turning and an accelerating aircraft),
  the split, tracker, truth from the other half and the unchanged metrics run end to end: every track is
  confirmed 4 s after its first report, the position error is under 25 m RMS on both the one-second output
  and the report-time rows, both classes are populated and T5 of the output is one second.
- **OA-TRK-028** `oa-track-run --adsb <clip.oaf> --out <dir>` writes `report.txt`, `report.csv`,
  `tracks.csv` and `esassp.csv`, each scored number labelled as on thinned input and not an ESASSP exit
  result, byte-identical output digest across runs; usage errors exit 2 and an unreadable clip exits 1.
- **OA-TRK-029** On the real ZAU hour (skipped when `OPENATCA_ADSB_CLIP` is unset) the tracker runs with no
  capacity refusals or lost rows, is reproducible, confirms more than 80 % of its tracks and scores under
  60 m RMS at report times against the other half's truth.
- **OA-TRK-040** Re-filtering keeps one anchor filter state and a ring of the last 12 measurements per
  track and rebuilds the state at a late measurement by applying the ring's measurements to the anchor; on
  three aircraft whose reports arrive scrambled by up to 14 positions, with wild positions and exact and
  different-sensor repeats, every output bit and every counter equals a reference that stores a
  full filter state in each ring slot and uses the same filter equations. Historical pins were
  recorded again at OA-TRK-077 and OA-TRK-078, which changed five duplicate counts; changes to
  prediction equations require a fresh comparison with that full-state reference.
- **OA-TRK-041** A tracker filled to `MAX_TRACKS` aircraft with more reports each than the ring holds
  refuses the next aircraft under its own counter, re-filters a late report of a track in the full table,
  and reallocates no buffer.
- **OA-TRK-042** Tracker memory is linear in the track capacity and under 2.2 kB per track (4,096 tracks:
  8.8 MB; 38,000 tracks: 81.7 MB; it was 8.9 kB and 339 MB with a filter state in every ring slot).
- **OA-TRK-043** The chunked benchmark's millisecond histogram gives the statistics of the values it was
  made from: count, median, p95, p99 and maximum exactly, mean and RMS to rounding (bit for bit for whole
  seconds), and adding two histograms gives the histogram of the union.
- **OA-TRK-044** Scoring a tracker's output in address ranges (one aircraft per range, a few, or all)
  gives exactly the metrics, exclusions, pair count, association counts and ESASSP-style split of scoring
  it in one piece with `run_benchmark`; the ranges tile the address space.
- **OA-TRK-045** A run whose output is written to column files as it is produced has the digest of the
  same rows held in memory, the same counters and the same rows and scores when read back in ranges, and
  removes its files when dropped.
- **OA-TRK-046** The latency stages are stamped from an injected clock: with a fixed-step clock every
  stage difference, percentile and the capacity overflow count are exact, a clock that goes backwards is
  counted, and the tracker's output is the output without a clock.
- **OA-TRK-047** `oa-track-run --no-score` runs the tracker core and the latency replay without truth or
  scoring, writes `core.txt` with throughput with and without the one-second output, per-stage latency,
  and resident memory per phase, and the replay's output digest equals the scoring run's.

### Radar fusion with registration (oa-track: radar, assoc, registration, fusion; study binary oa-track-fusion)

- **OA-TRK-030** A radar plot (slant range, azimuth, Mode C altitude) becomes a position in the track's local
  plane by the exact inverse of the WGS-84 east-north-up forward model the simulator uses (spherical start
  for the elevation, Newton on the target height), earth curvature and height included; the result inverts
  the forward model to within a millimetre and agrees with `oa-sensorsim`'s forward model.
- **OA-TRK-031** The plot covariance is first order in range, azimuth and height (along-range from range and
  height error, cross-range from azimuth times the ground range) with the range and altitude quantisation
  variance added, and matches a Monte Carlo of the conversion.
- **OA-TRK-032** Height comes from Mode C, else from the track the plot is tested against, else the plot is
  refused; Mode C altitude carries its quantisation noise.
- **OA-TRK-033** A registration bias is subtracted from the raw plot and its variance added to the plot's.
- **OA-TRK-034** An invalid site or plot (non-finite, out of range, zero period) is refused with a named
  error and never converted.
- **OA-TRK-035** The assignment of plots to tracks within a gate is solved exactly (components of the gate
  graph by union-find, a padded square problem per component, Hungarian method with potentials) and equals
  an exhaustive search on random instances; nearest neighbour is worse where two plots want one track;
  components above 160 fall back to greedy and are counted.
- **OA-TRK-036** A pair at or above the gate is never assigned; a plot whose assigned track has a competitor
  within the ambiguity margin is marked ambiguous.
- **OA-TRK-037** The per-radar range and azimuth bias estimate converges to the true bias with an uncertainty
  that is honest (the error is within 3 sigma over repeated runs).
- **OA-TRK-038** A residual beyond 4 sigma is rejected and the estimate is clamped to its limits.
- **OA-TRK-039** A reference with a larger position uncertainty moves the estimate less.
- **OA-TRK-048** Radar-only initiation needs 3 plots in 4 scans of one radar, consistent in position,
  speed, acceleration and altitude; one or two plots never make a track.
- **OA-TRK-049** Two isolated false plots never make a track.
- **OA-TRK-050** A Mode A code alone never assigns a plot, on any track.
- **OA-TRK-051** A plot carrying a Mode S address goes to the track keyed by that address (created if need
  be) and to no other, whatever the kinematics say.
- **OA-TRK-052** No decision reads the simulator's truth label: scrambling every label leaves the tracker's
  output byte-identical.
- **OA-TRK-053** Two tracks crossing at one altitude are held (the plot is not applied) while an assignment
  is ambiguous; identity is never swapped on kinematics alone.
- **OA-TRK-054** Registration learns a radar bias from ADS-B tracks of NACp 9 or better (at least 3 updates,
  newest at most 5 s old, predicted position sigma at most 15 m, altitude known, range at least 5 km),
  applies it to radar-only plots and publishes per-radar estimates over time. Invalid configuration and
  input are refused.
- **OA-TRK-055** Synthetic truth is exact by construction: a smooth analytic path (straight legs, turns,
  climbs) written as one-second knots, interpolated by `oa_truth::truth_at` within a centimetre of the
  analytic path.
- **OA-TRK-056** Synthetic paths are ordered and bounded (addresses, legs, times); invalid input is refused.
- **OA-TRK-285** A synthetic path is refused when its start or any knot position is not finite or lies more
  than 20,037,508 m (half the Earth's circumference) from the plane origin, or when an altitude or vertical
  rate is past 204,800 ft or 262,144 fpm; no truth is built from it.
- **OA-TRK-286** A row whose only altitude is geometric passes it flagged geometric whether the source flag
  or the geometric column carried it, so the tracker counts it as a skipped altitude either way.
- **OA-TRK-287** A radar site's scan epoch must lie in [0, 4e18] ns (1970 to about 2096); any other epoch
  is refused at configuration. A scan number is computed with checked arithmetic: a plot timed before
  the site's epoch is refused and counted with the plots refused by validation, never given a clamped
  scan number, so radar-only confirmation counts hits one rotation apart.
- **OA-TRK-288** A latency record whose receive time minus sensor time does not fit a 64-bit integer is
  refused and counted, not stored with a clamped value.
- **OA-TRK-289** The tracker configuration the fusion layer derives from a site passes the tracker's
  validation for every rotation period a site accepts (100 ms to 600 s, both ends included): the drop
  time is never below the clock-step limit it must exceed.
- **OA-TRK-290** Once the output table is full, an advance computes no further row: the rows it could not
  emit (the remaining ticks of each confirmed track, to the tick at which the track is dropped) are
  counted in the table's overflow count in one pass over the tracks, with no work per tick. One
  confirmed track, a one-row table, 100 ms ticks and an advance of one hour compute at most two rows
  (36,001 before) and count exactly the rows left out.
- **OA-TRK-291** The fusion layer's processing path (queues, ready inputs, per-radar association lists,
  events, initiation lists, candidates, track records, registration history, assignment log) runs in
  buffers made at start with named capacities and cleared and reused; a buffer at its capacity
  refuses and counts. Under three radars, registration, Mode S and radar-only plots over 384 s no
  such buffer changes address or capacity (a check on the buffers: it does not see a temporary
  allocated and freed within a call). The two per-key tables (hold runs, registration keys) are
  among the buffers checked.
- **OA-TRK-292** The fusion layer's per-key tables are fixed-capacity open-addressing hash tables
  made at start (linear probing, backward-shift deletion, a fixed hash with no seed): lookup,
  overwrite and removal agree with an ordered map; a new key into a full table is refused and
  counted and the table never grows; removal under collisions leaves every other entry findable;
  retention of a subset leaves exactly that subset. The order in which entries are evicted is the
  order the ordered maps gave.
- **OA-TRK-293** The tracker, fusion layer and their helpers call no platform maths function in
  production code (`sin`, `atan2`, `exp`, `ln`, `hypot`, `powi`, `mul_add` and the rest of a fixed
  list); the elementary functions are `oa-math`'s, so a run is the same bits on every IEEE-754
  machine (RULESET B6). The check is itself tested against a planted call.
- **OA-TRK-294** The model probabilities after an update equal the softmax of the log weights
  `ln(prior) - q/2 - ln(det)/2` to 1e-12 in every regime (large and small innovations, overflowing
  determinants, a singular model, an infinite form), the larger being exactly 1 before the floor;
  the stay probabilities of the Markov transition are `exp(-dt/tau)` for each dwell time.
- **OA-TRK-295** A filter's carried sine and cosine of its frame origin's latitude stay within 1e-13 of
  those of the origin it labels through thousands of updates, and the frame rebuilt from them
  projects as `frame_new`'s does.
- **OA-TRK-296** The `oa-track` run checks the split of the clip with `oa_bench::split_check::check_split`
  right after `split_reports`; a violation is a hard error of the run. The scorer builds the legacy table by
  truth leg (the chunk ranges are by address, so all legs of an address are in one chunk).
- **OA-TRK-297** The fusion layer records where a bucket's work goes and what its work limit leaves out: the
  tracks predicted for association, the coarse window width, the anonymous plots never gated because the
  limit was reached, the initiation tests and candidates alive at most, and (apart from the counters, in
  `FusionTimers`) the wall time of each stage. None of it is read back by the layer.
- **OA-TRK-307** Proved (Kani, `fixed_map_agrees_with_a_reference_model_2_slots`): `FixedMap` agrees with a
  reference list through any sequence of insert, get, remove and retain: no key is lost or duplicated, `len`
  is the number of held keys, an insert is refused exactly when the key is new and the table is full, and
  nothing panics or indexes out of bounds.
- **OA-TRK-308** The scoring split treats reports of one address and time that differ in any field scoring or
  quality handling reads (`oa_bench::split_check::observation_key`: flags, source, category, SIL type among them)
  as different observations kept together on one side, in key order; only exact copies are removed. Reversing the
  input rows changes neither the retained evidence nor the scores.
  limit was reached, the coarse visits, exact tests and initiation tests, the candidates alive at most and the
  buckets processed. They are counters only (the layer reads no clock, RULESET B2); none is read back.
- **OA-TRK-298** WITHDRAWN: proximity-based candidate merging and cancellation conflicts with E7.
  Replaced by OA-TRK-323 and OA-TRK-324; anonymous hypotheses stay separate and visibly unidentified.
- **OA-TRK-299** A plot the bucket's work limit left ungated is deferred: it is not unassigned, so it starts
  no candidate and joins none, and `anon_plots_ungated` counts it. The association loop starts at a plot that
  depends deterministically on the bucket and the radar, so the plots left out are not always the same
  sector of the scan; when nothing is cut the result does not depend on where the loop started.
- **OA-TRK-300** The coarse window of an association is per track: a track whose window (the coarse
  margin plus eight standard deviations) is above the standard search radius is on a wide list of fixed
  capacity that every plot also tests, and the grid is searched at the standard radius, so one uncertain
  track does not widen the search of every plot. The tracks tested for a plot are every track the single
  widest window would have tested that could pass its own window. A track that finds the wide list full is
  left out of that association and counted in `wide_full`.
- **OA-TRK-301** Initiation tests an anonymous plot against the anonymous candidates near it, found in a
  grid of the candidates by predicted position (cells and a wide list of fixed capacity for the candidates
  whose reach exceeds the standard one), and converts the plot once. The candidates tested for a plot are
  every candidate the all-candidates test could pass; a candidate that finds the wide list full is not
  tested in that bucket and is counted in `init_wide_full`; `init_tests` counts the exact tests and
  `init_visits` the candidates the grid returned.
- **OA-TRK-302** A track's expected update interval is the shortest scan period among the radars that
  updated it within their last three periods (as of its newest update), and it is flagged coasting when
  its newest update is older than three of those intervals; a track no radar updated that recently keeps
  three of the configured expected interval (the longest scan period). The time to drop a track is
  unchanged.
- **OA-TRK-303** A candidate takes no hit later than its window: a candidate whose first hit is more than the
  initiation window of scans before a radar's plots of the bucket is expired before those plots are
  offered to it, whether or not the radar delivered plots in the buckets between.
- **OA-TRK-304** WITHDRAWN: fresh-track proximity does not prove ownership of a free plot.
  Replaced by OA-TRK-323; the plot remains available to its own candidate.
- **OA-TRK-305** WITHDRAWN: exactly one track per anonymous aircraft assumed proximity established
  identity. OA-TRK-323 retains source hypotheses; tracking quality reports their duplicates explicitly.
- **OA-TRK-306** The grid of the coarse window has cells about 5.6 km on a side everywhere on earth (bands of
  0.05 degrees of latitude, each cut into as many cells of longitude as keep it that wide at its
  edge nearest a pole), and the window's rectangle is cut into pieces of at most 20 km, each with a box of
  its own, so a plot visits few tracks. The candidates hold every track the window passes at the poles, on
  the seam and at the equator.
- **OA-TRK-311** A radar-only track keeps its aircraft through the onset of a climb: a level aircraft that
  starts to climb between two scans does not start a second track (the altitude gate of the association is
  widened by a standard deviation of `GATE_VRATE_SIGMA_FPM` of climb rate over the time since the update, at
  most `GATE_MANEUVER_MAX_DT_S`).
- **OA-TRK-312** A track position that cannot be represented (not a number, past the projection's chart,
  or not within an `i32` of 1e-7 degree) is never a coordinate: a tick or update row for it is not emitted
  and is counted in `TrackerStats::rows_unrepresentable`, and a coordinate conversion in the tracker, the
  radar conversion and the scorer refuses such a value instead of saturating it to an integer (a row at
  0, 0 degrees).
- **OA-TRK-313** A plot at the antipode of a filter's frame, where the chart has no differential, is a
  counted position jump (`jump_rejected`), leaves the filter unchanged, and never puts NaN in a state; the
  tracker uses the `try_*` forms of the frame functions of `oa_geo::aeqd` and handles `None`.
- **OA-TRK-309** The allowance for a change of climb rate widens the gate and not the cost: a pair inside the
  gate only by the allowance costs `RESCUED_COST_FRACTION` of the total gate, so it is assigned only where no
  better pair wants either side, and the allowance stops growing at `GATE_MANEUVER_MAX_DT_S`.
- **OA-TRK-310** A plot placed under the vertical (its range under its altitude by less than the slack, so the
  point is over the antenna) converts to a position covariance the tracker accepts: where the covariance is a
  line (the determinant under `COV_LINE_RATIO` of the squared trace) its diagonal is raised by that fraction of
  the trace, so the plot is applied and not refused for a non-positive-definite covariance.
- **OA-TRK-323** Anonymous radar candidates retain their own source-local track hypotheses at confirmation.
  Proximity to another radar's track and equal, absent or conflicting Mode A codes never cancel a
  candidate, suppress its free plots or establish duplicate aircraft identity.
- **OA-TRK-324** Every emitted or predicted row keyed in the reserved radar-only namespace carries
  `O_IDENTITY_UNCONFIRMED` (bit 16): the key is a local tracking handle and may describe the same aircraft
  as another hypothesis. Full aircraft addresses do not carry that flag solely for being addresses.
- **OA-TRK-325** Initiation has a named work budget shared by all radars of a bucket. Candidate
  visits, address lookups, index construction/query, exact tests and retirement are charged before
  their work; exhaustion preserves unvisited candidates, explicitly refuses omitted plots and
  restores scratch buffers. Partial plot pairs never reach the assignment solver.
- **OA-TRK-326** Aircraft addresses offered to fusion or radar validation fit 24 bits plus the non-ICAO
  flag (at most `0x01ff_ffff`), or the radar's absent-address marker. An address outside that domain is
  refused and counted before it can enter a pending queue or claim a reserved local radar key.
  Local key exhaustion is refused without wrapping, issuing the absent marker or reusing a key.
- **OA-TRK-327** A standalone, isolated allocation check runs real tracker and fusion entry points
  after initialization with an observing global allocator. Its three allocation API controls
  must be observed, every claimed route has explicit exercise evidence, and any observed
  allocation, counter overflow, failed control or missing route fails the check. Results name
  the exercised scope and do not imply coverage of every possible input or real-time crate.
- **OA-TRK-328** Every fusion-study RMS row reports the sample count for each position, speed
  and heading class. The candidate evaluator refuses missing, empty, malformed or inconsistent
  class counts. Different variants' emitted primary-track cohorts remain explicit; adding counts
  never changes the metric's samples, values, thresholds or truth classification.
- **OA-TRK-329** The coordinated-turn predictor computes the state mean from ten equally weighted
  spherical-radial points of the stored five-dimensional covariance. A second ten-point set
  applies the existing turn-discovery variance floor only to the predicted uncertainty: its
  second moment is centered on the first set's predicted mean, then existing process noise is
  added. The discovery regularization cannot shift that mean. The deterministic motion map and
  initial turn-rate prior are preserved. Fixed-size square-root factorization accepts positive
  semidefinite covariance, including exact zero directions, and refuses non-finite, asymmetric
  or indefinite inputs without jitter or a fallback model.
- **OA-TRK-330** A failed horizontal numerical prediction refuses the measurement, preserves
  the prior filter state and increments a named numerical-refusal counter. Unavailable predicted
  output is explicit and counted when an output row is withheld; a numerical refusal prevents
  a capacity or synthetic-accuracy pass.
- **OA-TRK-331** Numerical refusal while advancing a history anchor or replaying a late update
  leaves the per-track state, history, category and output unchanged. The incoming update is
  counted once as numerically refused; no partial replay or uncounted history loss is committed.
- **OA-TRK-332** Read-only forecasts distinguish an absent track from numerical or coordinate
  failure. Fusion counts failed eligible forecasts as loss, including association and registration
  queries; an ineligible tentative hard-address track is classified before forecasting. A failed
  forecast never establishes that an existing key has disappeared.
- **OA-TRK-333** Starting with `output_new` and using the output append/clear operations,
  every stored row fills all fifteen columns, preserves prior rows bit for bit (including
  floating-point payloads), and keeps their lengths equal and within the configured capacity.
  A full buffer preserves every column and saturating-increments its overflow counter.
  Clearing empties every column while preserving the configured capacity, accumulated overflow
  and each column's reserved capacity.
- **OA-TRK-334** For a per-track report decision whose resulting track is confirmed, an applied,
  begun or resynced report makes exactly one non-tick row attempt at that report's time and full
  track address. Each attempt is accounted as a stored row, an explicitly unrepresentable row,
  or output-capacity overflow. A rejected or unconfirmed report makes no report-row attempt.
  During late replay only the newly inserted report may produce a report-time row; later
  replayed history entries do not create backfilled report rows. Tick output is accounted
  separately. Public lookup, clock admission and held-release behavior retain their own checks.
- **OA-TRK-335** A numerical refusal inside a per-track update or late replay restores every
  saved track-transaction field, history entry, prior output column and overflow value, including
  refusal after a partial replay emitted output. The saved statistics are restored except for
  one saturating numerical-refusal increment. Event admission and any clock/tick work preceding
  that transaction remain recorded. No partially replayed state or row is committed.
- **OA-TRK-336** At the production sensor-table capacity, live-sensor counting stays within the
  table bound and cannot increase when only the clock advances with the same nonnegative drop
  interval. Updating an existing sensor preserves its maximum accepted timestamp; inserting a
  new sensor into a full table replaces the first least-recent slot and saturating-increments
  eviction exactly once, preserving every other entry. Proof results state their fixture,
  timestamp domain and capacity explicitly; they do not imply whole-clock-policy correctness.
- **OA-TRK-318** The coarse query lists each track at most once as it appends, so its output never
  holds more entries than the index has tracks and a buffer sized for the tracks is never grown, whatever
  the pieces of the rectangle share (tracks on the boundary of two pieces included); the tracks listed
  are the same as when the output was sorted and deduplicated afterwards.
- **OA-TRK-057** The base world has 44 aircraft: 20 straight, 12 turning (1.5, 3 and 6 deg/s, both
  directions), 12 climbing or descending at 1,500 fpm, with no same-altitude approach inside 5 NM.
- **OA-TRK-058** Every pair geometry (crossing and parallel at 3 NM, crossing and same-track at 1,000 ft, and
  the stress geometries) has its stated closest approach.
- **OA-TRK-059** Every world builds into an exact truth that `oa-bench` accepts.
- **OA-TRK-060** Scoring a copy of the truth through `oa-bench` metrics gives no error beyond the 1e-7 degree
  representation; the same copy shifted 100 m scores 100 m; straight and turning rows are both populated.
- **OA-TRK-061** Identity accounting from simulator labels counts swaps (a label change within a track),
  wrong plots, false plots applied, false tracks (false plots at least as many as true) and duplicate tracks
  on hand-made logs.
- **OA-TRK-062** The simulated ADS-B has the NACp 10 error (a 2-D RMS of 5.8 m), 25 ft altitude steps,
  millisecond stamps, only equipped aircraft, and is the same for the same seed.
- **OA-TRK-063** A run is reproducible (output digest, assignments, counters). Radar-only, on a small world
  with the MSSR profile: every aircraft is tracked, no swap, no false track, no duplicate, and the position
  error of the track is below the raw plots' error against the same exact truth.
- **OA-TRK-064** Under a 150 m and 0.15 degree bias the registration estimate is within 20 m and 0.03 degree
  of the bias, radar-only aircraft score more than 40 % better with registration than without, and
  ADS-B-equipped aircraft are not made worse.
- **OA-TRK-065** `oa-track-fusion --out <dir>` writes `report.txt` and `tables.csv` stating the truth, its own
  error (zero), the baselines on identical plots and each table's ESASSP bars; usage errors exit 2.
- **OA-TRK-066** An output row without an altitude has NaN in the altitude, altitude variance and vertical
  rate columns (not 0 ft) and the flag clear; the scorer takes the altitude through
  `scored_altitude_ft`, which gives the absent value for a clear flag or a non-finite column; the
  output digest hashes absent columns as zero, so it equals the earlier digest of the same run.
- **OA-TRK-067** The tracker's plausibility check on the second report of a tentative track is the speed
  rule `oa_adsb::jump_verdict` (category, ground speeds, hard cap), not a fixed 700 kt: a 950 kt
  aircraft reporting 950 kt is not restarted, a 300 kt one that moves as far is, and one report
  printing a fast category does not excuse a 2,782 kt displacement. The category is the track's
  trusted one under `oa_adsb::category_observe`. The output digest of the ZAU clip is unchanged.
- **OA-TRK-068** A measurement more than the step limit (10 s, the out-of-order window, a configured
  value) ahead of the tracker clock does not move the clock or touch any track until a run of three in a
  row, none earlier than the one before by more than the window, confirms it (the others are counted as
  held and applied after the step; see OA-TRK-191, OA-TRK-192, OA-TRK-199); a confirmed step, and `tracker_advance` over a long gap, emit ticks
  only while a track could still be live and jump the tick grid over the rest in one sweep, counting the
  skipped ticks. `tracker_advance` is not subject to the limit (its caller, the replay clock, has its own
  confirmed rules).
- **OA-TRK-069** A time of applicability or an advance beyond about 146 years either side of the epoch
  is refused and counted under its own reason, and a tick must be between 100 ms and a quarter of the
  range, so rounding to a tick and stepping from tick to tick cannot overflow or stop advancing.
- **OA-TRK-070** The assignment solver refuses a gate that is not a finite positive number, with a typed
  error and every plot left unassigned (a NaN or infinite penalty makes the padded problem meaningless);
  the same solver then works on a valid gate.
- **OA-TRK-071** The assignment (union-find, component grouping, Hungarian method, greedy fallback, the
  pair lookup and the ambiguity pass) runs in buffers allocated at start: their addresses and capacities
  are the same after solves of every shape, and an input with more tracks, plots or pairs than the buffers
  hold is refused with a typed error (counted by the fusion layer), never grown into. Fusion's own
  association buffers are likewise unchanged across a run with holds and forced resolutions.
- **OA-TRK-072** A plot with a Mode S address is applied to the track of that address only inside a wide
  gate (chi-square, 3 degrees of freedom, tail about 8e-11); a gross mismatch is counted and not applied. A
  track takes at most one plot per radar and bucket: of several Mode S plots the nearest, and no anonymous
  plot once a Mode S plot has taken the track.
- **OA-TRK-073** A flooded bucket costs a bounded amount: plots of one radar beyond 16,384 in a bucket,
  pairs beyond the pair limit and exact gate tests beyond the test limit are counted and not processed;
  the ambiguity and pair-lookup passes are one pass over the pairs, so 20,000 plots over 20,000 tracks are
  associated in under 3 s in a debug build (the quadratic scans took over 3 s of that alone and grow with
  the square), and a call that finds no input ready does not scan the queue.
- **OA-TRK-074** Radar registration uses a reference aircraft only after it has made several residuals and
  at least four distinct references are current, and only when its mean residual agrees with the median of
  the others'; a few references that misreport their position (two of twelve, 250 m) do not move the
  estimate, which stays within 10 m and 0.015 degree of the truth, and two references alone teach nothing.
  Through the fusion layer, two of eight ADS-B aircraft displaced 200 m leave the estimate within 20 m.
- **OA-TRK-075** A step change of a radar's bias (a realignment, 50 m and 0.05 degree to 200 m and 0.2
  degree) is followed within a few scans of the references: a CUSUM over the residuals that passed the
  agreement test restarts the estimate at their mean, counted and published with each estimate; an honest
  steady radar over 9,600 residuals is never restarted.
- **OA-TRK-076** Two aircraft flying together 90 m apart, every plot of which is ambiguous, are not starved:
  a track whose last two plots were held takes the next on the assignment's evidence (counted), where the
  plain hold applied none of the 74; both tracks are confirmed and no third track is made.
- **OA-TRK-077** A report is a duplicate when any report in the track's ring has its time and sensor, not
  only the newest or the one just before it: a repeat of one sensor's report after another sensor's report
  at the same instant is counted and not applied again, in order and out of order.
- **OA-TRK-078** A late report applied by re-filtering keeps the track's run of rejected reports when the
  replay itself had no rejection, restart or resync (OA-TRK-231), so the rejection that would have
  re-initialised the filter still does.
- **OA-TRK-079** A spill refuses a directory that exists and has anything in it, creates its column files
  new, and on drop removes only those files (by name) and then the directory if it is empty; a file put
  into the directory meanwhile survives.
- **OA-TRK-280** A spill that fails partway through creating its column files removes only the files that call
  created, and the directory only if that call created it: files another writer made in the same directory
  survive.
- **OA-TRK-080** Reading a spilled run checks every column file's size against the row count and refuses a
  truncated or extended column of any of the 15, with a typed error naming the column and the bytes found
  and expected.
- **OA-TRK-081** A plot in the overhead cone (within about three degrees of the vertical, where the
  height barely depends on the elevation) is converted, by bisection on the elevation, to within 5 mm of
  the point it was made from; a plot whose altitude is higher than its range reaches by up to 500 m (the
  range error of a radar) converts to the point under the vertical, and only a larger discrepancy is
  refused, with its own error.
- **OA-TRK-082** Time passes for the tracks when every sensor is quiet: advancing the fusion layer to a
  time with nothing pending still runs the tracker to the end of the last completed bucket, so a track
  with no reports gets its fixed-rate rows, is marked coasting and is dropped at the drop time.
- **OA-TRK-083** The antipode of a frame's origin is not projected onto the origin: it is placed half the
  sphere's circumference away, so a report of a position on the far side of the earth is a jump the
  tracker refuses, not a coherent update.
- **OA-TRK-084** The fusion layer's bookkeeping does not grow with the run: the per-address registration
  table is cut back to the tracks the tracker holds whenever it reaches the tracker's capacity (an
  address the tracker refused is not recorded), and the radar-only track records stop at a limit, counted,
  and can be drained by the caller.
- **OA-TRK-085** Every live track up to the configured capacity takes part in association: the
  association's limit is not below the tracker's capacity, and a tracker configured for more than the
  limit is refused when the fusion layer is made.
- **OA-TRK-086** The digest of a spilled run equals the digest of the same rows in memory when rows have
  no altitude: the altitude columns of such a row hash as zero in both.
- **OA-TRK-087** A report the tracker did not take (a repeat, or one refused for its covariance) does not
  count toward a track's eligibility as a registration reference.
- **OA-TRK-088** A bucket already processed is not processed again: inputs for it that arrive after the
  advance past it are counted late and dropped, so batching the calls does not change what is
  associated. An advance closes every bucket before its time, whether or not the bucket held an
  input.
- **OA-TRK-150** A report of a track whose newest report gives no ground speed (no category, a radar-only
  track) is judged against the filter's own estimated ground speed once the track has two updates, not
  against the 2,000 kt ceiling of "no category": a track steadily flying 300 kt is restarted by a report
  displaced as a 1,500 kt move, a 900 kt track is not restarted by its own next report, and a track with
  one update (no estimate yet) is judged as before.
- **OA-TRK-151** The matrix solve of the assignment breaks an exact cost tie by the lowest track index: two
  tracks equally good for one plot give it to the lower, whatever the order of the pairs.
- **OA-TRK-160** A report received before its sensor time is counted as clock skew
  (`LatencyLog::sensor_negative`); its signed difference is stored as it is.
- **OA-TRK-170** The runs of the tracker and of the fusion layer on the real ZAU ADS-B clip (radar plots
  simulated from its trajectories, one and two radars, with and without ADS-B and registration) have
  fixed output digests: a change of how work is found or bounded does not change what is decided.
- **OA-TRK-171** The tracker run on the same clip, at report times, has a fixed output digest.
- **OA-TRK-172** A radar plot whose time is outside the tracker's own limit on a time of
  applicability (about 146 years either side of the epoch) is refused by validation and counted with the
  other refused plots; plots at the smallest and largest times and one past the limit either way are
  refused, and no time difference taken of a plot's time can overflow.
- **OA-TRK-173** The tracker keeps the newest update time any track has had and uses it, not a scan of
  every slot, to jump over the ticks that no track could still use: the rows of a run with drops and gaps
  of hours are those the full scan gave, bit for bit, and the kept time is never older than any live
  track's.
- **OA-TRK-174** A radar's registration estimator has a configured number of reference slots (256 by
  default, allocated at start). A reference keeps its slot while its aircraft makes residuals and gives it
  up only after six scans without one; an aircraft that finds every slot held is ignored and counted. With
  300 aircraft eligible in every scan registration starts within a few scans and the estimate is as good as
  with twelve.
- **OA-TRK-175** The run of plots held for ambiguity is kept per track and radar, so each of several
  overlapping radars holds its own plots before one is applied on the assignment's evidence; a full table
  drops the entry touched longest ago (the lowest key among those) and counts it.
- **OA-TRK-176** A track is marked (a flag on its output rows) while a Mode S plot of its address has
  fallen outside the wide gate of its position within the last three scans of a radar, and the mark is
  counted; the position, the filter and the assignment are not changed. An ADS-B track 5 km from where two
  radars see its Mode S plots carries the flag, an honest one does not, and the flag clears after three
  scans without a contradiction.
- **OA-TRK-177** The coarse window of a plot is applied only to the tracks in the latitude and longitude
  grid cells it can reach, in ascending track order, and finds exactly the associations that testing every
  track finds (across the +-180 degree seam and at the poles); 10,000 plots over 20,000 tracks associate
  in seconds in a debug build; candidates visited are charged to the bucket's work limit and counted; when
  the limit is reached the radars of a bucket have each had an equal share, so one radar's flood does not
  leave another radar's plots ungated.
- **OA-TRK-190** The step limit is a configuration value (`clock_step_limit_ns`, default the
  out-of-order window); a configuration with a limit below the window or not below the drop age is
  refused.
- **OA-TRK-191** No single measurement moves the tracker clock by more than the step limit once tracks are
  live: one measurement 299 s ahead is held, the clock does not move and no track is swept; repeated
  measurements 299 s, 598 s and so on ahead from one address and one sensor never move it, and the real
  measurements that follow are all applied.
- **OA-TRK-192** A measurement more than the limit ahead is held unless it completes a run of three
  consecutive far-ahead measurements (each no earlier than the one before by more than the window) from at
  least min(2, live sensors) distinct sensors and min(3, live tracks) distinct addresses, the run kept in a
  bounded buffer of the last 8; with no live track, or with every live track its own address and every
  live sensor its own sensor, it steps at once (nothing else is at stake). A run from one sensor does not
  step while two are live and steps when a second joins; a whole-feed pause of 30 s steps after three
  measurements and the tracks continue; one aircraft from one sensor after a 15 s gap steps at once.
  A live sensor is one with a measurement accepted within the drop age of the clock, kept in a table of 64
  (a full table reuses the least recently accepted place and counts it).
- **OA-TRK-193** Proved (Kani, `gate_decide_is_total`): the clock gate is total and free of overflow and
  out-of-bounds access for every time (the clock `i64::MIN` included), address, sensor and track count.
- **OA-TRK-194** Proved (Kani, `gate_step_forward_needs_a_confirmed_run`): with live tracks, a far-ahead
  measurement steps at once when only its own track and sensor are live, and otherwise only as the third of
  three consecutive ones from the distinct sensors and addresses of OA-TRK-192.
- **OA-TRK-195** Proved (Kani, `gate_step_back_needs_span_count_and_spread`): a step back needs at least six
  late refusals with no accepted measurement between, spanning at least the out-of-order window of their
  own times, from at least min(2, live sensors) sensors and min(3, live tracks) addresses, and moves the
  clock to the newest of them; a sensor counts as live for this only if its newest accepted time is no
  later than the run's newest time plus the window (a sensor live only because of data ahead of the real
  time must not raise the bar for the real ones).
- **OA-TRK-196** A tracker clock moved ahead of real time (a confirmed step from colluding sensors) comes
  back: after a run of late refusals as in OA-TRK-195 the clock takes the newest late time, the tick grid
  restarts after it, every track updated after the new clock is dropped and counted
  (`time_future_dropped`), the step is counted (`time_steps_back`), the measurement that completed the run
  is applied at the new clock, real tracks are rebuilt and output continues. Output rows after a step back
  carry earlier times than rows before it.
- **OA-TRK-197** A stale batch from one slow sensor while another delivers in-window data never steps the
  clock back (an accepted measurement ends the run of late refusals).
- **OA-TRK-198** Proved (Kani, `gate_accept_is_within_window_and_limit`): an accepted measurement is never
  more than the step limit ahead of the clock nor older than the out-of-order window.
- **OA-TRK-199** Confirmation delays measurements, it does not lose them: the far-ahead measurements of a
  run (the last 8, kept in a buffer allocated at construction) are applied in arrival order at the new
  clock, before the measurement that completed the run, each through the track's own ordering (in order,
  duplicate, late), and counted (`time_held_applied`); a run that is refuted (an accepted measurement, a
  step back, or a break in the run) discards its held measurements, which stay counted in `time_held`. One
  aircraft from one sensor at 17 s intervals has every report applied and none held; two aircraft reporting
  together 17 s apart on one sensor lose no report.
- **OA-TRK-200** Sensors live only because of data ahead of the real time (a forged clock's) do not lock a
  real sensor out of the step back: two colluding sensors that put the clock ahead, then one real sensor
  with many aircraft, step it back after the run.

- **OA-TRK-230** A report applied late (older than its track's newest update, inside the window) is decided
  again with the stored reports after it, in time order, through the decision an in-order report gets
  (initiation window, plausibility, position gate, rejection streak, resync): the track's hits,
  confirmation, restarts, filter state and stored history equal those of the same reports arriving in time
  order. A far, huge-covariance late report cannot confirm a tentative track that in-order arrival restarts.
- **OA-TRK-231** The rejections, rejection streak and resync that the replay of a late report produces are
  kept (a streak of 3 and a resync at the fifth rejection equal those of in-order arrival); the live streak
  is put back only when the replay had no rejection, restart or resync (OA-TRK-078).
- **OA-TRK-232** The tracker's velocity limit is the supported speed envelope of `oa-adsb`
  (`SPEED_ENVELOPE_KT`, the highest category cap, 4,088 kt): one definition, no limit of the tracker's own.
- **OA-TRK-233** A velocity up to the envelope is accepted (2,916 and 3,000 kt with and without velocity pass
  alike); one above it is refused as a bad velocity and counted, and no track is made from it.
- **OA-TRK-234** Exact repeats (same time, address and sensor) of a far-ahead or a late measurement do not add
  members to a run, in the gate or in the held buffer: three repeats of one far-ahead report do not step the
  clock, with other tracks live or with one track live that another sensor delivers.
- **OA-TRK-235** When only the measurement's own track and sensor are live, the first far-ahead report steps
  the clock at once and its exact repeats add nothing (counted as duplicates, none applied or held).

- **OA-TRK-210** A radar plot's position covariance is in the target's own east-north frame: the range,
  azimuth and height errors are linearised exactly (the elevation adjusting to hold the height) and rotated
  into the target's local frame, so the ellipse is oriented along the true range axis at any latitude and
  bearing, and matches a central finite-difference Jacobian of the conversion to 1e-6 of its trace at sites
  from the equator to 85 degrees and targets 50 to 600 km away.
- **OA-TRK-211** At 80 N and 500 km east, where the meridians have converged by 24 degrees, an elongated
  plot covariance accepts a residual along the range axis and rejects the same residual across it.
- **OA-TRK-212** The simulated ADS-B noise of a sample depends only on the seed, the address and the
  sample's time, so the two legs of one aircraft do not repeat each other's draws and a simulation of a
  subrange reproduces exactly the samples of the full run that it covers.
- **OA-TRK-220** A pole inside the coarse search rectangle (judged in the rectangle's plane, with a margin)
  makes the grid take every longitude up to that pole, so a track beyond a pole that the window passes is
  a candidate at any index size, including where the sampled points are all far from the pole.
- **OA-TRK-221** At 80 N with the plot 1,100 km north, tracks in rings of 3 and 10 km round the pole are
  all candidates the window test can pass, at index sizes either side of the scan fallback.
- **OA-TRK-222** When the visit budget is shorter than the candidates, the nearest by coarse distance
  (then lowest track number) are kept, not the lowest numbered; the plot is still marked capped.
- **OA-TRK-250** A fusion configuration whose radars share a sensor id is refused, because the tracker
  identifies a report by time and sensor id and would drop the second radar's report as a duplicate. The
  study radars carry distinct ids (terminal 1, en route 2); with distinct ids every report of both radars
  is offered and none is dropped as a duplicate.
- **OA-TRK-251** The ADS-B simulation refuses, before allocating, a reporting period that is zero or
  negative, a NACp or NACv without a bound, and a run of more than 2^25 samples (a 1 ns period over seconds);
  the study's one-hour clip at one report per second is 657,824 samples and runs.
- **OA-TRK-252** A synthetic truth refuses a path whose duration is too long to count (including an
  infinite sum), a start time that overflows the clock with the epoch, and an origin that is not a finite
  latitude and longitude, with an error and not a panic.
- **OA-TRK-253** Interval statistics of measurements at the extremes of the clock, and the fusion tracker
  configuration for a radar period at the largest value, saturate and do not overflow.
- **OA-TRK-254** A tracker configuration whose output or log table exceeds 2^24 rows is refused; the
  registration estimator and the association scratch size their buffers at a stated maximum (4,096
  reference slots; 2^22 tracks, plots or pairs) whatever is asked for.
- **OA-TRK-270** Reports of one address with the same time and the same position, altitudes, velocity
  and quality fields are one observation: one copy is kept, the removed copies are counted and shown in
  the report. The split of [r0,r0,r2,r2,r4,r4] has no observation in both sets.
- **OA-TRK-271** Duplicating a whole recording gives the same two sets and the same split counts except
  the number of copies removed.
- **OA-TRK-272** Reports of one address and time with different content are a conflict: all stay on the
  side of the first, and the conflicting times and reports are counted. A row that cannot be read is
  left out and counted.
- **OA-TRK-273** The per-flight position RMS is per truth leg: two trips of one address are two flights,
  so a failing short trip is not pooled with a good long one and counts above the R4 value.
- **OA-TRK-260** A plot's position covariance, given in the target's own east-north axes, is mapped into
  the filter's plane as `J R J^T` (J the differential of the projection at the plot) before the gate and
  the update: at 89.99 degrees, 0 to 90 degrees east, the normalised innovation equals the one computed
  with an independent finite-difference J, and at 45 degrees likewise.
- **OA-TRK-261** A known anisotropic physical ellipse, position and velocity, is the same physical
  ellipse and velocity in east and north after the frame is re-centred, 1.6 km away near either pole and
  at midlatitude (within 0.3 %).
- **OA-TRK-262** A change of frame carries every model's position mean, velocity mean and the whole
  state covariance (position and velocity blocks and their cross terms) by the differential of the
  change of frame; a quarter turn exchanges the east and north blocks and cross terms.
- **OA-TRK-263** A track's output position and velocity covariances are in east and north at the output
  point: a track started from a plot returns its covariance and velocity at the start, at 45 and 89.9999
  degrees and in the south, to 1e-4.

## Speed and capacity harness (oa-perf)

- **OA-PRF-001** Percentiles are by the nearest-rank rule (the 99.9th of 1,000 values is the 999th),
  with weights (a call that processed n inputs counts n times) and in a fixed-bin histogram whose
  maximum, mean and count are exact; a key=value file round-trips.
- **OA-PRF-002** The synthetic traffic is a pure function of its seed and the second asked for: the
  same seed gives the same aircraft, ADS-B reports and plots, another seed does not; one ADS-B report
  per equipped aircraft per second; plots at the scan rates of the radars (12 s en-route, 4.8 s
  terminal), in time order, inside the second asked for, and valid for the tracker.
- **OA-PRF-003** A capacity run feeds the traffic to the fusion layer one second at a time, times each
  advance, and reports the time per second of traffic, the per-advance distribution, every capacity
  counter, the data age of the rows, the input-to-effect delay and the memory; two runs of one
  configuration count the same things.
- **OA-PRF-004** A run that overfills the tracker is refused and counted, not stopped: the refusal
  counters move and the buffers are the same at the end as at the start.
- **OA-PRF-005** Memory is read from `/proc/self/status` (`VmHWM`, `VmRSS`) in kilobytes.
- **OA-PRF-006** A latency pass offers every input of a time-ordered feed to the fusion layer and
  advances it to the input's time, timing each call; it reports the time of every call, of the calls
  that processed inputs and of those shared over the inputs they processed, and the tracker alone is
  timed per report.
- **OA-PRF-007** The command line refuses unknown commands, flags and values, and the overload
  comparison reads the counters of the knee run and the overload run.
- **OA-PRF-008** Every plot of the synthetic traffic is where its aircraft is at the time the plot
  carries, to within the plot noise; a revolution in which the aircraft crosses the beam's starting
  azimuth and the crossing cannot be solved gives no plot.
- **OA-PRF-009** A capacity run passes (`keeps_up`, and the strict knee with it) only if every
  measured second's advance took under 1 s of wall time and every loss-of-work counter (`loss_counters`:
  refusals, overflows, capped or ungated association work, full tables and buffers, late and dropped
  inputs, output rows lost) is zero over the measured window; a run that is fast but loses work is
  reported as lost work, not a pass. The mean and 99th percentile are reported, not tested. Every
  counter of the fusion and tracker statistics is listed as loss or as not loss.
- **OA-PRF-010** The command line refuses a `--max-wall` that is not a positive finite number, an
  `--adsb-frac` outside 0 to 1, and traffic times (`--warmup`, `--duration`, `--clip-secs`) whose
  sum overflows or passes the time range the layer accepts, each with a message naming the flag.
- **OA-PRF-011** Every aircraft of the synthetic traffic is inside its box at every second of the fed
  interval (warm-up plus measurement, up to the traffic's horizon): a track that would leave the box flies
  the reverse heading, and one that would still leave flies straight to a point from which it turns at the
  standard rate (3 degrees per second) onto a circle that fits in the box and circles there. Velocity is
  continuous and of constant magnitude, and no aircraft turns faster than the standard rate. So in the
  stress layout every aircraft is in the coverage of every radar throughout, no plot is lost to leaving it,
  and no aircraft makes a manoeuvre a real aircraft could not fly. Deterministic per seed.

- **OA-PRF-012** A capacity run independently counts unique generated truth aircraft offered during
  measurement, using ADS-B addresses and radar scoring labels in the shell only. All configured
  aircraft must appear and unknown labels must be zero; repeated sightings count once. Tracker
  hypothesis count is diagnostic and does not establish input load or distinct aircraft identity.

- **OA-TRK-337** The standalone allocation checker ties every declared measured tracker/fusion
  route to its production source symbol and requires a positive route-specific runtime evidence
  field. Tracker fixtures include the default 4,096-track capacity followed by refusal and recovery,
  history wrap/replay and retention refusal, output/log overflow with fully preserved aligned row
  data and output reuse, and clock hold/forward/backstep with valid follow-up. Fusion seeds a
  confirmed geometry-matched key, fills one radar bucket past `MAX_BUCKET_PLOTS`, counts the
  deliberate overflow and unknown hard identities, and proves a later existing-key plot matches.
  Each armed window independently observes the injected-allocation control.
- **OA-TRK-338** A coasting track's tick row is published only while the 95% radius of its
  predicted horizontal position (the semi-major axis of the 95% ellipse of the output covariance)
  is at most 926 m (0.5 NM, the size ESASSP uses for a large position error); a row past that, or
  with a covariance that is not a number, is withheld and counted in `rows_coast_withheld`. The
  track is kept until its drop age, and its next update is published as usual. Withheld rows are
  designed behaviour, not lost work, in capacity checks.
- **OA-TRK-339** The ADS-B adapter takes the per-axis velocity noise from a configurable table by
  NACv (`vel_sigma_mps`, 1-sigma m/s; entries 1 to 4 must be positive and not a number), whose
  default is calibrated against the measured error of the reported velocity on the 2026-11-14 ZAU
  clip (`NACV_VEL_SIGMA_MPS`); the NACv-bound table (`NACV_BOUND_VEL_SIGMA_MPS`, the bound over
  2.448) remains selectable. A velocity whose table entry is not positive is not used and is
  counted.
- **OA-TRK-340** The tracker gates a measurement's velocity against the filter's predicted
  velocity: if the smallest normalised squared velocity innovation over the two models exceeds
  `vel_gate_chi2` (default 13.8155, chi-square with 2 degrees of freedom at 99.9 %), or is not a
  number, the velocity is dropped for both models and the position of the same measurement is
  applied alone, counted in `velocity_gated`. A velocity the gate passes is applied unchanged, a
  late replay takes the same decision, and the counter is designed behaviour, not lost work, in
  capacity checks.
- **OA-TRK-341** The position gate can take a manoeuvre allowance (`gate_manoeuvre_mps2`, m/s^2,
  default 0 = none): the gate decision, and only it, adds an isotropic variance of
  `(0.5 a dt^2)^2` to the innovation covariance, `dt` the time since the track's last applied
  update; an accepted update is the same as without the allowance. A single forged report 2 km off
  a steady track is refused with any allowance in use at ordinary report intervals.
- **OA-TRK-342** The tracker can re-anchor on a run of rejected reports that agree with each other
  (`resync_consistent_k`, 0 = off, else 2 to 5): after that many consecutive rejected reports,
  each plausible after the previous rejected one by the speed rule (`oa_adsb::jump_verdict`), and
  the last also plausible after the track's last applied position, the filter restarts at the
  last (counted as a resync). A run of forged reports that the aircraft could not have flown to
  since its last applied report never re-anchors the track.
- **OA-TRK-343** (supersedes the withholding of OA-TRK-338, whose text stands as the history) A
  confirmed track emits its row at every tick while it is alive, coasting or not: no row is
  withheld for its uncertainty. A row whose 95% horizontal uncertainty radius (the semi-major axis
  of the 95% ellipse of its position covariance, `r95_m_of`) is over `coast_max_r95_m` (926 m by
  default; infinity turns it off) carries `O_UNCERTAIN`, whether the track is coasting or live,
  and is counted in `rows_uncertain`. A coasting track keeps its coasting flag, so a consumer sees
  live, then coasting, then coasting and uncertain, and never an absent track.
- **OA-TRK-344** The first update row after a track was uncertain (an uncertain row was emitted
  since its last update row), after a gap of more than `reacquire_gap_ns` (15 s by default;
  `i64::MAX` turns the gap rule off) since its previous applied update, or after a re-anchor on a
  report, carries `O_REACQUIRED`; the following update rows do not.
- **OA-TRK-345** A row whose position covariance is not a number is never emitted: it is counted
  in `nan_covariance` as lost output, and, like a row not representable, in `rows_computed`, so
  that the rows computed equal the rows emitted, refused as not representable, refused for a
  covariance that is not a number, and lost to a full table. `rows_coast_withheld` is replaced by
  `rows_uncertain`, which is designed behaviour and not lost work in capacity checks. The
  uncertainty of a track coasting through a turn that began after its last report is known to be
  too small (the true error passes the radius); that limit is recorded by a test, not hidden.
- **OA-TRK-346** (supersedes the default values stated in OA-TRK-339, OA-TRK-340 and OA-TRK-341 by
  reference; their text stands as the history) The defaults are: velocity noise by NACv the
  steady-flight table (`NACV_VEL_SIGMA_MPS` = 0.6142, 0.5938, 0.5209, 0.7070 m/s for NACv 1 to
  4; the all-conditions table stays as `NACV_VEL_SIGMA_ALL_CONDITIONS_MPS`), velocity gate
  chi-square 400 (`VEL_GATE_CHI2`; the 13.8155 gate stays as `VEL_GATE_CHI2_STRICT`), position
  gate manoeuvre allowance 5 m/s^2 (`GATE_MANOEUVRE_MPS2`), consistent-run resync off
  (`resync_consistent_k` 0). Evidence: tuned on 2026-11-14 (position RMS 50.3 m with the table
  and gate 400 alone, 30.8 m with the allowance of 5, jump_rejected and resyncs cut by about
  85 %, a forged 2 km report accepted at a gap of 10 s instead of 12 s); the allowance of 5 was
  chosen on that day, before 2026-11-15 was examined. Validated on 2026-11-15 without
  re-tuning: position RMS 58.3 m (table and gate alone) against 32.0 m (allowance 5), jump_rejected
  22,624 against 3,257, resyncs 1,327 against 3; the variants with allowance 8 (31.3 m) and with
  the consistent-run resync (31.3 m) were not adopted, being chosen among on validation numbers.

## NAM ACC-to-ACC interface (oa-nam)
- **OA-NAM-001** The crate models exactly the 25 messages of the NAM Common Coordination ICD
  (NAS-IC-21009205 Rev G, Part II Table 2): FPL CHG EST CNL CPL MOD ABI MIS IRQ IRS TRQ TRS ASM RTI
  RTU RLA RTA POI PLA POA POJ TOC AOC LAM LRM; any other mnemonic is refused with Table A-1 code 60.
- **OA-NAM-002** Whatever `parse` accepts, `encode` reproduces byte for byte, and whatever `encode`
  produces parses back to the same value; where the ICD allows more than one spelling the model
  keeps which one was used. Arbitrary bytes never panic any parser.
- **OA-NAM-003** Every example message printed in Part II (sections 3.1.1 to 3.8.2) parses, encodes
  to its canonical text, and frames and unframes, each with its section and page as the citation.
- **OA-NAM-004** The tolerant parser accepts the typography of the printed examples (en dashes,
  a space beside a hyphen), counts the spaces it dropped, and refuses an example with a real
  defect (POA 3.7.3.3); the strict parser refuses any space beside a hyphen.
- **OA-NAM-005** An input that breaks a field rule is refused, never repaired, with an error that
  names the reason, the field, the offset and the Table A-1 LRM code a receiver would send.
- **OA-NAM-006** A message is 17 to 1800 bytes of printable ASCII between one pair of parentheses;
  shorter, longer, non-ASCII, or with parentheses inside is refused, and no input panics.
- **OA-NAM-007** The transport frame is a four-digit uppercase hexadecimal length followed by the
  message; partial input waits, lowercase or non-hex digits and lengths outside 17 to 1800 are
  refused, and the frame length must match the message.
- **OA-NAM-008** Field 22 amendments are accepted with or without a leading zero in the indicator,
  keep the spelling used, and obey the per-message rules (CHG and MOD change 07(a) alone; ABI
  carries fields 9 and 15).
- **OA-NAM-009** The Field 18 of an LRM is `RMK/` followed by an error code, a field number and
  text separated by oblique strokes; the LRM code tables of Table A-1 and Attachments 2 and 3 are
  data.
- **OA-NAM-010** In INACTIVE, an Activate request sends an IRQ and goes to ACTIVATING; every
  other event is ignored, and a request to send is refused (NAS-IC-82422100 Figure 3-3).
- **OA-NAM-011** In ACTIVATING, an IRS answering one of our IRQs goes to ACTIVE; an IRS timeout
  resends the IRQ; the optional retry limit sends a TRQ and goes to INACTIVE; a received IRQ is
  answered with an IRQ and an IRS; a received TRQ is answered with a TRS; a Deactivate request
  sends a TRQ and goes to INACTIVE; anything else is ignored (3.2.2.4.1.2).
- **OA-NAM-012** In ACTIVE, silence for the idle time sends an ASM; a LAM clears it; no LAM in time
  sends an IRQ and goes to ACTIVATING; a received ASM is answered with a LAM and a received IRQ
  with an IRS; a received TRQ is answered with a TRS and the session re-activates; a Deactivate
  request sends a TRQ and goes to DEACTIVATING (3.2.2.4.1.3).
- **OA-NAM-013** In DEACTIVATING, a TRS or a TRQ or the TRS timeout goes to INACTIVE; responses are
  still processed and sent; pending messages are abandoned and counted when the session ends
  (3.2.2.4.1.4).
- **OA-NAM-014** Message numbers run 000 to 999 and wrap; a received message from or to the wrong
  unit is discarded; received numbers are classified first, in order, duplicate or gap.
- **OA-NAM-015** An answered-message table of at most 128 entries matches each LAM, RLA, PLA or LRM
  to the message it answers by kind and number, reports an overdue one (resending first when
  configured), and refuses a new message, numbering nothing, when full.
- **OA-NAM-016** The session step is a pure function of configuration, state, event and time; it
  returns at most 8 actions, never reads a clock, and reports the next timer deadline.
- **OA-NAM-017** Every LRM code the codec can produce is in the code table, in increasing order,
  with the ICD's field and text.
- **OA-NAM-018** The ICD workload figures (Table 29 per-flight counts, Table 3-IV peak rates, the
  13.2 RTUs per handoff arithmetic, the latency and size bounds) are carried as data and agree
  with each other and with the codec's size limits.

## NAM ACC-to-ACC interface, review corrections (oa-nam)
- **OA-NAM-019** A CHG carries 13(b) and Field 18 (Table 4), except that a change of the ACID
  alone, a Field 22 holding the single amendment `07/`, may omit both (3.1.2.3, the ICD's own
  example); a CHG that changes anything else without them is refused with the missing field.
- **OA-NAM-020** Field 10 is checked for syntax: 10(a) designators valid and not repeated (codes
  71, 73), `N` alone and not `S` with `V`, `O` or `L` (75); 10(b) valid, not repeated, one Mode S
  letter at most, ADS-B `B1 B2 U1 U2 V1 V2` and ADS-C `D1 G1` (65, 66, 72, 74, 76); and, where the
  message carries Field 18, `N` with COM/, NAV/ or DAT/ (64), PBN/ without `R` (86), `R` without
  PBN/, `Z` without COM/, NAV/ or DAT/, `G` without NAV/ (91); more than eight PBN values is 84.
  Checks that need flight-plan state are not made and are listed in the README.
- **OA-NAM-021** Validation runs under a profile, `Core` (Part II) or `EramAttachment2` (the
  default); block altitudes in 14(c), unknown speed and heading in Field 32, and the RTA contact
  data in 31(c) (28 characters, `frequency/name/function`) are accepted only under
  `EramAttachment2`, and Field 31(c) of every other message is 20 alphanumeric characters.
  Supersedes the single 28-character text rule of OA-NAM-003 for those messages.
- **OA-NAM-022** An IRQ received in INACTIVE is reported to the application and nothing is sent
  (Rev G B.1.6); with `inactive_ignores_irq` it is ignored as in the IRD Figure 3-3. Supersedes
  "every other event is ignored" of OA-NAM-010.
- **OA-NAM-023** Messages awaiting a response stay pending across ACTIVE to ACTIVATING, are timed
  out while ACTIVATING, and a late response still matches; they are abandoned only when the
  session goes INACTIVE. Supersedes the abandonment on re-activation implied by OA-NAM-012.
- **OA-NAM-024** A message whose sender or receiver in 03(b) is not the partner or us is counted
  and answered with an LRM action (code 1 or 2, field 3), never dropped silently, except an LRM,
  which is counted and not answered, and in INACTIVE, where it is ignored and counted.
  Supersedes "discarded" in OA-NAM-014.
- **OA-NAM-025** An LRM that answers no message of ours still pending, or answers our ASM, is
  delivered to the application with its reference; other unmatched responses are discarded.
- **OA-NAM-026** A response whose 03(c) names other units is discarded as a foreign reference;
  the received-number sequence counts only messages the session processes; an ASM's LAM is still
  processed in DEACTIVATING.
- **OA-NAM-027** The retry number of a repeated IRQ follows the profile: the same number under
  `Core` (Rev G B.1.6), a new number under `EramAttachment2` (Attachment 2 2.10).
- **OA-NAM-028** The LRM code and field follow the profile: a missing time in 13 or 16 is code 21
  and in 14 is 24; under `EramAttachment2` a bad message number and a bad 03(c) in RTI, RTA, RLA
  and RTU are 57, and codes 17 and 63 to 66 take the field of Attachment 2 Table 2-2.
- **OA-NAM-029** The amendments of an ABI Field 22 may come in any order, each of 8, 9, 10, 15, 18
  at most once. Supersedes the ascending-order rule of the first version.
- **OA-NAM-030** A POA with the SSR of Rev F in 07 parses and encodes back to itself; a POJ with
  one is refused.
- **OA-NAM-031** `Profile::RevE` validates Part II of Rev E (15 April 2016): the ASM carries 03(c),
  the POI does not, the POA carries 07 a, b, c, the POJ is a Field 18 reason, metric altitude is
  allowed in Field 32(e), none of the Attachment 2 additions apply, and a POA or POJ expects no
  response. All other messages and fields are as under `Core`.
- **OA-NAM-032** A message kind with no definition in the schema is an error (`Mnemonic`, code 60),
  never another message's definition: `spec` and `spec_for` return `None`, and `parse_header`,
  `fmt_header` and `encode` refuse.
- **OA-NAM-033** Message-number arithmetic in the session is reduced modulo 1000 before it is
  used, so no `u16` value in `next_number`, `rx_last_number` or a header number can overflow it.
- **OA-NAM-034** The session counts every discard and every refused send under its reason
  (saturating, `discard_count` and `refusal_count`), in addition to returning it as an action.
- **OA-NAM-035** After `new_session`, `session::step` allocates nothing through activation,
  message traffic, retries, a full pending table and deactivation (the `nam_session` route of
  `validation/oa-alloc-check`).

## Release replay comparison (oa-track canonical output, oa-replay-diff)
- **OA-TRK-360** A tracker output row has one canonical byte form of 85 bytes: track address, time,
  position, velocity, turn rate, altitude terms, position and velocity covariance terms, model
  probabilities and flags, every field little-endian at a fixed offset (`oa_track::canon`), floats as
  their IEEE bits with every NaN made the one fixed NaN first (`oa_math::canon_nan_f32`, whose sign and payload
  would otherwise differ between machines), nothing else zeroed or normalised. The same row
  gives the same bytes on every call, and the form built from a row in memory equals the form built
  from the column files of a spilled run.
- **OA-TRK-361** The digest of an output second is the SHA-256 of the canonical rows whose time
  falls in that Unix second, in the order the tracker emitted them; the chain hash of a second is the
  SHA-256 of the chain hash before it (32 zero bytes at the start), the second, its digest and its row
  count. A change to one second changes that second's digest and every chain hash from it on, and
  leaves the digests of the other seconds unchanged.
- **OA-TRK-362** `oa-track-run --digest <file>` writes the digest as text lines
  `second chain_hash second_hash rows` after one `#` header line, and `--rows <file>` writes the
  canonical rows, grouped by ascending second (each second in emission order), behind a versioned
  binary header (magic, version, row size, row count). With neither flag the run does what it did
  before and writes nothing more.
- **OA-TRK-363** Two runs of the tracker on the same input write byte-identical digest and rows files.
- **OA-RDF-001** `oa-replay-diff` calls two digests identical only when every line, chain hash
  included, is equal; otherwise it reports the first differing second, the number of differing
  seconds (a second whose rows differ in hash or count, or that only one run has) and how many seconds
  only one run has.
- **OA-RDF-002** A malformed digest file (wrong field count, a number or hash that does not parse,
  seconds not strictly increasing) is an error naming the line, and the tool exits 2.
- **OA-RDF-003** Given both rows files, for each differing second the tool reports the rows added,
  removed and changed (rows matched by track address, time, tick flag and order among equals), the
  tracks affected, the changed rows by class (position, velocity, covariance, flags, model), the
  largest absolute difference of every field over finite values, the count of changed non-finite
  values, and the flag bits that differ; a second whose rows are equal and only reordered is reported
  as reordered. Totals follow, and a `key=value` summary.
- **OA-RDF-004** With an expect file naming the declared classes (`position`, `velocity`,
  `covariance`, `flags`, `model`, `added`, `removed`, `order`), a difference is accepted (exit 0) only
  if every class present is declared; an undeclared class, or an expect file without rows files to
  tell the classes, is a failure (exit 1 or 2), and an unknown class name is an error.
- **OA-RDF-005** A rows file with a wrong magic, version or row size, truncated, not grouped by
  ascending second, or whose row counts disagree with its digest file is an error (exit 2), never a
  comparison.
- **OA-RDF-006** `tools/replay_compare.sh` accepts only commit hashes or refs of the repository,
  refuses a working-tree or option-like spec, builds each commit in its own clean detached worktree,
  one build at a time, and records both commits and the clip's SHA-256 in its report.
