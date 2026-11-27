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
  argument with the sign of the second after `canon_nan` (a NaN magnitude gives the canonical NaN);
  `signum_canon_f64`/`_f32` give `1.0` or `-1.0` by the sign bit (`-1.0` for `-0.0`) and the canonical NaN for
  any NaN, tested over every kind of value. Code calls these in place of `total_cmp`, `is_sign_negative`,
  `is_sign_positive`, `copysign` and `signum`, whose results depend on the sign and payload of a NaN (see
  OA-TRC-247 and OA-TRC-260).
- **OA-MATH-036** `min_f64`/`max_f64`/`clamp_f64` and the `f32` forms ignore a NaN argument (the other is
  returned; two NaNs, or a NaN `x` in `clamp`, give the canonical NaN), return `-0.0` from `min` and `0.0`
  from `max` for the two zeros in either argument order, and never panic (`clamp` with `lo > hi` gives `hi`).
  Tested for every combination of sign, zero, infinity and NaN. Core code calls these in place of the
  float `min`, `max` and `clamp` (see OA-TRC-251).
- **OA-MATH-037** Outside `order.rs`, no source of `oa-math` calls `total_cmp`, `is_sign_negative` or
  `is_sign_positive` (method, path call or path as a value): the sign and order of a NaN come from the `*_canon_*`
  functions only. The purity scan fails on one, and a planted-detection test shows the matcher finds each form.
- **OA-MATH-038** `hypot` was split into the operand ordering (`order_by_bits`), the first step of the
  slow path (`hypot_slow_operands`), the double-double core (`hypot_dd`: `dd_operands`,
  `hypot_dd_root`, `scale_round`) so that each is proved apart: the split code equals the code before it
  bit for bit (`fast`, `slow` and `hypot`, on a million inputs: zero, subnormals, the limits of the fast
  paths, the domain edge 5e7, ratios near 2^-60, arbitrary bit patterns). The reference-data tests of
  OA-MATH-005 are unchanged.
- **OA-MATH-039** Proved (Kani, `root::proofs`), for finite `x` and `y` with `|x|, |y| <= 5e7`, the parts of
  `hypot` that do not depend on an exact product: the ordering (`hypot-operands-ordered`), the first step
  of the slow path (`hypot-slow-operands`), the scaling of the operands (`hypot-dd-operands`), the sum of
  two squares given bounds on their parts (`hypot-dd-sum`) and the scaling back (`hypot-scale-round`).
  Not proved, each over 10 minutes in CBMC (registered disabled): the exact square of a scaled operand
  (`hypot-dd-square`), the double-double root (`hypot-dd-root`), the plain fast path
  (`hypot-fast-plain`) and the scaled fast path (`hypot-fast-scaled`); all four need the exactness of
  the Veltkamp-split products, which is a multiplier-equivalence problem for the SAT solver. So
  `hypot` finite and not negative on that domain is not proved.

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

- **OA-CORE-006** Airspace model construction accepts caller-provided archive bytes and has no
  project-specific sealed-data dependency. The private `oa-adapt` command owns path policy,
  guarded-opens the input descriptors it consumes, and leaves model algorithms unchanged.
- **OA-CORE-007** Decoder time-window values and their half-open interval operations live in the
  dependency-light `oa-time-window` crate. The five XML codec crates depend on that value crate,
  not on `oa-sealed`, for their production decoder APIs; guarded callers obtain the active window
  through `oa_sealed::sealed_drop_window()`. Global pre-scan and row/frame drop behavior stays
  unchanged.

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

## Adaptation package integrity and activation

- **OA-ADAPT-001** An adaptation package manifest is canonical, versioned and bounded. Its exact bytes have a
  SHA-256 identity that must match a separately supplied expected digest. The manifest identifies release,
  facility, change record, reader API range and the schema's exact required component set. Versions 1 and 2
  require exactly the airspace snapshot; version 3 requires that snapshot and one CA preset companion. Unknown schema
  versions, incompatible reader ranges, duplicate or missing components, noncanonical encodings, extra files,
  malformed fields and unsupported paths refuse.
- **OA-ADAPT-002** Before packaged route conversion admits a model, the loader reads the manifest and required
  component through guarded descriptors, checks the opened file's bounded inventory identity and declared byte
  length before allocation/read, hashes and decodes the same snapshot bytes, and matches the decoded facility.
  Package creation returns its pin only after writes, syncs, and read-back succeed; on error the retained directory
  is not automatically provisioned. A complete valid byte set after a publisher error could still verify against
  a separately supplied pin: verification establishes byte consistency, not publication durability. The package
  is rechecked on every verified route command.
  Root/file identities and bounded inventory are checked before and after reads, but this is a point-in-time
  consistency check, not an atomic multi-file filesystem snapshot or lock against hostile replacement/restoration.
  SHA-256 checks agreement with an independently supplied pin; it does not authenticate an issuer or software.
- **OA-ADAPT-003** `oa-adapt issue` writes one canonical, bounded issued-manifest schema that records the next
  positive adaptation version computed by the command, previous manifest SHA-256, change class (`initial`, `routine`, or `field-change`),
  distinct issuer and approver principal IDs, and a bounded reason. The first issue is version 1 with class
  `initial`; every subsequent issue must verify the previous package against its separately supplied manifest
  pin and increment exactly by one; the caller does not supply the new version. A field change uses this same command with class `field-change`. The returned
  manifest SHA-256 identifies those exact bytes; recorded principal IDs are claims, not authenticated approval.
  Startup and on-demand processor inventories compare each processor's loaded and on-disk adaptation digest to
  the issued manifest digest, retaining every expected processor and any mismatch. The current
  `oa-techops::assess_inventory` result remains `ClaimsMatchUnverified` at best; it is integrity/status evidence,
  not an authenticated issuer, deployment attestation, or operational certification.
- **OA-ADAPT-004** `oa-adapt issue --ca-preset` accepts a bounded versioned CA preset only after the shared
  `oa-ca` parser validates its exact facility identity, schema, geometry and numeric bounds. It issues manifest
  schema v3 with strict component identities and byte length/SHA-256 for both the snapshot and preset; v1/v2
  encodings and package component sets remain unchanged. Issuance returns its digest only after read-back
  verifies the exact preset body, component set, manifest pin and facility. Digests establish byte integrity,
  not issuer authentication or operational approval.

### Synthetic performance verification and certification evidence (oa-certification)

- **OA-CERT-001** The synthetic PVER target catalog is a separate, versioned, bounded adaptation-data component issued in a G2 v4 package. The canonical manifest binds its exact length and SHA-256 digest; normal PVER and certification verify that package and refuse a missing or damaged catalog without falling back to an embedded copy. `FULL` covers all six catalog targets; `ABBREVIATED` covers exactly its declared three-target subset. The catalog records target IDs, positions, velocities and tolerances and is bound into the evidence record with the verified G2 adaptation snapshot digest and issued adaptation version. These targets and runs are synthetic development evidence, not recorded, live or authority-provided data.
- **OA-CERT-002** PVER feeds the fixed catalog targets through the production `oa-track` tracker and compares emitted output positions and east/north velocities with each target's expected values at the emitted timestamp. Residuals and tolerances are each rounded to the nearest micrometre (or micrometre per second) and the same integer values drive both threshold decisions and recorded evidence. Every required target must emit a comparable row within its catalog tolerances. Tracker refusals, velocity gates, capacity refusals, unrepresentable rows, output overflow, missing rows or tolerance failures fail PVER; a partial or lossy run cannot pass.
- **OA-CERT-003** A separately supplied, bounded, versioned expected processor roster carries each expected software artifact SHA-256 and expected adaptation snapshot SHA-256; its exact input bytes are hashed and retained in the certification event. Observations are read from a distinct bounded input and use `oa-build-info` for local artifact hashing and successful `oa-adapt` G2 package verification against a separate manifest pin. Missing, refused and invalid values carry an explicit state and no digest; they are never replaced by a zero digest. These local observations are not authenticated processor identity, loaded-image attestation or deployment proof.
- **OA-CERT-004** A decision is `certified` only when the actual full or abbreviated PVER passed and the complete expected processor inventory has present, matching software and adaptation digests. An omitted responding processor is recorded with explicit missing evidence and forces `not_certified`. `certified_with_exceptions` has the same prerequisites and requires nonempty exception text. Any failed PVER, missing/refused/invalid evidence or processor digest mismatch produces `not_certified`, regardless of exception text. Principal IDs and UTC timestamps are caller-supplied claims; this workflow does not authenticate, qualify or authorize a technician and does not claim operational certification.
- **OA-CERT-005** The certification evidence event is appended to the verified hash-chain audit log before success is returned. It records the caller principal and UTC timestamp claims, service, PVER mode/result, catalog and G2 binding digests, the expected roster and separately observed processor ID list (including multiplicity), each processor's expected and observed digests with evidence states, recomputed missing/duplicate/unexpected findings, outcome and exception text. Append revalidation derives those findings from the preserved roster, observed IDs and evidence; a disagreement or failed append prevents a success result.
- **OA-CERT-006** The text certification report is generated only from certification events read through audit-chain verification against the caller's expected checkpoint. The report retains the recorded outcome, principal/time claim, service, PVER scope, catalog/G2 identities, processor digests and exception text; it does not upgrade claims to authenticated or operational certification.
- **OA-CERT-007** Artifact digest requests accept only byte caps in `1..=512 MiB`; the cap is checked before guarded file access, and hashing is refused if the opened regular-file descriptor changes identity, length, or timestamps during the bounded read. This is a local byte observation, not loaded-image attestation.
- **OA-CERT-008** Certification input counts, expected roster rows, observation rows, registry rows, and file sizes are preflighted against fixed limits before processor paths are examined. The separately supplied expected roster, never the observed rows, defines the processor denominator. Missing, duplicate, or unexpected observations remain explicit and prevent a certified result. The versioned local principal registry must contain one unique `configure` claim for the supplied principal; the exact registry digest and role claim are recorded. The registry is caller-provided policy data, not authentication, identity proof, or an external authorization service.
- **OA-CERT-009** Every PVER record binds the fixed tracker configuration, runner build claim, locally observed runner artifact digest, and each target's output time, residual, tolerance, and pass/failure state. These bindings identify the reported local run; they do not prove remote deployment or runtime identity.
- **OA-CERT-010** The `certify` command reports a committed outcome only after the append-only audit shell returns its receipt following file and directory sync. All three outcomes are audited; a failed append emits no committed result. The principal, timestamp, registry, and processor inputs remain caller-supplied claims.

### Route conversion (oa-airspace)

- **OA-RTE-001** A route fix is resolved by its complete stable identity, and a direct connector exists only when an explicit direct step is supplied; otherwise a fix remains an isolated anchor.
- **OA-RTE-002** Duplicate exact fix identities are reported as ambiguous and refuse the route; lookup order never hides a duplicate.
- **OA-RTE-003** Airway expansion follows only a contiguous supported forward sequence from the exact entry to exit, retains MEA data, and refuses a gap or unsupported reverse constraints.
- **OA-RTE-004** Supported CIFP IF/TF/DF procedure transitions preserve exact point and transition identity and their published altitude, speed and turn constraints; unsupported path terminators are refused.
- **OA-RTE-005** Malformed model columns, exhausted work limits and invalid route-step counts are refused atomically with no partial converted route and with bounded refusal accounting.
- **OA-RTE-010** The normalized route instruction parser preserves full fix identity and refuses labels or numeric fields that exceed their declared bounds rather than truncating them.
- **OA-RTE-011** Parser refusals account for admitted, refused and unexamined instruction suffixes; an overlong input whose instruction count is unknown is reported as such, and step limits are explicit.
- **OA-RTE-012** The route command converts a supplied model snapshot and binds the report to the exact snapshot and instruction bytes with their SHA-256 hashes.
- **OA-RTE-013** The route command refuses a sealed snapshot path before conversion output is written.

### Synthetic demo scenarios (oa-demo-scenario)

- **OA-DEMO-001** A normal-landing approach final requires exactly one published MAP marker in
  CIFP description column 43, includes the MAP fix, and excludes later missed-approach legs from
  normal-landing geometry. Missing or ambiguous MAP markers refuse the final route.


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
- **OA-TRC-242** `canon_nan` and `canon_nan_f32` in core are `oa_math`'s: reached as `oa_math::canon_nan(..)`
  (an import is not allowed, see OA-TRC-259); a fn, binding, method or import of that name from anywhere else
  fails. The bits and bytes of a float are OA-TRC-258.
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
  `Sized`, `?Sized`, a lifetime or a sealed trait of a core crate (`Copy` is not, it implies `Clone`); `Clone`, `PartialEq`, `Eq`,
  `PartialOrd`, `Ord`, `Default`, `Debug`, `Hash`, `Iterator`, `Into` and every other trait fail unless the
  allowlist names the item. Private and `pub(crate)` items may carry any bound, a type parameter with no bound
  is not checked; the header of an `impl` is OA-TRC-252. A bound is read through parentheses, `?` and
  `for<..>`; one that still has no name (anything but `?Sized` or a lifetime) fails.
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
- **OA-TRC-250** The `src/lib.rs` of every crate of class core (`oa-ieee` included) and split has, at the top level and
  outside test code, `#[cfg(not(target_pointer_width = "64"))]` followed by `compile_error!(..);`, so the crate does
  not build for a target whose `usize` is not 64 bits. The attribute stands alone: at the start of the file, or
  after the `]` of an inner attribute, a `;` or a `}`, never directly after another outer attribute
  (`#[cfg(any())]` stacked on it). In a split crate's `lib.rs` this is the one attribute and the one macro call
  allowed besides `mod core;`, `mod shell;` and re-exports (B7). That is the one target cfg core code may have;
  any other, any other `target_pointer_width` cfg included, still fails. The check reads the string, so `"32"`, a
  missing `compile_error!` or a guard inside a module fails.
- **OA-TRC-251** A float `min`, `max` or `clamp` (a float receiver or float argument by the evidence of
  OA-TRC-248, or the path `f64::max`, `<f32>::min` and so on, also as a value in `fold(.., f64::max)`) fails in
  core code (`oa-math` excepted), with `oa_math::min_f64`/`max_f64`/`clamp_f64` (and the `f32` forms) named, whose
  results for the zeros and the NaN are defined (OA-MATH-036). Integer and iterator `min`/`max`/`clamp` pass.
- **OA-TRC-252** The header of every `impl` in core code (inherent or trait, any self type: `&F`, `Box<F>`,
  `Wrap<T>`, `Box<dyn Fn()>`) is a boundary: its generic bounds, `where` clause and callback types are judged
  like those of a reachable item (OA-TRC-245, 246), and a trait `impl`'s are judged whether or not it has `pub`
  methods; the body of an impl is core's own code. An inherent `impl` header counts for its plain `pub` methods.
  The allowlist names a header by trait and self type with their generic arguments and no spaces
  (`impl@Default/for/Wrap<T>`, `impl@Wrap<T>`), not by a head word.
- **OA-TRC-253** A `pub trait` is sealed only by the leading path of a supertrait (`sealed::Sealed`), never by a
  path inside its generic arguments (`Iterator<Item = sealed::Private>`), and a `use` tree is read per item
  (`use crate::{sealed::Sealed, open::Other}` seals through `Sealed` only). `pub use` of any glob, and of a sealing
  trait, fails, and so does a `pub use` of a module that holds one (`crate::sealed`, `sealed::{self}`).
  A module is reachable only if it and every module around it is `pub`, so a `pub mod` inside a private module
  seals. A blanket impl of a sealing trait fails when a type parameter appears anywhere in the self type.
- **OA-TRC-254** A type alias that holds an `Fn*`, `dyn` or `fn(..)` is resolved through other aliases of the crate
  (`type B = Vec<A>;`, in any order, generic or not), so a reachable signature or field that names any of them
  fails. An `as` rename in any `use` of core code fails.
- **OA-TRC-255** The float evidence of OA-TRC-248 follows type aliases whose body mentions a float (`type V3 =
  [f64; 3]`, `(f64, f64)`, `f64`, and an alias of an alias), also in struct fields, and follows functions that
  return a float aggregate (`pair().0`, `make()[0]`, `let p = make();`). Aliases are matched by name over all core
  crates; the rest is in the RULESET's known lexical limits.
- **OA-TRC-256** A name bound to an expression whose outermost shape is an integer (`.count()`, `.len()`,
  `usize::try_from(..)` with `?`/`.unwrap()`, or a final `as <integer>`) is not float evidence, whatever the
  expression mentions inside.
- **OA-TRC-257** `fmt::Arguments` (it carries the caller's `Display` impls) is not taken by a reachable item.
- **OA-TRC-258** (an integer primitive name cannot be declared, renamed or made a generic parameter, in core or, for `signum`, anywhere) In core code a `to_bits` token fails (method or path, any qualifier) unless it is the method on
  the result of `oa_math::canon_nan(..)` / `canon_nan_f32(..)`, alone or in extra parentheses. `to_le_bytes`,
  `to_be_bytes` and `to_ne_bytes` are allowed only as a path with an integer primitive qualifier
  (`u64::to_le_bytes(x)`); the method form fails whatever the receiver. No receiver is inferred, so a shadowing
  closure parameter, a `match` binding, a type alias, `<f64 as T>::to_bits`, a user `len()` or a struct-literal
  field cannot pass. Planted failing and passing cases, the review's bypasses and RT-281 among them.
- **OA-TRC-259** In core code the helpers of `oa_math` (`canon_nan`, `canon_nan_f32` and every public function
  of `order.rs`, listed in the checker and compared with the source by a test) are called only as
  `oa_math::<name>(..)`. A `use` of one, `use oa_math::*`, and any other occurrence of the name (a bare call, a
  local, a parameter, a definition, a path through another crate) fails. The planted case imports `canon_nan` and
  passes a parameter of that name; it fails on both. `oa-math` itself defines them. `oa_math` itself is only the
  external crate: a core `mod oa_math`, `use .. as oa_math`, `extern crate .. as oa_math`, a local of that name or a
  `crate::oa_math` path fails. A test compares the list with the public functions of `order.rs` and `nan.rs`
  (indented, `const` and re-exported forms included) and with the crate root's re-exports.
- **OA-TRC-260** In non-test code of every crate of the workspace (`src`, `examples`, `benches`, any class), the
  five names `total_cmp`, `is_sign_negative`, `is_sign_positive`, `copysign` and `signum` (method, path call or
  path as a value) fail, except `signum` as an integer path (`i64::signum(x)`) and the file
  `crates/oa-math/src/order.rs`. Code uses `cmp_canon_*`, `is_sign_negative_canon_*`, `copysign_canon_*` and
  `signum_canon_*` of `oa_math`. A site the core scan reports is reported once. Test directories, `#[cfg(test)]`
  items, comments and strings are not read; a crate-root `build.rs` is not scanned and the workspace has none.
- **OA-TRC-261** `capacity` as a method call, as any path (`Vec::capacity`, `Vec::<u8>::capacity`, a path value
  passed to `map` or bound by `let`) and `spare_capacity_mut` fail in core code, since the allocator chooses the
  value; a field named `capacity` and `with_capacity` do not, and `try_reserve`-style probes are left alone. The
  allocation probe of the display tests is in a shell module.
- **OA-TRC-262** The only C ABI unsafe exception is the exact `crates/openatca-core-ffi` package classified `shell`,
  with local `unsafe_code = "deny"`, explicit runtime Clippy denials, exact direct dependencies `oa-geo`,
  `oa-ieee`, `oa-math`, `oa-sha256`, `oa-csv`, `oa-json`, and `oa-xml`, an exact named provisional export inventory enforced by `oa-trace`, and
  exactly `src/lib.rs`, unsafe boundary `src/ffi.rs`, and safe adapter `src/text.rs` in its Rust source inventory. The root denies unsafe; `text.rs` forbids unsafe; only `ffi.rs` has one
  module-level unsafe allowance, and each unsafe block has an immediately preceding `// SAFETY:` comment. The
  checker also pins the reviewed unsafe-block counts by owning function. A
  different package name/path/class, extra source file, dependency, allowance, or unexplained unsafe block fails.
- **OA-TRC-263** The WASM display-shell omission is accepted only for `oa-display`, with the exact inverse
  `wasm32-unknown-unknown` cfg on both `mod shell` and its shell re-export, and `oa-sealed` declared exactly once
  under the matching non-WASM Cargo target table. Wrong packages, targets, modules or unconditional dependencies
  fail closed; this exception does not permit shell code in `core`.
- **OA-TRC-264** The `wasm32-unknown-unknown` width-guard exception names exactly the reviewed F2 runtime closure
  packages in `WASM32_PACKAGE_ALLOWLIST`; every other core or split package remains 64-bit only.
- **OA-TRC-265** `oa-demo-wasm/build.rs` matches the reviewed guarded-input and two-output source
  shape. No additional file or process effects are accepted; a changed build script requires a
  deliberate update to the checker’s pinned source and negative regressions.

- **OA-TRC-266** The F1a dependency check reads offline Cargo metadata for every workspace package and dependency kind, refuses cycles, refuses outside-repository dependencies without an adjacent F1 approval reference, and refuses every core-to-shell or core-to-tool edge. Internal path dependencies must resolve to a classified workspace package.

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

- **OA-CORE-005** Reusable ZIP and NASR packages have no project-specific sealed-data dependency.
  ZIP accepts caller-owned bounded readers; every OpenATCA private archive/data caller retains
  guarded opens on the descriptor actually consumed. Generic library path conveniences do not
  apply project policy; private real-data/seal tests live in the guarded caller harness.
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
  zero-padded counter from 0001 within one naming second. If the local clock steps back while
  recording, each new file keeps the latest naming second and advances its counter so name order
  remains creation order. These rotations are counted. Recording counts existing files toward
  the total cap and stops once with a warning at the cap. Default recording never deletes files;
  explicit measured-retention mode may remove only expired segments with its exact ownership marker
  (OA-FEED-192 to OA-FEED-195).
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
- **OA-FEED-027** A compressed recording is sync-flushed when the local whole second changes in
  either direction, on a write or an idle flush. It is also flushed on rotation and exit (each
  file is finished with its trailer): a file read while recording, or after a crash, holds every
  frame up to the last flush.
- **OA-FEED-028** Rotation and the total cap count the compressed bytes on disk, including
  existing `.oaf` and `.oaf.gz` files, rotate on frame boundaries, and stop once at the cap. Default
  mode never deletes a file; opt-in measured retention follows OA-FEED-192 to OA-FEED-195.
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
- **OA-FEED-152** A frame repeating the last `seq` is a duplicate (including when the expectation
  has saturated at `u64::MAX`) and leaves the expectation unchanged; the expected `seq` is never a gap; one ahead
  is one gap of one missing frame; one below the last is a regression unless it fills the
  most recent open gap (OA-FEED-172).
- **OA-FEED-153** Latency is the receive time in whole milliseconds minus the JMS time: exact
  values land in the maximum and the histogram, equal times are zero (not skew), a stamp ahead by
  one millisecond is skew.
- **OA-FEED-154** The latency quantile is `None` without samples, with a zero denominator, for a
  fraction above one, or in the overflow bin. For other fractions it returns the upper edge of
  the bin holding the rank, without overflowing for any histogram counts or fraction inputs.
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
  (`lost_frames`); a raw recording holds none back. In opt-in closed-segment sync mode, generic write/flush/finish
  failures also count accepted active-segment frames not classified as lost as persistence-unconfirmed; the write
  that triggers the failure remains counted as lost. Tested with a writer that fails on demand.
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
- **OA-FEED-181** A CRC-work refusal leaves a complete frame buffered at the same offset with no CRC
  bytes charged; retry with sufficient allowance returns that frame and charges exactly the CRC input bytes.
- **OA-FEED-182** A copy-work refusal leaves the reader unchanged and charges no copied bytes; retry copies
  the complete offered input exactly once. A copy allowance equal to the bytes taken passes and one byte less
  is refused; likewise a compaction allowance equal to the buffered bytes that must move passes and one byte
  less is refused with nothing moved.
- **OA-FEED-183** A maximum-size frame with a damaged CRC is refused when its exact CRC byte allowance is one
  short, then reported as a CRC error after retry with the full allowance and charges exactly those bytes.
- **OA-FEED-184** After the first bad-magic byte is reported, a resynchronisation scan refused with zero
  candidate-position allowance retains the unexamined suffix at its current offset; retry finds the next frame,
  counts each examined start position and records every skipped prefix byte. The scan is charged the buffered
  bytes minus three: that allowance passes and one position less is refused.
- **OA-FEED-185** A shared remaining allowance across overlapping corrupt maximum-size frames stops CRC work
  at its exact cap, retains the next candidate on refusal, and after budget renewal yields the valid tail frame.
- **OA-FEED-186** A work refusal is renewable only when its kind-specific required amount fits a fresh profile;
  cumulative counter overflow and amounts above the fresh allowance require explicit stream cancellation. The live
  loop decides each turn by pure functions: it stops only when stopping, the stop condition holds and no input
  or drain is owed; it services when input or a drain is owed, else it receives. A retryable push refusal
  defers with no drain pending, a retryable drain refusal defers with a drain pending, and a refusal a fresh
  allowance cannot satisfy, a counter overflow, or a push that takes no bytes cancels the stream with an error
  naming the refusal, the reader-buffer bytes, the unpushed input bytes and the deferral count.
- **OA-FEED-187** Work cancellation leaves complete buffered frames unclassified as malformed and retains them
  for explicit disposition; only normal end-of-stream finalization reports a truncated tail.
- **OA-FEED-188** The release binary `oa-feed-work-check` reports the maximum service-turn wall time and exits
  nonzero above 1 s; the `ci` workflow runs it in release. It separately labels valid-capacity and malformed-input safety
  cases, accounts CRC refusals/skips as rejection rather than valid-frame loss, and checks defer/resume and
  compaction ownership exactly; its `#[test]` asserts only these deterministic checks, not wall time.
- **OA-FEED-189** Recorder startup inspects at most 16,384 directory entries when summing existing recording
  bytes; unrelated and zero-byte entries count toward the cap, and an excess entry returns an explicit error.
  `Recorder::open` accepts a directory of exactly 16,384 entries and refuses one of 16,385.
- **OA-FEED-190** The compatibility `FrameReader::push` and `next` use a work allowance sized to the reader's own
  buffer, so a large buffer never stalls silently: an 8 MiB buffer takes 6 MiB offered in one `push`, decodes it
  with `next`, and reports no work refusal.
- **OA-FEED-191** The live loop prints a stderr warning (not recorded output) once each time the work-deferral
  count reaches a power of two of 1,024 or more, and not at any other count.
- **OA-FEED-192** Opt-in measured retention requires at least 45 days, a nonzero operator-supplied measured peak
  byte rate per hour and a nonempty measurement source path/provenance string supplied with
  `--peak-rate-source`. Its checked storage cap is derived from the supplied rate and rounded-up retention hours;
  the CLI records the supplied value but does not itself verify that a measurement file exists or validate its
  measurement. No default measurement or guessed storage capacity is supplied. Legacy mode
  retains its existing fixed cap and never deletes recordings.
- **OA-FEED-193** Retention cleanup runs before the storage-cap decision and removes only canonical expired feed
  segments with a V2 owner marker bound to the segment device/inode and a matching closed receipt. The path and
  opened regular-file descriptor identities are checked before deletion; a replacement inode, unmarked file,
  malformed marker, non-regular file or symlink is preserved. The cleanup is bounded by the recorder's
  directory-entry limit. These sidecars are not cryptographic authentication: retention assumes the recording
  directory is not concurrently modified by an untrusted writer, and path-based unlink retains a narrow TOCTOU
  window against a concurrent same-directory replacement.
- **OA-FEED-194** The technician `recordings` query accepts a nonempty half-open UTC Unix-second range and lists
  regular requested-feed segments whose known closed interval overlaps the range, in path order. Open, unowned, or
  malformed-interval segments with a start before the range end remain visible as candidates with an unknown end;
  no end is fabricated. The query reports segment intervals, not per-frame coverage; replay remains a separate command.
- **OA-FEED-195** Measured-retention configuration refuses a window below 45 days, absent/zero measurement, empty
  source, arithmetic overflow, and conflicting explicit cap; its summary reports the exact configured retention,
  supplied rate, derived cap and source. On Linux, combining retention with `--sync-closed-segments` preserves
  file-then-directory sync ordering; a closed receipt is created only after both sync stages succeed.
- **OA-FEED-196** `oa-feed measure-rate` accepts at most 256 guarded regular raw-OAF files, with per-file and
  aggregate byte caps, and uses the production frame reader/sealed-window gate to aggregate only validated frame
  byte counts by receive-time UTC hour. It refuses gzip, damaged/truncated/unframed input and sealed-window drops,
  emits aggregate counts and first/last frame times without payloads or IDs, and discloses that edge buckets may be
  partial and capture continuity is unverified. Its observed sample peak is operator-supplied measurement input;
  it does not establish a future traffic maximum or continuous hourly coverage.
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
- **OA-FIXM-022** The assigned altitude is decoded with its form as a state: `simple` and `vfrPlus` (the
  value in `assigned_ft`), `vfr` (no number and no `AssignedAlt` field), `block` (limits `above` to `below`; that `above` is the lower limit is read from the recording and is
  unconfirmed) and `altFixAlt` (altitude before and after a named fix; the meaning of the two is unconfirmed); the
  form is its own field and no number stands for VFR.
- **OA-FIXM-023** A doubled assigned-altitude form, a block with a limit missing or with `above` over `below`, and
  an altitude at a fix with the fix or a value missing or unreadable, are a counted field error on the form; the
  row is kept.
- **OA-FIXM-024** The interim altitude is a value, or `xsi:nil` (flag `FLAG_INTERIM_NIL`: ERAM states there is
  none); an absent element is neither, and an empty element without nil, a nil that is not a boolean and a doubled
  element are a field error.
- **OA-FIXM-025** The requested altitude is decoded as `simple`, `vfrPlus` or `vfr`, in its own columns, and a
  doubled or unreadable form is a field error; the `vfr` form is a known path.
- **OA-FIXM-026** The handoff `event` (initiation, acceptance, update, retraction, failure, take control) and the
  accepting unit are decoded; a handoff with no event has none; an event outside the six is a field error and
  the rest of the row is kept.
- **OA-FIXM-027** The ADS-B (Mode S) address of `aircraftDescription@aircraftAddress` is decoded as exactly six
  hexadecimal digits, not all zero; any other value is a field error.
- **OA-FIXM-028** Encoding is lossless for the explicitly supported `oa-fixm` event subset: eligible decoded
  rows round-trip without changed represented values, while malformed-field rows and forms with no
  unambiguous encoder representation are refused with a specific error; no partial XML document is returned.

### IWXXM METAR/SPECI initial subset (oa-iwxxm)

- **OA-IWXXM-001** The bounded shell decoder accepts the explicitly supported IWXXM 2025-2 METAR/SPECI
  fragment only under exact IWXXM 2025-2, AIXM 5.1.1 and GML 3.2 expanded names, preserving kind,
  status, permissible usage, station designator, automated-station flag,
  issue and observation times, exact supported temperature/QNH quantities, and a required but
  nillable `surfaceWind` property. Within a non-nil wind object, mean speed is required but nillable;
  mean direction is optional, as are variability and gust, with supported units/operators/nil reasons.
- **OA-IWXXM-002** An unsupported observation construct is refused visibly with a stable element
  context; unsupported wind variability extremes, supplementary hazards, and extensions are never
  silently omitted.
- **OA-IWXXM-003** Inputs exceeding the document/table/work/scalar bounds are refused, and malformed XML,
  timestamp, quantity units and decimal forms do not produce a normalized report. Inline property
  wrappers have exact child cardinality/order; selected element-only parents reject hidden text.
- **OA-IWXXM-004** Nil values and their reason strings remain distinct from numeric measurements, and a
  schema-permitted absent observation stays distinct from a nil observation. This subset supports the
  IWXXM/GML `nilReason` attribute form and refuses `xsi:nil` as unsupported. Temperatures below
  absolute zero, nonpositive QNH, negative wind speeds/gusts, and directions outside 0..360 degrees
  are refused. Timestamps use a declared positive-four-digit-year UTC-Z subset, converted to Unix
  milliseconds without fractional rounding; offsets, year zero, 24:00, leap-second `:60`, and
  sub-millisecond precision are refused. Only NORMAL, AMENDMENT and CORRECTION statuses are
  accepted. This limited decoder is not an XSD/Schematron validator and does not claim full edition
  conformance.

- **OA-IWXXM-005** The bounded METAR/SPECI projection retains supported prevailing and minimum
  visibility, runway visual range, present/recent-weather code URIs, cloud layers, wind-shear
  runway designators, and trend groups as typed values. The bounded TAF projection retains its
  valid period, base group, and BECOMING, TEMPORARY_FLUCTUATIONS, FROM, and PROBABILITY_30/40
  change groups as typed forecast values; the IWXXM FROM group carries the documented TAC FM
  interval semantics. One base and at most five change groups are accepted. Cancellation reports
  and unsupported forecast elements refuse with stable element context. Every file in the official
  WMO IWXXM v2025-2 `IWXXM/examples/` listing is byte-pinned and has a tested outcome: supported
  METAR/SPECI/TAF values decode, while collection wrappers, translation-failed reports, and the
  cancellation report refuse with named context. Probability-group and wind-shear examples are
  schema-derived synthetic fixtures, not separate official examples. See the source manifest for
  URLs, byte lengths, SHA-256 values, and outcomes. This is a partial profile, not general IWXXM
  conformance.

- **OA-IWXXM-006** The encoder emits deterministic, XML-escaped, size-bounded IWXXM 2025-2
  METAR/SPECI documents from the supported normalized report model. Encoding followed by this
  decoder preserves every represented value; unsupported report kinds and model states that
  cannot be represented are refused rather than defaulted or omitted; in particular, trend
  `FROM` is refused because it is absent from the pinned encoder change-indicator enumeration.
  Tests check the finite emitted profile against pinned WMO v2025-2, AIXM 5.1.1, and GML 3.2 XSD bytes:
  expanded namespace URIs, declared elements, emitted child ordering/cardinality, required
  attributes, generated-ID uniqueness, station/runway designator restrictions, and supported
  measured-unit checks. Product-level physical-domain bounds are enforced separately from the XSD
  simple-type checks. This is not a general XSD validator, full imported-type validation, or
  Schematron validation; see the schema source manifest for the exact schema closure and limits.

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
- **OA-DSP-053** The display API accepts exactly 2^26 pixels (8192 by 8192, 16384 by 4096 and 65536 by 1024 alike)
  through `check_frame`, `scratch_for`, `build_scene` and `rasterize`, and refuses the smallest larger product
  (65536 by 1025) at each before writing pixels; it refuses frame/list combinations whose conservative preflight
  work charge, including repeated polyline/text span visits and the tested maximum glyph stroke count, exceeds
  `MAX_RENDER_WORK` (a charge equal to it is accepted, one more is refused); a smaller retry succeeds.
- **OA-DSP-054** `build_scene` precharges, against `MAX_SCENE_WORK` and before clearing the destination list, table scans,
  per-boundary-span subdivision (including overlapping and zero-length spans), airway-leg subdivision, the point-label
  sort (the in-crate bottom-up merge sort's bound of n * ceil(log2 n) comparisons, 0 for n <= 1, each costing the longest identifier plus 4, not the string pool) and label-grid slot inspections; each term moves the
  bound by its own cost, a charge equal to the limit is accepted and one more is refused, over-limit work leaves the previous
  complete list intact, and a valid retry succeeds.
- **OA-DSP-055** `scratch_for` refuses a display profile above `MAX_SCENE_PIXELS` before allocating grid storage, while preserving the earlier `MAX_GRID_BOXES` refusal precedence; a profile within the limit succeeds.
- **OA-DSP-056** The raster text stroke-work ceiling equals the largest built-in glyph and covers all glyphs, the fallback glyph and every byte-valued input character.

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
- **OA-RPL-007** TBFM rows set the metering columns from their named source points: meter fix,
  ETA at the meter fix, an STA only when an STA at that same meter fix is present, runway and the
  frozen flag (`sfz`). An STA at the scheduling fix is not substituted for a missing meter-fix
  STA. A TBFM row older than the flight's last TBFM message time is refused whole and counted as
  out of order.
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
- **OA-RPL-024** In normal mode the `oa-replay` binary prints the checkpoints and a final summary (frames and
  rows per feed, TBFM linking, table totals, errors by kind, flights live and the final state
  hash); checkpoints are printed as they are recorded; a progress line (replay time, frames per
  second, replay-time to wall-time ratio, resident memory) goes to stderr every N minutes of
  replay time (default 10); every stdout line except the `timing` line is identical from run to
  run, whatever the flag order or read size; exit status is 2 for usage errors and 1 for an unreadable file.
- **OA-RPL-277** Opt-in offline replay verification takes a positive admitted-frame count, an
  ordered-input digest and a replay-state digest from an independent witness. The prefix flag
  checks its exact admitted-frame boundary. The terminal flag checks the final count and both
  digests, including when a recording ends cleanly at the wrong boundary. Either flag suppresses
  intermediate checkpoints and withholds the final summary on a mismatch or a partial replay.
  A bridge `REJECT` marker is counted and makes the replay partial.
  A partial replay that reaches the final verification check reports aggregate blocker counts
  on standard error.
  The optional settings precondition verifies a separate versioned digest of the effective
  replay settings and ordered logical feed streams before any recording path is used.
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
- **OA-NOTAM-026** The bounded FAA FNS AIXM 5.1 event encoder round-trips retained event fields,
  ordered feature time slices, supported per-feature designators/airport indicators and associated
  polygon geometry; it escapes XML text, round-trips a `PERM` effective end with a concrete
  event-valid end, and emits GML `indeterminatePosition="unknown"` when the retained model has no
  concrete permanent end. The pinned GML schema defines `unknown`, `after`, `before` and `now` as
  distinct positions; unsupported non-unknown indeterminate states are refused rather than
  collapsed. Its default sixteen-feature-per-row reservation is a memory budget, not an AIXM
  occurrence limit; a caller may provide a larger total feature capacity. A refusal or undersized
  destination leaves the caller's output bytes unchanged. This is not a general AIXM encoder, a
  SWIM publisher, or an XSD-conformance claim.

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
- **OA-RPL-276** Offline stream advance renews its finite reader allowance only between bounded steps; over input of
  six scan quotas of garbage followed by a valid frame it emits that frame, conserves source bytes, takes at least
  five steps, and no step's measured scan, CRC, copy or compaction work exceeds one quota.
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
  layout: header, 80-byte little-endian records (`ADS3`: the last byte NIC_baro plus one, 0 absent; `ADS2`: that byte reserved).
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
  The trace's `nic_baro` is read as 0 or 1, and a value outside that, or a null, is counted by field.
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
- **OA-ADSB-173** An `ADS3` payload (the magic the writer uses) carries NIC_baro in byte 79 as NIC_baro plus one (0
  absent, 1 for 0, 2 for 1) and reads it back; a byte above 2 is counted as a reserved byte and NIC_baro is absent. An
  `ADS2` payload (an older clip) reads with NIC_baro absent, and a non-zero byte 79 under `ADS2` or `ADS1` is counted as a
  reserved byte and is never read as NIC_baro. A reader of `ADS1` and `ADS2` only refuses an `ADS3` payload by its magic.
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
- **OA-ADSB-174** Bounded public V2 snapshots retain full source identity and available quality fields, estimate source time only from envelope time minus explicit position age, count every exclusion, and never fall back to a historical position. Required source arrays follow the V2Response_AcItem schema; contradictory source flags cannot be admitted as direct ADS-B.
- **OA-ADSB-175** Snapshot decoding refuses oversized structures, row arrays, inconsistent counts, duplicate keys and invalid or sealed envelope timestamps before admitting rows; sealed row timestamps are separately dropped and counted.
- **OA-ADSB-176** The offline surface census verifies a bounded completion manifest and archive parts before counting up to 1,000,000 positioned address/time bins. It preserves explicit ground flags even when geometric elevation is present; absent altitude never implies ground. Missing, stale, mixed and non-direct-source observations remain counted and labeled. Counts are conditional on position-bearing trace observations; they imply neither total airport coverage nor runway/taxiway occupancy. Input/output provenance and partial decoding are visible, inputs remain sealed-path guarded, and every capacity refusal stops or marks the run incomplete.
- **OA-ADSB-177** The offline surface census remains counts-only by default. Its explicit `--position-csv <private-path>` option may write every valid positioned observation in the verified bbox/time window to the fixed normalized schema, including transponder address; it must create a new distinct path, enforce the 256 MiB byte cap before each write and the 10,000,000 report-row cap, and remove/refuse incomplete output after any error. Provenance records the private CSV path, row count and hash after flush/close, plus the complete readsb `addrtype_t` source-index mapping. A partial trace decode must label the CSV partial, never complete. The optional export adds no raw positions or addresses to the counts report or stdout.
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
  reason, each with its total. The arrival assigned-altitude rescan also breaks unsupported FIXM roots down into
  absent roots, other local names and `MessageCollection` roots with a non-NAS namespace, without reporting raw
  names or namespace values. A row or field that did not reach scoring is in one of the counts.
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
- **OA-BENCH-132** Where a flight's `aircraftAddress` fails the plausibility check or is unknown to truth, and its
  `ADSB_02M_*` address hint is a different address, known to truth, that passes the check (not merely unchecked),
  the flight is associated with the hint under its own status and method (`hint over plan`); its pairs are scored
  in the primary rows as that method split, and each such flight is listed with both addresses and the reason the
  plan address failed. A plausible plan address always wins; an unchecked hint never replaces it, and a plan
  address that is only unchecked (too few shared times) is kept. `AssocConfig::hint_over_plan` (default on) turns the
  rule off for protocols frozen before it. The case list holds the first 200 cases by severity; the ASSOC table
  counts the cases left out ("more cases not listed") and the truth addresses associated with more than one ERAM
  flight at the same time (time spans, first to last sample, that overlap; consecutive legs do not), and lists them.
- **OA-BENCH-133** The flight table keeps the last transponder code an SFDPS message gave
  (`currentBeaconCode`, four octal digits read as a decimal number like the ADS-B squawk) in `Flights::beacon`, and
  `NO_BEACON` when no message carried one; a message without a valid code leaves the stored one.
- **OA-BENCH-134** `Legacy::beacons` holds one row (flight, message time, code) each time a message gives a flight a
  code other than the one it had, the first code included, so the code in force at a time is the last row of the
  flight at or before it. The table is capped (`Limits::beacons`); a row that does not fit, or whose message has
  no time, is counted under the notes and not stored.
- **OA-BENCH-135** A beacon change beyond the cap is counted under the label `DROP_SFDPS_BEACON_FULL` and the flight
  still holds the last code.
- **OA-BENCH-136** `Legacy::tfms_actual` holds the TFMS arrival times of type ACTUAL, apart from `Legacy::tfms`; a row that does not fit is counted in `overflow_tfms_actual`, and neither the table nor the counter appears in any oa-bench report line.

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
- **OA-SIM-033** `adsb_error` gives a position bias to round(`bias_fraction` * N) of the N distinct addresses and a
  latency to round(`lag_fraction` * N), chosen by a seeded shuffle, exactly for every seed; the two sets are chosen
  independently.
- **OA-SIM-034** A biased aircraft's reports are offset from the truth by `bias_mult` times one bound per aircraft, in a
  direction fixed per aircraft.
- **OA-SIM-035** A row with no usable NACp takes `bias_fallback_bound_m` as its bound where the aircraft has no usable NACp at all.
- **OA-SIM-036** A lagged aircraft's report at time t carries the truth position and velocity of time t - L (whole
  milliseconds), stamped at t.
- **OA-SIM-037** Each lagged aircraft's L is constant and within `[lag_min_s, lag_max_s]`.
- **OA-SIM-038** Each output row carries the label bits (`ERR_BIAS`, `ERR_LAG`) of the errors its aircraft has; a row
  with no bits is the truth at its time.
- **OA-SIM-039** The output depends only on the truth, the base reports and the parameters, and changes with the seed.
- **OA-SIM-040** With no error the output is the truth at the base reports' times and addresses, every other field
  copied, and rows the truth cannot answer are counted as dropped.
- **OA-SIM-041** A fraction outside 0 to 1, a negative or non-finite multiple, bound or latency, an inverted latency
  range or a latency above `MAX_LAG_S` is refused with `BadSpoof`.
- **OA-SIM-042** With no error injected, a tracker run scored against the exact truth and against the truth rebuilt
  from the generated reports gives the same Ticks and Updates position RMS within the stated tolerance (the optimism
  harness, with the clip set; without it the test prints "skipped" and passes).
- **OA-SIM-043** A biased aircraft's magnitude uses one bound for all its rows: the median (upper middle) of its usable
  NACp bounds, so a NACp that changes in flight does not change the offset.
- **OA-SIM-044** With both fractions at one half, the biased and the lagged sets differ, for each of five seeds.
- **OA-SIM-045** A generated row's time is its base row's time rounded to the nearest whole millisecond, exactly.
- **OA-SIM-046** A lagged row's altitude and vertical rate are the truth's at t - L where it has them, else blank; its
  geometric altitude is blank.
- **OA-SIM-047** Rows left out are counted in total and by the label their aircraft would have carried; the counts add
  up with the rows kept.
- **OA-SIM-049** A lagged row whose time t - L the truth cannot answer is the truth at t stepped back along its
  velocity by L (counted as extrapolated, by label and by reason, with its altitude and vertical rate blank), and is
  left out only when the truth cannot answer at t or its velocity is not finite. Base columns of different lengths
  (including `nac_p`) are refused.
- **OA-SIM-048** Per-report noise is seeded, independent per row, and per axis of standard deviation
  `hypot(noise_sigma_m, noise_bound_frac * row bound)`.
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
- **OA-AST-108** `uap_021(Cat021Edition::Ed24)` is Table 2 of CAT021 Ed 2.4 (section 5.3): 49 slots, the FRNs,
  items and lengths of the table, FRN 43 to 47 unused, 48 RE, 49 SP.
- **OA-AST-109** The Ed 2.4 and Ed 2.7 UAP tables are one table apart from the spare bits of I021/090
  (the codec is parameterised by the edition, not copied).
- **OA-AST-110** A record worked out by hand from the Ed 2.4 item definitions (I010, I040 with first extension,
  I131, I080, I073, I140, I090 with two extensions, I210, I145, I155, I160, I170, I020) decodes to the expected
  values and encodes to the same octets in Ed 2.4.
- **OA-AST-111** Ed 2.4 refuses, on encode and on decode, what only Ed 2.7 defines (MOPS version 3, a third
  extension of I021/040, a fourth of I021/090); Ed 2.7 accepts it.
- **OA-AST-112** Bit 4 of the third I021/090 extension is a spare bit in Ed 2.4 (refused when set) and SRC in
  Ed 2.7; bits 3/2 are spare in both.
- **OA-AST-113** The functions without an edition are Ed 2.7; records at the limits of I021/131 and I021/073
  round-trip in Ed 2.4.

## Interface versioned conformance inventory
- **OA-IFACE-001** The machine-readable `validation/interface-conformance-v1.json` pins public source
  versions for NAM/ERAM non-U.S. ACC session requirements, ASDI XML 1.8, TFMDI XML 1.7, and ASTERIX
  CAT021/048/062. Synthetic ASTERIX and NAM vectors accept selected declared examples and refuse listed
  edition, framing, repetition, coordinate and profile defects. Generic XML tests establish only
  well-formed XML parsing and explicit security refusals; ASDI XML payload/XSD semantics and TFMDI file
  profiles are not implemented, and the SWIM TFMData decoder is not treated as a TFMDI decoder.
  Explicit CAT021, CAT048 and CAT062 edition tables mark the tested profiles and partial typed-field
  coverage. For every interface, the registry distinguishes implemented session or parser scope from
  unsupported wire fields/extensions; it does not claim whole-ICD conformance, operational
  interoperability, or parity with unavailable legacy traffic.

## ASDI XML feed header profile (`oa-asdi`)
- **OA-ASDI-001** `parse_header` accepts only an exact 32-byte ASDI XML ICD v1.8 header. It parses the
  timestamp's 14 digits, NUL and blank pad, and the four network-order 32-bit fields; malformed
  timestamp/calendar fields and packet types outside 1 and 2 are refused. The timestamp is exposed as
  components only, without timezone or epoch interpretation.
- **OA-ASDI-002** Type 1 is accepted as a header-only heartbeat only when compressed and decompressed
  sizes are zero, per the public 2010 TFMS-to-ASDI ICD §3.2.1.3.2.2.10. Type 2 is not decoded;
  `inspect_packet` returns the typed
  `XmlCompressedNotDecoded` status after validating exact declared packet length.
- **OA-ASDI-003** Packet length and declared compressed/decompressed sizes are checked against explicit
  caller-supplied caps before any payload processing. Caller caps cannot exceed the provisional 1 MiB
  hard ceiling for each payload size, matching the current `oa-xml` input default; this is an
  implementation bound, not a size promised by the ICD. No allocation occurs; both caps are
  explicit function inputs.
- **OA-ASDI-004** Sequence values outside 0 through 100000 are refused. `observe_sequence` treats
  100000-to-1 as the ordinary wrap and reports other nonzero mismatches as a gap/discontinuity.
- **OA-ASDI-005** A later sequence zero is reported as `ZeroObserved`; it is not asserted to prove a
  restart or identify the cause of a discontinuity.
- **OA-ASDI-006** Synthetic tests establish only the declared header, size and sequence behavior. The
  profile does not implement sockets, registration, credentials, zlib, XSD validation or ASDI message
  semantics, and makes no live interoperability claim.


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
- **OA-TRK-017** After the tracker is built, every tracker-owned vector keeps its capacity through a run with
  confirmations, late measurements, drops, restarts and overflow of the output, the log and the track
  table. The fixed-capacity diagnostic checks capacities only; it does not prove that no same-capacity
  reallocation occurred. The standalone counting-allocator gate is the allocation proof required by C5.
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
- **OA-TRK-042** Tracker table memory is linear in track capacity. The full default table and
  rollback pool fit within 2,300 bytes per track plus the raw-altitude history from OA-TRK-355
  (32 bytes per track and 32 bytes per reserved snapshot). At 38,000 tracks without the fixed
  rollback pool, the table still uses less than 2,300 bytes per track.
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

- **OA-TRK-388** `oa-track-run --adsb <clip.oaf> --out <dir> --realtime [--with-output] [--max-tracks <n>]` offers
  every report of the clip to the tracker (no split, no scoring; the configuration is `scoring_config` of the
  full clip, printed with the run) and times each second's updates together with the shell's monotonic clock.
  It writes `realtime.txt` and `seconds.csv`: for every second from the first to the last, including seconds
  with no measurements and the last, the measurements, the processing time and the confirmed and tentative
  track counts after the last update in the second (a second with no updates carries the previous counts;
  the tracker is not advanced to the second boundary, and the peaks are sampled once per second). It reports the time's p50, p99 and maximum over all those seconds
  (an empty second counts as zero) and, labelled as such with their count, p50 and p99 over the seconds with
  at least one measurement; the seconds that did not finish within the second and the longest backlog under
  this queue model: each second's reports form one batch, available when the second ends, processed during
  the next second, leftover work carried on (backlog = max(0, backlog + work - 1 s)), a second finishing late
  when backlog is left, so a report waits at most 1 s plus the backlog ahead of its batch before processing
  starts and a batch completes at most its own work later (latency at most 1 s + backlog + that second's work), an upper bound because real reports arrive spread through the second; the final advance and
  flush apart (including the output spill with `--with-output`), CPU and wall time of the update window,
  which also hold the per-second track-count scan and bookkeeping, next to the sum of the per-second timers
  and the difference named as outside them (the sum is at most the window) and, read right after the final flush and before any
  digest, of the run from its start (ingest, set-up, updates, flush) against the traffic's span with the CPU
  source named (and, when the `/proc/self/stat` fallback served any reading, how many and its assumed 100 Hz
  tick rate), the conditions (input decoded and held in memory before timing, sorted by time, one thread:
  the figures exclude network input, decoding and interrupts of a live feed; a second with no measurements
  does no tick work, its tick rows are emitted with the next update and charged to that second), every refusal and overflow counter, resident memory with the process peak
  labelled, and the tracker table bytes. A run summary digest (the track log and the counters, taken after the
  timed window; `oa-track` has no tracker state hash) is always printed; `--with-output` also writes the output
  rows to column files and prints their digest. These digests are comparable only between realtime runs of the
  same clip, the same `--max-tracks` and the same build, not with the golden digests of the split, scored run. The run fails (non-zero exit) on an empty
  clip, on no measurements after the adapter, on any measurement outside the per-second log (no figures are
  computed over a truncated span), and, after printing its report, on any capacity refusal (not a full-load
  measurement) and on any lost output row or track log entry (the digests would cover truncated data). `--with-output` and `--max-tracks` need `--realtime`; `--realtime` refuses `--digest`,
  `--rows`, `--no-score` and `--no-compare`. The per-second log is a pure function of what it is given.

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
- **OA-TRK-297** The fusion layer records where a bucket's work goes and what its work limits leave out:
  tracks predicted for association, coarse-window width, wide-list tracks and overflow, emitted coarse
  candidates and visits, exact gate work, polar build/query work and attempted query count, anonymous
  plots deferred when association work is incomplete, initiation tests and work, the separate bounded
  initiation-refusal pass visits, and candidates alive at most. Polar work shares
  `MAX_BUCKET_POLAR_WORK` across all radars in a bucket. `polar_index_query_work` per
  `polar_index_queries` is charged work per attempted lookup (including a capped lookup);
  `polar_index_build_work` per `polar_index_track_visits` is charged work per slot visit across the
  wide-track classification and posting-build passes, including sorting work in the numerator. These
  are work counters, not elapsed-time or latency measurements. Candidate visits and exact gates share
  `MAX_BUCKET_TESTS`; neither budget promises an equal per-radar share. Counters are diagnostics and
  are not read back by the layer.
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
- **OA-TRK-299** If a work or pair limit leaves any anonymous gate unfinished in one radar-bucket,
  every anonymous assignment from that radar-bucket is invalid: an omitted edge can connect apparent
  components or change a winner. Every anonymous plot is deferred from both update and initiation and
  the plots whose own gating was unfinished are counted by `anon_plots_ungated`, all plots deferred
  because the graph is incomplete are counted by `assoc_incomplete_deferred`, and the batch is counted
  by `assoc_incomplete_batches`. Hard-address assignments remain independent. The association loop starts
  at a plot that depends deterministically on the bucket and radar, spreading completed gate work around
  the scan. When gating completes, the result is independent of that starting point. A solver refusal on
  a complete graph likewise defers every anonymous plot and counts it in `assoc_refused_deferred`.
- **OA-TRK-300** The coarse window of an association is per track: a track whose window (the coarse
  margin plus eight standard deviations) is above the standard search radius is on a fixed-capacity wide
  list included for each anonymous plot; the bounded polar index is queried at the standard radius, so one
  uncertain track does not widen every query. Width-band upper bounds and outward-padded radial cells
  select postings, and the exact `coarse_passes` predicate filters them. The ascending index results
  merged with the ascending wide list are a superset of every eligible track that could pass its own
  window, as if every eligible track had been tested. This universal superset property is an implementation
  requirement; finite regression cases do not prove it for all inputs. A track left out for want of
  wide-list room is counted in `wide_full`; because it could pair with any anonymous plot, the graph is
  incomplete and OA-TRK-299 defers it whole. An index build or query that reaches `MAX_BUCKET_POLAR_WORK`
  clears partial results, marks the graph incomplete, and also defers it whole. If a candidate list exceeds
  its remaining gate-visit share, all candidate distances are ranked by a packed nonnegative distance-bits
  and track-index key under the separate bounded ordering allowance; only the nearest candidates are
  gated, in distance then track-number order (OA-TRK-222). These budgets bound charged work; this
  requirement makes no latency or strict real-time qualification claim.
- **OA-TRK-301** Initiation tests an anonymous plot against the anonymous candidates near it, found in a
  grid of the candidates by predicted position (cells and a wide list of fixed capacity for the candidates
  whose reach exceeds the standard one), and converts the plot once. The candidates tested for a plot are
  every candidate the all-candidates test could pass; a candidate that finds the wide list full is not
  tested in that bucket and is counted in `init_wide_full`; `init_tests` counts the exact tests and
  `init_visits` the candidates the grid returned.
- **OA-TRK-403** Radar initiation reserves enough shared bucket work to read and compact the
  largest candidate list a radar's plots could leave. A candidate that reaches confirmation can
  retire into the tracker when search uses all its remaining allowance. Other radars can use
  any credit left after actual retirement work is charged.
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
- **OA-TRK-306** The production association index is a fixed-capacity polar posting array: one posting per
  standard-width track, keyed by an upper-bounded 512 m window band and a 500 m slant-range cell, with
  unsupported or singular track terms in an ALWAYS class. A query seeks only the radial cells widened by
  a full cell at both ends, then applies `coarse_passes` (including its wrapped azimuth/cross-range test)
  to each visited posting. It returns ascending track indexes; the polar query uses budgeted heapsort or
  a stable two-pass radix ordering according to the candidate count. For every plot in the supported
  range/azimuth domain, the output is a superset of the tracks `coarse_passes` accepts, including
  width/range boundaries, wrapped azimuths, nadir and non-finite terms. This universal property is an
  implementation requirement, not a claim established by a finite test set. The fixed build/query work
  cap and incomplete-graph fallback are OA-TRK-300; this association index does not use geographic
  projection, latitude bands, or longitude seam cells.
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
- **OA-TRK-325** Initiation has a named work budget shared by all radars of a bucket. The remaining
  budget is divided among radars with plots, carrying unused credit forward. Candidate visits,
  address lookups, index construction/query, exact tests and retirement are charged before
  their work; exhaustion preserves unvisited candidates, explicitly refuses omitted plots and
  restores scratch buffers. Partial plot pairs never reach the assignment solver.
- **OA-TRK-326** Aircraft addresses offered to fusion or radar validation fit 24 bits plus the non-ICAO
  flag (at most `0x01ff_ffff`), or the radar's absent-address marker. An address outside that domain is
  refused and counted before it can enter a pending queue or claim a reserved local radar key.
  Local key exhaustion is refused without wrapping, issuing the absent marker or reusing a key.
- **OA-TRK-327** A standalone, isolated allocation check runs selected real tracker and fusion, conflict-alert cycle and shared-predictor probe, SUA probe, runway final-monitor and assigned-runway-verification entry points after initialization with an observing global allocator. Its three allocation API controls must be observed; each claimed route and normal, degraded or explicit-overload outcome has exercise evidence; and any observed allocation, counter overflow, failed control or missing route fails the check. Results name the exercised synthetic scope and do not imply coverage of every possible input or real-time crate.
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
  eviction exactly once, preserving every other entry.
  - Proved with Kani, at the production capacity: the count cannot increase when only the clock advances
    (`full-sensor-monotonicity`), and the update and insert behaviour (`full-sensor-note-existing`,
    `full-sensor-note-new`). Each proof states its fixture, timestamp domain and capacity; none implies
    that the whole clock policy is correct.
  - Tested, not proved: the count equals the number of sensors inside the drop interval, against an i128
    reference, on 20,000 random full tables with clocks over all of `i64`, `i64::MIN` and `i64::MAX`
    stored, and drops up to one day.
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
- **OA-TRK-177** Association uses the polar posting index and per-track wide list specified in
  OA-TRK-300 and OA-TRK-306. Each returned candidate is checked with `coarse_passes` before exact gating.
  Candidate visits and exact gates are charged to the bucket-shared `MAX_BUCKET_TESTS`; polar index
  construction and queries are charged to the bucket-shared `MAX_BUCKET_POLAR_WORK`. No equal per-radar
  work share is promised. A cap that leaves an anonymous candidate graph incomplete is counted and defers
  that radar-bucket's anonymous plots as specified by OA-TRK-299. Universal candidate-superset correctness
  is required by OA-TRK-300/306; finite tests and observed run times are evidence for selected cases, not
  proof of that invariant or a strict real-time latency qualification.
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
- **OA-TRK-220** The polar coarse index works in the radar's slant-range and wrapped-azimuth terms, so
  geographic poles and the antimeridian do not create coordinate cells or discontinuities. A track
  with singular or unsupported polar terms is in the always-included posting.
- **OA-TRK-221** At nadir and near nadir, including ground range below 1 m, every track the exact
  coarse predicate accepts is retained by a completed polar query. If its work cap prevents completion,
  the query clears partial output and the incomplete anonymous graph is deferred under OA-TRK-299.
- **OA-TRK-222** When the visit budget is shorter than the candidates, the nearest by coarse distance
  (then lowest track number) are tested, not the lowest numbered; the plot is marked capped and the
  incomplete radar-bucket is deferred under OA-TRK-299 rather than applied from a partial graph.
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
- **OA-TRK-274** `split_reports_with(r, false)` is the split with the halving off: the same copy removal,
  conflict handling and quality handling, every distinct report in the first set, the second empty, and the
  same removal and conflict counts as the halving run.
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
- **OA-TRK-264** No order or selection in the tracker reads the sign or payload of a NaN: the greedy order of
  a component larger than `MAX_COMPONENT` uses `oa_math::cmp_canon_f64`, so positive, negative and payload
  NaN costs have one canonical order. When a plot has more coarse candidates than its visit allowance,
  `coarse_distance` maps NaN geometry to zero; the bounded ranking packs the nonnegative distance bits in
  the high portion and the track index in the low portion, then orders the integer keys. Thus all NaN
  geometries produce the same selection and ties use track number. Real-data digests are unchanged where
  no NaN reaches a migrated sort; a sort that does see NaN uses the canonical ordering rather than the NaN
  sign (OA-TRC-260 keeps raw float-order and sign methods out of non-test workspace sources).

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
- **OA-PRF-013** `oa-perf one --radar-no-altitude` removes Mode C altitude from every generated
  anonymous radar plot in the measured window, after an unchanged warm-up. It reports the start
  second and exact number removed. The radar schedule, plot count and truth labels remain unchanged,
  so a control and degraded run with the same seed exercise identical aircraft and plot times.

- **OA-PRF-014** Radar conversion refusals retain fixed-size reason counters for every conversion
  stage, distinguishing missing height, invalid plot, unreachable altitude and numerical geometry
  failures. These diagnostic subsets preserve existing aggregate loss totals and do not double-count
  loss or change qualification acceptance thresholds.

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
- **OA-TRK-347** (red-team finding RT-239) The position gate widens with the time since the track's
  last update, so after a gap it passes a single forged report of any plausible size. A report of
  a confirmed track that passes the gate but lies farther than `confirm_move_m` (926 m, a fixed
  distance that does not scale with the gap or the covariance) from the track's predicted position
  at the report's time is applied at once, exactly as a tracker without the rule applies it, and
  counted in `moves_unconfirmed`. The track keeps its measurement state from before the report
  (the update's own transaction capture without the tracker's counters, 2232 bytes) in a bounded
  pool of `confirm_pool` entries (64 by default, allocated once; the per-track columns are
  39 bytes: the move's flag, start, first report (time and sensor), old-path vote time, the end of a resynchronised move's marking and the snapshot index); a move that finds the pool full is applied and marked all the same, without a
  snapshot (`move_no_snapshot`): it can be confirmed or left standing but not rolled back. Until
  the next report decides (OA-TRK-348) every row the track emits, update row or tick row, and
  every forecast of it carries the flag `O_MOVE_UNCONFIRMED` (128), and the row's position
  covariance is scaled up so that its 95% radius, recomputed from the stored `f32` covariance,
  is at least the distance from the old path's prediction to the row's position plus the old
  path's own 95% radius. If that cannot be computed (no snapshot, an old position that cannot
  be represented, a result that is not finite) the row gets a circle of `FAIL_CLOSED_R95_M`
  (500 km: larger than any sector served, finite through every consumer; the largest `f32` once
  made the fusion layer's window statistics overflow) and `move_widen_failed` counts the row.
  The forecast (`tracker_forecast`) carries the flag and both covariances: its row has the
  filter's own, which `tracker_predict_checked` returns and the fusion layer's association and
  radar-disagreement test use (a plot on the old path must still refute a forged move: with the
  widened covariance it would pass the gate), and the widened one for output and display
  (`tracker_predict`; a failure to widen is fail closed there too, not counted). The reacquisition marking of OA-TRK-343 follows the filter's own radius,
  not the widened one. A late re-filter that moves a confirmed track's estimate by more than
  `confirm_move_m` marks it likewise, with the state from before the replay (`moves_late`, also
  in `moves_unconfirmed`; the row the replay emitted is flagged and widened as OA-TRK-347's later sentence says, RT-275). A tentative track
  is not covered (its second report has no velocity and moves far from its prediction as a
  matter of course; the plausibility rule judges it). The rule is on by default;
  `confirm_moves = false` turns it off only by an explicit choice. The move's report refreshes
  the drop timer like any update. A display must show the flag (follow-up). The row a late replay emits
  (before the move starts) is flagged and widened too (RT-275): its radius at least the distance
  to the old path's position at the row's time, the track as it was before the late report,
  derived from the ring, plus the old path's radius, `O_UNCERTAIN` set if that is over the
  threshold (`rows_uncertain` follows), the fail-closed radius if it cannot be derived; no other
  row changes. (Kani, what it proves, and only this: production `update_in_order`, given a large
  move, starts the move and the `emit` it calls marks the row it makes whether or not the old
  path's row exists; a row whose widening cannot be computed carries the fail-closed covariance
  (500 km) and is counted. Not covered by it, integration tests only: `update_in_order_held`,
  tick rows, the forecast, the rows of a late replay, the numeric value of the widened radius.)
- **OA-TRK-348** The track's next report decides an unconfirmed move, by distances from the report
  to the moved track's prediction and to the old path's (the snapshot's) at the report's time,
  and only if it is itself applied: the normal update path runs and the decision is committed by
  what it did, before the report's row is made, and only if the step commits: nothing of a move
  (its start, its end by a decision, a restart or a resynchronisation) changes until then, so a
  numerical refusal that undoes a step, in order or in a late replay, leaves the move, its
  snapshot and the pool as they were. Under 0.5 s after the move it decides nothing (a
  copy or a burst). Within the track's window and at least 0.5 s after: it agrees with the moved
  track (within the agreement limit, and not nearer the old path; a tie or a move with no
  snapshot counts as agreeing with the moved track) and the move is confirmed once the report
  is applied to the moved track, the flag cleared, the snapshot returned to the pool
  (`moves_confirmed`); or it agrees with the old path and not with the moved track, which a first time is a vote
  (`moves_old_path_votes`: applied as it is, the move stays, marked) and a second time, at
  least 0.5 s after the first, rolls the move back (OA-TRK-349, `moves_rolled_back`; OA-TRK-352); or it agrees with neither, or comes after the
  window, and the move stands once the report is applied (`moves_undecided`), the flag cleared. If
  the report is then itself a large move, the track stays marked, keeps the original snapshot, and
  the window runs from the later move (the earlier move counts as undecided, the later as a new
  unconfirmed one). A report that is not applied (the gate refuses it, or the update is refused)
  decides nothing: the mark and the snapshot stay, and `moves_decision_rejected` counts it. The
  window is four of the track's expected report intervals (the track's own, else the
  configuration's), not under `confirm_window_ns` (5 s: live 1 Hz data keeps it) and not over
  `drop_after_ns` (`check_config` refuses `drop_after_ns` under `confirm_window_ns`). The
  agreement limit is `confirm_agree_m` (300 m) plus `0.5 a t^2` (`a` the gate's manoeuvre
  allowance, `t` the time since the move), never over `confirm_move_m`. A distance that is not
  a number agrees with nothing, and with a snapshot present an old path that cannot be predicted
  at the report's time is no agreement either (OA-TRK-353). Every unconfirmed move ends in exactly one counted outcome:
  confirmed, rolled back, undecided, or abandoned (the track ended, restarted, the filter
  resynchronised, or a late re-filter demoted the track: `moves_abandoned_ended`, `_restarted`,
  `_resync`, `_demoted`), so that
  `moves_unconfirmed` equals the sum of the outcomes plus the moves still unconfirmed
  (`tracker_moves_active`); the report prints it. (Kani, what it proves: the pure
  function `resolve_decide`, of times, distances and the configuration only, for every
  configuration `check_config` accepts and every time and distance: a lone move decides nothing,
  a confirmation or rollback needs a report at least 0.5 s after the move, within the window and
  within the limit, a rollback an earlier old-path report 0.5 s before it, a tie confirms, an
  unpredictable old path confirms and rolls back nothing; and, separately, production
  `update_in_order_held` with a report that is not applied, see OA-TRK-354. Not proved: that
  the real filter's applied report decides as described, which the integration tests check.) Residual risk, stated, every way to a false track that is not marked: (1)
  a second forged report at least 0.5 s after the first that agrees with the moved track and
  not with the old path confirms the move (`moves_confirmed`): the track is unmarked at the
  false place and nothing in the tracker refutes it; (2) a report in the gate that agrees with
  neither path, or comes after the window, leaves the move standing (`moves_undecided`); (3) a
  run of consistent reports the gate refuses re-synchronises the filter onto them, which ends
  the move (`moves_abandoned_resync`). Each is counted in its own outcome; only a second source
  (radar or multilateration) can refute a forger who keeps the false place up.
- **OA-TRK-349** The snapshot is derived from the update's own transaction capture
  (OA-TRK-354 states what is shown of that), and a rollback puts the track's measurement state
  back to it (filters, measurement history, times, counts, category, state) and the report
  is then applied to that through the normal path; if the report is then not applied the
  rollback is undone and the move stays. The columns the fusion layer writes
  (`disagree_until_ns`, `expected_ns`), the tracker's counters and its bound on the newest
  update are not the snapshot's and are kept. A late re-filter while a move is unconfirmed
  (routine with a second sensor) makes the snapshot wrong for the new history, so the rollback
  point is derived again from the ring: the track as it was before the report that made the
  move (the state after the entry before it, derived as the late re-filter derives its base
  state), written into the move's pool entry; the move's mark and window stand. Only if that
  report is no longer in the ring (the replay itself refused it, or it is the oldest held) the
  snapshot is returned to the pool and `moves_rollback_lost_late` counts it (the mark stays; a
  later refuting report cannot roll back). A replay that demotes the track to tentative,
  re-synchronises it or restarts it ends the move (counted); a replay that is undone leaves the
  move as it was. A track's end, restart or resynchronisation ends the move (counted). A report
  refused after a rollback was tried still counts in the moved track's run of refusals. (Kani,
  what it proves: the rollback restores the scalar columns, the category and the current
  filter's time and first state value, leaves the two outside columns and the newest-update
  bound, and returns the snapshot to the pool; `snap_take` then `snap_restore` puts back the
  current filter, the anchor filter and the ring with its length and head, bit for bit, one proof
  each. The rollback uses `snap_restore`, not `restore_track_columns`, which the OA-TRK-335
  proofs are of.)
- **OA-TRK-352** A rollback to the old path needs the same corroboration as confirming a move,
  and the two are symmetric (RT-273): `CONFIRM_REPORTS` (2) reports at a place decide a move,
  the move's own report counting for the moved place, so one further report at least 0.5 s
  after confirms it, and the old path needs two reports of its own at least 0.5 s apart. The
  first report agreeing with the old path (and not the moved track) is a vote: applied as it is
  (the gate usually refuses it), the move stays on the moved state, marked, the time kept
  (`moves_old_path_votes`); a second one, at least 0.5 s after the vote and inside the window,
  rolls back; one less than 0.5 s after it waits. A report that agrees with the moved track
  confirms as before and ends the count. One stray report at the old place, or one replayed by a
  forger, therefore cannot undo a genuine relocation. A report the gate refused counts as the
  vote exactly as one it applied would (the moved track usually refuses it), and it counts when
  it is one of a run of refusals heading for a resynchronisation. The exception is the
  resynchronisation itself: five refused reports in a row (the filter's own rule) re-synchronise
  the track onto them, which ends the move (`moves_abandoned_resync`) but not silently: the
  rows after it carry `O_MOVE_UNCONFIRMED` and `O_UNCERTAIN` (the radius lifted over the
  uncertainty threshold, no old path being left) until the move's window would have ended, and
  `moves_resync_marked` counts it. The marking is judged at each row, so a silent track keeps
  its marked rows past the window until its next report or its drop (nothing clears it by the
  clock alone); a new move on the same track ends it at once (the new move's own mark rules, its
  own window and decision). A late replay that releases the move's snapshot clears the
  vote. (Kani: `resolve_decide`, OA-TRK-348.)
- **OA-TRK-353** An old path that has a snapshot but cannot be predicted at the report's time
  (its distance is not a number) is not agreement with anything (RT-276): the report is applied
  as it is, `moves_old_path_unpredictable` counts it (only if the report is applied; a refused
  one counts as a decision rejected), and the move stays, marked, until the
  window ends, when the next applied report leaves it standing (undecided) as usual. A report
  that agrees with the moved track does not confirm the move meanwhile; with no snapshot at all
  (the pool was full) the old path is unknown, not unpredictable, and the moved track decides
  as before.
- **OA-TRK-354** What the rule's proofs say, exactly (RT-274). The snapshot taken from the
  update's own capture equals the track's state taken apart from it in every field (unit test
  on a track with a wrapped ring; four Kani proofs with the scalars and category, the current
  filter, the anchor filter and the ring symbolic in turn, each comparing every field).
  `update_in_order_held`, with `decide` and `filter_apply` stubbed to what the real rejected
  path does and any distances, given a report that is not applied: the whole track as the
  transaction capture sees it, its move columns and its snapshot are as before, except the
  current filter's run of refusals and the counters `jump_rejected` (one more),
  `moves_decision_rejected` and `moves_old_path_votes` (at most one of them one more), which
  change as the real rejected path changes them; `moves_old_path_unpredictable` never changes
  (a refused report decides nothing and is counted as a refusal); no row, no outcome.
  Nine harnesses over eight groups of columns (the move's own columns and the counters, which two harnesses take; scalar columns; first and confirmation times with the move's first report; state and counts; category; current filter; anchor filter; ring): in each the moved track differs from the snapshot in every group by one fixed value and exactly one group is symbolic, because a proof with every group symbolic at once does not fit the memory budget. Symbolic in all nine: the move's start time, the old-path vote time (or none), the report's time (a 1000 ns window after the newest update) and flags, the stubbed distances. Symbolic in one only: the last-update time and update count; the track's first and confirmation times and the move's first report time and sensor (`mv_first_ns`, `mv_first_sens`; fixed in the others, and not made symbolic there again without a run to show it fits); the state (confirmed or tentative), hit count, ring length and ring head; the category; the current filter (run of refusals below `RESYNC_AFTER`); the anchor filter; in the ring harness only the first ring entry (`ring_m[0]`). Everything else is at one fixed value, including `disagree_until_ns`, `expected_ns` and `newest_update_ns` (checked unchanged at that default only) and the rest of the report. `decide`, `filter_apply` and `filter_miss_m` are stubs. Loops over the ring are bounded by the constant `RING_DEPTH` (12), no unwind bound of the harness's own. Afterwards the snapshot in the pool
  equals the one taken, field for field, bit for bit; the move's start, first report time and
  sensor are as before and the hold flag is off. They hold for the default configuration
  (`resync_consistent_k` 0 and the run of refusals below `RESYNC_AFTER`): the refusal that
  resynchronises is the integration test of OA-TRK-352. Not covered by any proof (integration tests): a report that is applied or is itself a large
  move, ticks, forecasts, the late replay and the rows it emits.
- **OA-TRK-355** The vertical filter cross-checks a reported barometric vertical rate against
  accepted raw barometric altitude history when the earlier altitude is more than 0 s and at
  most 10 s older. A rate above four unknown-state prior standard deviations (7,200 ft/min) is not used to
  initialize velocity until its integrated displacement both exceeds the quantization uncertainty
  and agrees with the independent altitude delta. An insufficient high-rate sample does not move
  the raw-altitude anchor, so dense samples can accumulate evidence. A clearly contradictory rate
  is omitted while an accepted altitude still updates the filter. The raw history advances only on
  accepted, strictly later barometric altitudes; repeated altitude-gate rejection resynchronizes
  from the raw altitude-derived rate only when its displacement exceeds the same uncertainty
  bound. Coverage: tracker-entry tests for stable altitude under -15,000 ft/min, corroborated
  rapid descent, and a contradictory rate during an established descent; Kani rollback field
  comparisons include the raw history.
- **OA-TRK-350** A report whose distance from the predicted position is at most `confirm_move_m`
  is applied exactly as before: its outputs are bit for bit those of a tracker with the rule off.
  A move over it is applied exactly as without the rule too: a confirmed move leaves every row's
  position, time and flags, and the covariance of every unmarked row, and every counter that is
  not the rule's own, those of a tracker with the rule off, except `rows_uncertain`, which is
  higher because a marked row's widened radius can make it uncertain by definition.
- **OA-TRK-351** `check_config` requires `confirm_move_m` finite and positive, `confirm_agree_m`
  positive, finite and below `confirm_move_m`, `confirm_window_ns` over 0.5 s (and at most one
  day) and not over `drop_after_ns`, and `confirm_pool` at most 2^20. The counters are in the
  statistics and the report. The tracker's tables hold 39 bytes of columns per track and the
  pool: the tables go from 8.66 MiB to 8.94 MiB at 4096 tracks and from 80.5 MiB to 82.1 MiB at 38,000 (141 kB of pool and its free list, and 39 bytes a track; the per-track figure is the sum of the columns' element sizes, checked by a test). The pool's size is analysed beside `CONFIRM_POOL`: the most moves unconfirmed at once on the two development days was 7 on each (the 2026-11-14 day, and the 2026-11-15 day as the check), and a forger who fills it costs rollback, never the mark.
  A report the filter holds back as a large move when the rule is off cannot happen; it is
  refused and counted (`moves_internal_refused`), never applied.

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
  unit is discarded; received numbers are classified first, in order, duplicate or gap. Entering
  INACTIVE clears the inbound sequence baseline so the next tracked number is classified first.
- **OA-NAM-015** An answered-message table of at most 128 entries matches each LAM, RLA, PLA or LRM
  to the message it answers by kind and number, reports an overdue one (resending first when
  configured), and refuses a new message, numbering nothing, when full. A response-expected send
  whose next number remains pending is refused and counted without advancing the sequence.
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

- **OA-NAM-036** Item 15 preserves and validates the ICAO speed/level header, DCT and ATS
  designators, significant-point forms, speed/level changes, IFR/VFR transitions, and cruise
  climbs in FPL and CHG. Missing or malformed groups return a Field 15 refusal. Parsing preserves
  filed point spellings and does not guess coordinates or resolve duplicate names; route
  expansion and semantic refusal remain the responsibility of a caller using the current
  NASR/CIFP model.
- **OA-NAM-037** The field codec parses and round-trips the ICAO Appendix 3 DLA message fields
  3, 7, 13, 16 and 18. A DLA updates departure time and does not create, replace or amend Item 15.
- **OA-NAM-038** Given syntactically parsed ICAO Item 15 route elements and the current NASR/CIFP
  model, resolve named, coordinate and bearing-distance points only to exact model identities;
  preserve full point identity while expanding ATS routes and coded SID/STAR paths; enforce
  available airway MEA values; require explicit source-backed magnetic variation for
  bearing-distance points; and return stable Field 15 refusals for missing, ambiguous,
  disconnected, unsupported, invalid-source or over-capacity cases without inventing a point or
  selecting an arbitrary candidate. Semantic refusals map to correlated LRM replies under the
  active NAM profile. Intake of NAM FPL/CHG/DLA/CNL messages into the regional runtime is a
  separate caller integration.

- **OA-NAM-039** The regional shell applies FPL/CHG/DLA/CNL only from opaque `SessionAcceptedMessage`
  deliveries returned by the active NAM session ingress. A positive full flight key, plan ID,
  facility centre, and capture clock are supplied by verified source context; callsign, header
  number, or route text never creates or joins identity. FPL creates the authoritative `oa-flight`
  plan and an immutable resolved route revision in the existing regional `RouteStore`; CHG replaces
  only the route when Field 15 is present; DLA updates the revised departure time only; CNL cancels
  the plan. Duplicate, gapped, stale, missing-context, malformed semantic, over-capacity, and
  catalogue failures return correlated LRM responses and do not apply flight or route state.
  Accepted deliveries produce a correlated LAM value. Synthetic coverage exercises the ordinary
  framed-session ingress through the regional runtime flight and route stores. This does not
  establish a preferred-route or restriction database, operational network delivery, FAA approval,
  facility qualification, or a source-backed live full-day result.

## ACC-to-ACC framed-byte peer fixture (A7)
- **OA-A7-001** Two active facility sessions exchange Rev G RTI, RLA, RTA and LAM messages through
  the canonical encoder, four-digit transport frame, unframe and parser. Message numbers,
  sender/receiver and response references are preserved. Both receiving-center RTA acceptance and
  initiating-center RTA retraction are exercised. RLA is the protocol response to RTI; LAM only
  confirms computer handling of RTA. Neither is treated as proof of the other center's operational
  ownership state.
- **OA-A7-002** A framed LRM matched to an outstanding RTI is surfaced as that RTI's rejection
  (Rev G Part II B.1.8.1 and Table B-1); a subsequent RTI uses a new sequence number. This is a
  fixture-level later attempt, not an automatic retry or a Rev G-defined recoordination algorithm.
- **OA-A7-003** The peer fixture exposes the session's `First`, `InOrder`, `Duplicate`, and `Gap`
  classifications for framed RTI traffic, including a lower number after a gap; messages remain
  visible to the application with their classification (Rev G Part II B.2.1).
- **OA-A7-004** The locally configured response timer for a flight-planning FPL reports an
  unanswered message at its configured 60-second deadline. Rev G Part III §6.1 specifies under
  60 seconds for at least 99% of flight-planning LAM/LRM responses in normal operations; this is
  not an RTI/RTA decision timeout, for which §6.1 states a less-than-six-second interface
  communication maximum. A separate RTI test exercises a 100 ms local response-timer setting only;
  it does not claim Rev G sets that interval or implements the application-level VSP failure rule.
- **OA-A7-005** The framed peer delivers an EST followed by a CHG in sequence (Rev G Part II
  §§3.1.2–3.1.3). It asserts NAM transport ordering and parsing only; applying the amendment to
  `oa-flight` remains a separate caller integration requirement.
- **OA-A7-006** The regional adapter accepts an RTI only with an explicit existing full
  `FlightKey`, exact Field 31 facility/sector mapping, and caller-configured endpoint unit IDs;
  it never synthesizes GUFI or joins by callsign. It applies RTI/RLA/RTA/LRM receipts to that
  same full key and refuses a response whose 03(c) reference or peer facilities differ.
- **OA-A7-007** The adapter keeps NAM semantics distinct in the `oa-flight` operation mapping:
  RLA is an offer routing/update, target-originated RTA is acceptance, initiator-originated RTA
  is retract, and target LRM is failure. RTU is not converted to an ownership operation, and
  missing RTA/RTI timeout is not synthesized by the parser adapter.
- **OA-A7-008** Framed input reaches the handoff mapper only through a message token produced by
  the public `oa-nam::session::step` state machine: ATM handoffs require `Action::Deliver`, and
  RLA/LRM response handling requires `Action::Response` correlated to the outstanding RTI.
  Duplicate deliveries produce no flight operation. Sequence-gap deliveries remain visible to
  the application, are mapped according to their message semantics, and increment an explicit
  ingress gap counter; they are not silently discarded. Wrong-peer, discarded, unmatched-LRM and
  unmatched-response actions produce no flight operation. The adapter keeps no second
  message-number cursor and does not replace NAM timeout or acknowledgement behavior.

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

## COMPASS export (oa-compass)

A tool crate (`tools/oa-compass`, class `split`) that writes one recorded UTC day of OpenATCA tracker output
and of ERAM tracks as raw ASTERIX CAT062 Ed 1.21 for OpenATS COMPASS. The conversion of a row to the values of
a record is core (`src/core`); the encoder glue, the files and the binary are shell. CAT021 export is not in
scope.

- **OA-CMP-001** A tracker tick row becomes the values of one record in the LSBs of the specification: position
  in counts of 180/2^25 degree (I062/105), velocity in 0.25 m/s east and north (I062/185), barometric altitude in
  1/4 FL (I062/135, no QNH correction), vertical rate in 6.25 ft/min (I062/220), time in 1/128 s since midnight
  UTC (I062/070), each rounded to nearest. An ERAM sample becomes position, time, velocity (0.1 kt to 0.25 m/s)
  and the reported altitude as a measured flight level (I062/136).
- **OA-CMP-002** The time of a row is rounded to 1/128 s and the UTC day is that of the rounded time: a row within
  1/256 s before midnight is on the next day with time of day 0, never at 24:00:00. A date is checked against the
  calendar (leap years, month lengths) and the day count and its inverse agree.
- **OA-CMP-003** A value a row does not have, or that does not fit its item (not a number, outside -15 FL to 1500 FL,
  outside the 16-bit velocity or climb range), is left out of the record, never replaced by zero or clipped, and is
  named in the row's notes; the shell counts each note per system and day and the README lists the counts. A row
  whose latitude or longitude is out of range, or whose time does not convert, is refused and counted, not written.
- **OA-CMP-004** I062/500 subfield 3 (APW) of an OpenATCA row is the standard deviation of the position in
  latitude and in longitude from the position covariance: `sqrt(nn)` and `sqrt(ee)` metres over the WGS-84
  meridian radius and over the prime vertical radius times the cosine of the latitude, in counts of 180/2^25
  degree, rounded to nearest; a component of 0 counts is raised to 1 and one past 65 535 is the maximum, each
  noted; a covariance that is negative or not a number leaves the item out. ERAM rows carry no I500 and say so.
- **OA-CMP-005** I062/080 has CNF 0 (confirmed) and SIM, SPI, MON, MRH, SRC at 0; a coasting row adds the three
  extents with CST (bit 8 of the third) set and an OpenATCA radar-disagrees row sets AFF (bit 4 of the first).
  The row flags `O_UNCERTAIN` and `O_REACQUIRED`, which CAT062 has no field for, are not encoded and are counted.
- **OA-CMP-006** I062/380 carries the target address only for a 24-bit Mode S address (a track keyed by a radar
  handle or a non-ICAO address has none, noted) and, for ERAM, the callsign as an identification only if every
  character is `A-Z`, `0-9` or space (NUL padding becomes spaces); no other character is changed or dropped.
  I060, I200, I290 and I295 are never written, and counted as omitted on every row.
- **OA-CMP-007** Encoding then decoding with the `oa-asterix` decoder gives records equal to those encoded, for
  generated rows of both systems and for the tick rows of the tracker run on a slice of a real recording (skipped
  when the recording is absent); the decoded position differs from the source position by at most 27 units of
  1e-7 degree.
- **OA-CMP-008** A data block holds at most 256 records, a record is at most 49 octets (every item present), so a
  block is at most 12 547 octets, under the 65 535 a LEN holds; the encoder refuses a block over it.
- **OA-CMP-009** A file holds one UTC day. Without `--split-days` a row on another day than the one asked for
  refuses the export (E5) and no file is left behind; with it, one directory per day holds `openatca.ast`,
  `eram.ast`, `data_sources.json` and `README.txt`, and no file holds a row of another day.
- **OA-CMP-010** Track numbers (I062/040) are assigned per file in order of first appearance, from 1, one per key and
  stable over the file; more than 65 535 distinct keys in a file is a refusal, not a wrap.
- **OA-CMP-011** Records in a file are in non-decreasing I070; a record earlier than the one before it is refused.
- **OA-CMP-012** ERAM samples are sorted by position time, flight and input order; a sample with the same flight,
  position time and content as an earlier one is dropped and counted, one with different content is kept and
  counted as a conflict, and a sample whose flight is not in the flight table is counted and not written.
- **OA-CMP-013** Every finished file is read back from disk with the decoder and must hold exactly the records and
  octets written, with the right data source and time order; a truncated or altered file fails the export.
- **OA-CMP-014** `data_sources.json` is `{"content_type":"data_sources","content_version":"0.2","data_sources":[...]}`
  with one `Tracker` entry per system, OpenATCA (SAC 255, SIC 11) and ERAM (SAC 255, SIC 12), and one `ADSB` entry, ADS-B (SAC 255, SIC 13), which parses as JSON.
  The SAC is a test pair of this export, stated as such in the documentation and the README.
- **OA-CMP-015** The README states the UTC date to give at import, the SAC/SIC of each system, the items of each
  file with the rows that carry them, the rows written, every omission and note by reason with its count, the
  inputs, the I500 conversion and the encoder build.
- **OA-CMP-016** The tracker run behind the OpenATCA file is the scorer's: set A of the split, the scoring
  configuration, one tick row per second per confirmed track; update rows are not exported and are counted; a
  run that lost rows to a full output table is refused.
- **OA-CMP-017** Every count the SFDPS ingest keeps of what it saw and dropped (documents, refusals, rows without
  a GUFI or position time or position column, rows past the document table, field errors, coast and altitude
  counts) is printed in the README's ERAM block with its reason, and the export is refused if the flight table or
  the track-sample table overflowed (rows were lost). Counters saturate, and an index that does not fit a
  `u32` is a refusal, not a truncation.
- **OA-CMP-018** `--inputs all` (default) offers the OpenATCA tracker's adapter every ADS-B report of the clip, `--inputs split`
  only set A of the scorer's split; the README states the mode, whether the two files are like-for-like, and, for
  `split`, that a reference built from set A would be leaked truth. I062/080 MON is 1 for OpenATCA (ADS-B only) and
  0 for ERAM; the README says why. ERAM altitude, whose reference the feed does not state, is I136 at or above
  18 000 ft and I135 with QNH = 1 below, stated in the README as an inference. A valid rate of climb is exported
  on a row without an altitude and counted; a longitude of +180 degrees is written as -180 and counted.
- **OA-CMP-019** One ADS-B report converts to the counts of CAT021 Ed 2.4's items exactly as the item definitions
  give them: I073 (1/128 s), I131 (180/2^30 degrees), I080, I145 (1/4 FL), I140 (6.25 ft), I155 or I157
  (6.25 ft/min, by the report's rate flag), I160 (2^-14 NM/s, 360/2^16 degrees), I170, I020, I090, I210 and
  I040 (ATP, GBS, CL 2); a full report (every I090 field known) notes only what the source never has.
- **OA-CMP-020** What a report lacks, or has outside the range of its item (FL -15 to 1500, height -1500 to
  150 000 ft, vertical rate 15 bits, a track of 360 degrees, a character outside A-Z 0-9 space, an emitter
  category without an I020 code, MOPS version 3), is left out of the record and counted by item and reason;
  nothing is clipped.
- **OA-CMP-021** A non-ICAO address is written with ATP 3, a ground report sets GBS and has no flight level, a
  field that does not fit its bits counts as unknown, a callsign padded with NUL is the callsign padded with
  spaces (a NUL inside it is refused), and the emitter categories map to the I020 codes.
- **OA-CMP-022** A position beyond 90 or 180 degrees refuses the report; +180 degrees is written as -180 and
  counted; the position error is at most 1 unit of 1e-7 degree; a time within 1/256 s before midnight belongs
  to the next day.
- **OA-CMP-023** `adsb.ast` decodes with the Ed 2.4 decoder to the values converted; every block is decoded
  again before it is written and the file is read back from disk; reports are written in time order, and stale
  positions and source types that are not ADS-B are left out and counted. The README reconciles the reports
  offered with stale, not ADS-B, written and refused ("reconciles" or "DOES NOT RECONCILE").
- **OA-CMP-024** A report earlier than the one before it, or on another UTC day than the requested one, refuses
  the export unless days are split, which writes one directory per day.
- **OA-CMP-025** `--inputs all` runs the clip through `oa_track::score::split_reports_with` (the scorer's split
  function) with the halving off: copies removed, every conflicting report kept (rule stated), counts printed in
  the README; the same reports are offered to the tracker's adapter and to `adsb.ast`; the tracker uses the subset
  that passes its quality gates (OA-CMP-028).
- **OA-CMP-026** `data_sources.json` lists the ADS-B source (SAC 255, SIC 13, DSType "ADSB", a string of the COMPASS manual's set); the README states
  the Ed 2.4 format, how each value was made, every omission with its count, and that a reference built from
  `adsb.ast` is not independent of OpenATCA's inputs (UK-008) and that ERAM also uses ADS-B.
- **OA-CMP-027** `oa-compass slice --in DIR --from HH:MM:SS --to HH:MM:SS --out DIR2` reads each `.ast` file with
  the decoder (CAT062 Ed 1.21, CAT021 Ed 2.4), keeps the records with I070 or I073 in [from, to), writes them
  again unchanged in verified blocks, reads each file back, copies `data_sources.json` and writes a README with
  the window, the source directory and the records in and kept per file; a window across midnight or empty, a
  category other than 21 and 62, and a damaged file are refused, leaving no partial file.
- **OA-CMP-028** I021/090 is in every ADS-B record (Ed 2.4 5.2.16, Encoding Rule: "This item shall be present in every
  ASTERIX record. Extensions are sent only if at least 1 bit is set to 1", quoted from the specification text used). Its primary subfield is always written: an
  unknown NACv or NIC is the MOPS code 0 (unknown), each counted; NIC is readsb's value written as NUCp/NIC, and
  under MOPS version 2 it is written unchanged with the mapping UNVERIFIED and each row counted; for version 0
  the NUCp is written 0 and counted (readsb's `nic` is a NIC) and the NUCr (the NACv field) is written 0 whatever
  the report's NACv holds, counted as "NUCr not available" and not as an unknown NACv; a version 0 row is never
  mapped through the NUCp column of the PIC table. Beyond the primary subfield no zero stands in for a missing value: each extension
  (first NICBARO, SIL, NACp; second SILS, SDA, GVA; third PIC) is written only while every field in it and in
  those before it is known, the chain stops at the first extension with an unknown field and the rows cut short
  are counted per extension; an all-zero extension is sent only before a later one that has bits, so a decoder
  cannot tell a known 0 from an extension not sent. NICBARO is the clip's NIC_baro (a clip written before the
  column carries none). PIC is taken from NIC and the MOPS version (1 or 2) by the Ed 2.4 table (`pic_of`; NIC 6 gives none; PIC 0 is
  never generated), is written when the chain reaches it, and rows for which it cannot be taken are counted. NIC or NACp above 11 and NACv above 4 are not encoded and count as unknown. I040 GBS is 1
  when the report says ground and 0 when it carries an altitude (an inference from readsb); a row with neither
  is written GBS 0 and counted as "ground state unknown"; ARC, CL, RC, RAB, DCR, SIM, TST and SAA are defaults,
  listed in the README as defaults and not facts. I210 LTT is 2 for the source types adsb_icao and adsb_icao_nt
  (ASSUMED from the source type, not proven: such a report can come through a UAT-to-ES converter), 1 for a UAT
  source (the clip's types have none), and 0, counted, for a clip without the source type column or for
  adsb_other; the source types mlat, tisb_*, adsr_*, mode_s and the rest are not ADS-B reports, are not written
  and are counted by source type. The I090 and I210 octets are checked against bytes worked out by hand from the
  specification. I160 is omitted on a ground report and counted; the README states that adsb.ast is a superset
  of what the tracker was fed, with the tracker adapter's removals per reason, and keeps the independence
  statement (UK-008).
- **OA-CMP-029** The three data sources are SAC 255 with SIC 11 (OpenATCA), 12 (ERAM) and 13 (ADS-B), one constant
  each; 255/1 is left free because COMPASS writes its reference there, and the README says so.
- **OA-CMP-030** The export refuses an `--out` that exists and is not empty (the same rule as `slice`), before
  anything is written. The README lists exactly the files this run wrote into its directory, and its title,
  import instruction, data-source entries and system paragraphs follow which systems were exported (OpenATCA,
  ERAM, ADS-B): an ERAM-only run's README does not mention `openatca.ast`.
- **OA-CMP-031** The position error limit is enforced: when a decoded position differs from its source by more
  than `MAX_POSITION_ERROR_E7` (27 for CAT062, 1 for CAT021) the export fails with a verification error and no file
  is committed.
- **OA-CMP-032** Every README claim about the rows is backed by code: the tracker's `O_IDENTITY_UNCONFIRMED` rows
  are counted and written with CNF = 0 (the flag means an unconfirmed identity, not a tentative track, and the
  README says so); MON = 0 and CNF = 0 for ERAM are stated as assumptions ("not in the feed") and counted as a
  note on every ERAM row; each system's README section states the nature of its rows and the measured interval
  between consecutive rows of one track (median, 90th percentile, longest, pairs), computed by code; ERAM track
  numbers are the export's own (one per flight) and the flights per address are counted.
- **OA-CMP-033** The commit the README names is embedded at build time (`build.rs`, `git -C` the crate's own
  directory, with a note if tracked files are modified), `unknown` only if git is unavailable at build.
- **OA-CMP-034** The main export commits like `slice`: every file (data, `data_sources.json`, README) is written as
  `.partial`, all are renamed in one pass, and on any error every file this run created is removed.
- **OA-CMP-035** Every ERAM flight with an address is checked against the ADS-B reports of the same address
  (nearest report within 10 s, at least 10 pairs, median distance over 25 km contradicts; the rule and its reason
  are in the README); the check is report-only by default (`--eram-contradicted-address keep`: the address ERAM
  published is written, a contradicting flight is counted and listed with its evidence); with `drop` the
  contradicting flight is written without I380 address and the README says the file was altered and lists the
  flights; a flight with too few pairs is counted as not checked in both modes.
- **OA-CMP-036** `--openatca-rows ticks|updates` (default `ticks`, unchanged) selects the tracker's one-second tick
  rows or its update rows (one per applied report, at the report's time, earlier-than-written rows counted and
  left out); the README states the mode prominently and counts the rows of the other kind.
- **OA-CMP-037** The README states that OpenATCA publishes a calculated altitude (I135) and no measured flight level
  (I136), so COMPASS's "Mode C Present" does not apply to it.
- **OA-CMP-038** The address-check counts are exact: a non-ICAO address (above 0xFFFFFF) is its own outcome,
  samples refused for position or time are not paired, copies and same-time conflicting samples are reduced to
  the first before pairing, agree + not checked + contradicts + non-ICAO equals the flights with an address (or
  the run fails), the arithmetic is checked, and the median is the lower middle value, as in `stats`.
- **OA-CMP-039** The nearest-report search pairs a sample with the nearest report in time (the earlier when equally
  near) within 10 s inclusive, and a median over 25.0 km (not 25.0 km itself) contradicts.
- **OA-CMP-040** The row counters (ticks, updates, coasting, uncertain, reacquired, unconfirmed identity) and the
  time of the last update row change only for rows that were written; a refused row is counted under refused
  only; written + refused equals offered. In updates mode each batch is taken in time order.
- **OA-CMP-041** The export and `slice` are all-or-nothing for directories too: `--out` is created only after the
  refusal checks, every level a run creates is recorded, and on any error each is removed, deepest first, with
  `remove_dir` only.
- **OA-CMP-042** An `--out` that holds only `.partial` files is refused with a message that an earlier run was
  interrupted and names the files; they are not removed.
- **OA-CMP-043** An interval histogram with no pairs is stated as "no consecutive rows of one track (0 pairs)", not
  as zeros.
- **OA-CMP-044** The position error limit is tested at the limit plus one for CAT062 and CAT021, a scale error
  fails the whole run with nothing left in `--out`, and a failure after the first partial (ERAM load, ADS-B
  export) leaves nothing.
- **OA-CMP-045** `slice` refuses a source directory whose `README.txt` is missing or unreadable.
- **OA-CMP-046** The build commit's modified flag follows the tracked files of the trees the crate is built from
  (the build script reruns on the git index and on those trees).

- **OA-TRK-370** The canonical row layout is tied to the spill column table at compile time: the
  fields tile bytes 0 to 84 of the row once each, entry `i` of the field table is column `i`, and
  every field range is checked before use, so there is no write that lands nowhere. Hex hashes
  parse only as 64 lowercase digits.
- **OA-TRK-371** A digest accepts at most 2^20 seconds and 2^20 rows in one second; a row that
  would pass either is an error and is not counted, and the rows-file writer therefore never holds
  a whole oversized second.
- **OA-TRK-372** The header of a rows file is checked in full (magic, version, row size, exact
  length), and the hash of a second's rows is computable from the rows alone (`hash_rows`).
- **OA-TRK-373** The scorer's flag join counts duplicate (address, millisecond) keys and never
  overwrites one; rows that disagree make the key a conflict. A sample with no tracker row, or only
  a conflicting one, is a third state, scored in its own `UNMATCHED` group and written as
  `unmatched`, never as "no". A run that is neither `--development` nor `--allow-dirty` fails if
  any sample is unmatched, counted over every point after the join.
- **OA-TRK-374** In every coverage-penalised metric (M4, M4t, M7, M7t) a present value that is not a
  number is a failure, and the count of such values is printed; an aircraft whose M6 RMS is not a
  number is a failure for the share and is left out of the sign test; samples left out of a metric's
  common set for a non-finite value are counted per system apart from samples not in all three; aircraft with no usable time fit are
  counted per system with their samples and are scored unfitted (offset 0), and the output says so.
- **OA-RDF-007** A digest is accepted only with the `v2` header as its first line and with every
  chain hash equal to the chain recomputed from the line and the lines before it; a line that fails
  is named. A `v1` digest is refused (there is no legacy path), and so is a blank line before the
  header.
- **OA-RDF-008** A digest or expect file over its size limit, a second with no rows or over 2^20
  rows, and row counts that overflow are errors (exit 2), never a panic; a failing standard output
  (a closed pipe) is exit 2 with a message.
- **OA-RDF-009** Rows with the same address, time and tick flag in one second are ambiguous
  pairings: they are counted per second and in total (`ambiguous_pairs`) and are a class that a
  declared difference must name (`ambiguous`). Rows left over with the same address and time but
  another tick flag are one changed row (class `flags`), not a removal and an addition.
- **OA-RDF-010** `order` is read from the matched rows alone, whatever else differs, so a declared
  class next to an undeclared reorder fails the expectation.
- **OA-RDF-011** Digests that differ only in a chain hash are different (exit 1, `chain_only`),
  and no declaration makes them pass.
- **OA-RDF-012** A rows file is used only after the hash and count of every one of its seconds are
  recomputed and found equal to its digest line, no digest second is missing from it and none is in
  it that the digest lacks; the first bad second is named and the exit is 2.
- **OA-RDF-013** A rows file whose header count differs from its digest's total, that has bytes after
  its last row, or that ends early is an error (exit 2).
- **OA-RDF-014** `tools/replay_overlay_apply.sh` applies the overlay patch only to files whose blob
  is the patch's preimage, at zero fuzz, checks the postimage blobs, and records the sha256 of the
  patch and of every patched file; any mismatch is a refusal (exit 2).
- **OA-RDF-015** The overlay patch creates `canon.rs` and `canon_spill.rs` byte for byte as they are in
  `crates/oa-track/src/`, so an old commit writes the digest line, second hash, chain and end line
  of the tree. Reading the spill columns back turns every NaN among the `f32` fields into the one
  canonical NaN, so rows are canonical whatever bits the old commit spilled.
- **OA-RDF-016** `tools/replay_compare.sh` builds with `--locked` and with compiler flags
  neutralised: the environment variables `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS` and `RUSTDOCFLAGS`
  are not passed, `CARGO_ENCODED_RUSTFLAGS` is passed empty (which cargo reads before every
  config file, so a `target.<triple>.rustflags` or `target.<cfg>.rustflags` in the user's config
  is not read), and `--config build.rustflags=[]` is given; its report states what the caller's
  four flag variables held, that they were cleared, and the toolchain. Flags in a
  `.cargo/config.toml` of the tree under test are not neutralised.
- **OA-RDF-017** A digest ends with `end <seconds> <rows> <chain>` (format v2), which the reader
  recomputes from the lines before it and requires, compared field by field on single spaces (tabs,
  repeated or trailing spaces are refused); a digest cut at a line boundary, an end line that
  disagrees, or text after it is an error.
- **OA-RDF-018** Two digests that both have no seconds are exit 2 ("no seconds to compare"), not
  identical.
- **OA-RDF-019** When both rows files are given they are verified against their digests whether or
  not the digests are identical, and the summary says `rows_known=true`.
- **OA-RDF-020** A second the digests call different but whose rows do not differ, or a differing
  second that was not diffed, is an inconsistency: exit 2, never a pass.
- **OA-RDF-021** An option given twice is a usage error (exit 2).
- **OA-RDF-022** `tools/replay_compare.sh` runs cargo and rustc under `env -i` with only PATH, HOME,
  RUSTUP_HOME, CARGO_HOME, TMPDIR, an empty `CARGO_ENCODED_RUSTFLAGS` and the two variables it
  sets, with `--config build.rustflags=[]`;
  it records the dropped variable names, the sha256 of both digests, both rows files, both
  tracker binaries and the diff binary, and a missing diff binary is exit 2.
- **OA-RDF-023** `tools/replay_compare.sh` refuses an output directory that exists and is not
  empty (exit 2, before any build, touching nothing), and a run that does not reach its end removes
  the rows files it made.

## Comparison protocol

- **OA-CPR-200** The frozen development seed set, including its default seed, is disjoint from every
  qualification seed; protocol validation rejects any overlap.
- **OA-CPR-201** Qualification seed IDs are unique and their declared count equals the list length;
  duplicate IDs or a count mismatch fail protocol validation.
- **OA-CPR-202** Competitive thresholds remain fixed at position RMS ratio ≤0.70, p99 position
  error ratio ≤0.80, p99 receive-to-usable latency ratio ≤0.50, and throughput ratio ≥2.0, with
  equal quality and resource envelope required; mutation fails protocol validation.
- **OA-CPR-203** The truth, measurement, and output CSV headers and allowed comparison stages are
  versioned contract values; unsupported headers or claim stages fail protocol validation.
- **OA-CPR-204** Qualification declares at least 10,000 simulated aircraft-hours and a seed ×
  aircraft × duration allocation whose arithmetic exactly matches the declared total, with the
  100 qualification seeds assigned 20 each to the five nominal scenarios and zero to dropout.
- **OA-CPR-205** The qualification uses the frozen IDs 1,000,001 through 1,000,100, distinct from
  development IDs 0 through 31; generator, distribution, and allocation are frozen before candidate
  freeze.
- **OA-CPR-206** A frozen input/output ledger gives each event exactly one terminal disposition
  and each required score-grid point exactly one status; missing outputs are scored and any
  unaccounted work fails the strict gate.
- **OA-CPR-207** The actual holdout is consumed once after freeze, is unavailable for tuning, and
  is reclassified as development if its outcome informs a change; consumption state is published.
- **OA-CPR-208** No unqualified professional tracker or unauthenticated legacy export is claimed;
  a controlled or Python baseline cannot be labelled as a professional reference.
- **OA-CPR-209** Qualification seed replicates are not represented as independent references or
  aircraft; shared world/reference-model dependence is stated in the protocol.

## Independent comparison evaluator (OA-CMP)

- **OA-CPR-001** The evaluator parses the shared truth, measurement and output CSV schemas by header name and refuses absent required columns.
- **OA-CPR-002** CSV byte, row and field sizes are bounded and malformed, non-UTF-8, duplicate-header and non-finite numeric values are refused with source context.
- **OA-CPR-003** Truth keys are unique and strictly ordered by scenario, time and flight; numeric flight IDs compare numerically.
- **OA-CPR-004** Output score keys outside the truth grid are counted as false-identity rows and prevent qualification.
- **OA-CPR-005** Duplicate output grid keys are counted and prevent qualification.
- **OA-CPR-006** Missing output rows and explicit missing statuses are counted for each system.
- **OA-CPR-007** Candidate and baseline outputs are evaluated on the common truth grid.
- **OA-CPR-008** Output timestamps require score time equal publish time for logical accuracy scoring.
- **OA-CPR-009** Outputs whose source time follows publish time are refused.
- **OA-CPR-010** The evaluator checks coordinate ranges and computes horizontal position error in a local tangent plane from Earth-centered coordinates.
- **OA-CPR-011** The evaluator reports velocity error separately from position error.
- **OA-CPR-012** Aggregate position and velocity results report sample count, RMS, mean and nearest-rank 95th and 99th percentiles.
- **OA-CPR-013** Position and velocity metrics are split by scenario, flight and truth phase.
- **OA-CPR-014** Coast, missing and refused outputs are accounted separately from valid outputs, with each status count and fraction of the full truth grid.
- **OA-CPR-015** A source-timestamp join to input receive time is reported only as a labeled diagnostic proxy; receive-to-usable delay requires an event-level first-valid-use ledger.
- **OA-CPR-016** Receive-to-usable delay and its competitive ratio are unavailable without a complete event-level first-use ledger that records no-usable-completion dispositions.
- **OA-CPR-017** A negative receive-to-usable delay is refused.
- **OA-CPR-018** The evaluator uses fixed thresholds of 0.70 RMS ratio and 0.80 p99 position-error ratio, with hierarchical 95% upper bounds under those thresholds; nominal scoring starts at scenario epoch plus the predeclared two-second initialization window.
- **OA-CPR-019** A speed claim requires a complete event-level first-use ledger, p99 receive-to-usable delay no greater than 0.50 of reference, and at least 2.0 throughput ratio.
- **OA-CPR-020** Accuracy and speed qualifications are reported separately from measured logical metrics.
- **OA-CPR-021** A zero or near-zero reference error produces an explicit unqualified ratio, never a fabricated finite ratio.
- **OA-CPR-022** Development-control comparisons cannot qualify performance claims.
- **OA-CPR-023** Missing comparison provenance disables qualification.
- **OA-CPR-024** Qualification requires an explicit reference qualification state and input fingerprint.
- **OA-CPR-025** Qualification requires content-validated, matching-input timing ledgers for candidate and reference.
- **OA-CPR-026** Hashed timing ledger contents identify the monotonic wall clock, elapsed wall time, work units, drain state and input fingerprint.
- **OA-CPR-027** Speed qualification requires a drained workload and zero reported loss counters.
- **OA-CPR-028** The evaluator never derives a speed claim from Python runtime measurements.
- **OA-CPR-029** Paired uncertainty resamples scenario clusters and flight clusters within each scenario rather than individual pings.
- **OA-CPR-030** The hierarchical cluster bootstrap is deterministic and reports scenario and flight cluster counts.
- **OA-CPR-031** Fewer than two paired scenario clusters produce unavailable confidence bounds.
- **OA-CPR-032** The independent CV Kalman baseline consumes measurements and emits outputs on the supplied truth grid.
- **OA-CPR-033** The baseline estimator does not consume truth coordinates, truth velocity, or truth phase.
- **OA-CPR-034** The baseline estimator does not apply measurements whose sensor time follows the output grid time.
- **OA-CPR-035** The baseline uses fixed, declared process noise and measurement uncertainty fields, confirms after two of the last three distinct reports, and marks output coast when source age exceeds two seconds.
- **OA-CPR-036** The baseline identifies itself as a development control and is never a speed reference.
- **OA-CPR-037** The command line reports malformed input or metadata with a nonzero exit status.
- **OA-CPR-038** The command line writes machine-readable JSON with distinct metrics, qualification and accounting sections.
- **OA-CPR-039** Qualification metadata uses only registered comparison classes and records reference status and input fingerprint.
- **OA-CPR-040** Throughput is derived from hashed ledger work units and elapsed wall time for equal workload counts.
- **OA-CPR-041** Timing evidence binds both system timing ledgers to their recorded hashes, and each ledger's contents are parsed and validated.
- **OA-CPR-042** The evaluator reports the number of measurement events independently of output rows.
- **OA-CPR-043** The evaluator validates a hashed independent system/scenario/flight/time/track/truth-flight binding and does not silently discard false flight identities.
- **OA-CPR-044** Empty metric populations report count zero and null statistics.
- **OA-CPR-045** Percentiles use nearest-rank selection with no interpolation.
- **OA-CPR-046** The evaluator never divides by a zero reference metric.
- **OA-CPR-047** Loss, refusal, missing and stale-output coverage remain visible even if accuracy is measurable; nominal post-initialization usable coverage must be at least 99.9% for both systems and candidate coverage cannot be below reference coverage, while dropout runs remain diagnostic-only and cold-start rows stay reported separately.
- **OA-CPR-048** The baseline emits one valid, coast or missing row per truth grid key; it reports matching M-of-N startup and source-age coast behavior.
- **OA-CPR-049** The evaluator and baseline use only Python standard-library modules.

- **OA-CPR-050** The comparison command requires a fresh output directory outside the checkout and
  never writes build artifacts under the checkout; output and target directories cannot overlap.
- **OA-CPR-051** Every run records Git head, dirty status, tracked source digest, patch digest,
  untracked source digests, and before/after snapshots; a changed source snapshot fails the run.
- **OA-CPR-052** Without a prebuilt binary, the command checks Cargo 1.89.0 and builds the standalone
  runner with the locked release manifest in offline mode, one Cargo job, and an external target
  directory; every subprocess has a fixed timeout and retained bounded logs.
- **OA-CPR-053** The command records hashes for the runner binary, run configuration, every CSV/JSON
  input and output, stdout, stderr, command list, and comparison result.
- **OA-CPR-054** The CLI supports a named scenario or `all`; `all` executes six scenarios in fresh
  isolated directories, then reports each result without turning process success into a score pass.
- **OA-CPR-055** Baseline generation receives only a derived grid containing `scenario_id`,
  `flight_id`, and `time_ns`, plus the shared measurements; truth coordinates and phase are not
  passed to the baseline.
- **OA-CPR-056** The wrapper validates candidate and baseline output coverage against the truth grid
  and emits one identity binding per output row, normalizing runner identity to `candidate` and
  deriving baseline mapping only from known input flight identifiers.
- **OA-CPR-057** Wrapper metadata labels the Python constant-velocity control `development_control`
  and never represents it as a professional tracker or qualified comparison baseline.
- **OA-CPR-058** Replay-call-cost timing is preserved as such and is never converted to first-use
  wall latency; absent valid timing ledgers leave speed comparison unknown.
- **OA-CPR-059** The reference validator's evidence status is recorded exactly; a failed or unknown
  reference cannot be upgraded by the wrapper to qualified evidence.
- **OA-CPR-060** The qualification seed set is not launched by the development wrapper; runs use the
  explicit development seed and record source cleanliness and seed/configuration identity.
- **OA-CPR-061** `--require-competitive-pass` returns nonzero unless every scenario passes accuracy,
  all-quality, and speed bars; missing, unqualified, or unknown evidence fails that option.
- **OA-CPR-062** Any build, runner, validator, or evaluator failure exits nonzero and retains the
  command record, logs, source snapshots, and all outputs created before failure.
- **OA-CPR-063** JSON artifacts are size bounded, parse as a JSON object with finite values, and
  malformed content fails with a retained error record when execution has begun.
- **OA-CPR-064** The command wrapper's aircraft and duration bounds match the standalone runner's
  300-aircraft / 1,200-second profile and report the normative profile ID separately from the
  runner implementation model ID.
- **OA-CPR-065** Standard output and error are drained into fixed-size bounded buffers while the
  process runs; exceeding a limit or timeout terminates the entire child process group and records
  the failure.
- **OA-CPR-066** Each run binds its cohort ID and scoring role to the frozen protocol plus the raw
  runner manifest seed/scenario; metadata must agree, and the development command cannot launch
  nominal qualification or sealed fault-cohort seeds.
- **OA-CPR-067** The wrapper pins the comparison protocol hash (equal to the evaluator's pinned value) and refuses a protocol file or recorded `protocol_sha256` that differs.
- **OA-CPR-068** A run's status is `completed` only for a clean, reference-validated run; a dirty source tree gives `exploratory (source tree not clean)`, and every status other than `completed`, including `reference_failure`, exits 3 whether or not `--require-competitive-pass` is given.
- **OA-CPR-210** A `missing` output is an explicit row with scheduled `score_ns == publish_ns`,
  empty `source_ns` and empty position/velocity values; no required grid row may be omitted.

## Independent synthetic comparison reference

- **OA-CPR-150** The stdlib-only reference validator accepts explicit truth, measurements, manifest, protocol and output paths, applies bounded strict CSV/JSON parsing, and rejects malformed, non-finite, duplicate, reordered or incomplete rows.
- **OA-CPR-151** The validator independently recomputes supported v2 scenario truth (including all-positive turns and alternating crossing signs), WGS84 ground velocity and regional extent, and sanity-checks velocity with ECEF central differences.
- **OA-CPR-152** Measurement IDs, causal receive schedule, dropout denominator and reported covariance reconcile with the manifest, including truth-grid event ordinals absent from dropout measurements.
- **OA-CPR-153** The declared Box-Muller seed/event/axis noise model is checked against deterministic latents, distinct axis keys and finite-sample residual bounds.
- **OA-CPR-154** Evidence records bind input artifacts and protocol by SHA-256; a passing result requires the supplied runner source bytes to match the manifest's declared implementation SHA-256.
- **OA-CPR-155** Passing evidence is labeled `independent_simulation` with `declared_model_only` scope and explicitly makes no physical-accuracy claim; it preserves the exact recognized protocol version/status and reports the v1.0.3 dropout fault gate as blocked.
- **OA-CPR-156** Unknown model/protocol versions or missing required artifacts produce `UNQUALIFIED`; readable contradictions produce `FAIL` with a reason.
- **OA-CPR-157** The validator checks v1.0.3 paired post-init key-set rules and the separate 20-seed dropout fault cohort, exact allocation and pending gates; it never presents that blocked cohort as a passed quality gate. Unit tests cover evidence hashes, negative controls, and missing/mismatched runner source.

- **OA-CPR-211** The scenario epoch is the score origin; candidate initialization is 2-of-3 distinct
  events, and the development control requires two distinct input events before usable output.
- **OA-CPR-212** Every run retains raw cold-start and all-grid results and separately scores only
  `time_ns >= epoch + 2 s` as post-initialization metrics.
- **OA-CPR-213** Coast status is determined from actual supporting source age and is required when
  that age exceeds two seconds; only `valid` rows count as usable in post-initialization coverage.
- **OA-CPR-214** Nominal scenarios straight, turn, acceleration, crossing, and delayed require
  post-initialization usable coverage ≥0.999 and candidate coverage no lower than baseline.
- **OA-CPR-215** Dropout is diagnostic-only under v1 and is excluded from qualification gates for
  its predeclared 4-of-12 absent-source schedule and resulting 2-of-12 coast interval.
- **OA-CPR-216** The runner, reference validator, evaluator, and wrapper preserve and report the
  frozen v1.0.3 initialization, scenario-role, paired-key, and fault-cohort values without tuning
  them from candidate results.
- **OA-CPR-217** The v1 dropout fault cohort is separate from the 100 nominal performance seeds,
  uses seeds 1,000,101 through 1,000,120 and 2,000 additional simulated aircraft-hours, remains
  unopened, and blocks any all-quality pass until every independently reviewed fault gate is
  measured and passes; nominal seed count cannot erase the fault-mode gate.
- **OA-CPR-218** Qualified v1 accuracy requires identical candidate and baseline usable post-init
  truth-grid key sets; a mismatch blocks qualified accuracy while raw per-system metrics and
  keyset intersection/difference counts remain reported.
- **OA-CPR-219** The evaluator pins the SHA-256 of `validation/comparison-protocol-v1.json`; any other protocol bytes make accuracy qualification `unqualified_protocol_hash_mismatch` with the reason "protocol file does not match the frozen v1 hash", and every threshold it applies (ratios, coverage minimum, coast age, initialization window) is read from the verified protocol.
- **OA-CPR-220** Every unpaired position, velocity, age and per-group statistic carries its coverage (rows used, rows expected, fraction) in the same object as its figures.
- **OA-CPR-221** Parsed integers and numbers must match a strict decimal pattern; underscores, spaces, a leading plus sign, and inf/nan text are refused.
- **OA-CPR-222** The cluster bootstrap reports its replicate count and how many replicates were dropped for a zero denominator.

## Motion-model development study

- **OA-ACC-150** The versioned study preregistration is committed before experiment outputs and fixes its developmental-only claim scope.
- **OA-ACC-151** The study evaluates exactly `H_Q_CV` values 1, 0.1, and 0.01 while all other motion and measurement settings remain frozen at default.
- **OA-ACC-152** Training runs exactly three candidate settings across seeds 1–3, six v1 scenarios and onset-exit (63 candidate run cases), with 20 aircraft for 300 seconds; each is paired to frozen default and winner selection follows the preregistered pooled nominal RMS rule subject to all gates.
- **OA-ACC-153** Seeds 4–6 are used only once for the selected candidate paired with the frozen-default control across all six v1 scenarios and onset-exit; no other experimental setting is run, and results cannot retune or replace the selected candidate.
- **OA-ACC-154** The study excludes qualification seeds 1,000,001 and above and the sealed fault cohort.
- **OA-ACC-155** Candidate selection requires zero lost, pending or refused inputs, full nominal post-initialization coverage, identical usable key sets and the preregistered 1.02 regression bounds.
- **OA-ACC-156** Dropout remains diagnostic-only and is compared to frozen default using the preregistered paired p99 and coverage checks.
- **OA-ACC-157** Mirrored onset-exit transitions are scored separately by direction and entry/exit in fixed 30-second windows; both motion probability and turn rate must cross their thresholds for three samples, candidate lag is at most one second worse than default, and censoring fails the response gate.
- **OA-ACC-158** Posterior 2D containment is computed from candidate ENU covariance using a seed-then-flight paired cluster bootstrap; preregistered one-sided lower-bound floors apply per nominal scenario and each onset-exit direction/window group, invalid covariance fails the setting, and overcoverage is reported.
- **OA-ACC-159** The study does not claim NIS, physical accuracy, professional performance, or independent qualification; thresholds are not changed after observing results.
## Motion filter diagnostics study

- **OA-ACC-160** Every candidate setting has a recorded pass for the preselection `oa-track core accelerating_flight_is_tracked` regression guard; missing or nonzero evidence blocks selection, and the selected candidate receives the full core suite afterward.
## Independent track-set accuracy scoring

- **OA-ACC-050** The set scorer is a standalone Python standard-library tool with explicit truth, outputs, manifest and evidence paths.
- **OA-ACC-051** Truth and output CSV headers exactly match the registered snapshot schemas.
- **OA-ACC-052** Input bytes, records, fields, grid ticks, per-tick points and matching work are bounded; exceeded exact-work limits return `UNQUALIFIED`.
- **OA-ACC-053** Invalid IDs, timestamps, statuses, coordinates, non-finite values and out-of-range geodetic positions are excluded and counted.
- **OA-ACC-054** Manifest schema version, scenario, run configuration, sources, grid and expected truth counts are validated before scoring.
- **OA-ACC-055** Truth, output and manifest artifacts are hash-bound; declared source file hashes and row counts reconcile with supplied bytes; whole and measured fusion input fingerprints/counts match their offered input counts.
- **OA-ACC-056** Manifest accounting requires explicit export validity, strict processing state, reconciliation flags and complete nonnegative counters; unassigned plots remain reported, not treated as loss by default.
- **OA-ACC-057** The truth manifest inventory explicitly names every score tick and expected cardinality, including zero-truth ticks.
- **OA-ACC-058** The output manifest inventory explicitly names every score tick and emitted row count, including intentional empty output sets.
- **OA-ACC-059** An output snapshot count mismatch or incomplete truth inventory is `UNQUALIFIED`; absent outputs are never silently interpreted as valid empty sets.
- **OA-ACC-060** A declared intentional empty output set at a required tick is scored as an empty track set and counts all truth objects as misses.
- **OA-ACC-061** Missing whole output ticks are separately reported; expected nonempty output ticks with no emitted rows are `UNQUALIFIED`.
- **OA-ACC-062** A zero-truth snapshot is valid and charges the false-track penalty for each emitted output.
- **OA-ACC-063** Duplicate truth and output IDs are detected per time, counted, excluded after the first occurrence and invalidate trace quality.
- **OA-ACC-064** Malformed rows are counted and excluded; scoring cannot pass with malformed rows.
- **OA-ACC-065** Scenario/time exclusions and invalid identity rows are included in named excluded-truth and excluded-output counts.
- **OA-ACC-066** Both `valid` and `coast` outputs with positions participate in the set metric.
- **OA-ACC-067** Output source timestamps are nonnegative, within the signed timestamp domain and no later than their score time.
- **OA-ACC-068** Truth IDs and output track IDs are unique within each snapshot; IDs may persist across snapshots.
- **OA-ACC-069** Every row is joined only to the manifest scenario and a declared grid timestamp.
- **OA-ACC-070** The preregistered score is GOSPA with p=2, alpha=2 and c=1000 m.
- **OA-ACC-071** Pair distances use the declared zero-height WGS84 ECEF chord, including correct behavior at the dateline and antipodes.
- **OA-ACC-072** A bounded 3-D spatial hash generates every possible pair strictly inside the cutoff without all-pairs distance evaluation.
- **OA-ACC-073** Pair distances at or above 1000 m are never matched; a spatial-bin margin cannot change the final metric cutoff.
- **OA-ACC-074** The score uses a minimum-cost one-to-one partial assignment; each truth and output occurs in at most one match.
- **OA-ACC-075** Each unmatched truth contributes c²/2 to squared GOSPA cost.
- **OA-ACC-076** Each unmatched output contributes c²/2 to squared GOSPA cost.
- **OA-ACC-077** Each selected in-cutoff pair contributes its squared ECEF chord distance to localization cost.
- **OA-ACC-078** Snapshot GOSPA distance is the square root of localization, missed-object and false-object squared costs.
- **OA-ACC-079** The exact assignment solver is implemented in-tree without SciPy or other external dependencies.
- **OA-ACC-080** Candidate graphs are decomposed into independent connected components before exact assignment.
- **OA-ACC-081** Equal-cost assignments use stable sorted-ID row/column order and a deterministic first-minimum solver rule.
- **OA-ACC-082** Local equal-nearest candidate ties are counted and labeled with their numerical tie tolerance.
- **OA-ACC-083** Global assignment uniqueness is explicitly reported as not assessed; a deterministic representative is not claimed unique.
- **OA-ACC-084** Track-ID to truth-ID assignment changes between adjacent ticks are reported by both IDs and time.
- **OA-ACC-085** Truth-ID to track-ID changes between adjacent ticks are reported by both IDs and time.
- **OA-ACC-086** Proximity-based assignment and association changes are never represented as proof of physical identity continuity.
- **OA-ACC-087** Every manifest score tick has a result row with truth/output cardinality, matches, misses, extras, costs and GOSPA.
- **OA-ACC-088** Aggregate output reports summed localization, missed, extra and total costs and counts across all ticks.
- **OA-ACC-089** Intentional empty outputs and missing output snapshots are distinguishable through the per-tick manifest inventory.
- **OA-ACC-090** Inventory rows are ordered, unique, nonnegative and cover the complete inclusive grid.
- **OA-ACC-091** Truth point counts and truth file declarations reconcile with per-tick truth inventories before any score is emitted.
- **OA-ACC-092** Offered, input, processed, applied and pending counts reconcile using the declared fusion accounting identities.
- **OA-ACC-093** Late/refused events, input-address refusals, queue overflow, work failure, counter overflow, prediction failure and output overflow block the strict processing-quality pass; complete structurally valid exports retain descriptive metrics with status `UNQUALIFIED_PROCESSING`.
- **OA-ACC-094** No approximate assignment fallback is permitted when a component or work bound is exceeded.
- **OA-ACC-095** Retained candidate edges have explicit per-tick and whole-run caps checked before storing each edge.
- **OA-ACC-096** Assignment component size has an explicit hard cap; exceeding it produces `UNQUALIFIED`.
- **OA-ACC-097** Candidate-search and Hungarian cubic work have explicit hard caps; exceeding them produces `UNQUALIFIED`.
- **OA-ACC-098** Small-set exact assignments are exhaustively cross-checked against a separate brute-force partial-matching implementation for all sizes through four.
- **OA-ACC-099** The report is a set-level synthetic metric only; it makes no full-quality or physical identity claim.
- **OA-ACC-161** Study CSVs enforce exact bounded schemas, finite numeric values, unique truth/diagnostic/output keys, unique event IDs and nonregressing input order.
- **OA-ACC-162** Candidate and frozen-default artifacts bind identical truth and measurement bytes, the six compiled motion parameters, source/configuration hashes, and the preregistered `H_Q_CV` difference only.
- **OA-ACC-163** The scorer reports all-grid, cold-start and post-initialization position/velocity metrics plus valid/coast/missing counts and fractions, with paired post-initialization position/velocity metrics also split by truth phase.
- **OA-ACC-164** The native regression guard passes only with hash-verified result and log artifacts, matching source identity, the exact pinned test command and a one-test successful log summary.
- **OA-ACC-165** Candidate and frozen-default builds share the same source head and runner, have parameter-source bytes differing only at the registered `H_Q_CV` declaration, bind a source diff limited to that declaration, and both carry passing native regression evidence.
- **OA-ACC-166** Run manifests use the frozen `adsb_kinematic_v2` model for the six established scenarios and `adsb_motion_study_v1` only for onset-exit.
- **OA-ACC-167** Onset-exit truth validation compares geographic velocity using the WGS84 Jacobian at each generated latitude while retaining the frozen position and velocity tolerances.
- **OA-ACC-168** Containment uses position error in candidate-local ENU axes matching emitted covariance, and turn-rate residuals use the tracker's clockwise-positive sign convention.
- **OA-ACC-169** The motion-study scorer, selector and checker load `validation/motion-study-v1.json` only if its bytes match the pinned v1 SHA-256, and otherwise fail closed with "protocol file does not match the frozen v1 hash".

- **OA-RDF-024** `tools/replay_compare.sh` reads from each commit's tree, before any build, the
  digest format it will emit (the header constant of its `canon.rs`, or v2 for a commit that gets
  the overlay); a commit that would emit v1, a half-way commit (only one of `canon.rs` and
  `--digest`) or an unknown header is exit 2. It runs both `oa-track-run` calls in the same
  `env -i` environment as the builds, and records the diff tool's build commit and whether that
  tree was clean in the report and in `summary.txt`.
- **OA-RDF-025** Replay witness comparison accepts two guarded regular files containing only
  the versioned settings, complete ordered-input, and final-state lines. Each file is limited
  to 4 KiB. Positive frame counts and zero refusal counters are required. The verdict checks
  settings first, then admitted frame count and ordered-input digest, then final-state digest.
- **OA-TRK-377** A P-002 hold-out run (neither `--development` nor `--allow-dirty`) needs
  `--frozen-commit <40 hex>` and is refused (exit 2) unless the commit embedded in the binary at
  build time (`OA_BUILD_COMMIT`, with `OA_BUILD_DIRTY=0`, set by `tools/p002_build.sh`) equals it,
  and the working tree at run time is at that commit and clean. This gates which runs count as
  hold-out scores; it does not change what is computed.
- **OA-TRK-389** The P-002 scorer scores every system, passthrough included, on the truth legs that contain
  an aircraft's grid samples, and on no others; no system chooses its own flight (P-004). The grid is the
  paired samples of the aircraft's most-paired ERAM flight. The output prints each system's coverage under that rule and under the replaced per-system rule.
- **OA-TRK-390** The reacquired group is OpenATCA's rows within 10 s after an update row of the same address
  flagged `O_REACQUIRED`; ERAM and passthrough are shown on the same samples.
- **OA-TRK-391** Every grid sample at which a system has no value is counted in exactly one class per system:
  before its first row for the address, after its last, in a gap, address never present, other.
- **OA-TRK-392** M1, M3t and M5 and their breakdowns are computed on the two-way common set (OpenATCA and ERAM
  both have a value); the three-way set is used only for the reference-floor test; the n of each set is printed.
- **OA-TRK-393** M1 median, M5 ground-speed median and M4t at 300 m are the primary metrics. Each run prints,
  per primary metric, whether the day's interval of OpenATCA minus ERAM excludes zero in OpenATCA's favour,
  in ERAM's favour, or neither, as `CLAIM` lines; `p002 --combine <dirA> <dirB> <dirC>` applies the cross-day
  rule (OpenATCA's favour on A and B, C not in ERAM's favour) and refuses a day that is not a hold-out run, days
  scored at different commits or by different binaries, days with the same clip hash, a `samples.csv` that is
  not the one its run recorded, and primary metrics that differ from those recomputed from the three `samples.csv`.
- **OA-TRK-394** `--candidate-commit <40 hex>` is required with `--frozen-commit`; the run is refused unless
  `git diff --name-only <candidate> <frozen>` is exactly `crates/oa-sealed/sealed_paths.txt`; both commits are
  printed.
- **OA-TRK-395** The scorer writes `p002.txt` and `samples.csv` to `.partial` names, flushes them and renames
  both (after the ledger line of a hold-out run, OA-TRK-399), only if neither final name exists; otherwise it refuses and writes nothing; a leftover `.partial` is
  reported. It prints the SFDPS loader notes, the replay hash and the ingest drop counts.
- **OA-TRK-396** On a development run, `--exclude-window <from ISO> <to ISO>` (repeatable, whole-nanosecond
  or whole-second ISO times) removes every grid sample inside any window for all three systems alike, before any metric; each window and
  the number of samples it removed are printed and written to the `CLAIM` lines.
- **OA-TRK-397** Each `CLAIM metric=` line carries `floor=0|1` and the three-way OpenATCA and passthrough
  values it was judged on (M4t on the share over all grid samples); `--combine` prints "AT REFERENCE FLOOR:
  no claim" for a metric whose floor applies on any day and never "RULE MET" for it.
- **OA-TRK-398** A hold-out run takes its excluded windows only from `--damage-report` (an `oa-feed-gaps`
  report: the `window:` line, the `exclude <iso> <iso>` lines and the verdict line `CLEAN`), and refuses
  `--exclude-window`, a report that is not CLEAN, a window under 600 s, windows over 3,600 s in all, and a
  report whose window is not the scored window: it must lie inside the clip's UTC day and match S = [max(day
  start, first SFDPS message), min(day end, last SFDPS message, last clip report)] to within 60 s at each end
  (both windows are printed on a refusal). The parser is tested on reports printed by the tool itself.
- **OA-TRK-399** Every run needs `--ledger <file>` and is refused when the clip's SHA-256 is in it; a hold-out
  run appends one line (clip SHA-256, scored commit, candidate commit, UTC time) after both `.partial` files are
  written and flushed and before either is renamed; if the append fails the partials are deleted and the run
  fails, so a ledger line exists only when both result files were written.
- **OA-TRK-400** OpenATCA's and ERAM's time offsets are fitted on the two-way common set, the passthrough's on
  the three-way set (P-008 §1).

- **OA-TRK-401** Velocity lag. A measurement carries `vel_lag_s`, how far its velocity lags behind the
  aircraft's.
  - It is zero unless the source sets it. ADS-B sets `VEL_LAG_S`, 1.0 s. A lag outside 0 to 10 s, or not a
    number, refuses the velocity as bad.
  - The horizontal filter treats the velocity as the aircraft's velocity `L = vel_lag_s` seconds before the
    report. Against the state, the expected measurement is `z = R(-w L) v`: the state's velocity turned back
    by its turn rate `w` over `L`. The update is linearised at the state, so a lagged velocity also
    corrects the turn rate, and the turn-rate column of the Jacobian is the derivative of `z`.
  - The velocity gate compares the report against the same expectation, with the same innovation
    covariance as the update.
  - At a lag of zero the update and the gate are the plain velocity ones, bit for bit. The straight model's
    turn rate is zero, so its update is always the plain one.
  - A report that agrees with the expectation leaves the state unchanged. The covariance stays symmetric
    and not negative. A singular innovation covariance changes nothing.
  - Limit: with the ADS-B lag, the shipped gate accepts a report forged 2 km away after a 9 s gap in the
    track's reports, against 10 s without the lag (docs/TURNS_EXPERIMENTS.md).
- **OA-TRK-402** `turn_diag` reports OpenATCA's and ERAM's cross-track and track-angle error on the common
  samples of a P-002 `samples.csv`: n, median, p95 and RMS of the absolute values, by nearest rank.
  - It bins them by the reference's turn rate, the turn phase, the tracker's turn-model probability, the
    time since the track's last report, and the gap the sample falls in. A value goes to the highest bin
    whose lower edge it reaches.
  - It reports how far the reported velocity's track angle lags behind the reference's.
  - With `--rescore`, `--thin N` keeps every Nth input report per aircraft and `--vel-lag` sets the lag, so both effects can
    be measured on the same day.
- **OA-TRK-404** The opt-in development turn-onset export requires a checked, increasing,
  whole-second-aligned half-open UTC-millisecond window, refuses overflow or overlap with sealed
  timestamps, retains full source history for filter warmup, exports only rows in that window for
  truth/OA/CV/SFDPS, preflights the selected truth grid against its row cap using overflow-safe
  arithmetic, and records selected/outside row counts and exact endpoints. Synthetic example tests
  cover interval boundaries, invalid intervals, sealed-span overlap, and extreme/inverted leg times.
  The standard export/scorer profile permits at most 250,000 data rows and 32 MiB per CSV;
  a separately opt-in development profile permits at most 1,000,000 rows and 128 MiB. The
  exporter requires `--turn-onset-extended-budget` with `--turn-onset`; the scorer requires
  `--extended-budget --protocol v3`. Both profiles refuse excess rows/bytes explicitly. Export
  row formatting uses at most 512 bytes of scratch and checks accumulated bytes before append;
  parsing checks the guarded descriptor byte limit, physical record/field/column bounds and sealed
  timestamps before grouping. Provenance records the selected caps and actual CSV sizes. Capacity
  refusal is incomplete; the capacity profile does not alter event or matching definitions.
- **OA-TRK-405** The opt-in development scoring profile `fixed-10-30-120-v1` sets the expected
  interval to 10 seconds, initiation interval to 30 seconds and drop age to 120 seconds without
  inspecting measurement cadence, reference or SFDPS data. It retains the scoring output capacity
  and other base settings. Changing measurements strictly after a cutoff cannot change this
  profile's configuration or the real tracker output through that cutoff. A synthetic regression
  compares complete tracker state rows from two shared-prefix inputs with different later cadences;
  both inputs extend beyond the cutoff to exclude final-flush artifacts.
- **OA-TRK-406** Without an explicit timing profile, scoring preserves the existing measured
  median/p90 intervals and one-second empty-distribution fallback; its 120-second drop age remains
  fixed. The new profile is development-only: `p002` refuses it without `--development`, with
  frozen/candidate bindings, combine mode or export-only mode; `vs_eram` requires turn export.
  Opt-in reports record the profile and actual constants and state that interval statistics were
  not computed. OA and CV turn replays share these timing settings; the CV model choice remains
  explicit. Legacy reports acquire no profile lines.
- **OA-TRK-407** The opt-in `p002 --oa-publication-grid sfdps-v1` mode requires an explicit
  `--development` run and refuses frozen/candidate, combine, and export-only modes. It quantizes
  only OpenATCA tick rows after the single tracker replay and before legacy adaptation, pairing,
  and scoring: latitude/longitude to nearest whole arcsecond with exact integer signed half-away
  rounding, six-decimal-degree publication rendering, then E7 conversion; east/north velocity components independently to nearest
  1 kt from raw tracker f32 m/s before the existing deciknot conversion. M5 continues to use the
  vector component magnitude, not a scalar ground-speed field. Truth, ERAM, passthrough, tracker,
  timing and row times are unchanged. The ERAM-owned grid construction, pairing and metric
  algorithms, thresholds and defaults are unchanged; quantization can affect paired values and
  eligibility. Opt-in output
  records the profile, units, rounding stage and counted invalid-position/unavailable-velocity
  rows (nonfinite velocity pairs retain their original components; finite components outside the
  adapter's representable range refuse the run); the exact-zero count and nonzero-vectors-rounded
  to zero count are disjoint. `samples.csv` carries a matching comment stamp.
  Without the option, scorer reports and
  CSV bytes remain unchanged. Synthetic source tests cover signed ties, canonical coordinates,
  poles/dateline, invalid positions, zero/nonfinite velocity, refused modes, and row-count
  accounting.
- **OA-TRK-408** B1/H2 public-window runner loads and splits the full public clip once, filters existing set-A measurements to its half-open warm-up interval, and emits canonical rows/digests only on the half-open score interval with an explicit timing profile. Fixed constants do not inspect cadence statistics; legacy full-set-A calibration is an explicitly noncausal secondary arm.
- **OA-TRK-409** B1/H2 public-window input/output paths, strict UTC conversion and nanosecond bounds are checked before use. Canonical output identity excludes wall time; ordinary runner behavior remains unchanged when the mode is absent.
- **OA-TRK-410** B1/H2 private P010 scoring requires the exact registered full-day/split/window/box contract and explicit timing arm; incompatible hold-out, export, damage, address-drop and dirty-tree options are refused. Private per-arm ledgers are new and empty and are never original hold-out ledgers.
- **OA-TRK-411** B1/H2 P010 eligibility is determined from public set-B truth at UTC minutes, with one unambiguous truth leg, finite position/velocity in the registered box and the P006 pairing direction threshold. Actual-time samples must retain that same leg; addresses, legs, eligible minutes and work have explicit caps and all exclusions are counted.
- **OA-TRK-412** B1/H2/D1 P010 keeps actual local ERAM times inside its score interval for all systems and time fits, reports availability with its scoped denominator and all input-health counters, and distinguishes measurement completion from qualification. Public reproduction has no FAA input; private per-flight evidence never enters the aggregate allowlist. Wrapper input/source/build/binary/archive identities, fresh outputs and resource caps are enforced, with partial failures preserved.
- **OA-TRK-413** The reusable `oa-track` package has no direct normal Cargo dependency on the private
  benchmark harness (`oa-bench`), simulator (`oa-sensorsim`), or sealed path guard (`oa-sealed`).
  Scoring, fusion-study and simulator-backed command support lives in the separate shell package
  `oa-track-tools`, which retains the `oa-track-run` and `oa-track-fusion` executable names and all
  existing examples. Engine consumers retain the public tracker API; shell callers use
  `oa-track-tools` for scoring helpers and the guarded adapter for filesystem-facing commands.

- **OA-CMP-047** The sealed hold-out day (2026-11-19, one named constant) is never exported: `--date` of it is
  refused in every date syntax. Any input path the shared sealed list refuses (`oa-sealed`, OA-SEAL-001), and every file the run opens
  (each file of the feed directory, the ADS-B clip, each file and the README of a `slice` source), is checked
  as given and after resolving links and opened through the shared guard; a path that cannot be resolved is
  refused. `--out` of a run or of `slice` is refused in the same way (the nearest existing ancestor of a path yet
  to be created is resolved). `slice` refuses a source whose README import date is the sealed
  day, or has no readable date line. Limits: a hard link, copy, rename or bind mount of a sealed file has no
  sealed component in its path and is not detectable by path, which is why OA-CMP-051 exists. A flag value that
  starts with `--` is a missing value and a repeated flag is an error.
- **OA-CMP-048** A track restart gets a new I062/040 number, from the tracker's own end events and not from gaps
  between exported rows: a confirmed track instance the tracker dropped is logged with its last applied update;
  a row of that address later than that time plus the drop age (`oa_track::score::SCORE_DROP_AFTER_NS`) is of a
  later instance, exact to the nanosecond, in ticks and updates mode, also when both instances' rows are in one
  batch. The tracker holds one live track per address (one slot per key). A full track log fails the run. Any
  `time_steps_back` or `time_future_dropped` above zero fails the run, whether or not a confirmed track was
  dropped, because the tracks a step back drops at once ended at the step and not at their last update plus the
  drop age; the failure leaves nothing under `--out` (tests
  `a_tracker_clock_step_back_fails_the_run_instead_of_misplacing_a_restart`,
  `a_step_back_on_the_very_last_measurement_fails_the_run`,
  `a_step_back_before_any_track_was_confirmed_still_fails_the_run`; the ADS-B inputs of a job are sorted by time
  before the tracker, so a step back is not reachable through `run_job`). The row
  pair that spans a restart is left out of the interval statistics, and the README counts restarts. ERAM rows use
  one track number per flight.
- **OA-CMP-049** Track numbers are one per (key, segment), allocated in order of first appearance, refused when
  the 16-bit space (65 535 numbers, restarts included) is exhausted; the README gives track segments, keys, keys
  with more than one segment and the numbers used of the 65 535 available.
- **OA-CMP-050** I062/185 Vx and Vy are the true east and north velocity at the target (the frame of the CAT062
  specification and of COMPASS); the README names the frame for OpenATCA only. For ERAM the README says the frame
  is as the feed gives it and not verified. Each sentence is printed only when its system is present.
- **OA-CMP-051** Before any directory or file is created and before the tracker runs, every raw input time (every
  ADS-B report of the clip including stale and other-source ones, every ERAM sample including unknown-flight,
  duplicate and refused ones) is checked: one on the sealed day by its raw time or once rounded to 1/128 s (the
  day the export would file it under) refuses the run, naming the system and the count, with nothing created.
  The tracker's rows and every pushed row are checked again at the push. Records the clip loader itself drops
  as undecodable are not reports and cannot be checked. The count of rows and frames the readers dropped in a
  sealed window is given on every exit of the run, a refusal in the pre-scan or a failure after it included.
  `oa-compass slice` reads the day from the source README (none, or one that does not parse, is a refusal),
  computes each record's absolute time from that day and its time of day, tests it against the sealed day and the
  sealed windows and, in a file whose time of day goes back (midnight wrapped), on the day after as well; if any
  record of any file could be sealed the whole slice is refused with a non-zero exit, and the drop line is
  printed on every exit.
## Shadow comparator (oa-shadow)

A tool crate (`tools/oa-shadow`, class `shell`) that runs OpenATCA's tracker beside ERAM's published track
(SFDPS) on a recording and reports where the two disagree. It uses neither system's accuracy to decide what is a
difference: a live shadow has no truth. Every threshold is a value of one table (`thresholds::THRESHOLDS`) and a
value is a difference only when strictly greater than it.

- **OA-SHD-001** Flights are paired by exactly one rule. An ERAM flight with a Mode S address that OpenATCA has an
  aircraft for pairs with that aircraft. Any other ERAM flight with a beacon code pairs with the OpenATCA aircraft
  that is not already paired (by address or by beacon) to an ERAM flight whose span overlaps this one's (beacon
  claims are exclusive within overlapping spans), was offered the code within the flight's span plus 30 s, and is
  nearest to ERAM's first in-box, in-window report of the flight within 5 NM (ties to the lowest address). The code
  is the one in force at that first report, or the earliest if the first message came later. Several ERAM flights
  of one aircraft form one group.
- **OA-SHD-002** A flight or aircraft with no partner is a difference (`UNPAIRED_ERAM`, `UNPAIRED_OA`) with its
  first and last time and duration and its own row in the flight table; it is not dropped.
- **OA-SHD-003** At each ERAM report time the raw cross-track and along-track separations from OpenATCA are judged
  separately: above 0.5 NM (926 m) and up to 1 NM (1852 m) is `POS_*_0P5_TO_1NM`, above 1 NM is `POS_*_GT_1NM`; a
  separation exactly at a threshold is not in the class above it.
- **OA-SHD-004** ERAM stamps whole seconds. The classification uses the raw along-track separation; the event
  carries OpenATCA's ground speed times 1 s (`along_credit_m`) and a flag `within_1s_stamp_allowance`, true when the
  raw separation less that credit is not over the event's threshold; the summary counts the events with and
  without the flag. Cross-track events carry no flag. Below 2 kt the direction is undefined and the whole
  separation counts as cross-track.
- **OA-SHD-005** OpenATCA is linearly interpolated to the ERAM report time between two of its one-second rows at
  most 2 s apart, or taken from the row at that time; with no such rows it has no sample there.
- **OA-SHD-006** An altitude difference above 300 ft either way is `ALT_GT_300FT`, only where both systems have an
  altitude; the others are counted as not comparable. Each event names the band of ERAM's altitude (below or at or
  above 18,000 ft), and the summary gives the event counts and the median signed difference of both bands (no
  altimeter setting is applied).
- **OA-SHD-007** Identity: a report disagrees on the beacon when ERAM's code in force (each message's code carried
  forward) differs from every squawk the OpenATCA adapter was offered within +-60 s of the report time (inclusive,
  the one in force at the window start included); a disagreement counts only when it lasts at least 3 consecutive
  reports of the ERAM flight, and each such run is one `ID_BEACON` event with its span (the first report's codes in
  the detail). Nothing is compared where either side has no code, and a report that agrees or is not compared ends
  a run. `ID_CALLSIGN` is the same for ERAM's callsign against the callsigns offered (case and padding ignored).
- **OA-SHD-008** A paired aircraft's first compared ERAM report and OpenATCA's first in-box row (and the last of each)
  differ by more than 12 s plus the ERAM report interval at that end: `START_TIMING`, `DROP_TIMING`, with the
  direction. Gaps inside the track are `ERAM_REPORT_GAP` only. The summary prints the median ERAM report interval.
- **OA-SHD-009** ERAM reports with no OpenATCA sample are `ERAM_ONLY_SPAN` (consecutive reports one span);
  OpenATCA rows outside the span of every ERAM flight of the aircraft, each widened by 12 s plus the report interval
  at that end, are `OA_ONLY_SPAN` (consecutive rows one span). A span's duration is first to last plus half the
  local sample interval at each end (a lone sample gets one interval; an interval counts at most 30 s).
- **OA-SHD-010** Where ERAM has a report and OpenATCA has a row at exactly that time, ERAM flagging coast and
  OpenATCA not is `COAST_ERAM_ONLY` and the reverse is `COAST_OA_ONLY`; the flag is not interpolated and the summary
  says the two flags are different concepts.
- **OA-SHD-011** With truth, each position event also names which system was closer to the truth at the ERAM
  report time (`ERAM`, `OpenATCA`, `tie` within 1 m, or empty without truth), computed after classification. The
  events, their order and values are the same with and without truth. The summary states that the truth shares
  its source with OpenATCA's input and favours OpenATCA, and counts the truth queries that gave no answer.
- **OA-SHD-012** The report is deterministic: `events.csv`, `flights.csv` and `summary.txt` are byte-identical for
  the same inputs; the summary holds no path, clock or run time and ends with a digest line over the three texts;
  it counts every class (events, flights, span, largest) and gives no aircraft address or callsign. A value that
  rounds to zero is written without a sign.
- **OA-SHD-013** The thresholds are one table and the summary prints every one.
- **OA-SHD-014** The flight table has one row per group and per unpaired flight or aircraft, with a flight index
  (the only name in the summary), the compared count and the worst cross-track, along-track and altitude
  difference.
- **OA-SHD-015** Both systems are restricted to the clip box (38.5 to 46.5 N, 83.5 to 93.5 W, inclusive); the
  identity the adapter was offered is the squawk and callsign carried forward from the aircraft's reports, one point
  per change.
- **OA-SHD-016** An input or output path the shared sealed list refuses (OA-SEAL-001), as given or after
  resolving links, is refused before anything is opened, exit 2.
- **OA-SHD-017** A missing, repeated, unknown or valueless flag (a value starting with `--` is missing) is a usage
  error, exit 2.
- **OA-SHD-018** The two recordings are cut to the overlap of their in-box recorded extents (ERAM's in-box track
  positions, OpenATCA's rows) before anything is compared, edges inclusive; a flight or aircraft left with no
  samples is gone, and the summary states the window in UTC. Recordings that do not overlap are an error.
- **OA-SHD-019** A gap of more than 30 s between two reports of one ERAM flight is `ERAM_REPORT_GAP` with its
  length; OpenATCA rows inside the flight's span are not `OA_ONLY_SPAN`.
- **OA-SHD-020** Two ERAM flights of one aircraft with overlapping spans: the overlap is `DUP_ERAM_TRACK` (its
  duration, the union of the overlap with the flights before it in priority), and a flight's reports inside the
  segments of those flights (gaps over 30 s not covered) are not compared; the flight with the earliest first report
  (ties: the lower flight index) is the primary. Each (aircraft, ERAM time) is compared once.
- **OA-SHD-021** An address-paired ERAM flight more than 20 NM from the paired aircraft for 3 or more consecutive
  reports is `ADDR_CONTRADICTS_TRACK` (with the span) and its reports in the span are left out of the position
  classes and of the worst-case columns.
- **OA-SHD-022** Every file the run opens is canonicalised and refused when it resolves into a sealed recording, so a
  link inside an unsealed directory is caught; the run itself checks its arguments before creating anything.
- **OA-SHD-023** A start or drop difference is not reported as `START_TIMING`/`DROP_TIMING` where an `OA_ONLY_SPAN` or
  `ERAM_ONLY_SPAN` already covers the interval between the two times; the span classes are disjoint by construction
  and the summary says span totals add.
- **OA-SHD-024** `DUP_ERAM_TRACK` is one event per secondary flight, its value the union of overlap time with the
  primary flights; a secondary's reports inside a gap of over 30 s of the primary are compared.
- **OA-SHD-025** `ERAM_REPORT_GAP` is measured on the compared (not duplicate-excluded) reports only.
- **OA-SHD-026** A beacon-paired flight is measured against the OpenATCA row nearest its first report within 12 s plus
  the ERAM report interval, else the first row inside its span; no interpolated sample is needed.
- **OA-SHD-027** Reports in an `ADDR_CONTRADICTS_TRACK` span are also left out of altitude, coast and identity; a
  report with no OpenATCA sample neither extends nor breaks a contradiction run.
- **OA-SHD-028** An output path that does not exist yet is resolved through its nearest existing ancestor before the
  sealed check.
- **OA-SHD-029** Every nonzero load count is printed in the summary header; a beacon table that overflowed
  (`DROP_SFDPS_BEACON_FULL`) fails the run.


## M5 close-encounter record (oa-encounter, tools/oa-encounter)

- **OA-ENC-001** A pair is in a class when its horizontal separation is strictly under the class's horizontal limit and its vertical separation strictly under the vertical limit; the record volume has the same strict edges, and the class of a finished encounter is the tightest class any tick with a known altitude met.
- **OA-ENC-002** An encounter starts at the first tick a pair is inside the record volume and ends when the pair has been outside it for the hysteresis (end time the last tick inside); a re-entry within the hysteresis continues the same encounter and a later one is a new encounter.
- **OA-ENC-003** Each encounter records the time of the closest horizontal approach, the minimum horizontal separation, the vertical separation and the mean altitude at that time, the minimum scaled three-dimensional separation, both tracks' flags and whether either was on the ground at that time.
- **OA-ENC-004** A pair with both aircraft on the ground inside the record volume is not recorded and is counted; one aircraft on the ground, or an unknown ground state, is recorded.
- **OA-ENC-005** A pair inside the horizontal radius with an altitude unknown on either track is recorded by horizontal distance, and an encounter with no tick of known altitude is class ALT_UNKNOWN; none is dropped.
- **OA-ENC-006** The grid scan and the pair evaluation count their work against configured budgets; over budget the pairs are evaluated in order of smallest separation, the remainder is counted as not evaluated, an open encounter whose pair was not evaluated is kept alive and marked, and every shortfall (tracks not scanned, candidates dropped, encounters not opened) is a count.
- **OA-ENC-007** The record is a deterministic function of the rows: the same rows in any order give the same encounters and counts; rows sharing an address, or with no valid position, are left out and counted.
- **OA-ENC-008** The grid finds every pair that the exact separation test finds inside the record radius, including across the antimeridian and at high and southern latitudes up to the limit `atan(0.02 / dphi)` (`dphi` the radius over 6,356,752 m, 85.8 degrees for 5 NM), which the core reports; rows above it are used and counted.
- **OA-ENC-009** The tool refuses, with exit 2 and before opening or creating anything, an input or output path that is, is inside, or resolves (through a link or the nearest existing ancestor of a new directory) into a sealed 2026-11-19 recording; its arguments are strict.
- **OA-ENC-010** A cycle whose time is not after the previous cycle's is refused and counted, an out-of-range configuration is refused, and a full encounter log counts what it loses.
- **OA-ENC-011** The tool writes `encounters.csv` and `summary.txt` deterministically, sorted by start time; the summary has counts per class, altitude band, hour and 0.25 degree cell, the work and overflow counts, a degraded state when any shortfall is non-zero and the digest of `encounters.csv`, and contains no aircraft address.
- **OA-ENC-012** A synthetic close pass run through the tracker and the record gives one encounter of the expected class and distance, repeats exactly, is not recorded at a large vertical separation, and the on-ground state at a tick is the latest report not older than 30 s.
- **OA-ENC-013** The true minimum horizontal separation of an encounter is found by moving the pair's relative position in a straight line between two consecutive ticks that were both evaluated, and its time is recorded beside the sampled minimum, which is kept; the interpolated minimum is never above the sampled one, and no line is drawn across a tick that was not evaluated. The smallest vertical separation and the vertical rates at the closest approach and their largest magnitude are recorded.
- **OA-ENC-014** Each encounter has one context at its closest approach: SURFACE (either aircraft on the ground, or within 5 NM of a public airport and under the field elevation plus 200 ft), else DEGRADED_TRACK (either track coasting or uncertain), else AIRBORNE (both altitudes known) or AIRBORNE_ALT_UNKNOWN; the distance classes apply within each context.
- **OA-ENC-015** The summary leads with AIRBORNE and then AIRBORNE_ALT_UNKNOWN, counts SURFACE and DEGRADED_TRACK with the rule that defines each, lists every encounter at or above 18,000 ft pressure altitude without an address with its breakdown (vertical at the level of 975 ft or more, a climb or descent, anything else), and gives the AIRBORNE vertical separation at the closest approach from 900 to 1,000 ft in 25 ft bins.
- **OA-ENC-016** For an encounter at or above 18,000 ft the tool finds the smallest separation between the two aircraft's own reports (one aircraft's position at the other's report times, straight between its reports when under 30 s apart (the clip's reports are about 20 s apart at altitude)) during the encounter, and reports none for a pair with no such time.
- **OA-ENC-017** An AIRBORNE encounter whose vertical separation at the closest approach is at least the record volume's vertical limit less one 25 ft altitude quantum, with both vertical rates known and under 300 fpm, is AT_VERTICAL_MINIMUM: it stays in the record and the CSV (a column), is counted on its own line with its rule and is left out of the AIRBORNE class, band and hour tables.
- **OA-ENC-018** The airports of the surface rule are the NASR `APT_BASE.csv` rows of site type airport and public use, with position and field elevation, read from the CSV archive; a missing file or a bad archive is an error.
- **OA-ENC-019** A shortfall never ends or splits an encounter: when the candidate table overflowed, or rows were removed (a duplicate address, no valid position, over capacity), every open encounter not evaluated at the tick whose pair could have been lost to it is kept alive and its unevaluated ticks counted; the counts add up (`evaluated + not_evaluated = candidates`, the dropped ones a part of `not_evaluated`), and a hysteresis of zero or less is refused.
- **OA-ENC-020** The grid skips, and counts, a pair whose two altitudes are known and not under the record volume's vertical limit apart (strict, as the volume test); an unknown altitude is never skipped, and the record is the same as without the skip.
- **OA-ENC-021** The core states the latitude limit of the grid test, rows above it are counted, and the summary prints both and calls the record degraded when any is non-zero, as for duplicate and invalid rows.
- **OA-ENC-022** The offline A3 rate join consumes actual `oa-ca` alerts and `oa-encounter` episodes from the same clip; requires matching clip digest and exact 5 NM / 1,000 ft profiles; canonicalizes unordered address pairs; and matches only closed same-pair interval overlap. Encounter class values are exactly `C1` through `C6`, `VOLUME_ONLY`, and `ALT_UNKNOWN`; missing, unknown or misspelled classes are refused before any hit, miss or nuisance classification. Fixed 2026-11-19 UTC bins are half-open `[hour_start,hour_end)` from 00:00 through 23:00 exclusive; episodes are assigned once by onset. A 120-second warmup is applied locally from first usable clip time: an early episode before that context is unresolved, while later fully covered hours remain eligible. Right censoring is also local: events reaching clip coverage end are counted and unresolved for negative labels. Per-hour covered seconds are reported; nuisance alert rate divides covered nuisance episodes by covered hours. Warning sensitivity is matched known-altitude episodes divided by resolved known-altitude episodes (matches plus misses). Global `oa-ca` and `oa-encounter` summary shortfalls conservatively make every label unresolved. Known-altitude encounter-volume episodes without a matching alert are misses only with complete global run summaries, no unevaluated ticks, sufficient local warmup, and no right censoring. Unmatched alerts are nuisance only with complete global run summaries, no unevaluated ticks, sufficient local warmup, no right censoring, and no overlapping `ALT_UNKNOWN` episode. Unknown-altitude labels, unevaluated episodes, global run shortfalls, and event-local coverage gaps remain explicit unresolved counts. Inputs are opened only through the production sealed guard as bounded regular, single-link files; episode timestamps intersecting a guarded holdout window refuse the join. Output is a new private directory with create-new 0600 aggregate files, no pair identifiers/event rows, and no sealed-path bypass. The matching v2 profile has fixed ceilings of 200,000 rows per input CSV, 512 MiB per CSV, and 10,000,000 same-pair interval comparisons; any exceeded ceiling refuses v2 output. A separate after-the-fact count-only diagnostic profile can stream at most 1,000,000 rows per input CSV (still at most 512 MiB), emitting 23 hourly onset counts, coverage and censoring, plus available run shortfall counters. It performs no matching, classification, or rate calculation; every in-window row remains unresolved, degraded run status is preserved, and this diagnostic does not make the unavailable v2 rate result available. Claims are limited to ADS-B-derived encounter-volume coincidence proxies, never independent FAA/human truth or facility false-alert qualification. Synthetic coverage is in `tools/oa-encounter/tests/test_a3_rate_join.py` tests `test_exact_encounter_class_vocabulary_is_accepted`, `test_unknown_missing_and_typo_encounter_classes_refuse_input`, `test_direct_aggregate_refuses_invalid_encounter_class_before_labeling`, `test_closed_interval_endpoint_overlap_is_a_match_and_unordered_pair_matches`, `test_wrong_pair_does_not_match`, `test_altitude_unknown_truth_and_alert_stay_unresolved`, `test_incomplete_run_or_unevaluated_episode_never_creates_negative_claim`, `test_episode_onset_is_counted_once_in_utc_half_open_hour`, `test_unmatched_alert_is_nuisance_only_with_complete_coverage`, `test_clip_without_full_window_and_warmup_makes_negative_claim_unresolved`, `test_missing_preroll_only_affects_near_start_episodes`, `test_right_censoring_is_local_to_the_event_time`, `test_cli_requires_matching_clip_profiles_and_emits_only_aggregate_data`, `test_episode_csv_guard_requires_regular_file`, and `test_episode_interval_guard_rejects_holdout_overlap_including_closed_boundary`, `test_count_only_diagnostic_reports_bounded_unresolved_counts_without_rates`, and `test_count_only_diagnostic_refuses_more_than_fixed_row_limit`, `test_summary_counter_map_stops_before_later_numeric_sections`, and `test_coverage_validation_rejects_nonfinite_and_reversed_timestamps`.
  Additional synthetic coverage checks degraded-summary clip digest preservation and mixed-run unresolved classification in `test_degraded_summary_keeps_clip_digest_and_mixed_runs_stay_unresolved` and `test_different_degraded_clip_digests_never_alias`.
- **OA-ENC-023** The offline A8 probe obtains each active plan's positions only from the shared route walk in `tools/oa-legacy-traj/src/core/trajectory.rs`: the established `issue_trajectory` API and the bounded `trajectory_profile_offsets` plus `issue_trajectory_samples` APIs use the same route-walk implementation. Each profile contains its state epoch, the complete 20-minute horizon at 30-second intervals, and the rounded-millisecond arrival time of every active filed route vertex within that horizon; a partial horizon or any sample gap over 30 seconds is invalid. The established 5-, 10- and 20-minute outputs remain equal to the sampled API at those durations. Pair profiles are evaluated only when their state epochs match; different epochs return `Unavailable` instead of aligning relative durations. Equal-epoch profiles are merged into one sorted absolute-time pair timeline, and both plans are reevaluated at every merged duration. Unavailable or degraded horizons, nonzero `over_capacity_unexamined`, mismatched identity, missing altitude or invalid time axes produce `Unavailable`, never a fallback forecast. For an aircraft pair, adjacent aligned samples are joined by the shared probe's stated segment approximation: project both aircraft into an azimuthal-equidistant chart centred at the first aircraft's segment-start position, then use straight-line relative motion. Actual filed route vertices are included so a pair segment does not span a modeled route turn; between samples this remains a piecewise-linear probe approximation, not an independent route or aircraft-performance model. The probe uses strict `< 9,260 m` horizontal and `< 1,000 ft` vertical limits over the fully covered 20-minute window, reports the earliest integer millisecond inside both strict loss intervals, and reports the minimum horizontal CPA and its corresponding vertical separation over all examined adjacent sample pairs (ties retain the earlier pair). A detached trial-plan input is evaluated against the other active plan without mutating either active input; only the route/plan revision may differ, while the candidate must retain the active aircraft's GUFI, address, centre, observed track, cutoff, ownership, clearance, ETA target and age policy. A caller-supplied SUA geometry with an authoritative half-open active-time interval and MSL floor/ceiling is evaluated only against a complete 20-minute profile with gaps no greater than 30 seconds. The airspace core checks geodesically interpolated positions and linearly interpolated MSL altitude at samples no more than 30 seconds apart. It reports `Entry` only for an interior sampled point inside the geometry and altitude band; a boundary-quantized point, unresolved/tangent segment, schedule coverage beyond a truncated tail, or any other uncertified interval returns `Unavailable`, never `Clear`. Unknown schedule/altitude and oversized geometry also return `Unavailable`. These are offline screening results, not operational separation assurance or independent trajectory truth.

## M5 — Safety nets: state and inhibitions (oa-safety, oa-display)

Build order step 1 of docs/M5_DESIGN.md (rules 1 and 2). `oa-safety` is a core crate with no
allocation after start; times are nanoseconds, positions 1e-7 degree, altitudes feet.

### Net state (oa-safety)

- **OA-SAF-001** A net that has never reported reads as `Unavailable(NeverReported)`, one whose last
  report is older than the staleness limit (age strictly over it) as `Unavailable(StateStale)` with
  its age, and one whose report is dated after the query as `Unavailable(TimeInconsistent)`: a net
  that stops reporting is never shown as running. Extreme times do not panic.
- **OA-SAF-002** The staleness limit and the future horizon come from configuration with no default
  (a non-positive limit or a negative horizon is refused); an age equal to the limit is still current.
- **OA-SAF-003** Each net's state (running, degraded with reason, area and count, unavailable with
  reason) is reported with the age of the state, for every net, in a fixed order.
- **OA-SAF-004** A running net with at least one inhibition in force reads as inhibited, derived
  from the inhibition table (a net cannot report it), and a degraded or unavailable net keeps its
  state and shows the inhibitions in force as well.

### Inhibitions (oa-safety)

- **OA-SAF-005** An inhibition request is refused, counted and changes nothing when: the
  configuration has no positive maximum duration for the net (there is no default), the end is not
  after the start, the end is already past, the start is further ahead than the configured horizon
  (no horizon configured is refused too), the area is larger than the configured extent (likewise),
  the duration is over the maximum by one nanosecond (or
  overflows), the polygon is not valid, the altitude band is reversed, or the reason or author is
  empty, longer than its field or not printable ASCII. A duration exactly the maximum is accepted.
- **OA-SAF-006** A full table refuses the request and counts it; no inhibition is evicted, and an
  id is never reused.
- **OA-SAF-007** An inhibition is in force at `t` iff `start <= t < end`: the start instant is in, the end
  instant out, and an empty or reversed span is never in force. Tested at both boundaries and on an empty
  and a reversed span (`an_inhibition_is_in_force_from_its_start_to_just_before_its_end`).
- **OA-SAF-008** The altitude band is inclusive at both ends and empty when reversed. Tested at both
  boundaries and on a reversed band (`the_altitude_band_is_inclusive_at_both_ends`).
- **OA-SAF-009** The query names the inhibitions that hide an alert at a position, altitude and
  time: on the net, in force, the point strictly inside the area (a point on an edge or vertex is
  not hidden), the altitude in the band. An unknown altitude is hidden by none. The pure query
  logs nothing.
- **OA-SAF-010** Every suppression is recorded (net, inhibition id, time, position) in a bounded
  log; an entry that does not fit is counted as overflow, the suppression still applies, and no
  entry is evicted.
- **OA-SAF-011** Removing an inhibition before its end is an explicit action that names who did it,
  is refused for an unknown id or an invalid name, and is logged with the time and the name.
- **OA-SAF-012** An inhibition whose end has passed is reported once as an expiry event (review) and
  leaves the table; it no longer suppresses; creation is logged with the author.
- **OA-SAF-013** If the event log is full an expired inhibition stays in the table (counted, not in
  force) until the log is drained, then is reported once: no expiry is lost.
- **OA-SAF-014** A polygon of 3 to 32 vertices is accepted only if no vertex is at a pole or off the
  globe, no consecutive vertices repeat, no edge spans 180 degrees of longitude, no edge folds back
  over its neighbour, no two edges cross or touch and its area is not zero; point-in-polygon is
  integer arithmetic in 128 bits, correct for a concave polygon and at the extremes of the range.

### Display indications (oa-display)

- **OA-DSP-060** When every net is running with a fresh state, no inhibition is in force, every
  feed is live and the newest track update is within the limit, the safety build draws nothing at
  all, so a traffic frame (and each ZAU map frame, sha256 e34d3091...a062, 306bc3de...a0c3 and
  06dca517...ac84) is byte for byte what it was.
- **OA-DSP-061** Every net that is not plainly running is one banner line in the alert colour above
  the other layers, under the legend strip, giving the name, state, reason, area, count and age of
  the state (and the inhibitions in force); numbers are capped so no line is cut; the banner is a
  function of the state alone, so it cannot be dismissed and ends with the condition.
- **OA-DSP-062** An inhibition in force is drawn as a closed outline in the alert colour with a label
  giving the net, the end time in UTC, the author and the reason cut to 16 characters; one not yet
  started or already ended is not drawn; the net's banner line reads inhibited with the count.
- **OA-DSP-063** The banner says `PICTURE FROZEN` with the age when the newest track update is at
  least the configured limit old, nothing one nanosecond before it, and says so when there has been
  no update; a limit that is not positive is refused.
- **OA-DSP-064** A feed that is slow or lost is one banner line with its name and age, a live feed
  none, and more feeds than the display lists is refused, not cut (design ND-04).
- **OA-DSP-065** The worst state (64 inhibitions of 32 vertices, every net degraded with extreme
  numbers, eight lost feeds, a frozen picture) fits `safety_caps` and `MAX_SAFETY_WORK`, loses no
  text and rasterizes.
- **OA-DSP-066** The UTC date and time of an instant are integer civil time from the epoch,
  correct across leap days, the year 2000 and negative times, and total over every `i64`.
- **OA-SAF-015** A report dated before the net's stored one that is worse than it (unavailable over
  degraded over running) replaces its state at once and keeps the later time, and is counted; any
  other backdated report is refused and counted.
- **OA-SAF-016** A report dated more than the configured horizon after the report-time clock
  (including `i64::MAX`) is refused and counted, and the net reads `Unavailable(TimeInconsistent)`
  until a valid report arrives; the stored report is untouched, so no one timestamp locks a net out.
- **OA-SAF-017** A request is refused while the event log has no room for its record (creation: one
  slot, counted; removal: one slot, two for an expired inhibition still waiting, which then emits
  its expiry and then the removal), the inhibition staying visible.
- **OA-SAF-018** A start horizon and a maximum extent are required configuration (no default) and
  enforced to the unit: a start exactly at the horizon is accepted, one nanosecond more is not; an
  area exactly at the extent is accepted, one unit more is not.
- **OA-SAF-019** Suppressions lost to a full log mark the net degraded (`SuppressionLogFull`, with
  the count) until the log is cleared; the total lost stays counted.
- **OA-SAF-020** The independent offline track monitor checks up to 128 caller-paired observations
  and claims against an explicit clock and local-frame motion envelope. It distinguishes a
  contradiction from missing, duplicate, stale, future, or unsupported evidence. A capacity refusal
  leaves the output untouched. The mutation fixture reports detected and in-envelope missed changes.
- **OA-SAF-021** A pair suppression names two distinct nonzero aircraft addresses, one existing
  alert's exact start time, a finite start/end window, reason and author. Addresses are canonicalized;
  only that alert instance is suppressed. Invalid or overlong requests and a full bounded table are
  refused and counted; expiry is exclusive at the end instant.
- **OA-SAF-022** Group suppression is available only when the facility's issued adaptation enables
  it and its purpose is a military special operation. Its bounded member set contains distinct
  nonzero addresses; an alert is suppressed only when both endpoints belong to that set, so every
  outsider pair remains active. Other purposes, a disabled facility policy, an invalid set or a full
  table are refused and counted.
- **OA-SAF-023** Every actual pair, group or area suppression records a bounded receipt with the
  canonical aircraft pair, the alert instance start, source and time; that receipt is the
  acknowledgement of that instance. A full receipt log counts the lost entry while suppression
  still applies and degrades the net. Acknowledgement does not resolve or close the detected
  conflict, and cannot carry to a later alert instance.
- **OA-DSP-067** A newest track update dated after now is never fresh: the banner says `PICTURE TIME
  INCONSISTENT`.
- **OA-DSP-068** Any loss in the safety record (event log overflow, suppressions lost, expired
  inhibitions waiting, refused state reports, clipped texts) is a banner line `SAFETY RECORD
  INCOMPLETE` with the counts, second in the banner, and a net with suppressions lost reads degraded.
- **OA-DSP-069** A build that fails before drawing still draws the net lines and `SAFETY DISPLAY
  ERROR <code>` and returns the error (an alarm to the shell); a faulted list returns the error.
- **OA-DSP-070** A banner that does not fit the frame ends with `+N MORE STATES NOT SHOWN`, the
  count of hidden lines.
- **OA-DSP-071** The banner box is opaque; targets whose symbol is under it are counted on it
  (`TARGETS UNDER BANNER: n`) and in the stats.
- **OA-DSP-072** The offline raw replay uses bounded surveillance observations available to its
  caller as of the frame. Only the newest fresh on-map observation for each address can take a
  target slot. A newest ADS-B `F_STALE` observation suppresses its address without falling back
  to older observations; a same-time fresh/stale disagreement is refused as a conflict. Source-stale
  exclusions are counted separately from age-stale observations and rows withheld for later receipt.
  Conflicting latest observations are refused and counted. The frame carries `RAW SURVEILLANCE ONLY`,
  and a failed build makes its list unavailable to the rasterizer.
- **OA-DSP-073** The offline scope replay can show `FEED RADAR COVERAGE LIMITED` when same-instance
  interval counters report omitted work in radar association or initiation. The indication is
  independent of source freshness and does not total unlike or overlapping work counters. A
  decrease in a relevant counter is displayed as inconsistent health.
- **OA-DSP-074** Raw publication recovery verifies exact input, adaptation, receipt/settings and
  ordered-frame witness provenance before admitting an existing completed output prefix.
- **OA-DSP-075** Restart publication verifies completed prefix bytes without replacement and
  publishes only the ordered missing suffix and closed manifest. Partial staging and publication
  interruption must not silently replace completed files or accept damaged output.
- **OA-DSP-076** All cooperating publishers, including overwrite writers, hold one same-host
  exclusive lock through validation, removal and final publication. This is not distributed fencing
  or restoration of tracker/process state.
- **OA-SHD-030** `oa-shadow screen` compares ADS-B with ERAM's reports of address-paired aircraft: ADS-B is
  evaluated at each ERAM report time by oa-truth's smoother (default settings) and the offset ADS-B minus ERAM is
  resolved along and across the ADS-B velocity direction. The along offset is `c - L x speed`: L is the apparent
  latency, positive when ADS-B is late (a late report is behind); b is the mean cross offset, positive to the right.
  A known injected latency and offset are recovered within the bootstrap interval.
- **OA-SHD-031** Two stages, because one aircraft's speed barely varies and its c and L are collinear: (a) the fleet
  fit of c and L over all samples of the screened aircraft, by Theil-Sen (the reference: all pairs of 12 evenly
  spaced samples of each aircraft, the same for the point fit and every bootstrap resample; a bootstrap over
  aircraft for its interval) and by least squares (pulled by lagged aircraft); (b) per aircraft L_i alone, the
  median of (c_fleet - along) / speed, and its latency excess L_i - fleet L; the cross offset b is the median cross
  offset. A constant along-track offset and a latency are indistinguishable at constant speed, so a constant-speed
  aircraft is identified only through stage (b). The two-parameter per-aircraft fit is a cross-check for aircraft
  whose speed (5th to 95th percentile) spans 50 kt or more. Aircraft are listed by ascending address.
- **OA-SHD-032** LAG is the lower 95 % bound of the latency excess strictly over 0.1 s, meaning an along-track
  excess equivalent to a latency of 0.1 s or more; EARLY is the upper bound strictly under -0.1 s (counted, not a
  LAG). BIAS is |b - fleet cross reference| (the reference: the median of the aircraft's median cross offsets)
  strictly over max(3 x the 95 % bound of the aircraft's median NACp, the fleet's p95 of |b - reference|) with the
  lower 95 % bound of it strictly over it too; an unknown NACp never flags BIAS. A LAG or BIAS stays only if the
  aircraft recomputed without the one sample that most supports it still has it; the counts before and after are
  printed. LAG is also given with c at both ends of the Theil-Sen c interval and under the least-squares fleet fit.
- **OA-SHD-033** The aircraft interval is a block bootstrap (blocks of 10 consecutive samples, circular starts,
  1000 resamples) seeded from the aircraft address, the fleet interval a seeded bootstrap over aircraft (200
  resamples), so two runs give identical values; the valid replicates behind each interval are printed.
- **OA-SHD-034** A sample needs a smoother state, |ADS-B - ERAM| at most 2000 m, a direction of travel (over
  2 kt), a 95 % horizontal radius of at most 15 m, a turn rate (heading change over 2 s either side) of at most
  0.5 deg/s, a vertical rate of at most 500 ft/min in magnitude (edges kept) and finite offsets; an ERAM report is
  left out for the first reason that applies, in that order, and every reason is counted. No gap rule applies: a
  thinned ADS-B trace still gives samples; the distance of a used sample to the nearest raw report is printed. An
  unknown ERAM altitude has its own band code.
- **OA-SHD-035** Left out of the screen and counted: ERAM reports in a duplicate-track overlap (a flight lends its
  span only over the reports that survive the contradiction rule), in a contradicted address span (3 or more
  consecutive reports over 20 NM from the smoother), and coasted ones; the reasons of OA-SHD-034; the counts add up
  to the ERAM reports.
- **OA-SHD-036** Two minimums, both printed: the fleet fit and cross reference use every aircraft with 30 or more
  samples over 10 minutes or more; only aircraft with 100 or more samples (ten blocks) over 10 minutes or more are
  screened for flags, and every prevalence line is over those. Aircraft out of the fit are counted under their
  reason; an aircraft eligible for flags that cannot be screened is an error, so screened equals eligible; with
  fewer than 30 screened the summary says the day has too few long, gated tracks for a prevalence estimate.
- **OA-SHD-037** The screen summary carries the fixed caveat text and the statement that an offset and a latency are
  indistinguishable at constant speed, the fleet c and L with intervals, the BIAS threshold rule and both its terms,
  the counts and fractions of LAG, BIAS and EARLY, the distributions (p05, p50, p95) and a digest over
  `aircraft.csv` and the summary; the summary names no address.
- **OA-SHD-038** The screen reads only usable raw ADS-B positions (not stale, not on the ground, an ADS-B source, a
  non-zero latitude), one per millisecond per address, with the NACp carried forward from the last known value
  (1 to 15) within the truth configuration's carry age; an absent or zero NACp is unknown.

## M5 — Conflict alert (oa-ca, tools/oa-ca)

Build order step 3 of docs/M5_DESIGN.md (rules 1, 4 and 5 for conflict alert). `oa-ca` is a core crate
with no allocation after start; times are nanoseconds, positions degrees, altitudes feet, relative
motion in the projection's metres and metres per second. Every value of the alerting rules is
configuration with no default; the crate's tests use synthetic values.

- **OA-CA-001** The horizontal loss interval of a pair is the solved quadratic `|r + v t| < H` inside
  `[0, T]`, from the closest-approach time and the perpendicular miss; the edges are strict (a pass at
  exactly `H` is not a loss), a relative speed under 5e-5 m/s is zero (inside for the whole look-ahead or
  never), the discriminant is handled exactly, and the smallest horizontal miss over the look-ahead is
  reported.
- **OA-CA-002** The vertical loss interval is `|dz + dvz t| < V` inside `[0, T]` with strict edges; a
  relative vertical rate under 5e-5 ft/s is zero; the smallest vertical miss over the look-ahead is
  reported (zero when the altitudes cross).
- **OA-CA-003** A pair is in conflict when the horizontal and vertical intervals meet in an overlap of
  positive length or both hold at `t = 0`; the prediction reports the time to loss (the start of the
  overlap, zero when current), both predicted misses and whether the loss is current; an alert carries
  its first and peak time to loss and the misses.
- **OA-CA-004** The horizontal minimum of a pair is the volume's plus both tracks' 95 % radii (a poor
  track widens the alert and never hides it); a row whose radius is not finite, is negative or is over the
  configured cap takes the cap and is counted.
- **OA-CA-005** The vertical minimum is widened by a named constant (100 ft) for each aircraft whose
  track is coasting or uncertain; the alert carries the coasting, uncertain, radar-only and
  identity-unconfirmed flags of either track.
- **OA-CA-006** A pair with an altitude unknown on either track is tested horizontally only and its alert
  carries `ALT_UNKNOWN`; a pair that is horizontally clear stays clear; an unknown altitude never removes
  a conflict (rule 4).
- **OA-CA-007** A track that had a known altitude and has none raises one `ALTITUDE_LOST` event, once per
  loss and again only after the altitude has returned and gone; the event log is bounded, counts what it
  loses and degrades the net.
- **OA-CA-008** The minima of a pair are those of the first configured volume whose area holds the
  midpoint of the pair and whose altitude band holds the mean altitude; a pair outside every volume uses
  the required default volume; with an unknown altitude the larger horizontal and the larger vertical
  minimum among the volumes whose area holds the midpoint AND the default volume apply, so an unknown
  altitude never gets a smaller minimum than any known altitude at that place. An area is a latitude and
  longitude box or an `oa-safety` polygon. The volume is chosen once per evaluation, from the midpoint and
  mean altitude now, and holds for the whole look-ahead: a pair that climbs or descends out of the band
  during the look-ahead keeps the minima it started with.
- **OA-CA-009** The candidate grid finds every pair inside its radius (look-ahead times the closing speed
  bound, plus the largest horizontal minimum and both 95 % radii), the same pairs for any grid cell size and
  at least those a brute-force exact test finds; a pair with both altitudes and rates known that cannot come
  within the vertical minimum in the look-ahead is skipped and counted; a pair with an unknown rate is not
  skipped.
- **OA-CA-010** The grid scan and the pair evaluation count their work against configured budgets; over
  budget the pairs are evaluated in order of the least lower bound on the time to reach the limit (the
  flat separation less the widest limit, over the sum of the ground speeds, a missing speed taken as the
  configured maximum; zero inside the limit), ties by separation now and then by track numbers, so a
  distant pair closing fast is not put behind a near pair moving apart; the remainder is counted as not evaluated, an open alert
  whose pair was not evaluated is kept alive and marked, and tracks not scanned, candidates dropped, rows
  over capacity and alerts not opened are counts that make the net degraded. A candidate table that fills
  keeps the pairs with the smallest current separation (ties by track numbers), whatever the scan order,
  with bounded work per pair, and counts each pair it gives up.
- **OA-CA-011** Each cycle publishes the net's state to the `oa-safety` table: running when everything was
  evaluated; degraded, with the count of every shortfall of the cycle (`BudgetExceeded` when work was left
  undone or lost, `InputMissing` for rows left out, lost before the cycle or altered; the cycle result gives
  the work and input counts apart), otherwise; unavailable (`SurveillanceLost`) for a cycle whose time is not
  after the last, and unavailable (`InputMissing`) when the configuration was refused (the batch tool exits
  rather than publishing it); reports the table refuses are counted. A caller that lost rows before a cycle
  says so, and that cycle is not running. Lifetime `CycleStats` additions use checked arithmetic, saturate affected counts at `u64::MAX`, and set an explicit overflow flag; the normal summary reports overflow and remains degraded/incomplete rather than declaring a complete run.
- **OA-CA-012** One conflict is one alert from its first cycle in conflict to its last, ended when the pair
  has been out of conflict for the hysteresis (a return within it continues the alert, a later one is a new
  alert), with its start, peak (the smallest time to loss) and end.
- **OA-CA-013** An out-of-range configuration is refused with the name of the value: capacities, look-ahead,
  hysteresis, budgets, speed, radius cap, grid band, volumes (minima, altitude band, area), and a look-ahead
  and speed whose reach is beyond the bound the solver is proved for. Log capacities above 2^24, an area of
  more than 32 points and a configuration text above 1 MiB are refused; an allocation that fails is an
  error, never an abort.
- **OA-CA-014** A full alert log counts what it loses (`overflow`), the lost alerts degrade the net, and the
  consumer clears the log keeping the allocation and the count. Alerts ended when the run finishes and lost
  to a full log come after the last cycle's report: they are counted apart (`finish_dropped`) and the final
  net state is degraded with their count.
- **OA-CA-015** The alerts are a deterministic function of the rows: below the track capacity any row order
  gives the same alerts and counts; rows sharing an address are all left out, and rows with a bad position, a
  speed over the bound, a non-finite value or an altitude or rate out of range are refused, all counted. The
  summary also reports overlapping bad-row reason counts for latitude, longitude, velocity components,
  speed, altitude and vertical rate; these subsets are not additional rejected rows and must not be summed.
- **OA-CA-016** A pair with both aircraft on the ground is left out and counted; a pair with one on the
  ground is tested and its alert carries `ONE_GROUND`.
- **OA-CA-017** A pair with both altitudes known and a vertical rate unknown on either track is tested
  horizontally only (the rate is not assumed level), counted, and its alert carries `RATE_UNKNOWN`.
- **OA-CA-018** Proved (Kani, bounded): for every relative position, velocity, limit and look-ahead within the
  stated bounds, the horizontal solver returns no NaN or infinity and an ordered interval inside the
  look-ahead.
- **OA-CA-019** Proved (Kani, bounded): the same for the vertical solver and the vertical miss, which is not
  negative.
- **OA-CA-021** Proved (Kani, bounded): an unknown altitude never removes a conflict: whenever a pair is in
  conflict with the altitude known it is in conflict with it unknown, starting no later and no less current.
  (Kani proof `ca-unknown-altitude`.)
- **OA-CA-022** The tool's arguments are strict, and it exits 2, before opening or creating anything, for an
  input, configuration or output path that is, is inside, or resolves into a sealed 2026-11-19 recording.
- **OA-CA-023** The configuration file has no defaults: every key is required, an unknown or repeated key, a
  value that is not a finite number, a malformed volume (box or polygon) and a value the core refuses are
  errors naming the value.
- **OA-CA-024** A track's 95 % radius is the square root of the chi-square quantile (2 degrees of freedom, 95
  %) times the larger eigenvalue of its position covariance; an unusable covariance gives NaN.
- **OA-CA-025** The tool writes `alerts.csv`, `altitude_lost.csv` and `summary.txt` deterministically, sorted
  by start time; the summary has the alerts by context, by flag, per hour, the net-state cycles by state, the
  altitude losses, every shortfall with its count and a degraded state when any is non-zero and the digest of
  `alerts.csv`, and contains no aircraft address. Each file is written to a temporary name, flushed and
  renamed, `summary.txt` last, and the output files of an earlier run are removed before the run starts, so
  a failed run leaves none.
- **OA-CA-026** A pair converging through the tracker gives one alert that repeats exactly, with no shortfall
  and every cycle running; the same crossing a volume apart vertically gives none.
- **OA-CA-027** The conflict decision (both intervals together) agrees with a brute-force scan of sampled times
  within 1 m (1 ft vertically) over tens of thousands of pseudo-random pairs: every pair with a sampled time
  more than the tolerance inside both limits is a conflict, and a conflict that no sampled time shows is an
  overlap shorter than one scan step; the pairs include stationary and level ones, pairs inside both limits
  now by a hair, and pairs at the limit and at the end of the look-ahead. Not proved: the bounded proofs of
  this decision did not finish in an hour.
- **OA-CA-028** A pair inside the horizontal limit now, or inside the vertical limit now, always has a
  loss interval of that kind, current, starting at zero and ending inside the look-ahead. This holds even
  when the rounding of the solution puts the end of the interval a hair before zero; the exact edge is not
  inside.
  - The horizontal solver is split at the square root, in the same arithmetic and the same order as the
    unsplit code (`crates/oa-ca/tests/split.rs`: bit for bit against 2,400 vectors from the unsplit code at
    commit 4eee1aa, in `tests/golden/`, and against a copy of it on a million inputs).
  - Proved with Kani, bounded:
    - the squared speed and the discriminant stay within their bounds (`ca-horizontal-now-bounds`);
    - the time of closest approach is finite and bounded when the speed is not zero
      (`ca-horizontal-now-approach`);
    - the square root of a finite discriminant in `(0, 4e19]` is finite, not negative and at most 7e9
      (`ca-horizontal-now-sqrt`);
    - from any root under that contract, the span of a current pair is current (`ca-horizontal-now-root`).
  - Tested, not proved:
    - the decision of the first step, `step_of`: a current pair gets a current span or a root step with a
      positive discriminant. Tested at margins from 1 m to 1e-12 m and on 40,000 random inputs over the
      domain the bounds proof covers;
    - the hand-off between the two steps: the first step's output meets the root step's contract, on
      400,000 bounded inputs. A proof of the real `horizontal_step` does not finish in ten minutes.
- **OA-CA-029** A prediction is made from the horizontal and vertical spans, then their conflict span, the
  two misses, and `prediction_of`.
  - Proved with Kani, bounded:
    - the conflict span of any ordered horizontal span and a bounded vertical part, through `meet`, is
      finite and ordered inside the look-ahead (`ca-predict-span`);
    - the vertical miss is finite and not negative (`ca-vertical-span`);
    - the closest point of the horizontal miss is finite, within 5e7 m of the origin, at a time inside the
      look-ahead (`ca-horizontal-miss-point`);
    - the horizontal miss, the exactly rounded square root of `x*x + y*y` at that point, is finite and not
      negative (`ca-horizontal-miss`, on the real function). At most 5e7 m on each axis, the sum cannot
      overflow.
  - Tested, not proved:
    - `prediction_of`: the time to loss is the span's start, or zero for a current span, and lies inside the
      look-ahead; the misses are carried unchanged, a NaN vertical miss included. Tested on edge cases and on
      40,000 random spans and misses;
    - the miss agrees with `oa_math::hypot` to 2 units in the last place wherever the larger coordinate is
      at least 1e-150 m, and to 1e-150 m below that, where the squares underflow (`tests/miss.rs`, on the
      real function).
  - The chain rests on the exact rounding of `oa_ieee::sqrt`, as the root proof of OA-CA-028 does, and on no
    other library contract; the crate no longer calls `hypot`.
  - Beyond about 1e154 m the square overflows to infinity, far outside the bound the configuration check
    enforces (OA-CA-013).
- **OA-CA-030** The tool uses the shared sealed-path guard (`oa-sealed`) for every path: the clip, the
  configuration and the output directory are checked as given and after links before anything is opened or
  created, and the configuration is opened, then verified as the file opened (a link swapped between check
  and open is refused). A sealed directory, a sealed ADS-B day, a link into sealed data and a link swapped
  after the check are each refused, with the path named; a refusal exits 2. The clip is read by the guarded
  `oa_bench::input::load_adsb`.
- **OA-CA-031** The tool drops the clip rows whose time is in a sealed window (the reader's own rule,
  OA-SEAL-014) and prints `sealed-window rows dropped: N` once on standard error on every exit that comes back
  through `main`, a usage error, a refusal and a failed run included (with N the count, 0 when nothing was
  read), through `oa_sealed::DropReport`; the count is also a line of `summary.txt`. A failure while the clip
  is being read still reports the rows dropped up to that point.
- **OA-CA-032** The example `nas_hour` (`tools/oa-ca/examples`) is the M9 national-hour measurement. `census` counts
  the distinct airborne addresses (a barometric altitude, not on the ground) and the trace rows of each UTC
  hour of a clip, and the peak minute of each hour, and prints no address. `track` and `ca` feed every
  measurement of the clip to the tracker (and, for `ca`, the one-second rows to conflict alert), measure only
  from `--start` to `--start` + 3600 s, and print CPU, peak and working-set memory, reports, tracks, the
  latency of the update calls and of the tick-crossing calls apart, the latency of each second, the tracker's
  refusal and capacity counters and, for `ca`, the pairs evaluated, the cycle times and the cycles left short by
  a budget. With `--rate-hz`, the stream is interpolated linearly between consecutive points of an aircraft no
  more than 60 s apart, at that rate, with the earlier point's quality fields, and nothing is added past an
  aircraft's last point (`oa_ca_run::synth`); the output says "synthetic rate from interpolated trace points".
  The whole-hour real-time factor is available only when input passes framing and seal checks and an
  accepted measurement is offered in each second of the hour. Observed and expected seconds are printed
  with the input-integrity status. A sealed path is refused and `--start` is required. It scores nothing
  and changes neither the tracker nor conflict alert (`tests/nas_hour.rs`).
- **OA-CA-033** The prefilter never rejects a pair that the exact test would accept, at any latitude
  up to 85 degrees, in either hemisphere and across the antimeridian. Each pair is judged on its own
  radius and latitudes, so one fast or uncertain track no longer limits the latitude of every other
  pair:
  - A pair uses the flat-earth separation only while that over-estimates the distance by less than
    the margin, that is while the larger tangent of the two latitudes times the pair's radius is at
    most 0.02 of 6,356,752 m.
  - Otherwise it uses the chord between the two positions on a sphere of 6,356,752 m. That sphere is
    no larger than the Gaussian radius the exact test uses anywhere, so the chord is never longer than
    the exact distance. Such pairs are counted (`pairs_chord`); they are not a shortfall.
  - Rows above 85 degrees are still evaluated, but are counted (`rows_high_lat`) and degrade the net
    (`InputMissing`), because the cell search is shown complete only up to 85 degrees.
  - A 95 % radius that is unusable or above `r95_cap_m` is held to the cap and counted (`r95_clamped`),
    and degrades the net, as before; a radius exactly at the cap does not.
  - The development configuration's cap is 100 km. On the national hour of 19 November 2026 it bound
    9 rows in 3,601 cycles, against 26,872 at 20 km.
- **OA-CA-034** What is proved and what is tested for OA-CA-033:
  - Nothing in it is proved. Tested by deterministic random pairs in `grid::tests`:
    - the squared chord of two vectors with components in [-1, 1] is finite and not negative;
    - the chord is never longer than the exact distance the evaluation uses, on 200,000 pairs up to
      85 degrees and 400 km apart, the smallest ratio being 1.000000068;
    - where the flat test is valid it accepts every pair the exact projection accepts;
    - the chord is symmetric and zero for one point;
    - the flat test's validity only narrows as a tangent or the radius grows. A proof of this did not
      finish in ten minutes.
  - Tested in `tests/ca.rs` through the full grid and cycle against an exact oracle:
    - a pair at its reach at the tangent point, up to 85 degrees, both hemispheres, three band sizes;
    - pairs across ±180 degrees at 60, 75 and 84.9 degrees, found at 0.99 of their reach and not at
      1.05.
  - The cell-search reach is shown numerically and by these tests, not by an analytic proof.
- **OA-CA-035** The cycle that uses the facility inhibition/control table records area, pair and
  group suppression receipts for actual conflicts. A suppressed conflict remains in detector state
  under its original alert start with a current presentation-suppressed flag; when the finite control
  ends, that same instance is presented again and the flag clears. The report separates detected,
  suppressed/acknowledged and presented pair cycles; lost receipts and refused lookups are counted
  and degrade the net.
- **OA-CA-036** When an adaptation package and independent manifest pin are supplied to `oa-ca`, the shell
  verifies the package and requires a v3 CA preset component, checks its facility identity with the shared
  core parser, applies its typed volumes and policy before `ca_check`, and records the loaded manifest digest
  and adaptation version in run output. Missing, corrupt, wrong-facility or unsupported preset components
  refuse the run before detector processing.

## SWIM STDDS terminal data (oa-stdds)

- **OA-STD-001** Classifies a message by its root element and namespace into a product kind: TAIS
  `TATrackAndFlightPlan`, SMES/ASDE-X `asdexMsg` by its first report child (Cat11 `positionReport`,
  Cat10 `adsbReport`, Cat10 `mlatReport`), the other STDDS terminal-entities namespaces (APDS, ISMC,
  RVR, TDES, any other), and the FIXM-mediated `MessageCollection` by `metadata@messageType`. Element
  names and structure follow the FAA's public FIXM-Mediated STDDS Data Overview v2 (mapping tables,
  sections 2.3.1 to 2.3.4) and the STDDS R6P2 release notes (sections 4.2 to 4.5); no real STDDS message
  was available, so every unit and name marked ASSUMED in the crate docs awaits confirmation on real data.
- **OA-STD-002** A message of no known kind is counted by kind and its root name and namespace are kept
  in a bounded table; it is never an error.
- **OA-STD-003** A TAIS message parses into typed records with `src` and, per `record`, every track field
  (`trackNum`, `mrtTime`, `lat`, `lon`, `reportedAltitude`, `vx`, `vy`, `vVert`, `reportedBeaconCode`,
  `acAddress`, `status`) and flight-plan field (`acid`, `acType`, both beacon codes, both altitudes,
  `airport`, `runway`, both flight-rule fields, `ECID`, `sfpn`, `ptdTime`, entry and exit fix, `type`,
  `status`) in fixed integer units named in the field (1e-7 degree, Unix ms, feet, cm/s, feet per minute);
  no floating point is used.
- **OA-STD-004** A field absent from a record clears only its own presence bit and is no error; a
  latitude or longitude left without its partner is refused as an incomplete pair.
- **OA-STD-005** A field that is present but malformed (syntax, range, empty, too long) is a counted
  error naming the field and the byte offset of its element; the record is kept, the field is absent and
  its bit is set in the record's error word, the other fields are intact, and nothing is defaulted.
- **OA-STD-006** A field given twice in one record is a duplicate error at the second element and keeps
  neither value.
- **OA-STD-007** A latitude without a longitude, or the reverse (a delta-encoded message), is refused as
  an incomplete position pair in both TAIS and SMES; the other fields of the record are kept.
- **OA-STD-008** Values are checked exactly: beacon codes are four octal digits, Mode S addresses 1 to 6
  hex digits, decimals take no sign, exponent or spaces and round half away from zero, and each range
  (latitude +-90, longitude +-180, altitude -2,000 to 100,000 ft, velocity +-3,000 m/s, vertical rate
  +-30,000 ft/min) accepts its edge and refuses one past it.
- **OA-STD-009** Timestamps are strict ISO-8601 UTC (`Z` or `+00:00` only, real calendar dates, no leap
  second, years 1970 to 9999) read to Unix milliseconds; anything else is refused.
- **OA-STD-010** A field the sender marks `r="1"` is absent and counted as invalidated, not an error.
- **OA-STD-011** The records per message are limited (`Limits::max_records`); a message over it is refused
  whole with the offset of the first record over, and nothing from it is kept. Field errors past
  `Limits::max_errors` are counted as dropped, while each record's error word keeps every bit.
- **OA-STD-012** Input the XML reader refuses (truncated, DOCTYPE, bad bytes) is refused whole with the
  reader's byte offset and counted; every truncation and every single-byte deletion of every fixture
  returns a result without panicking.
- **OA-STD-013** Elements outside the known structure are counted per path in a bounded table (overflow
  counted too), only the topmost element of an unknown subtree is counted, and they are never errors;
  known elements that are deliberately not read are not counted as unknown.
- **OA-STD-014** An ASDE-X Cat11 `positionReport` parses into a typed report with `airport`, time,
  position, altitude, heading, speed, velocity, vertical rate, sequence and track numbers, Mode S address,
  Mode 3/A code, callsign, aircraft type, runway and target type; the Cat10 `mlatReport` and `adsbReport`,
  the FIXM-mediated forms and the other STDDS products are classified only.
- **OA-STD-015** A message of a classified-only kind returns its kind and is counted, with no record.
- **OA-STD-016** The statistics count messages per kind, refusals per reason, field errors per kind,
  records, records with errors, empty records, and per-field presence and refusal counts for TAIS and
  SMES; they hold counts only, never a message value.
- **OA-STD-017** A typed parser called on a message of another kind returns `WrongKind` with that kind and
  counts it, with no byte offset.
- **OA-STD-018** Parsing is deterministic: the same bytes give the same records whatever was parsed in
  between, and a parser and a statistics accumulator are reusable.
- **OA-STD-019** A `record` with no known field is kept with no bits set and counted as empty.
- **OA-STD-020** Element text is trimmed and its character references and CDATA are decoded before it is
  parsed.
- **OA-STD-021** The calendar helper refuses years outside 1970 to 9999, months outside 1 to 12 and days
  that do not exist (the table shared with the other crates' calendar helpers).
- **OA-STD-022** The namespace version (`vN-M`) is never compared, so a version change does not break
  classification, but the shape of the namespace (`...:terminal:entities:vN-M:service:name`) is.
- **OA-STD-023** Message frames of a recorded feed parse to the same counts as the same bytes parsed
  directly; the summary example reads such a feed (raw or gzip) and prints counts and schema names only,
  never a position, identity or time.
- **OA-STD-024** The summary example exits 2 before opening the file when the path, as given or with links
  resolved, is sealed by the shared guard (OA-SEAL-001, OA-SEAL-002), including through a link to the
  file, to the folder or to a link.
- **OA-STD-025** Forms the real feed uses are accepted: a decimal with up to 24 fraction digits (a heading
  printed as a binary double, still range checked), a callsign with an inner space, a runway with `*` or `+`,
  a flight plan status word up to 16 characters (`terminated`), and the sender's no-value words
  (`unavailable` for the vertical rate, `unassigned` for an entry or exit fix) as an absent field counted
  as present-empty, not an error. Encoding-damaged text (U+FFFD) and any other word stay refused.
- **OA-STD-026** An element with no text is a counted `present_empty` field, not a field error, for
  `flightPlan/rawFlightRules` (the real feed prints it empty as its normal absent form); every other field
  keeps an empty element as a refused field. A record with only present-empty fields is not an empty record.
- **OA-STD-027** The TAIS `track/pseudo`, `track/frozen`, `track/new` and `track/adsb` decode as booleans
  (`true`, `false`, `1`, `0`); the stats keep a `[false, true]` count per flag, which the summary prints (the
  flags are not identifying). `track/adsb` is the candidate radar-only marker.
- **OA-STD-043** The TAIS `track/coast` decodes as a boolean like the other track flags (`true`, `false`,
  `1`, `0`; an element is no longer skipped): absent stays absent, a present but unreadable value is a counted
  field error and the field is absent, and the stats keep its presence count and a `[false, true]` count
  (fifth flag), which the summary prints.
- **OA-STD-028** The TAIS record header decodes: `recType`, `recSrc`, `recSTARSSrcID` as codes,
  `recSTARSTimestamp` as milliseconds since midnight (ASSUMED from the observed digits; range 0 to 24 h) and
  `recSAFAReceiptTime` as ISO-8601. The stats count the latency (receipt time of day minus STARS time of
  day, wrapped into a day and folded into -12 h to 12 h, so a skew is negative and a midnight crossing small) in ms buckets, the records lacking a time, and the distinct `recSTARSSrcID` values,
  never printing a time or an identifier.
- **OA-STD-029** The five service-status roots in the `...:vN-M:status` namespace (`AirportDataServiceStatus`,
  `TerminalAutomationInformationServiceStatus`, `TowerDepartureEventServiceStatus`,
  `SurfaceMovementEventServiceStatus`, `STDDSStatus`) classify as kind `stdds-status`; the service name and
  state are decoded (the root's `serviceStatus` text, or the top-level `p@k` ending in `Status` or `State` with
  its `v` for `STDDSStatus`) and counted as a value histogram. Any other root of that namespace is unknown.
- **OA-STD-030** An `AssetMessage` in the `https://mmixm.aero/base/` namespace classifies as kind `mmixm-asset`
  and is counted, not decoded.
- **OA-STD-031** ASDE-X `status/*` sub-elements other than `vertRate` stay unknown and are counted.
- **OA-STD-032** A refused field is diagnosed by its text's shape (length and the first 12 character classes
  with runs collapsed), kept per field and error kind in the stats and printed by the summary; no value is
  kept or printed.
- **OA-STD-033** A message is counted once: `messages == sum(kinds) + errors[xml]` after every refusal path;
  a refusal after classification adds to `errors` only; a direct `parse_tais`, `parse_smes` or
  `parse_status` call counts its records and refusals but never a message. The records read before a
  too-many-records or too-deep refusal are counted.
- **OA-STD-034** The message-level `src` (TAIS) and `airport` (SMES) follow the field rules: `r="1"` marks
  them absent and counted, a duplicate keeps neither and is a counted field error, a bad value is a counted
  field error.
- **OA-STD-035** The latency (receipt time of day minus STARS time of day) is folded into -12 h to 12 h:
  a true midnight crossing is a small positive latency and a skew of seconds counts in `<0`.
- **OA-STD-036** Integer fields (TAIS altitudes, `vVert`, `recSTARSTimestamp`, SMES `vertRate`) take whole
  numbers only: no fraction, no `+`, no `-0`, no leading zero beyond a single `0`; the counters (`trackNum`,
  `sfpn`, `seqNum`, `track`) take no leading zero. More than 12 digits is out of range. The SMES altitude is
  a decimal in the real feed and keeps rounding to feet.
- **OA-STD-037** A bound is compared with the exact decimal before rounding (so 90.00000004 degrees is out
  of range though it rounds to 90); a heading is at least 0 and below 360 (a value that rounds up to
  360.00 is stored as 0).
- **OA-STD-038** A fraction finer than a millisecond is rounded half up on its fourth digit with a carry
  into the second, minute and day (not truncated).
- **OA-STD-039** A status key or value longer than 64 bytes is counted (`status_key_too_long`,
  `status_value_too_long`), an empty one or one past the per-message limit in `status_dropped`, and a state
  past the table of distinct values in `status_table_overflow`; none is silent.
- **OA-STD-040** Only elements with no namespace or in the root's namespace are fields (the real feed's
  children are unqualified); an element in another namespace is unknown, counted, and nothing below it is
  read.
- **OA-STD-041** An `r` attribute other than `"1"` or `"0"` is a counted field error; an unknown element
  path is stored cut to 120 bytes.
- **OA-STD-042** The summary example prints a status text only if it is `[A-Za-z0-9 _-]` (up to 64 bytes) and
  an unknown element or root name only if it is those characters or `:./{}`; anything else is counted as
  "other". It exits 3 with a count of the bytes dropped if the frame reader stalls, reads until two bytes to
  sniff gzip, and refuses (exit 2) a path with a sealed name inside any component, as given or resolved, and
  any path it cannot resolve.

## M4 flight data: the per-flight record (oa-flight, oa-flight-sfdps)

- **OA-FLT-001** A flight is keyed by its GUFI, or by the TFMS key (callsign, departure, arrival) where a source has no GUFI; a
  key with a missing part (a zero GUFI, an empty callsign, departure or arrival, or a GUFI key carrying TFMS parts) is refused
  as a bad key; one key is one flight and a GUFI flight is never a TFMS flight.
- **OA-FLT-002** The assigned, interim and requested altitudes are separate fields. Each holds a form: one altitude, VFR on top
  plus an altitude, VFR with no number, a block (assigned only), an altitude at a named fix (assigned only), or explicitly none
  (interim only). A form the field cannot hold, an out-of-range value, an unused number that is not zero, an altitude operation
  with no altitude, or an at-fix altitude without its fix is refused as a bad value and changes nothing.
- **OA-FLT-003** Every field records when it last changed (the source's time), when it became available and the message it came
  from; stating the value the record already holds changes nothing, not even the stamp.
- **OA-FLT-004** An operation is available no earlier than its stamped time plus the measured lag floor and no earlier than any
  operation before it (the clock never goes back); one stamped later than its receipt by more than the allowed clock
  difference is refused as a bad time.
- **OA-FLT-005** The plan lifecycle (proposed, active, departed, completed, cancelled, dropped) allows exactly the stated steps:
  proposed goes to active, cancelled or dropped; active departs, completes, is cancelled or dropped; departed completes or is
  dropped; dropped is reinstated by an active plan or completed or cancelled; completed and cancelled are final and a repeat of
  the same end is a restatement. A step back, a step that skips one, or an operation on a flight or plan that is not there
  (an operation other than setting a plan, which makes the flight) is refused and leaves the state; so is an amendment to a
  flight whose plans have all ended. A refused operation never makes a flight. The one way past an ended plan is the refile
  of OA-FLT-008.
- **OA-FLT-006** Every refused operation is counted by operation and reason and listed with its key, times, message and the
  state it found; a full list counts the entries it could not hold, so the counts stay exact whatever the list holds, and
  operations equal applied plus unchanged plus refused.
- **OA-FLT-007** A flight holds one plan view per reporting centre, because each centre reports its own status for the national
  plan; a second centre's proposal after the first one's activation is not a step back; a dropped view reinstated is counted;
  more centres than views are refused, never evicted.
- **OA-FLT-008** Two plans for one flight are listed as conflicts: a different plan identifier on a live plan, and the key filed
  again, which is a plan setting a plan identifier new to the flight on a view whose plan had ended (the view takes the plan) or
  for a centre the flight has no view for while all its plans have ended (a new view). This is the only way past an ended plan.
  A plan for a new centre on a flight whose plans have all ended with a plan identifier the flight already has, or none, is
  refused as terminal.
- **OA-FLT-009** An aircraft address held by two live flights, and a callsign, departure and arrival held by two live flights,
  are each listed as a conflict naming both; both flights stay in the table; the address lookup returns the latest claimant and
  a flight whose plans ended is no conflict.
- **OA-FLT-010** A handoff is offered, optionally updated, accepted and completed when the owner becomes the accepting unit, or
  is retracted, fails or ends in the accepting unit taking control; an offer does not move the owner; a step with no handoff
  pending, a second offer while one is pending, and an update or a different acceptance after acceptance are refused.
- **OA-FLT-011** A change older than the value a field holds is refused as stale and one at the same time with another value is
  refused (the earlier stays); an older restatement of the held value is no error; the rule holds for altitudes, the plan state
  and ownership.
- **OA-FLT-012** A flight leaves the table only by a removal that is written to the removal log first; a full table refuses a new
  flight and evicts none; the sweep retires flights whose plans ended or that went silent, logged each, and removes none when
  the log has no room.
- **OA-FLT-013** After every step of every sequence of up to four operations over a fixed alphabet covering every operation kind,
  and of random sequences of sixty operations, the audit of the table holds: keys, indexes and chains, views, altitude forms,
  handoff units, stamps against the clock and the logs.
- **OA-FLT-014** Over random sequences, a flight has at most one plan view per centre, flights created equals flights held plus
  flights removed, and every removal is in the removal log.
- **OA-FLT-015** The same operations in the same order give the same table, logs included, and the same digest after every step.
- **OA-FLT-016** The table allocates nothing after it is made: the capacity of every collection is unchanged by twenty thousand
  operations with sweeps.
- **OA-FLT-017** The SFDPS adapter turns one decoded message into the operations it carries and no others: the plan from the
  status, a departure from an actual departure time on an active plan, the assigned, interim and requested altitudes with
  their forms (an interim stated as none is the state "none", not a number; VFR is never a number), the controlling unit,
  the aircraft address and the identity; each carries the message's own time and the time it was received.
- **OA-FLT-018** A controlling unit the FAA gives as unknown makes no owner operation, a handoff `receivingUnit` with no event
  makes no handoff operation, and a handoff event missing a unit it needs makes none; each is counted. The handoff events
  make their own operations: offer, update, acceptance, retraction, failure and taking control.
- **OA-FLT-019** A replay reads SFDPS recordings in receive order into the record, counts the lag of every message (received
  minus stamped), skips frames before the window and ends at the first frame after it, and announces each operation's
  availability time to the caller before applying it, so a question asked as of a time sees only what had aged in.
  The report retains reader errors and sealed-frame counts encountered before a bounded window ends.
- **OA-FLT-020** The climbing and descending encounters near 1,000 ft (airborne, not at the vertical minimum, 900 ft up to
  the record volume apart at the closest approach, either aircraft at 300 fpm or more) are each split once, by the
  clearances of the two aircraft in the flight record at the closest approach: cleared 1,000 ft or more apart is a
  level-off, less apart is a climb or descent that does not stop; no record, no altitude and an altitude that is not one
  level (VFR, block, at a fix) are counted apart; the split is given on the assigned altitude and on the interim altitude
  over the assigned one.
- **OA-FLT-022** The operations of one message share its time and its message number and are not a tie on a shared stamp: a plan
  made active and then departed in one message, and an owner and the handoff it ends, all apply; another message at the same
  time with another value is still a tie. An owner older than the handoff it would end leaves the handoff and its stamp as they
  are and is counted. A question is not advanced past an operation refused for its key or its time. Taking control, once a unit
  has accepted, is by that unit.
- **OA-FLT-023** The SFDPS replay tool refuses a recording directory the shared sealed-data guard names, before it lists or
  opens anything, and exits 2; every recording it reads is opened through the same guard.
- **OA-FLT-024** A question about a flight is answered as of its time: an operation received before it but not yet aged in by the
  lag floor is not seen by it, and one that has aged in is.
- **OA-FLT-025** The callsign join is tried where the address names no flight, and may override the address flight under OA-FLT-036; it names a flight only when the ADS-B callsign is stable,
  exactly one flight that is flying (a plan proposed, active or departed; not ended, not dropped) states it, and the flight's own
  SFDPS position within 120 s of the question, where it has one, is within 10 NM of the ADS-B position; with no such position the
  join stands and is counted apart as unchecked. Each rejection (no callsign, unstable callsign, no live flight, several live
  flights, position disagrees) is counted.
- **OA-FLT-026** The callsign join is read as of the closest approach exactly as the address join is: a flight, an identity or a
  position that ages in after the question is not used by it.
- **OA-FLT-027** Where the address and the callsign both name a flight the two are kept and compared, and the counts of the same
  flight and of different flights are reported, as the measure of the callsign join's error.
- **OA-FLT-028** The ADS-B callsign of an aircraft is stable when the reports of the previous 60 s carry one callsign (upper
  case, trimmed); none is no callsign and more than one is unstable; the position is the latest report not after the question
  and not older than 60 s, and with none in that span it is the midpoint of the two aircraft of the pair's closest approach.
- **OA-FLT-029** The adapter makes a callsign upper case and trimmed, and the replay keeps the latest position of each flight,
  bounded, as of the time it ages in.
- **OA-FLT-030** Where several flying flights state one callsign, all with the same departure and arrival, the callsign join may take
  the one with the newest plan stamp (the later stamped time of any plan view, then the later message, then the larger GUFI), the
  same one whatever the order the flights arrived in, and counts it as one aircraft with several plans; with another route among
  them, or a route not stated, they stay rejected as several live flights.
- **OA-FLT-031** The replay remembers the time of the latest refile conflict of each flight, bounded.
- **OA-FLT-032** A disagreement between the address and the callsign is classed (same callsign and route, same callsign and another
  route, different callsigns, other), the distances between the flights' positions and from the ADS-B position are kept, and
  the two flights are looked at again 30 min later for whether either stopped flying; the second look joins nothing.
- **OA-FLT-033** The core lists every flight that claims an aircraft address, newest first, by walking the chain of the address
  index, bounded by the table, and counts them all.
- **OA-FLT-034** The address join walks every claimant of the address, keeps those flying at the query time under the same as-of,
  no-peek rule, takes the one that is flying if only one is, and with none flying finds nothing (counted as address claimants,
  none flying) and the callsign join is tried next.
- **OA-FLT-035** Among the claimants left after OA-FLT-037 and OA-FLT-038, the address join takes the one whose own position within
  120 s is within 10 NM of the ADS-B position; if none or more than one passes, it gives several flying claimants and the callsign
  join is tried next.
- **OA-FLT-036** Where the address join and the callsign join name different flights, the one whose position check passes is taken;
  if both or neither pass the address flight stands; each case is counted and kept in the cross-check.
- **OA-FLT-037** With several claimants flying, the claimants whose callsign equals the ADS-B callsign are kept first; if one remains
  it is taken and counted as several flying, callsign decides; a step that would leave none narrows nothing.
- **OA-FLT-038** Then claimants with a view active or departed are kept over those only proposed; if one remains it is taken and
  counted as several flying, state decides; then the position check; otherwise the claimants stay unresolved.
- **OA-FLT-039** A dropped flight is not flying; an address whose only claimant is dropped at the query time is counted on its own
  line. The walk counts every claimant of the chain (time linear in the chain, at most the table) and copies only the flying ones,
  at most 64 (FLYING_CAP, 64 flights of 768 B, 48 KiB per question); an address with more than 64 flying is counted on its own
  line and left unresolved, so the callsign join is tried.
- **OA-FLT-040** The replay drops the position and the refile time of a flight when the table removes the flight; positions held,
  positions refused (table full), refile times held and refiles refused are printed, also at zero, and so are positions left out for
  a parent-message stamp too far ahead of receipt or a position stamp later than its availability time; the two timestamp
  rejection counts are separate. A rejected position does not replace the last usable position.
- **OA-FLT-041** A flight's own position counts only when stamped at or before the question and no more than 120 s before it.
- **OA-FLT-043** H2 optional SFDPS replay observation retains source and receive time separately, identifies rows by stream/decoded-offset/row rather than recorder-sequence equality, and emits normalized operations, exact engine outcomes and resulting owner/handoff/altitude state. It is observed-state compatibility evidence, never independent controller prediction agreement.
- **OA-FLT-044** H2 replay observations have explicit frame, row, event and output-byte budgets, a 64 GiB aggregate reservation of full opened input-file lengths (compressed on-disk bytes for `.oaf.gz`, plain bytes for `.oaf`), a 64 GiB decoded-byte budget and a 20,000,000 reader-step budget. The reservation is calculated from the actual guarded descriptors before any input bytes are read; per-input `reserved_bytes` is distinct from `bytes` actually read, which may be a prefix at the selected window end. A backwards window is rejected before output creation and by the shared audit replay entry point. A cap or sink failure stops with incomplete evidence and retained counters; a completed calculation means the selected compatibility window completed, not that every source file reached EOF, and does not imply qualified input or controller behavior. Audit state is single-use and starts empty.
- **OA-FLT-042** Clearing an assigned altitude is represented as the explicit nil form with zero bounds and its own source stamp; it is not converted to VFR or an unstated altitude.
- **OA-FLT-021** The encounter tool takes a window of reports, the SFDPS recordings and the lag floor (58 s unless given), and
  refuses a window that is open at one end or runs backwards.

  sniff gzip, and exits 2 for a sealed path (the shared guard) and for a path it cannot open.

## STDDS adsb-flag census (tools/oa-stdds-census)

- **OA-CEN-001** The update interval of a TAIS track is the difference of consecutive `mrtTime` values of
  one track segment (a run of one key, facility, `recSTARSSrcID` and `trackNum`, that no restart has cut; see OA-CEN-019;
  a coasting, pseudo or frozen update takes part in the time state, so a coast keeps a segment alive across a gap, since
  STARS kept the track), attributed to the `adsb` flag of the update that ends
  it, kept in 10 ms bins to 120 s; the census prints per flag the count, median, 10th and 90th percentile
  and a histogram in 0.5 s bins to 15 s with the count at 15 s and over.
- **OA-CEN-002** Flag changes within a track segment are counted (total, segments with one, segments by number of
  changes), and for each direction the interval across the change, the one before it and the one after it
  are distributions of their own; intervals across a change stay out of the per-flag distributions.
- **OA-CEN-003** For the updates of each flag the census counts the share carrying a Mode S address, a
  Mode 3/A code, code 1200, a callsign, any flight plan field, address with code, and none of address,
  code and callsign, plus the share with the new flag (the identity shares leave out pseudo, frozen and coasting updates,
  OA-CEN-021); and the track segments whose adsb=false
  updates ever carried an address.
- **OA-CEN-004** Every count and distribution is also kept per source facility (the message `src`); a
  track number at two facilities, or under two `recSTARSSrcID`, is two tracks. A record with neither `src` nor
  `recSTARSSrcID` is in a facility printed "facility unknown", not a merged all-zero source id; with no `src`, the
  record's `recSTARSSrcID` names the facility.
- **OA-CEN-005** An update with no `adsb` element is counted apart (never as false); updates with no
  `mrtTime`, with a time before the track's previous one, or with a gap over 120 s (a reset) are counted
  and make no interval; a record with no `trackNum` is counted and not tracked; a frame cut short or a
  gzip file that ends inside a member (a recording still being written) is counted, never fatal. A damaged gzip file
  is counted and the run exits 3 after printing the report, as stalled bytes do.
- **OA-CEN-006** WITHDRAWN: the address-based cross-check (replaced by OA-CEN-009 to OA-CEN-012; the gateless check by address of
  adsb=true updates in OA-CEN-010 is a different, later check, not this one: the
  address field reads zero on almost all adsb=false updates).
- **OA-CEN-007** The output holds counts, shares and distributions only: no position, address, callsign,
  code, flight-plan content or individual time; the only text from the data is a source id of letters and
  digits, and the only times are the start (rounded down) and end (rounded up) to the second of the STDDS
  recording and of the ADS-B clip, which are the span of each recording and not the time of any aircraft. Every path (the directory, the ADS-B clip, each subdirectory before it is listed, each
  file before and after it is opened) goes through the shared guard (OA-SEAL-001), as given and with
  links resolved, and a sealed one is refused, naming the path (exit 2). A capture day directory is
  read from its `rec/stdds` folder and only files named `scds-stdds-*`; a day or `rec` directory with no
  `rec/stdds` is refused (exit 2).
- **OA-CEN-008** Track segments (not track numbers: STARS reuses them, so one key holds many aircraft) are classed by
  the flags seen on them (fewer than two flagged updates, all adsb=false, all adsb=true, mixed), with their update
  counts and per-flag interval distributions. Class, flag counts, changes, change bins, segments with adsb=false,
  false address counts and the cross-check by class are all taken per segment; the report says "track segments".
  A segment with no flagged update (only status updates, or only updates with no flag) is counted as "segments with no
  flagged update" and is in no class; the class "one flagged update" holds segments with exactly one.
- **OA-CEN-009** Position cross-check (with an ADS-B clip, read by the loader oa-encounter and oa-bench use):
  each tracked update with a flag, position and `mrtTime` is compared with every ADS-B aircraft interpolated
  linearly to `mrtTime` between the two reports around it (at most 5 s apart by default, 30 s at most; `--max-gap-s`; a
  segment faster than 600 m/s is left out and counted; ground reports are left out). A candidate is one aircraft inside the horizontal gate and, when both altitudes exist, the
  vertical gate (both inclusive; defaults 0.5, 1 and 2 NM with 500 ft, 1 NM primary). Per flag and gate the
  update is counted no candidate, exactly one, or ambiguous; only exactly-one is used. A horizontal-only
  count is kept at the primary gate. Updates with no position or time, with no flag, or on a pseudo, frozen or
  coasting track update are counted apart (a coasting position is dead-reckoned, not measured), as are updates whose
  `mrtTime` is outside the ADS-B clip's time span (OA-CEN-023). A
  gate above 10 NM, a gap above 30 s, or an unusable parameter is refused.
- **OA-CEN-010** For exactly-one matches at the primary gate, per flag, the
  horizontal difference, the vertical difference, and the along-track and cross-track differences (STDDS
  minus ADS-B, along the ADS-B track, positive ahead and to the right; aircraft at 100 kt or more) are summarised by count, median,
  10th, 90th and 99th percentile and a 20-bin histogram; the vertical differences of horizontal-only
  matches are summarised over a wide range to expose an altitude unit error. adsb=true is labelled
  ADS-B-fused and not independent of the truth (STARS fuses ADS-B into those updates, so its difference
  measures STARS latency and filtering, not accuracy). The adsb=false results are headlined by track class,
  and an adsb=false update on a mixed track is not ADS-B-free. Each matched cell prints the placebo-to-real
  match ratio, worded as a rough indication of unknown direction and not as an estimate of the chance share (OA-CEN-017). The ADS-B bracket width of the matches is counted
  (up to 2 s, 2 to 5, 5 to 10, over 10). A bias injected into synthetic STDDS updates shows in the
  matched distribution of adsb=false and not of adsb=true. adsb=true updates with a Mode S address are
  also checked by address, with no gate, and reported apart.
- **OA-CEN-011** The along-track difference is also given as time (along-track metres over the ADS-B ground
  speed, aircraft at 100 kt or more); the ADS-B time is receipt time, so this is the STARS latency minus the
  ADS-B latency; a known clock offset in synthetic data is recovered, positive when the
  STDDS position is where the aircraft is later than the stated time.
- **OA-CEN-012** The cross-check counts and difference medians are split by track class and by facility, and
  absent without a clip.
- **OA-CEN-013** The ADS-B reference is built with the gating of the tracker and the truth
  (`oa_truth::quality_check`, default config: source type adsb_icao, adsb_icao_nt or adsb_other, NACp, NIC,
  SIL, version, stale and range gates), in time order with per-aircraft carried state, not a parallel
  filter; reports on the ground are then dropped. The census prints the clip's reports by source type before
  any gating, what each gate removed, and what the time window left out. A TIS-B report on top of a
  radar-only update is not a candidate. `--from` and `--to` restrict both sides to a time of the UTC day
  (the ADS-B side padded by the interpolation gap and the placebo shift of 60 s, across midnight too); a window
  outside 0 to 86400 s, or a non-finite bound, is refused. The STDDS and ADS-B time spans are printed, and
  a run whose spans do not overlap (a wrong day or clip) exits 2.
- **OA-CEN-014** The candidate search is exact at any latitude to 85 degrees and across the antimeridian:
  each segment is stored in every grid cell its bounding box touches after widening by the largest gate
  (in longitude at the box's highest latitude), longitude cells wrap, interpolation and the east difference
  take the short way round. Distances are in the tangent plane at the interpolated ADS-B point. Segments and updates above 85 degrees, and segments faster than 600 m/s, are
  counted and not used.
- **OA-CEN-015** Coverage is counted apart from no candidate: per flag, the share of checked updates with
  any indexed ADS-B aircraft within 30 NM at their time.
- **OA-CEN-016** The cross-check counts reconcile exactly with the update counts per flag: updates equal
  checked plus no position or time, above 85 degrees, no trackNum, pseudo, frozen or coasting update, outside the
  ADS-B clip's time span, and updates with the same time and position as the track's previous update (updates with
  the same key, time, position and flags are dropped earlier, OA-CEN-022); updates with no flag equal the not-checked no-flag count.
  The report also reconciles, with or without a clip, the input TAIS records read against the updates, the duplicates
  dropped, the updates outside the window and the updates with no time outside the window; with a clip, per flag and
  shift of the placebo, matched plus not matched plus outside the clip against checked. Each line says "reconciles" or
  "DOES NOT RECONCILE".
- **OA-CEN-017** A null control is run beside the real match: the same positions with the STDDS time
  shifted by plus and minus 60 s, printing per flag the outcome counts and the horizontal difference
  distribution of its exactly-one matches; the placebo exactly-one count (mean of the two shifts) over the
  real one is printed as a rough indication of unknown direction, not as an estimate of the chance share: the
  placebo shares route structure with the real match (aircraft in trail about 60 s apart, slow or circling aircraft
  matching themselves) and is not suppressed by the single-candidate rule the way a real chance match is. A placebo
  lookup at a window's edge finds its shifted ADS-B; a lookup whose shifted time lies outside the clip's time span (no
  window, or near the clip's edges) is counted as "placebo outside the clip", never as a non-match.
- **OA-CEN-018** Per flag the census prints the matches without a vertical difference, the distinct matched
  ADS-B aircraft and the distinct STDDS track segments with a match, and the distribution over track
  segments of each segment's median difference (segments with 3 or more matches). A segment is a run of
  one track number that a restart of the interval state (a gap over 120 s, an update flagged new, a resync)
  has not cut, so a recycled track number is not one aircraft; along and cross metres, like the time,
  leave out aircraft below 100 kt; a value is kept inside its histogram range after rounding to f32; the along-track-as-time figure leaves out aircraft
  below 100 kt; a class or facility cell with fewer than 30 matches prints "n<30" for a flag instead of
  medians; `--gate-nm` and `--gate-ft` without `--adsb` are refused; the histogram's upper edge is
  inclusive.
- **OA-CEN-019** Track state is kept in step: a change of flag is counted only where an interval across it
  exists (so changes equal the changes by direction); a backwards update changes neither the time nor the
  flag of the track, an absent-flag update obeys the same backwards and reset rules, 3 backwards updates in
  a row restart the track (counted), and an update flagged new on a track number with state starts a new
  segment (counted).
- **OA-CEN-020** Three checks against 4 GB. Each is an estimate under stated assumptions, not a guaranteed
  bound. (1) Before the ADS-B index is built: 16 bytes for every clip report, and for each report the padded
  window keeps, its point and an assumed 8 cell entries and 2 new grid cells (72 bytes each), all with 2x
  growth slack; a run past it is refused (exit 2) before the index is allocated. The cell assumption fails for
  a clip of faster or sparser aircraft or a larger gate. (2) After it: the index's own estimate. (3) While
  reading: what the census holds (interval lists, cross-check records, segments, with 2x Vec slack) and the
  copies that `finish` makes of each match, address match, placebo value and segment (three accumulators:
  all, class, facility), the tracks map, the segments and the set of seen update fingerprints. The quality-state map
  of the index build is counted, per distinct aircraft, in checks (1) and (2). Not counted: the loaded clip columns, the
  read buffers and the output. The report and the refusal message call the figure an estimate, not a bound. Updates left out by the time window are counted. A test sets the budget through
  `OA_STDDS_CENSUS_BUDGET_BYTES`, which exists for tests only.
- **OA-CEN-021** Updates flagged pseudo, frozen or coasting (each read only when present and true) are never counted
  in the flag tallies, the track-segment class, the change counts or the interval census (the interval that starts at
  one is not counted either); they remain in the update totals and have their own counts per kind and flag, so the
  report shows how many coasts carried adsb=false and how many adsb=true. An all adsb=true segment with coasting
  adsb=false updates stays "all adsb=true". The identity shares (address, callsign, code 1200), the adsb=false share of
  flagged updates and the per-facility false share also leave out these updates, so they agree with the class lines;
  the update totals still count them.
- **OA-CEN-022** An update with the same facility, `recSTARSSrcID`, `trackNum`, `mrtTime`, position and flags as one
  already seen, across files or within a file, is dropped before any count and counted as "duplicate updates dropped";
  updates without a time or a track number are not checked for duplicates; the
  consecutive-repeat rule of the position check (OA-CEN-016) stays. With `--from` and `--to`, an update with no
  `mrtTime` is outside every window: it is excluded and counted as "no time, outside the window". `--from` and `--to`
  take whole seconds; a fractional value, or one whose milliseconds do not fit an i64, is a usage error (exit 2).
- **OA-CEN-023** An update whose `mrtTime` lies outside the ADS-B clip's time span is counted, per flag, as "outside the
  ADS-B clip", never as no candidate or no ADS-B, and not checked; the run goes on. Exit 2 stays for no overlap at all.
  The spans are printed to the second.
- **OA-CEN-024** `--surface-only` scans an immutable STDDS input snapshot without accumulating the TAIS census. It
  counts SMES Cat11 `asdexMsg/positionReport` rows only when the parsed message airport is exactly `KORD`, and
  reports their parser-UTC Unix-minute counts. Minute storage is bounded by a named 1,440-key one-day capacity;
  reports whose key cannot be retained, unreadable times, unreadable airport codes, whole-message refusals,
  damaged/unfinished frames, sealed drops, gzip damage/truncation, reader stalls and budget stops have named
  counters. Parser-kind and SMES field-error counts are explicitly all-facility. The report reconciles message
  frames seen with parse attempts and receive-time sealed frames and gives an explicit COMPLETE/INCOMPLETE
  verdict; COMPLETE requires nonempty input with no omitted, unassignable, untimed or refused input. Only the
  SMES Cat11 output is airport- and time-attributable; TAIS is a separate STARS product and Cat10 ASDE-X reports
  remain classification-only. This output is counts-only, guarded by OA-SEAL-001, and makes no inference that
  product absence or a capture gap establishes operational outage start or resumption.


## Sealed hold-out guard (oa-sealed, tools/sealed_guard.py)
- **OA-SEAL-001** The sealed hold-out data is listed once, in `crates/oa-sealed/sealed_paths.txt` (the data root, the entries
  `scds/rec-20261119/day2`, `day3`, `day4`, `runs*`, `scds/stdds-20261119-holdout*`, `scds/stdds-smoke-20261119*`, the days of OA-SEAL-010, and the windows of OA-SEAL-011); the Rust guard compiles the file in and
  the Python guard reads it, and both parse it to the same set. A list that does not parse refuses every path.
- **OA-SEAL-002** A path is resolved before it is checked (relative paths, `..`, every link, a symlinked parent directory, a
  path not yet created through its nearest existing ancestor) and is refused, with a message that names the rule, if it is,
  is inside, resolves into or names (an entry's components in a row, as given or resolved) a sealed entry, if it is a
  directory that holds one, or if a link on it cannot be resolved. The shared guard owns the path rule only; keeping a
  sealed day's rows out of a file that also holds unsealed rows is a content rule that needs the tool's own parser
  (OA-SEAL-014). A file with more than one hard link is refused when it is opened, with a message that says why: a second
  name for sealed data under a neutral name passes the name check, and the real path of a descriptor is only one of its
  names (OA-SEAL-007). Consequences: `cp -al`, `rsync --link-dest` and hard-link dedup trees give files with a second name and are refused when opened, so such data is copied (`cp`, `rsync` without links) before it is read; a file whose metadata cannot be read is refused too (the check fails closed).
- **OA-SEAL-003** A tool that walks a directory checks every file it opens, through the guard's opener, not only its
  arguments.
- **OA-SEAL-004** Nothing switches the guard off: it reads no environment variable, flag or file, and an unknown option of
  the checker is a usage error.
- **OA-SEAL-005** Every tool that reads recordings refuses a sealed input or output path with a nonzero exit, a message that
  names the rule, and nothing written: oa-compass, oa-encounter, oa-shadow, oa-track-run, oa-track-fusion, p002,
  oa-replay-diff, validate_candidate.py, comparison_reference.py, comparison_evaluator.py, run_comparison.py,
  track_set_score.py, replay_compare.sh, p002_build.sh, reproduce_zau.sh and zau_manifest.sh.
- **OA-SEAL-006** Every bin and example target of the workspace, found from the manifests and the source layout, uses the
  sealed guard or is listed with a reason in `crates/oa-sealed/exempt_targets.txt`; a target with neither, a listed target
  that does not exist, and a listed target that uses the guard all fail the test. In the `src/` and `examples/` of every
  member crate (core crates included), a bare `File::open`, `File::open_buffered`, `File::options`, `OpenOptions`,
  `fs::read`, `fs::read_to_string`, `fs::read_dir`, `fs::metadata`, `fs::symlink_metadata`,
  `fs::copy`, `fs::hard_link` or `Command::new` fails the test unless
  `// seal-allow: <why>` with a reason is on the call's own line (trailing) or is the comment line before the next code
  line it covers. The generic spellings count: `<File>::open`, `<std::fs::File>::open` and a turbofish (`File::<T>::open`); an `include_bytes!` or `include_str!` whose argument is not a plain string literal (`env!`, `concat!`, a macro) is a bare read; and a `pub use` of anything in `std::fs` or `std::process` is one too (it re-exports the functions to the rest of the crate). A manifest that gains `libc`, `memmap`, `memmap2` or `walkdir` needs a `# seal-note: <why>` comment on its line or the line before, or a test fails. A name counts when it is used at all, so a function passed as a value (`.map(fs::read_to_string)`,
  `File::open` as a value) is a use, and so is the `.read_dir()` method. The scan reads each file with a lexer that tells
  plain, raw and byte strings, characters, lifetimes, line comments and nested block comments apart, with string state
  carried over lines; imports are read as use trees (`use std::fs::*`, grouped `use std::{fs::{self, File as F}, ..}`,
  `as` aliases of functions, types and the modules `std::fs` and `std::process`), and `type X = File;` (also `OpenOptions`
  and `Command`, through a path or generics) makes `X` a bare type. Under `src/`, only a module declared `#[cfg(test)] mod`
  is skipped (an inline one with its body, a file module `mod name;` with the file and every module it declares); no file
  is skipped by its name, and any other `#[cfg(test)]` item is scanned. An allow with no bare call on the line it covers is
  stale and fails, and an allow inside a skipped test module is not counted; the number of allows is pinned exactly in the
  test (`ALLOW_COUNT`, now 152), not a budget. `tests/`, `benches/` and `build.rs` of every crate are scanned the same way
  when the file names the real data root (a string that holds `OPENATCA_`, which covers `OPENATCA_RAW`, `OPENATCA_DATA`,
  `OPENATCA_SCDS`, `OPENATCA_ADSB_CLIP`, `OPENATCA_SNAPSHOT`, `OPENATCA_REC`, `OPENATCA_SENSORSIM_CLIP`, `OPENATCA_NASR`,
  `OPENATCA_CIFP` and any other, or `openatca-data`; a comment or an identifier such as a constant does not count), and a
  `#[cfg(test)]` module, inline or a file, that names it is scanned too instead of skipped: there a bare call needs the
  same allow (a temp dir, a fixture, a built binary), and a read of the data goes through `oa_sealed`; a file that does
  not name the root reads fixtures and temp dirs bare. Stated exceptions, with the reason: oa-arinc424 (a no-dependency
  crate, so it cannot use oa-sealed), oa-geo (a core crate, which does not depend on the shell crate oa-sealed) and
  oa-trace (walks the repository's own sources and registers, no data). The bare opens of the first two are in tests only
  (fixtures and the env-named `OPENATCA_CIFP` and `OPENATCA_GEODTEST` inputs); a test pins the count of bare calls per
  file, found by the same scan with nothing skipped, and fails if either crate gains one. `oa-mva`'s `pdftotext` runs on
  a path the guard has checked as given and as resolved. Not covered: a test module that is declared `#[cfg(test)] mod`
  in a source file is not scanned even when it names the data root, and a file module is matched to its declaration by
  the usual module paths without reading `#[path]`.
- **OA-SEAL-007** The guard opens a file and then checks the real path of the opened descriptor (`/proc/self/fd`), so a
  link swapped in after the path check is caught and the file closed; the loaders of `oa-bench` and `oa-replay` run the
  guard before they stat a listed entry. `Guard.open` retains its generic read behavior; bounded offline readers that
  require a regular file use the separate `Guard.open_regular_read`, which opens nonblocking, verifies the same
  descriptor, requires a regular file, then returns it without reading first. An opened regular file whose link count is
  above 1, or whose metadata cannot be read, is refused (OA-SEAL-002); tests make a hard link to a sealed file under a
  neutral name and refuse a FIFO without blocking.
- **OA-SEAL-008** The list file is printable ASCII and newline only, each line `word`, one space, `value`; both parsers
  refuse anything else, and the same inputs are refused by both.
- **OA-SEAL-009** Of the four-day recording `scds/rec-20261119`, the sealed list seals the name prefixes `day2*`, `day3*` and
  `day4*` (hold-out days A, B and C of protocol P-001, and any renamed or partial copy beside them such as `day2.tmp`) and
  `runs*` (the recorder's run log, which covers every day); `day1` (the tune day, usable freely) and `bin` are not sealed,
  so a path below them is accepted. The recorder (`bin/run-days.sh`) writes only `day1` to `day4` and `runs.log` at the
  top of the folder. The folder itself holds sealed entries and is refused as a holding directory (OA-SEAL-002), like
  `scds`. A path through `..` or a link into day2 to day4 is refused, and so is a copy that keeps the components
  `scds/rec-20261119/day2` (or day3, day4, runs); a copy renamed outside those components is not caught by any path rule. Tests: `only_days_two_to_four_and_the_run_log_of_the_recording_are_sealed` (Rust,
  `crates/oa-sealed/tests/guard.rs`; Python, `tools/test_sealed_guard.py`).
- **OA-SEAL-010** The list seals each of the days 2026-11-20 to 2026-11-22 (`day` lines) below the directories `raw`,
  `scds` and `derived` (`dayscope`), except below `raw/rap` and `raw/awc` (`dayexempt`, the weather inputs: inputs, not
  scored data). A component below a scope is normalised (every `.`, `-` and `_` deleted) and the compact day
  (`20261120`) is matched as a substring of it, so `2026.11.20`, `2026-11-20`, `2026_11_20` and `20261120` are one
  name, at any depth, with any prefix or suffix: the day directory, its parts `v2026.11.20-planes-readsb-prod-0.tar.aa`,
  an archive `zau-2026.11.20.tar`, `raw/sfdps/2026.11.21/a.xml`, `scds/day-20261120`, `scds/rec-20261120`,
  `derived/zau-20261121.oaf` and any renamed or partial copy. The consecutive components `2026`, `11`, `20` (and
  `2026-11`, `21`, in any spelling) name a day too. A scope is matched as consecutive components anywhere in a path, so a
  copy under another root is refused. The match is on digits only and fails closed: `120261120` and an epoch name that
  holds the compact day are refused under these directories. A directory named `raw`, `scds` or `derived` anywhere in a path is a scope, so a path such as `/home/x/raw/notes.txt` is in scope and an ordinary file under such a directory can be refused (fail-closed refusals of ordinary files are expected); the digits are ASCII only, and full-width digits are not matched. Not matched: day-first dates (`20.11.2026`), month names and
  ordinals, and anything outside the scopes; a copy outside the data root that is named in none of these ways is
  caught only by the window rule on its rows (OA-SEAL-011, OA-SEAL-014). Also sealed: `scds/stdds-20261119-holdout*` (so
  `.tar`, `.bak` and `-copy` variants). Near misses are accepted: `scds/day-20261124`, `scds/rec-20261119/day1`,
  `raw/adsblol/2026.11.14`, `raw/rap/rap.20261120.t00z.grib2`, `raw/awc/2026-11-21.json`, `v2026.11.24-...`,
  `derived/adsb/zau-2026.11.14.oaf`, `stdds-20261119`, `raw/sfdps/2026/12/20`. A directory that only holds a day-named
  entry is not a holding directory. Tests: `every_sealed_adsb_day_in_both_name_forms_and_derived_clips_and_the_stdds_prefix_are_sealed`
  and `a_sealed_day_is_matched_on_normalised_components_split_components_and_every_raw_feed` (Rust and Python), and the
  parity of the two parsers on the `day`, `dayscope` and `dayexempt` lines (a month 13, `raw/*`, `/raw`, `raw//rap`, a `day` with no `dayscope` and the reverse, a trailing space and a non-ASCII digit are refused by both; a `dayexempt` alone parses).
- **OA-SEAL-011** The list holds `window <from-UTC> <to-UTC>` lines (`YYYY-MM-DDTHH:MM:SSZ`, half-open, from before to):
  every row or frame whose time falls in one is sealed whichever file holds it. `oa_sealed::in_sealed_window(unix_ns)` and
  `sealed_windows()` expose them, `sealed_guard.in_sealed_window` and `Guard.windows` do the same in Python, and both
  parsers read the lines to the same set and refuse a malformed or empty window. The list has
  `window 2026-11-20T00:00:00Z 2026-11-23T00:17:00Z` (the three ADS-B days and hold-out days A to C of the four-day recording).
  Both parsers widen every listed window by 15 minutes on each side when they read it (`oa_sealed::WINDOW_MARGIN_NS`,
  `sealed_guard.WINDOW_MARGIN_NS`; the listed text is kept), so the span the gates and `DropWindow::sealed()` use is
  2026-11-19T23:45:00Z to 2026-11-23T00:32:00Z. The margin covers the rounding of a fraction, differences between a scan and a
  parser, and the lag between a feed's own time and the receive time of its frame (the SFDPS lag floor is 58 s):
  development data within 15 minutes of either edge is dropped and counted. A list with no window line seals every instant
  (`Sealed::in_window`, `Guard.in_window`), and a tool prints `seal list has no window: every timed row is dropped` on
  standard error at start (`DropReport`, `sealed_guard.production`), still failing closed; a list that does not parse
  prints `seal list does not parse: every timed row is dropped and every sealed path refused` the same way (`oa_sealed::start_warning`,
  `sealed_guard.start_warning`). Several windows are read to one span, the hull from the earliest start to the latest end, with
  any gap between them inside it (`Sealed::drop_window`, `sealed_window_ms`; the gates `in_window` test each window).
- **OA-SEAL-012** A test or tool that walks a directory uses `oa_sealed::walk_unsealed` (Python `Guard.walk_unsealed`): a
  directory or file the guard refuses is skipped and counted, never descended into or opened, a directory that only holds a
  sealed entry (the data root, `raw/adsblol`) is walked, and a sealed walk root is an error. `oa_sealed::read` and
  `read_to_string` read through the guard's opener. `fuzz/make_corpora.py` reads and globs through the guard. The walk
  visits each canonical directory once: the entries of a directory are sorted bytewise before any is walked, so of two
  names of one directory (a sibling link such as `a/x -> ../real` beside `real`) the first in that order is the one
  walked; a repeat visit, and a link back, is counted as `already_visited` (`Walk::already_visited`; the Python guard
  returns it as the third item), apart from the sealed entries counted in `skipped`. Nesting deeper than 64 and a root
  that does not exist are errors, not a silent stop or an empty walk; the files are sorted bytewise by the string form of
  the path, the same in both guards. Tests: a self-link, two self-links, a sibling link, a deep tree, a missing root and
  the sort order (Rust and Python).
- **OA-SEAL-013** Every library function that takes a path and opens it for reading opens it through the guard: `oa_zip::read_archive_file`,
  `oa_nasr::open`, `oa_dof::read_file`, `oa_tiff::read_file` and the `oa-replay-diff` file readers; a sealed path is an I/O error that names OA-SEAL-001.
- **OA-SEAL-014** The readers drop every row or frame whose time falls in a sealed window (OA-SEAL-011) and count it:
  `oa_bench::input::load_adsb` (a frame with a row in the window is dropped whole and its rows counted;
  `AdsbClip::sealed_window_dropped`) and `for_each_adsb_frame` (a frame with a row in the
  window is not passed on), `oa_truth::clip::read_clip` (`read_clip_counted`; a frame with a row in the window is dropped whole,
  and is in no other count, `oa_adsb::reports_extend_unsealed`), the `oa-replay` recording reader (frames by
  receive time; `MergeReport::sealed_window_dropped`) and the `oa-stdds-census` recording reader (`ReadStats`). A row at the
  `from` instant of the widened window (the listed start less 15 minutes) and one at the last nanosecond before its `to`
  (the listed end plus 15 minutes) are dropped; one at `from - 1 ns` and one at `to` are kept. Every
  tool built on them prints `sealed-window rows dropped: N` (also when N is 0) and, where it writes a report or README,
  records the line there: oa-bench (report note), oa-compass (README), oa-encounter (summary), oa-shadow (report header),
  oa-stdds-census (the read section); oa-truth-check, oa-sensorsim, oa-track and its examples, oa-replay print it.
  The message loaders do the same by frame and by record: `oa_bench::input::load_with` (SFDPS, TBFM, TFMS; frames by receive
  time, and a document with a message whose own time is in the window even when its frame was received outside it (the keys of
  the list below, which the decoders read; the loader has no key list of its own); `Loaded::sealed_window_dropped`, `Counts::sealed_window_dropped`),
  the `oa-replay` pipeline (documents by the times of their messages, and an ADS-B frame by its rows, each dropped whole;
  `Pipeline::sealed_window_dropped`), `oa-feed replay` (frames;
  `Report::sealed_window_dropped`), the `oa-stdds-census` reader (TAIS records by `mrtTime`) and the `stdds_summary` example.
  For these the printed count is frames plus records and the tool says so. The same holds for `oa-flight` (`oa-flight-sfdps`: frames by receive time from the merge, rows by their own time; a run stopped early by `--to` does not count the frames dropped before the stop), the SFDPS join of `oa-encounter` (added to the clip's count), the recordings and clip of `oa-stdds-ref` (frames by receive time, TAIS records by `mrtTime`), and for `oa-scope-replay` (clip
  frames by receive time, and a frame with a row in the window dropped whole and in no decode counter, through
  `oa_adsb::reports_extend_unsealed` on a per-frame scratch), the
  `oa-replay` pipeline's ADS-B frames (whole frame, by any row's time; the frame is in no error, frame, message, byte or `fed`
  count) and FNS rows (issue or cancel time; the validity span is not a message time),
  the `oa-adsb` archive reader (trace rows; `Stats::sealed_window_dropped`, and `oa-adsb-clip` refuses, OA-SEAL-015),
  `oa-wind-check adsb` and the `stdds_summary` example, which
  drops inside the decoder. The window is a required argument of every decoder, so a caller cannot forget it:
  `oa_stdds::Limits`, `oa_fixm::Limits` and `oa_notam::Limits` have no `Default` and a constructor `Limits::new(DropWindow)`;
  `oa_tbfm::new_rows` and `oa_tfms::new_rows` take it as their last argument. `oa_sealed::DropWindow` is the half-open span in
  Unix ms; `DropWindow::sealed()` is the sealed window (`oa_sealed::sealed_window_ms()`) and is what every tool passes;
  `DropWindow::empty_for_tests_and_fuzz()` drops nothing and is for tests and fuzz targets only (its name says so). A test per
  shell asserts the window it passes equals the sealed one (oa-replay `doc_fixm_limits`, `doc_notam_limits` and the TBFM and
  TFMS row tables, which `oa-bench` and `oa-flight` use through `Reader::drop_windows` and `decoder_limits`, the census, the
  `oa-stdds-ref` loader and the `stdds_summary` example). A document is dropped whole by the times of its messages. Every decoder's first step is
  a pre-scan of the whole document (the whole message for stdds), before any counter, row, table or string store is touched. It
  reads each message's or record's own time by the feed's known path, directly from the parsed tree, in any attribute order:
  its own times by the feed's known path (the key list below). If any is in the window the whole document
  is dropped and a sealed outcome carries only the count of its messages (or records): `Stats::sealed_window_dropped` or
  `Outcome::sealed_dropped`. It is in no other count (no message, document, row, kind, rejection, refused-field,
  field-error, shape, flag, unknown-path or unknown-root count) and touches no table or string store. No refusal can be
  caused by an in-window message, because the pre-scan has no record or row limit and runs before the decode. There is no
  window rollback in any decoder. Every pre-scan reads each value with the decoder's own value reader and parser (the same
  function, not a copy; the fixm survey calls `oa_fixm::prescan`), so the scan and the decoder cannot differ in what they read. TFMS TMI-list counts are therefore for the whole document, not per entry. The rule for what is a window key: every observation time and every actual-event time the decoder parses into a row or a
  statistic is a key (a message stamped after the window can report an event inside it); a reference time (an estimate, a
  schedule or a validity time such as ETA, ETD, STA, effective, begin, end, original and filed times) is not. The keys per feed, exactly as the code reads them: TAIS `track/mrtTime` and `recSAFAReceiptTime` of each `record`
  (`recSTARSTimestamp` is a time of day, not a time); ASDE-X `time` of each report; SFDPS (fixm) the `timestamp` and
  `positionTime` attributes of any element, and the `time` attribute of an `actual` element (the actual departure); TBFM
  the `msgTime` attribute, the envelope time of an `env` (`envTime`) or `mis` (`misTime`) document, and `ctm`; TFMS the
  attributes `sourceTimeStamp`, `airlineOutTime`, `airlineOffTime`, `airlineOnTime`, `airlineInTime`, `runwayDeparture`,
  `runwayArrival`, `gateDeparture`, `gateArrival`, `boundaryCrossingTime`, `lastUpdateTime` and `flightCreation`, the
  `timeValue` of an `etd` with `etdType` ACTUAL or an `eta` with `etaType` ACTUAL, and the element text of `timeAtPosition`, of
  any element named `time` (an oceanic position's), of `eventTime`, `dateSent` and `lastUpdate` (flow and TMI messages), of
  `timeOfDeparture` and `timeOfArrival` unless `estimated="true"`, and of `coordinationTime` and `newCoordinationTime` with
  `type` ACTUAL; NOTAM the text of `issued` and `canceled`. Borderline times, keyed because when unsure a time is keyed (a
  false drop costs a document of development data; a missed key leaks hold-out data): the TFMS `flightCreation` (when the flight
  was created in the system, which is the time of the message that created it) and the TBFM `ctm` (a crossing time, which can
  be an actual one). Not keys, as reference times: ETA, ETD and STA of any other type, `igtd`, the original departure and
  arrival, the arrival and departure fix times, the `startTime`, `stopTime` and `endTime` of a restriction or advisory, the
  TBFM schedule and estimate times, and the validity times of a NOTAM. A scan reads a value with the decoder's own reader, so it
  reads what the decoder reads: character references are decoded and whitespace is trimmed where the decoder trims; text
  that a comment or CDATA section splits is one value for STDDS and TFMS and is a bad field, not a time, for NOTAM. A repeated
  element of a key name is read too. The STDDS scan skips a record outside the message's namespace, as the decoder does; the
  other scans read an element of a key name in any namespace (so such a document is dropped too: the closed failure). A time that cannot be
  read falls to the receive-time gate of its frame; a document refused whole (not XML, wrong root) has no known time and is
  counted as refused. Development-day documents near the window edges that carry an in-window message are lost whole, and
  counted. `DropWindow::sealed()` fails closed: if the sealed list has no window or does not parse it is a window covering
  all time (`DropWindow::all_time()`); `oa_sealed::sealed_window_ms()` is its span and `Sealed::in_window` (so
  `in_sealed_window`, the frame and row gates) seals all time for a list with no window line, so the gates and the
  decoders agree. The millisecond span rounds the start down and the end up, so a window that is not a whole number of
  milliseconds leaves no gap. The `oa-bench` scan (hints, addresses, handoffs, routes) runs only on documents that passed the pre-scan, and `docs` counts a
  document after it. The replay pipeline counts a frame and a message only after the decode, and only for a document that
  was not dropped (`Pipeline::last_doc_dropped`); the byte count of `oa-replay` leaves out the bytes of such a frame, as it
  does for a frame dropped by receive time. An ADS-B frame with any row in the window is dropped whole in the same way (its rows are counted as dropped; it is in
  no frame, message, byte or `fed` count). The replay clock and the sweeps use frame receive times, which the gate has
  checked are outside the widened window; the clock moves for the frame of a dropped document too, and what it prints (the
  replay time and its own counters) comes from those receive times, not from the document. `oa-feed` health and the
  `oa-bench` input digests count by receive time and are file identities, covered by the 15-minute margin. The
  `oa-flight-sfdps` merge report's bytes are not printed. The input digests and byte totals of `oa-bench` are
  file identities, like the SHA-256 that `oa-stdds-ref` prints, and not statistics. A recording's byte count (`oa-replay`, `oa-feed replay`) is
  the bytes after the window gate: the bytes of a frame dropped by receive time are not in it. A failed read still adds what
  was counted so far (`oa-stdds-ref` `load_file_counted`, `stdds_summary`, the fixm survey, which uses `DropWindow::sealed()`, drops a file with a message in the window whole before any count (`files` counts the kept files only; the dropped ones are in the drop line), and returns an error through
  `main` instead of unwrapping). The SHA-256 `oa-stdds-ref` prints is a file identity (of the file as a whole, as a name for it), not a statistic of its contents. `oa-bench` and `oa-stdds-census` record the line in their report and also print one
  stderr line at exit, which is the same count printed once. Other message kinds (service status, classified only) carry no row time and go by
  the receive time of their frame. Tests assert the `records:`, a TAIS field and a flag count, a refused message whose early
  records are in the window, the fixm, TBFM, TFMS and NOTAM stats, the oa-replay TFMS flow rows, and the oa-bench tables. `oa-scope-replay` prints its line on every exit, an empty window included, and its decoder `records` and
  `kept` counts are after the window filter. A row or record with no time of its own is kept, backstopped by the receive time of its frame where
  the reader has one: SFDPS rows without a timestamp, TBFM/TFMS rows without a message time, a TAIS record without
  `mrtTime`, a raw `.msg` file (no frame header, so nothing to count by receive time). `oa-compass slice` reads the `.ast`
  files of an export, which carry only a time of day, and relies on the export, which never writes sealed rows; the Python
  result tools (`run_comparison.py`, `track_set_score.py` and the like) read derived results, not data. The printed line is
  printed once per run, by an `oa_sealed::DropReport` that the tool makes at the top of its run and adds each reader's
  count to as the reader returns; the line is printed when the report is dropped, so every exit that comes back through
  `main` prints it: a `?` error, `--hours`, a stop at `--to` (which adds the merge's own count of frames dropped before the
  stop), a refusal and a failed read. No tool calls `process::exit` after reading has started. The count is the sum of the
  per-reader counts (`Loaded`, `MergeReport`, clip counts), each input counted once (an input read twice is not counted twice); the process-wide counter
  (`oa_sealed::window_dropped_total`) is kept only as a test cross-check. A test asserts the line for each tool that has
  an integration test.
- **OA-SEAL-015** `oa-adsb-clip` creates the sidecar and then the clip, each with `create_new` (`O_EXCL`), so nothing is replaced, truncated or
  followed (a dangling link included) and no second name for an existing file appears; an existing output or `.source` is refused, and a failure removes the files this run created and only those, and refuses to clip a report whose time is in a sealed window: exit 2, nothing written, the count
  printed as `sealed-window rows dropped: N` (the reports are counted, not clipped). Otherwise it writes `<out>.source` next
  to the clip (not for `--out -`): each input path with its SHA-256, the window (`--from`, `--to` in Unix nanoseconds, `none`
  when open) and the counts (rows seen, outside the box, outside the window, kept, frames, sealed-window rows dropped).
  A test makes the clip name appear between the pre-check and the create (a hook that exists only in test builds) and fails
  if the create is `File::create` again; another checks the cleanup of a write failure.

## Scoring against STDDS terminal tracks (oa-stdds-ref)

- **OA-SRF-001** A STDDS record is paired with the tracker and the ADS-B truth by its Mode S address
  (`acAddress`, 24 bits) only; a record without an address is counted and never paired.
- **OA-SRF-002** Every STDDS record has a class from its own flags, by one function, read as the census
  reads them and only when a flag's bit is in `present` (an absent flag is never false): `track/pseudo`,
  `track/frozen` or `track/coast` present and true is "pseudo, frozen or coasting"; otherwise `track/adsb`
  present gives "adsb=false" or "adsb=true", and absent gives "adsb absent". The names state the flag value,
  not a meaning: what `track/adsb` means is not confirmed (the census measures it).
- **OA-SRF-003** Every metric, count and time-offset distribution is reported per class; no line or section
  blends the classes, "pseudo, frozen or coasting" is never pooled with another, and a class's records are
  fitted as tracks of their own.
- **OA-SRF-004** Per class, three error sets are computed on the same paired samples: tracker minus STDDS,
  ADS-B truth minus STDDS, tracker minus ADS-B truth (the second is the diagnostic for STDDS following ADS-B).
- **OA-SRF-005** Per STDDS track the stamp offset is fitted against the ADS-B truth only, never the tracker,
  over -15 s to +15 s in 0.1 s steps minimising along-track error, ties toward zero; every metric is reported
  at offset 0 and at the fitted offset, and a track with no fit has no fitted samples.
- **OA-SRF-006** The offset search is exact at its edges, flags an offset at either end of the range, and
  refuses a fit with fewer than 8 samples that have a moving truth at every offset (OA-SRF-015 gives the
  reason).
- **OA-SRF-007** Tracker fixes are interpolated between rows at most 5 s apart; at a track end a fix is
  extrapolated by the end row's velocity up to 1 s and counted, and refused beyond that.
- **OA-SRF-008** Per set: horizontal error, along and cross track, altitude difference (feet) and velocity
  difference (m/s), each with n, rms, mean, p50, p95 and p99; an absent value is no data, never 0.
- **OA-SRF-009** The summary carries the SHA-256 of each input file and one digest over the summary and the
  per-sample CSV; the CSV (record place, class, status, offset and errors) carries neither. Both outputs
  are byte-identical for the same inputs given by the same paths.
- **OA-SRF-010** The tool uses the shared guard (`oa-sealed`, OA-SEAL-001, OA-SEAL-002) and no guard of
  its own: every path it is given (the STDDS files, the ADS-B clip, the output folder) is checked before
  anything is read or created, and a sealed one (a sealed folder, a sealed ADS-B day, a link into sealed
  data) exits 2 with a message that names the path.
- **OA-SRF-013** Every file the tool reads is opened by check, open and verify-the-open through the shared
  guard, so a link swapped between the check and the open is refused (exit 2, the path named). The tool
  walks no directory.
- **OA-SRF-014** The text and the per-sample CSV hold counts and aggregate statistics only: never a track
  number, callsign, Mode S address, time or position of a record.
- **OA-SRF-011** A `.oaf` or `.oaf.gz` recording is read into records with counts of frames, other products
  and damage, and the SHA-256 of the file's stored bytes.
- **OA-SRF-012** A record with no time or no position is counted as its own unpaired reason, never paired.
- **OA-SRF-015** Tracks that are not fitted are reported per class by reason: no ADS-B truth over the
  +-15 s window at enough samples (the track has at least 8 sample records), and fewer than 8 usable samples
  (too few records, or a covered truth too slow to give a direction).
- **OA-SRF-016** The tool creates `stdds_ref.txt` and `stdds_ref.csv` with create-new and never overwrites:
  an existing output (or link) exits 1 with its path named, before any work; each output path is checked
  through the shared guard before it is created, and one that leads into sealed data exits 2.
- **OA-SRF-017** The report states the assumed units (STDDS altitude feet, velocity m/s east and north) and
  which tracker altitude is compared, and prints per class the median STDDS altitude against the median
  truth altitude and the median ratio of STDDS speed to truth speed on the paired samples, with "units
  suspect" when the altitude medians (floored at 100 ft) differ by more than a factor of 10 or the speed
  ratio is outside 0.8 to 1.25.
- **OA-SRF-018** Offset calibration requires a positive address and complete source-facility/track
  key. Ambiguous duplicate timestamps are excluded without choosing class order; every unambiguous
  class transition and declared reuse gap starts a separate fit segment, including A-to-B-to-A.
- **OA-SRF-019** Comparison input, retained records, fit work and output bytes have checked limits.
  Reports distinguish each mode's cohort from the same-record raw/fitted intersection and publish
  per-metric denominators. In-sample calibration and unverified units remain explicit limitations.
- **OA-SRF-020** Summary, CSV and JSON are published without replacing existing outputs; a final
  content/provenance marker binds all three. Cooperating same-host writers hold one publisher lock.
  This does not establish distributed fencing or automatic recovery of a partial report set.
- **OA-SRF-021** Published-time cohort selection uses explicit half-open bounds before offset fitting, retains the complete ADS-B boundary context, and counts selected, untimed and outside rows against the post-seal denominator. Oversized retained sets are refused before selection mutates any row.

## M4 legacy trajectory measurement (tools/oa-legacy-traj)

- **OA-LTJ-001** The time an aircraft reached a point is the time of closest approach of its ADS-B truth path to the point, interpolated linearly between the two samples either side of it.
- **OA-LTJ-002** A crossing counts only when the closest approach is within the configured cut distance (development value 2 NM); the closest approach of every (flight, point) searched is counted by distance bucket in the report.
- **OA-LTJ-003** A passage is at an edge, and not scored (counted as such), only when the segment of its closest approach, or a segment directly next to it, begins or ends at the start or end of the truth path or at a hole in the truth; a path with no sample is counted separately.
- **OA-LTJ-004** An estimate is scored against one passage of the bound leg, chosen by the rule of OA-LTJ-049 from the passages whose closest approach is not before the time the estimate was issued. When every passage is before it, the last one is taken: the event happened before the message, and the error is kept. The estimated time never chooses the passage.
- **OA-LTJ-005** The touchdown time is the middle of the gap between the last airborne report and the first ground report, the gap at most the configured limit (development value 30 s); the last airborne report is within the runway radius (5 NM) of the airport reference point; a landing outside either is counted by reason.
- **OA-LTJ-006** The bin of an estimate is that of its stated lead, the estimate minus the issue time of the message that carried it; the bins are 0-5, 5-10, 10-20, 20-40, 40-60 and 60-120 minutes, lower edge included; a stated lead that is negative or outside them is excluded and counted. The actual time minus the issue time is a sensitivity binning.
- **OA-LTJ-007** The error is the estimate minus the actual time, in seconds: positive when the estimate is later than what happened.
- **OA-LTJ-008** Each bin reports n, the median, 95th and 99th percentile of the absolute error (nearest rank, OA-LTJ-025) as the headline columns, the signed median and mean as bias, the signed 5th and 95th percentile as spread, and the RMS and the share over 1,800 s, computed in a fixed order whatever order the values arrive in; the 95th and 99th percentiles and the spread percentiles are printed only for n of 100 or more, else `n<100` (empty in the CSV).
- **OA-LTJ-009** A flight that carries an aircraft address joins by it when a truth leg of that address overlaps the flight's interval; otherwise it has no truth, and its callsign is not tried.
- **OA-LTJ-010** A flight without an address joins by callsign when the truth legs with that callsign that overlap its interval are of one aircraft; several aircraft is ambiguous, none is no truth.
- **OA-LTJ-011** A flight with no address and no callsign, and a flight whose records name more than one address, each have their own join outcome.
- **OA-LTJ-012** Every flight is counted under one join outcome and every estimate is either in a bin or under one exclusion reason; a flight with no truth is counted and its estimates are excluded, none is dropped; a callsign join that fails the conditions of OA-LTJ-030 counts as no truth.
- **OA-LTJ-013** The report text and the CSV are a function of the counts alone, and every number carries its n.
- **OA-LTJ-014** ERAM's predicted time at each point of a converted route is scored against the crossing of the point, by look-ahead from the route message; a point's position is its own coordinates, else the one place its name has in the recordings.
- **OA-LTJ-015** TBFM's meter-fix and scheduled times are scored against the crossing of the meter fix, and its runway times against the landing at the arrival airport.
- **OA-LTJ-016** TFMS's arrival estimates are scored against the landing at the arrival airport; its departure estimates are counted and not scored.
- **OA-LTJ-017** The tool takes the three recording directories, one or more ADS-B clips and the output directory, and refuses a missing, repeated or unknown argument. A NASR archive and a places file are optional. Its usage text prints every default and every fixed setting, and the test checks that text against the values the tool uses.
- **OA-LTJ-018** Output files are written under a temporary name and renamed only after all are written. On any failure every temporary file is removed, and the error names the files already renamed, which keep their new content; a failed run can therefore leave new and old files side by side, and says so.
- **OA-LTJ-019** The tool refuses every input and output path that resolves into sealed data, before anything is opened or created.
- **OA-LTJ-020** An airport's place is taken from NASR (`APT_BASE`, by its FAA identifier and its ICAO identifier) or the places file, never derived from the recordings; an airport with no place is excluded and counted as airport unknown.
- **OA-LTJ-021** A named point's place is its own coordinates in a route message, else NASR (`FIX_BASE`, `NAV_BASE`, and an airport where a route names one) as overridden by the places file; a name with no place is unknown and counted as an exclusion; a malformed places line is refused naming its line.
- **OA-LTJ-022** A name with several places takes the one nearest the flight's route and the estimate is counted as ambiguous-resolved: for ERAM, the distance by great circle from the candidate to the nearest of the filed route's neighbouring points with coordinates (two each side in the same route), else of the flight's other route points with coordinates, else of the flown path (thinned to at most 200 places), and the report counts which was used; for TBFM the flown path; the name is refused and counted as ambiguous when its places are more than 500 NM apart or the flight has no reference place.
- **OA-LTJ-023** Scored rows are in four blocks: address-joined and airborne at issue (the headline), address-joined and not airborne at issue (pre-departure, labelled as including departure delay), and the same two for callsign joins; each block reports as OA-LTJ-008, with the rows kept whose event was before the issue counted per bin. Headline-only diagnostics: the error by actual lead, a histogram, the error against the TFMS actual arrival time, the touchdown at the last airborne and at the first ground report, the gap of the landings used, one row per flight and bin (the last message in the bin) for every kind, and the crossing at the cuts 0.5, 1, 2 and 5 NM.
- **OA-LTJ-024** `--nasr` reads `FIX_BASE`, `NAV_BASE` and `APT_BASE` from a NASR CSV archive through the sealed-data guard; the report gives, for ERAM name-only route points, TBFM meter-fix and scheduling-fix names and TFMS arrival codes, how many names and occurrences are not found, have one place, have several within 500 NM, or several over 500 NM, and lists the 20 most frequent names not resolvable.
- **OA-LTJ-025** Percentiles are nearest rank, as `oa-bench` and `p002` define them: the value at 1-based rank ceil(p n / 100) of the sorted values, no interpolation.
- **OA-LTJ-026** A passage in progress at the issue time is chosen by its closest approach like any other; one whose closest approach is before the issue is the last passage and is kept.
- **OA-LTJ-027** A change from airborne to ground is a landing only when no airborne report follows within 30 s of the last airborne report and that report is below the airport elevation (NASR) plus 1,000 ft or below 80 m/s; refused changes near the airport are counted (touch and go, blip, too high and fast); a landing elsewhere is a diversion exclusion; a leg with no ground report is excluded as `bound leg has no ground report`, checked after the look-ahead range; the landing exclusions are split by whether the bound leg ends inside the clip box or within 10 NM of its edge.
- **OA-LTJ-028** A row whose event happened before the issue time and whose estimate is not before it is scored, not excluded, and counted per bin.
- **OA-LTJ-029** Each message binds exactly one truth leg of the flight's own. A leg in progress at the issue time binds only when the aircraft is airborne on it (OA-LTJ-031), it lands after the issue, and it took off within 10 NM of the departure airport or lands within 10 NM of the arrival airport, or, with neither airport known, carries the flight's callsign; a leg that had landed before the issue never binds as in progress. Otherwise the first leg not yet airborne at the issue binds when it starts within 3 h after the issue and took off at the departure airport, or, with the departure airport unknown or not in NASR, carries the flight's callsign; else none (counted as no leg, no leg of the flight's own, or departure past the window). The crossing and the landing are searched only on that leg; joined flights whose interval touched more than one leg are counted.
- **OA-LTJ-030** A callsign join needs a leg that starts no later than 3 h after the flight's first issue (or is in progress at it) and, when the departure or arrival airport is known, that took off at the departure airport or landed at the arrival airport (within 10 NM of its place); a join refused is counted by the condition that refused it.
- **OA-LTJ-031** A row is airborne at issue when the issue time is between the bound leg's first airborne report and its landing (the first ground report of the first change to ground that holds up, OA-LTJ-027), or its last report when it does not land; otherwise it is pre-departure.
- **OA-LTJ-032** The report header gives, for each system, the flights in the recording, the flights joined to an ADS-B aircraft in the clip and their share, the flights with a scored estimate, and the estimates excluded because the bound leg has no ground report.
- **OA-LTJ-033** The NASR archive and the places file go through the sealed-data guard, by the tool's path check and by the readers themselves.
- **OA-LTJ-034** No two distinct NASR airports share an airport key; a shared key is detected and counted in the report header.
- **OA-LTJ-035** An in-progress leg that is another flight's is refused and counted, and the message falls through to the flight's own next leg: a turnaround where one fix lies on both the inbound and the outbound legs scores the outbound flight's message against the outbound passage only.
- **OA-LTJ-036** A leg that landed before the issue time (taxi-in) never binds as in progress; it is counted as refused.
- **OA-LTJ-037** The airborne span of a leg runs from its first airborne report to the first ground report of the first change to ground that holds up, else to its last report; a leg with no airborne report has none.
- **OA-LTJ-038** The landing persistence of 30 s is measured from the last airborne report.
- **OA-LTJ-039** A passage is at an edge only as OA-LTJ-003 says: a path that merely begins or ends within the cut, far from the closest approach, is not.
- **OA-LTJ-040** For each kind and stated-lead bin the report gives the estimates offered, those with no leg (of which no leg of the flight's own, and departure past the window) and those scored; the landing exclusions are split by where the bound leg ends (OA-LTJ-027); a table gives, per arrival airport (the 30 with most estimates), the estimates offered, scored and the share.
- **OA-LTJ-041** For every kind the report gives the headline as one message per flight and stated-lead bin (the last message of the flight in the bin, all its rows), beside the message-weighted one.
- **OA-LTJ-042** A leg ends at the clip box edge when its last place is within 10 NM of an edge of the box.
- **OA-LTJ-043** The estimate of a name with several places in an ERAM route is resolved against the route's neighbouring points, else the route's other points, else the flown path, and the report counts each.
- **OA-LTJ-044** A runway or arrival estimate whose stated lead is out of range is excluded for that reason before the ground-report check.
- **OA-LTJ-045** The headline crossing is searched at the cuts 0.5, 1, 2 and 5 NM, the headline stays at 2 NM, every row scored at a smaller cut is scored at each larger one (OA-LTJ-048; a violation is counted), and headline crossings whose closest approach is from 2 to 5 NM are reported apart as the abeam table; every cut reports its rows per bin.
- **OA-LTJ-046** The TBFM `sta_sfx` and `sta_rwy` kinds are labelled a schedule target (metering), not a forecast.
- **OA-LTJ-048** The passage of a (message, point) is chosen once, among the passages found at the largest cut, by the rule of OA-LTJ-049; each cut scores that passage when its closest approach is within the cut and it is not at an edge, so a row scored at a cut is scored at every larger one.
- **OA-LTJ-047** The report gives, for the ERAM headline rows at 60 to 120 minutes under the binding of the earlier rounds (in progress or next leg with no ownership test), what became of each under the present binding, counted by reason.
- **OA-LTJ-049** A run of consecutive truth segments within the cut is split into passages where the distance to the point rises 0.5 NM above the run's closest approach and then falls 0.5 NM below its largest, so each loop of a hold is its own passage. Of the passages not before the issue the first whose closest approach is within 0.5 NM is chosen; if none, the closest (the earliest on a tie); if none is at or after the issue, the last before it. The report counts the (message, point) rows with more than one passage and those whose choice differs from the runs-not-split closest rule.
- **OA-LTJ-050** Next to each headline the report gives, per bin, the share of the offered rows excluded as truth path never within the cut, passage at an edge, and no ground report, and the headline recomputed without the rows whose place was resolved against the flown path. Exact duplicate rows (same flight, point, issue time and estimate) are dropped before scoring and counted.
- **OA-LTJ-051** An in-progress leg is owned by a flight whose departure and arrival airports are both known only when the departure matches (or the leg begins at the clip box edge) and the arrival matches (or the leg ends there), at least one of them matching; the rows whose bound leg this changes are counted. ADS-B legs carry no flight-plan identity, so no binding by GUFI is possible.
- **OA-LTJ-052** A truth path segment longer than 30 s is a hole for the edge rule and is counted (the truth answers only within 10 s of a report on both sides, so none arises at a step of 30 s or less).
- **OA-LTJ-053** The opt-in ORD arrival experiment uses the route and flight identity recorded at
  receipt, the latest eligible airborne ADS-B report at or before that issue, and the latest
  available TBFM estimate at the same cutoff. Ambiguous observations are excluded and counted.
  The report is aggregate and states its offered, issued and scored denominators.
- **OA-LTJ-054** An issued arrival forecast provides 5, 10 and 20 minute positions and a
  selected-fix ETA when inputs support them. Scoring uses future truth without bridging gaps,
  the first valid post-issue meter passage, and optional comparisons available at issue time.
  A missing measurement remains absent.
- **OA-LTJ-055** The arrival forecast uses an assigned altitude only from a supported simple
  SFDPS assignment received by the issue cutoff. Selection is by source event time among
  available updates. Malformed assignments, valid non-simple replacements, conflicts at the same source stamp,
  uncertain availability that could change the selection, and incomplete bounded history
  withhold the scalar with a named reason. The report counts assignment states per issued forecast.
- **OA-LTJ-056** The opt-in arrival raw-rate diagnostic counts evidence over the frozen issue
  cohort using unique, compatible preceding barometric observations of the same ADS-B source
  class. It considers up to 128 prior rows and uses observations within 10 seconds to compare
  displacement against its illustrative 50-foot tolerance. It uses existing issues and forecasts.
- **OA-LTJ-057** Arrival prediction and truth interpolation follow the shorter longitude
  displacement across the antimeridian. Closest-approach segments also follow the shorter
  displacement across the planar chart boundary opposite the scoring point.
- **OA-LTJ-058** Arrival scores report along-track and cross-track error at each horizon with
  positive values ahead of and to the right of the truth direction. The components resolve
  the same spherical distance. Truth direction requires gap-free support and speed of at
  least 10 m/s. The report counts unavailable directions by reason.
- **OA-LTJ-200** At a positive issue cutoff, the route selector forecasts the latest received
  revision for the specified full flight GUFI, independently of candidate order. A selected
  amendment changes the supplied route geometry and its resulting meter ETA.
- **OA-LTJ-201** Other-GUFI and post-cutoff revisions do not participate in route selection.
  The forecast envelope and shell report retain the selected plan ID, full GUFI, receive time
  and issue cutoff; future evidence does not alter that issue.
- **OA-LTJ-202** Available revisions at an equal receive timestamp with differing plan identity
  or route geometry are refused. Identical duplicates are deterministic in the core selector.
- **OA-LTJ-203** Missing/blank plan identity, missing flight identity, nonpositive receive or cutoff
  time, more than 256 selector candidates, or more than 256 fixes in an eligible route is refused
  before unbounded route processing. Only the bounded selected geometry is copied for prediction.
- **OA-LTJ-204** The shell considers every captured revision of an admitted flight at its issue
  cutoff. A latest amendment removing the selected meter or changing the destination out of the
  admitted ORD flow cannot disappear in favor of an older route; that issue is refused.
- **OA-LTJ-205** Shell cutoff selection preserves the selected revision's geometry and provenance
  together and ignores later revisions. The experiment retains its first-eligible issue per flight;
  it does not update that frozen issue for later in-flight amendments.
- **OA-LTJ-206** The shell checks raw route spans against the 256-fix limit before route-point
  resolution or allocation in flow admission and cutoff revision construction.
- **OA-LTJ-207** Malformed and over-limit route rows are counted and skipped as issue candidates,
  while retained in the full flight's revision history. A future-only bad revision cannot veto
  an earlier issue; a bad revision available at an attempted cutoff refuses that issue.
- **OA-LTJ-208** Same-receive-time, same-plan, identical-geometry revisions carrying conflicting
  selected-fix ERAM estimate payloads are refused in either input order. No estimate is favored
  or used to select forecast geometry.
- **OA-LTJ-209** Per-flight histories are sorted once within the 500,000 captured-route limit.
  Each issue uses a binary-searched cutoff prefix and refuses more than 256 available revisions
  before examining geometry. Across candidate attempts, revision examinations are bounded by
  the captured-row limit times the revision limit; each examined raw span is bounded at 256 fixes.

- **OA-LTJ-210** Operational source-ordered route selection is bounded to 256 revisions and
  256 route fixes per cutoff, is independent of candidate input order, and never lets a
  later-received older source event roll back a newer source event already available.
- **OA-LTJ-211** Within the 256-candidate budget, revisions received after the issue cutoff
  cannot affect selection or cause refusal for malformed revision payload or geometry.
  When a newer source revision is not yet
  received, the latest older source revision available at the cutoff remains selected.
- **OA-LTJ-212** Contradictory eligible route payloads for the same full GUFI, reporting centre
  and source timestamp are explicitly refused independent of input order. Distinct reporting
  centres are independent.
- **OA-LTJ-213** Equivalent same-source route payload duplicates are harmless. Provenance is
  chosen deterministically by earliest receive availability, then message identity, retaining
  full GUFI, centre, plan ID, source time, receive availability and message identity.
- **OA-LTJ-214** The latest eligible source revision is never silently replaced by an older
  revision when its route is empty, invalid or over the route-fix bound. Identity, centre,
  source/availability timing, revision-capacity and route-capacity failures are explicit.
- **OA-LTJ-215** Every source-ordered route selection, successful or refused, retains its offered
  denominator. Other-flight, other-centre, future, eligible and unexamined counts partition all
  input candidates; selected, older-source, restated and withheld counts partition eligible
  candidates. Above the global cap no candidate is examined. A refused issue withholds every
  eligible candidate and returns its named reason with the counts.
- **OA-LTJ-216** Operational route payload equality compares canonical floating-point bits,
  including altitude windows. Signed-zero differences cannot choose a payload by input order:
  they are explicit contradictory evidence. Equivalent invalid NaN payloads compare alike
  after canonicalization and remain subject to forecast validation, rather than producing a
  false same-source conflict solely because NaN is unequal to itself.

## M4 shared trajectory baseline (tools/oa-legacy-traj)

These requirements describe the bounded, pure issue-time component in `core::trajectory`; they do not qualify it as an operational trajectory service.

- **OA-TRAJ-001** With a valid track and route, the baseline deterministically returns fixed 5-, 10- and 20-minute constant-groundspeed horizon positions and an ETA for the requested exact route fix.
- **OA-TRAJ-002** A supported clearance can capture the forecast at its altitude only when the observed vertical motion approaches or crosses that boundary; inverted altitude windows are refused as infeasible.
- **OA-TRAJ-003** Published speed windows are checked against observed groundspeed only when the source is explicitly groundspeed-qualified; indicated-airspeed or unknown-reference constraints degrade as unsupported and never rewrite observed groundspeed.
- **OA-TRAJ-004** Route, track and clearance evidence is checked against the supplied issue cutoff; track state time and newest measurement time are distinct, freshness uses checked age arithmetic, and missing groundspeed or stale/future evidence has an explicit result.
- **OA-TRAJ-005** ETA matching uses the complete fix identity including region and scope, and the predictor refuses to extrapolate beyond the filed route end.
- **OA-TRAJ-006** Forecasting starts at the uniquely resolved exact active cursor bound to the flight key and route payload hash; filed points before it are excluded from forecast geometry and counted.
- **OA-TRAJ-007** A route longer than the fixed point capacity is refused before scanning and reports every supplied point as unexamined.
- **OA-TRAJ-008** Missing, ambiguous, future-available or incorrectly bound active cursors are explicitly refused; no proximity-based active-leg inference is used.
- **OA-TRAJ-009** Horizon and ETA timestamp overflow is detected and reported rather than wrapping.
- **OA-TRAJ-010** Route revision evidence available by the cutoff is validated as a whole; a malformed eligible amendment withholds selection rather than falling back to an older valid route, while future-available evidence is gated before payload validation.
- **OA-TRAJ-011** Revision evidence over the fixed candidate capacity is refused before scanning and counts all candidates as unexamined.
- **OA-TRAJ-012** Foreign route evidence is counted in disjoint GUFI, aircraft-address and reporting-centre categories.
- **OA-TRAJ-013** Revision selection orders evidence by source time across plan IDs, preserves the selected full key and provenance, chooses exact duplicates deterministically, and refuses same-source conflicting revisions.
- **OA-TRAJ-014** An absent scope is a valid explicit fix identity, while a surveillance aircraft address that differs from the flight key is refused.
- **OA-TRAJ-015** Clearance capture does not teleport a level aircraft or reverse vertical motion that is moving away from the cleared boundary; malformed scalar clearance evidence is refused or degraded explicitly.
- **OA-TRAJ-016** Unsupported route altitude constraints are declared even when outside the displayed horizons, and unsupported speed references are not reported as groundspeed violations.
- **OA-TRAJ-017** Clearance values whose evidence is unavailable at the issue cutoff cannot affect route-clearance feasibility checks or suppress horizontal forecasts; their altitude outputs remain unavailable and degraded.
- **OA-TRAJ-018** Repeated exact ETA-target identities are refused as ambiguous, and a vector gap crossed by a horizon remains visible as degraded output with a gap flag; an ETA across a gap is refused.
- **OA-TRAJ-019** Required alignment bytes are explicit, zero-filled record fields; nonzero bytes in eligible inputs are refused, while future evidence remains gated before reserved-byte validation.
- **OA-TRAJ-020** An indeterminate antipodal great-circle interpolation is refused as unsupported geometry, with unavailable coordinates; it is not reported as an exhausted route.

- **OA-TRAJ-021** Route points retain independent altitude and speed constraint channels, including simultaneous restrictions; a supported groundspeed check still runs while altitude is degraded as unsupported.
- **OA-TRAJ-022** Indicated-airspeed and unknown-reference limits are retained and reported unsupported; they never compare to, replace or flag observed groundspeed.
- **OA-TRAJ-023** Route evidence after cutoff is gated before constraint or coordinate validation; changing future payload cannot affect issue output.
- **OA-TRAJ-024** Malformed altitude or speed windows are refused even when the other independent channel is valid.
- **OA-TRAJ-025** Opt-in vertical reachability uses explicit full-flight-bound performance evidence and sequential rate-limited intervals. Assigned altitude is a level-off target without a fabricated fix deadline; unsupported or missing evidence cannot report a feasible profile.
- **OA-TRAJ-026** Performance and clearance evidence after cutoff is gated before payload inspection. Eligible evidence has checked timestamps, limits, reserved bytes and fixed-point propagation; zero climb/descent capacity is a valid physical bound.

## Regional offline integration (tools/oa-regional-session)

This is a bounded normalized-input composition, not facility, deadline or operational qualification.

- **OA-REG-001** Authoritative normalized flight updates execute actual flight operations; offer and acceptance preserve current ownership, and only a valid take transfers it. Clearances and stale refusals retain source evidence.
- **OA-REG-002** Track/flight joins use positive full aircraft addresses and retain all claimants; ambiguous joins and future snapshots cannot select a flight for prediction.
- **OA-REG-003** Surveillance and route events cannot manufacture flight identity bindings. Flight source/availability timestamps require exact millisecond precision rather than silent truncation.
- **OA-REG-004** All collection capacities and overflow counters remain visible. Each completed frame uses the same emitted observed state epoch for actual conflict alerting and scope, with flight and route evidence frozen at its receipt cutoff.
- **OA-REG-005** Bounded filed route text retains its exact bytes and provenance. Reconstructing the admitted prefix gives identical subsystem evidence; later amendments cannot change earlier frames.
- **OA-REG-006** Normalized event kinds must match their source categories. A newer route/plan mismatch blocks fallback; an execution failure poisons the session until fresh prefix reconstruction.
- **OA-REG-007** Feed loss remains visible while coasting traffic retains identity. Transport receipt and accepted measurement freshness are distinct evidence.
- **OA-REG-008** Input/output paths and source/receipt timestamps obey the sealed-data guard. A new closed run records input, adaptation, executable, settings and per-frame evidence hashes; incomplete runs have no valid completion marker.
- **OA-REG-009** Local controller requests preserve receipt/source sequence order with authoritative flight updates. The bridge checks full displayed and current identity, generation, command/track freshness and actor authority; every queued request is completed explicitly, and invariant failure poisons the session.
- **OA-REG-010** Default process recovery drills kill synthetic workers before apply, after apply and around authority acceptance, reconstruct identical admitted prefixes, fence stale/duplicate generations and bound stalled-worker cleanup. This is same-host recomputation, not an external exactly-once guarantee.
- **OA-REG-011** The composed MSAW core receives the current tracker epoch and the age of its last accepted barometric observation. Missing or stale altitude and missing coverage remain explicit; horizontal receipts cannot refresh altitude evidence.
- **OA-REG-012** MSAW provider failure publishes unavailable without evaluating partial floor samples, preserves alert evidence and permits tracking, observed conflict alerts and scope presentation to continue. Synthetic floor/weather requires an explicit option; supplied sources retain hashes and receipt evidence.
- **OA-REG-013** Bounded recorded ADS-B and SFDPS imports preserve exact native receipt provenance, admit events no earlier than the next whole-second cutoff, retain source timestamps and count unsupported, malformed, sealed and capacity-refused work. Closed imports use guarded new outputs and a completion marker written last.
- **OA-REG-014** Vertical profile integration is opt-in and uses only explicit eligible performance evidence for the exact full flight key. Missing, duplicate or foreign evidence has a visible per-flight refusal and cannot change horizontal forecasts or observed safety nets.
- **OA-REG-015** Operated replay selection checks the exact displayed address, generation and full identity; refusal clears selection and remains visible. Bounded console redraws replace only control primitives, preserve the observation epoch and safety presentation, and retain selection/input evidence.
- **OA-REG-016** The operated recorded-session console caps action parsing, inserts explicitly bound local commands in source order with sequence audit, refuses stale or ambiguous targets, and closes a manifest only after frame and journal files are durably written. Script-mode CLI tests verify accepted commands, visible refusals and every listed output hash; native input is accepted only on the active VT and idle polling is not input loss.
- **OA-REG-017** Recorded ADS-B source rows and SFDPS decoded rows are classified before normalized output is committed. A refused item retains a source-local frame/row locator and class-partition accounting; healthy sibling items continue, while queue overflow is surfaced as immediate manual handling. Classification occurs after bounded work and is not parser preemption.
- **OA-REG-018** Runtime flight, route, and surveillance items are staged before accepted state mutation. Known malformed, unsupported, and bounded-capacity refusals enter class-partitioned quarantine without changing that item's accepted state; a following healthy item may commit. A rejected filed-route amendment retains a bounded full-key availability fence for as-of queries, so no older route becomes current at a cutoff after the refusal. Barometric evidence capacity is checked before tracker mutation. Unexpected internal and whole-batch finish failures still poison the runtime; quarantine timing is observational and does not preempt work.
- **OA-REG-019** An explicitly selected recorded-replay CLI profile applies its versioned fixed capacities without changing default limits; rendered high-volume replay retains finite track, alert, pair, work, frame, and presentation bounds, and capacity exhaustion remains a refusal.
- **OA-REG-020** A regional replay profile is persisted in settings covered by the run manifest digest. J2 export/reimport preserves those exact settings and cold replay selects only the matching known profile; absent profile metadata retains the historical default.
- **OA-REG-023** The normal regional runner loads its model and optional surveillance-mode component from one caller-pinned issued adaptation package. The component uses the shared bounded codec and is activated before the first frame; absence leaves mode policy unconfigured and preserves legacy behavior. Package digests bind content but do not authenticate issuer/approver or independently validate preferred-radar coordinates. Raw ADS-B metadata does not establish reliability, ISR, FL, FLAT, or physical-site identity; the normal caller leaves those inputs unknown, coasted targets are never separation-eligible, and the conservative CA configuration is unchanged.
- **OA-REG-024** OA-RS v1 remains accepted unchanged; v2 carries exact decoder-row ADS-B source, quality, category, version, flags, squawk and callsign values on each accepted position. Original row indices survive quality filtering, and the companion provenance record binds normalized event sequence to input stream/frame/row. Missing values remain unknown, blank and absent callsigns are not distinguished by the current decoder, and source class alone does not establish physical sensor/site, reliability, ISR, or separation eligibility. Malformed field domains refuse the row.

## Current alert presentation (oa-display)

- **OA-DSP-077** Bounded MSAW markers distinguish caution, warning and protection loss by shape and label, use the matching aircraft's current supplied position, and count off-map and unlocated alerts. Complete validation and list-budget checks precede mutation; a count header retains every alert category.
- **OA-DSP-078** Explicit scene and replay top insets preserve the default 104 pixel view, validate additional reserved header space before allocation, and clip map primitives to the resulting map viewport.

- **OA-DSP-079** The metering field compares exact supplied Unix-millisecond STA and ETA values using `STA - ETA`; the inclusive ±60,000 ms window is the only within-window classification.
- **OA-DSP-080** A paired value outside the window preserves the signed difference and distinguishes early (ETA before STA) from late (ETA after STA).
- **OA-DSP-081** Missing STA and ETA remain absent, including when both are missing; a later supplied schedule is compared against the current supplied ETA without substituting another meter point.
- **OA-DSP-082** Difference and aggregate-counter overflow are explicit; a counter overflow refuses the observation without changing any counter.
- **OA-DSP-083** A meter list refuses capacities above its fixed limit and refuses a push at capacity without changing prior rows; clearing permits reuse within the same allocation.
- **OA-DSP-084** The traffic renderer uses the solid Fusion circle only when the caller supplies an eligible 3 NM result; fallback and coast states retain the ordinary marker. ISR is shown as a separate data-block tag, and bounded mode overlays must match sorted visible addresses and refuse ambiguous inputs. The renderer does not infer mode, reliability, or separation eligibility.

## Controller interaction (oa-display)

- **OA-UI-001** Selection, intent queue, completion and presentation are pure functions over caller-supplied plain values.
- **OA-UI-002** Bound flight identity includes all GUFI network-order bytes, aircraft address, reporting centre and plan ID, compared in full.
- **OA-UI-003** Target-view operations refuse more than 4,096 rows before scanning.
- **OA-UI-004** Selection distinguishes absent, raw-track-only and full-flight-bound states.
- **OA-UI-005** Address selection requires exactly one current target row and the expected generation.
- **OA-UI-006** A raw-track-only selection is displayable but cannot enqueue a control intent.
- **OA-UI-007** Binding refuses absent, ambiguous or mismatched full identity claims.
- **OA-UI-008** Missing tracks, duplicate addresses, malformed reserved fields and stale views/generations have named refusals.
- **OA-UI-009** Clearing selection never erases or retargets pending intents.
- **OA-UI-010** Assigned and interim altitude set/clear requests are typed intents, separate from authoritative flight mutation.
- **OA-UI-011** Handoff offer, accept, take and cancel intents name stable unit identifiers.
- **OA-UI-012** The fixed 32-row SoA queue reports capacity without allocation, panic or silent drop.
- **OA-UI-013** Each intent retains full identity, generation, view/input sequences, supplied issue timestamp and original typed request.
- **OA-UI-014** Accepted input sequences increase strictly; retries and backwards sequences have explicit outcomes.
- **OA-UI-015** Dispatch preflight rechecks current address uniqueness, generation and full identity against a non-backwards view sequence.
- **OA-UI-016** Completion names the exact pending head sequence; a mismatch cannot remove it.
- **OA-UI-017** Address reuse, disappearance and selection changes never rebind a queued intent.
- **OA-UI-018** Selection/queue refusals and completion outcomes remain explicit and presentable.
- **OA-UI-019** Interaction status uses the existing display list with bounded text and explicit viewport/capacity refusal.
- **OA-UI-020** Display interaction does not mutate or depend on the flight subsystem; the regional bridge remains responsible for authoritative identity and authority validation.

## Experimental workstation core (oa-workstation)

These requirements describe only the recovered OpenATCA prototype grammar and its synthetic
state model. They make no claim about current ERAM/STARS command syntax, response text, or
operational procedure; see `crates/oa-workstation/README.md` and `docs/DEMO_FIDELITY.md`.

- **OA-WS-001** The experimental parser accepts only the documented bounded prototype grammar;
  unknown commands, extra fields, malformed numbers and entries over 96 ASCII bytes are refused.
  Source test: `bounded_grammar_refuses_unknown_ambiguous_and_malformed_commands`.
- **OA-WS-002** Prototype altitude edits preserve assigned/interim distinction, enforce the
  current owning-sector check before shared flight-state mutation, and clear interim altitude
  with the existing nil form. These are OpenATCA model behaviors, not an ERAM procedure claim.
  Source test: `shared_altitude_edits_require_ownership_and_preserve_interim_distinction`.
- **OA-WS-003** Prototype point-out request/acknowledgment and handoff offer/accept/control are
  distinct operations with recipient/owner checks; the tests exercise only the `oa-flight`
  coordination contract and do not assert operational-system parity. Source test:
  `handoff_and_pointout_have_distinct_shared_state`.
- **OA-WS-004** Display range and per-position data-block preferences remain local to each
  position, while the prototype annotation and voice status use their shared coordination state.
  Position preference storage is fixed at 32 records. Source test:
  `position_preferences_are_separate_while_annotations_are_shared`.

## M5 — Minimum safe altitude warning (oa-msaw, tools/oa-msaw)

Build order step 7 of docs/M5_DESIGN.md. `oa-msaw` is a core crate; the shell lookups of terrain,
obstacles and charts cross as plain values. Every configured value is a development value, not an
FAA value.

- **OA-MSAW-001** A configuration out of range is refused with the name of the key; every table is
  allocated whole at the start and a capacity above its bound is refused.
- **OA-MSAW-002** Aircraft on the ground are excluded and counted.
- **OA-MSAW-003** A track with no altitude is counted and listed on that track (a gap of the aircraft)
  and does not degrade the net; the other tracks are still evaluated.
- **OA-MSAW-004** A path point without terrain or obstacle coverage is counted and listed. The net
  reports degraded input (`InputMissing`) and evaluates the points with coverage.
- **OA-MSAW-005** Over the track budget the lowest predicted heights above the highest terrain of the track's own
  cell (a track with none given first; height plus the descent over the look-ahead, ties by address) are evaluated first, the rest are counted and listed, and the net reads degraded (`BudgetExceeded`) with the count; rows past the track
  capacity are counted the same way.
- **OA-MSAW-006** Synthetic Guam case: an approach descending early onto rising terrain alerts at
  caution and then at warning.
- **OA-MSAW-007** An inhibition of the net in force at the track's position and altitude hides the
  alert, is recorded against its id, shows the net as inhibited with the inhibition and its end time,
  and stops hiding at its end time; an inhibition of another net or band hides nothing.
- **OA-MSAW-008** Outside the corridors the floor is the higher of terrain plus the general clearance and
  obstacle top plus its clearance; the chart altitude is no floor; no terrain or obstacle coverage gives no floor.
- **OA-MSAW-009** A final-approach corridor gives its floor only where the whole query circle is inside it:
  the higher of the surface and terrain or obstacle plus the approach clearance;
  elsewhere the normal floor applies.
- **OA-MSAW-010** An alert is a warning when the predicted violation is within the warning time, else a
  caution; a track already below its floor is a warning.
- **OA-MSAW-011** The net keeps one alert per track. A fully evaluated clear path ends the alert
  after hysteresis. If a path has missing floor coverage, the net keeps its open alert with a
  partial-coverage flag. Ending the run closes all open alerts.
- **OA-MSAW-012** An alert that does not fit the open table is logged at once and counted, and the net
  reads degraded.
- **OA-MSAW-013** What a full alert or skip log cannot hold is counted and the net reads degraded.
- **OA-MSAW-014** A cycle not after the previous one, or with floors that do not match the planned
  points, publishes the net unavailable.
- **OA-MSAW-015** Each track's path is sampled in a straight line from its velocity; a point's radius is
  the 95 % radius plus half the distance to the next point.
- **OA-MSAW-016** A point's floor is judged against the lowest predicted altitude over the span the point
  stands for, and the violation time lies inside that span.
- **OA-MSAW-017** The net report gives the reason of highest priority (budget, input) and the count
  of every shortfall of the cycle; rows told lost upstream degrade the next cycle.
- **OA-MSAW-018** An unknown vertical rate predicts level and is flagged.
- **OA-MSAW-019** A bad row is counted and listed; a 95 % radius is never shrunk: it is looked up as it is, an unusable one (NaN, negative) or one above 10 km takes 10 km (counted); one above 2 km is flagged `wide_uncertainty` on the track and its alerts and counted, and the net reads degraded when more than `wide_degrade_share` of the airborne tracks of a cycle are.
- **OA-MSAW-020** The altitude predicted at any time inside the look-ahead is finite for bounded inputs (Kani).
- **OA-MSAW-021** The floor is monotone in the terrain and the obstacle, so a higher floor never removes an alert (Kani).
- **OA-MSAW-022** The time of a predicted violation is clamped inside its span for bounded inputs (Kani).
- **OA-MSAW-023** A capacity above its bound is refused with the name of the key.
- **OA-MSAW-024** The same cycles give the same alerts.
- **OA-MSAW-042** In a final-approach corridor a normal approach does not alert and an early descent over
  a ridge does (by the surface, and by terrain plus the approach clearance alone); outside the corridor
  or straddling its edge the normal floor applies.
- **OA-MSAW-043** The corridor floor is never below terrain or obstacle plus the clearance the corridor hit carries (Kani).
- **OA-MSAW-044** No obstacle data is no coverage: the floor is unavailable and the net reads degraded.
- **OA-MSAW-045** A track flagged with no altimeter setting is counted, listed, evaluated and the net
  reads degraded (`InputMissing`).
- **OA-MSAW-046** A point's radius grows with its prediction time by the velocity uncertainty.

### The tool (tools/oa-msaw)

- **OA-MSAW-030** The arguments are strict; a sealed input, configuration, output, terrain, obstacle, chart or
  CIFP path exits 2 before anything is opened or created.
- **OA-MSAW-031** The shared guard refuses sealed folders, links into them and a link swapped after the check.
- **OA-MSAW-032** Every single-valued configuration key is required (no defaults), an unknown or repeated key or
  a value that does not parse is an error naming it, and the core's refusal names its key; corridors,
  CIFP airports and inhibitions are lines of the file, and the file labels its values as not FAA values.
- **OA-MSAW-033** Terrain is the conservative maximum under the point's circle in whole feet rounded up and
  absent where any sample is missing or nodata; obstacles are the highest top within the circle; the chart is
  the highest FUS3, FUS5 or TAV altitude of the sectors containing the point and absent outside them all.
- **OA-MSAW-034** A runway end with an approach gets a corridor from its threshold to the final approach fix
  (5 NM if the CIFP does not place it), a capture box of constant half-width `approach_half_width_nm` (1 NM), with
  the lowest fix altitude its approaches publish; an airport the file lacks is an error.
- **OA-MSAW-035** The output files are written whole or not at all and a failed run leaves none of an earlier run's.
- **OA-MSAW-036** Clip rows in a sealed window are dropped and counted, and the count is printed once on every exit.
- **OA-MSAW-037** Synthetic Guam case through the tracker: the early descent alerts at caution and then warning.
- **OA-MSAW-038** Synthetic Guam case with an inhibition over the area: no alert, the net reads inhibited for the
  run, and inhibitions.csv shows the inhibition with its end time, reason, author and what it hid; once it
  ends the alert comes and its expiry is recorded.
- **OA-MSAW-039** With no terrain coverage the tool reports degraded input and issues no alerts for
  the uncovered points. The tool counts and lists tracks without altitude and tracks over budget.
- **OA-MSAW-040** The files are deterministic and the summary names no aircraft.
- **OA-MSAW-047** Terrain is reduced once per tile to a grid of 0.002-degree cells and looked up from it; a
  missing neighbour and a nodata sample are counted apart, each tile is reduced alone (no common lattice is
  needed), and a tile is read once.
- **OA-MSAW-048** Below FL180 the altitude takes the altimeter setting of the nearest METAR station within the
  radius with an observation within the age; at and above FL180 it is used as it is.
- **OA-MSAW-049** With no usable setting the altitude is lowered by the allowance, the track is counted and
  listed (`QNH_MISSING`) and the net reads degraded.
- **OA-MSAW-050** The summary gives the one-off load times and peak memory, the CPU seconds per simulated second
  of the steady state and its split, the alerts per hour by grade, the worst floor kind, the alerts near an
  airport with an approach and a first classification of what the tracks were doing.
- **OA-MSAW-051** A coarse terrain lookup is never below the fine lookup over the same circle, and gives no
  answer where the fine lookup has none (dense bounded grids; the fine path is the oracle).
- **OA-MSAW-052** The chart raster holds, for each 0.002-degree cell, the highest minimum altitude of any sector
  touching the cell, and nothing where no sector touches it; never below the sector lookup.
- **OA-MSAW-053** A departure corridor and a runway corridor hold the whole-circle rule; a departure that climbs
  normally does not alert and one that levels off below terrain plus the departure clearance does; the chart
  does not apply in either.
- **OA-MSAW-054** The corridor grid index gives the answer of looking at every corridor.
- **OA-MSAW-055** A runway end with a SID, or the reverse of an approach end, has a departure corridor from its
  far end; each approach corridor continues along the runway.
- **OA-MSAW-056** A track is not alerted only when SFDPS has a VFR flight for it, or it has no flight, an ambiguous
  or an inactive one and its beacon code is a VFR code (1200 and the `vfr_beacon_code` lines); every other track is
  alerted, so the SFDPS lag never suppresses an alert; the VFR tracks are evaluated, written to `advisory.csv` and
  counted by reason.
- **OA-MSAW-057** A track predicted below the chart altitude and not below its alert floor is written to
  `below_mva.csv` and counted per hour; it is no alert, and a missing chart sector is counted in the summary only
  and is no shortfall of the net.
- **OA-MSAW-058** The clearance above terrain outside the corridors is `gtm_clearance_ft` (a development value),
  or that of a `gtm_clearance` box or polygon file holding the point (the highest wins); corridors keep their own.
- **OA-MSAW-059** Within `airport_area_nm` of an airport with a CIFP approach, below `airport_area_ceiling_ft_agl`,
  the clearance above terrain and above obstacles is `airport_area_clearance_ft`, where the whole query circle is
  inside the area; never an inhibit; the lowest of overlapping areas applies and a `gtm_clearance` line overrides it
  for terrain.
- **OA-MSAW-060** An approach corridor is a capture box of constant half-width `approach_half_width_nm` from the
  threshold to the fix (5 NM when absent or farther); a track outside the box falls to the general rules; the
  departure corridor widens to 2 NM.
- **OA-MSAW-062** The summary gives distributions of the remaining alerts: depth below the floor, time to violation,
  query radius, distance to the floor-setting cell, and alignment with a runway heading.
- **OA-MSAW-063** With `--diagnose` the tool writes `diagnosis.txt`: the altitude judged against the geometric one,
  the terms of the query radius, the obstacle and terrain records behind the floors and what the depth is made of,
  and the alerts 500 ft or more below the floor classified and grouped; numbers only, addresses hashed.
- **OA-MSAW-064** The prediction uses `terminal_lookahead_s` when the track starts inside an aligned
  approach capture box and outside a departure corridor. Otherwise it uses `look_ahead_s`. Both
  include the present position. When `warning_s` is at least `terminal_lookahead_s`, violations found
  on the shorter prediction are warnings.
- **OA-MSAW-065** A track whose latest barometric altitude is older than `stale_hold_s` is never planned for
  judging, is counted and listed as altitude stale on that track, and an alert open on it is held open as
  "protection lost: altitude stale" until the altitude returns or the track ends; the same for a missing altitude;
  an alert regained within the hysteresis continues its episode; the counts of tracks with protection lost and of
  those with an open alert are published whatever the share (Kani on the classification, the plan in a test). An
  unknown age is taken as fresh and counted.
- **OA-MSAW-066** The summary reports alert episodes (alerts of one track 20 s or less apart count as one) and the
  tracks with an alert beside the alert count; the alerts and the hysteresis are unchanged.
- **OA-MSAW-067** A `gtm_clearance` area's clearance is a lower bound of the terrain floor inside a corridor too;
  the airport area's is not.
- **OA-MSAW-068** At any age above zero a track is judged at `min(alt, alt + vrate x age)` (a descent, never a
  climb); past `max_altitude_age_s` it is flagged stale-extrapolated on the alert and the track, and a rate that is
  not known is carried as a descent of 1,000 ft/min, flagged; an alert open on it is held open.
- **OA-MSAW-069** The net reads InputMissing when more than `altitude_loss_degrade_share` of a cycle's airborne
  tracks are not monitored for altitude.
- **OA-MSAW-070** In the runway strip and last 1 NM of a capture box, the floor is the highest of the
  approach surface less its allowance, terrain and obstacle top. A predicted touchdown requires
  an aligned track whose 95 % circle fits within the pavement plus 60 m. It requires a known
  nonpositive vertical rate, no rise over the last 10 s, and altitude with a current altimeter
  setting and without stale extrapolation or cold-temperature correction. At that point, the
  predicted altitude must be at or below the runway elevation interpolated between the thresholds.
  The net omits the touchdown and subsequent rollout points from floor checks and counts the
  rollout points, including those outside every runway strip.
- **OA-MSAW-071** The airport area's clearance applies only where the whole query circle is inside the area; a
  `gtm_clearance` area applies where the circle overlaps it at all.
- **OA-MSAW-072** With no setting within the radius and age, the lowest setting of the last 12 h within 200 NM is
  used; only with none the allowance; the track stays flagged and the net InputMissing. `--metar` names a directory
  of hourly files, METAR files, or several of either separated by commas; each path is checked against the sealed days.
- **OA-MSAW-073** The tool estimates coverage for each loaded obstacle file on a 0.5 degree
  grid, then takes the union. Coverage includes cells with obstacles and enclosed empty
  cells with obstacle cells within four cells on both sides of their row or column.
  Empty cells with a path to the file's bounding edge through adjacent empty cells
  remain uncovered. The tool counts track rows outside coverage and rejects an empty
  obstacle file. `--dof` accepts a comma-separated list.
- **OA-MSAW-074** Tracks on the ground are listed per track; an altitude with no barometric measurement behind it is
  no altitude, and is counted apart.
- **OA-MSAW-075** An approach corridor's surface is its procedure's own path: the threshold crossing height over the
  threshold elevation rising at the lowest published vertical angle of the runway end, less `approach_allowance_ft`,
  at the circle's centre; where the CIFP gives neither, 3 degrees and 50 ft, and the runways so are counted. The
  angle is that of the final approach legs only (from the final approach fix, 2 to 6 degrees).
- **OA-MSAW-076** `--landing-of <alerts.csv>` counts the alerts of another run whose worst point is in the landing
  zone, by the corridors and geometry of this run; the diagnosis gives the landing alerts' depth, floor, height and
  distance, flown angle and altimeter, and how many alerts used the lowest-in-12-h setting.
- **OA-MSAW-077** Where a lookup circle is wholly inside surface-bearing corridors (approach corridors and runway
  strips) and no departure corridor reaches it, an obstacle whose top is not above the highest of their surfaces,
  less the allowance, at the obstacle's own position does not raise the floor, but only where the tangent of the
  corridors' path angle times that lookup's own radius is not above the allowance; elsewhere every obstacle does.
- **OA-MSAW-078** An altitude below FL180 with a setting is lowered by the ICAO Doc 8168 cold-temperature correction
  from the METAR temperature at the altimeter station, up to 5,000 ft above the station (never raised, flagged);
  with no station temperature, or higher than that, there is no correction, flagged.
- **OA-MSAW-079** The approach capture box rules (the terminal look-ahead, the approach surface as a floor, the
  obstacle-under-surface filter) apply only to a track whose ground track is within `capture_heading_deg` of the box's
  inbound course; a runway strip holds a track within it of either direction of the runway. Any other track, a
  reciprocal or crossing one or one too slow to have a ground track, gets the general rules (the full look-ahead,
  terrain and obstacle floors). Where boxes overlap a track takes those it is aligned with. The alert records the
  ground track and the corridors that held its worst point.
- **OA-MSAW-080** With `cifp_area_margin_nm` and `--cifp`, the approach capture box and runway strip of every runway
  end with a published instrument approach at every airport within the area of the run's rows, widened by the margin,
  are built from the CIFP; the airports of the `cifp_airport` lines keep their own, with departure corridors. The
  summary reports the airports, boxes, strips, build time and memory.
- **OA-MSAW-081** A floor lookup is made at the point itself over its own radius, with no cell and no radius bucket.
- **OA-MSAW-082** `--rows-of <8 hex digits of an address hash>` writes `rows_of.csv`: that aircraft's clip rows
  (barometric and geometric altitude, vertical rate and its kind), its measurements and the tracker's rows.
- **OA-MSAW-041** The 95 % radius of a covariance is the chi-square radius of its larger eigenvalue, and the
  ground codes of the core are the close-encounter record's.

### Offline encounter evaluation (tools/oa-msaw)

- **OA-MSAW-200** The offline evaluator compares independently supplied same-address hazard windows
  with a caller-declared complete alert log. Each accepted encounter has exactly one timely, late
  or missed warning-onset outcome, and these counts sum to the encounter denominator. This is a
  synthetic development measure, not operational qualification.
- **OA-MSAW-201** A warning onset inside the window at its deadline is timely; after the deadline
  through the inclusive end is late. A compressed episode with a first warning before the window
  and an end inside or after it does not prove continuing warning: the evaluator refuses an
  ambiguous outcome unless an independent in-window onset already establishes timely credit.
  Neither the episode end nor a loss observation extends warning evidence.
- **OA-MSAW-202** Nuisance records are supplied alert records whose inclusive alert interval
  overlaps no full-address hazard window; the alert record count is reported as their denominator.
- **OA-MSAW-203** Terminal lost-altitude records and records with a recorded loss observation
  inside a same-address hazard window are counted separately. A compressed record's `protect_ns`
  is a point observation; it does not establish an interval or every protection transition.
- **OA-MSAW-204** An absent evidence source is unavailable, not an empty log. A supplied empty
  source has zero rows; metrics requiring both sources remain absent unless both are supplied.
- **OA-MSAW-205** Evaluation refuses malformed intervals and alert transitions, unknown terminal
  reasons, inconsistent protection timestamps, overlapping same-address truth windows, more than
  100,000 rows per source, or aggregate pairwise work over 10,000,000 comparisons. Work is checked
  with overflow-safe arithmetic before pairwise processing, charging all three hazard-alert
  passes and all unordered hazard-window comparisons. Accepted counts are independent of row order.

## Runway geometry and occupancy (oa-runway)

- **OA-RWY-001** An airport's local east-north plane is a tangent plane about its reference point with the WGS-84
  local scales and a curvature term. It is defined on the disc within 20 km of the origin, for an origin up to 80
  degrees of latitude; a point outside the disc, a non-finite point or an origin outside those limits gives no
  position. On the disc a point's distance from the origin differs from the geodesic distance by at most 1.5 m, the
  plane error bound: test `plane_error_against_geodesic_is_within_the_stated_bound`, 11 latitudes to 80 degrees, 72
  bearings, out to the edge (worst 1.05 m, at 80 degrees and 20 km). The distance between two points on the disc
  differs from the geodesic by at most twice the bound: test
  `plane_distance_between_two_points_against_geodesic_is_within_twice_the_bound`, 2,800 random pairs (worst 1.90 m).
  These are measured bounds on the cases tested, not proved ones.
- **OA-RWY-002** A runway is built from the published length, width, alignment, end coordinates, displaced-threshold
  coordinates and declared distances. It keeps each physical end and each landing threshold apart, and the rectangle is
  the one between the physical ends at the published width (tests on KMDW 04L/22R, displaced at both ends, and on KAUS
  18L/36R and KSFO 10L/28R, from the NASR 2026-10-29 values).
- **OA-RWY-003** Per OA-ASP-067, an end with a displacement and no NASR displaced coordinate takes its landing
  threshold from the CIFP record and is marked as such; with no CIFP threshold the runway is refused. Where NASR has
  no end coordinate, the CIFP threshold is taken as the physical end only when the published displacement is exactly
  zero; with an unknown displacement the runway is refused. An end with both a NASR displaced coordinate and a CIFP
  threshold is refused when they lie more than 30 m apart, because one of them is wrong and nothing here can tell which.
- **OA-RWY-004** A runway is refused, and counted under its own one of 16 reasons, when its width is missing, zero or
  over 500 ft, its length is missing, an end name is bad, a displacement is negative or not finite, a physical end or
  landing threshold is missing, a displaced coordinate has no displacement length to check it against, a coordinate is
  invalid or outside the plane, or the ends are under 1 m apart. Nothing is guessed.
- **OA-RWY-005** A runway is refused when the published length and the distance between the physical ends differ by
  more than 60 m, the published alignment and the azimuth between the ends by more than 2 degrees, a published
  displacement and the measured one by more than 30 m, or a threshold lies more than 15 m off the line between the
  ends or outside the runway (tests include a threshold past the far end, and one 25 m behind the near end).
- **OA-RWY-006** The occupancy regions are the rectangle, an approach area for each end measured outward from the
  landing threshold (1,500 m long, 100 m wider than the runway on each side) and a departure area beyond each physical
  end (1,000 m long, 50 m wider on each side). These dimensions are development values, not FAA values. The test
  `every_edge_of_every_region_at_the_interval_ends` probes each of the four edges of all five regions on the edge,
  inside, and outside by the margin and just past it, and the KMDW test checks the hand-worked interval ends.
- **OA-RWY-007** `occupies` reports whether a target touches the rectangle, either approach area or either departure
  area, with its position widened by its 95 % radius plus the 1.5 m plane error bound and a 1e-6 m rounding pad. A
  non-finite position or radius, negative radius, coordinate beyond 1e9 m, non-finite runway geometry,
  or inconsistent published and derived runway widths counts as occupying every region and is marked unknown.
  Every comparison is written so that a NaN reads as touching. Tests `unknown_position_or_radius_occupies_everything`,
  `a_runway_with_a_non_finite_number_never_clears_a_target`,
  `a_torn_finite_width_change_is_unknown_while_a_coherent_geometry_change_can_clear` and
  `real_coordinates_run_end_to_end_through_the_plane_and_the_test` (KMDW, from latitude and longitude).
- **OA-RWY-008** Raising the radius never removes a region from the result. Tested by a sweep of radii at six
  positions (`a_larger_radius_only_adds_occupancy`) and by OA-RWY-009.
- **OA-RWY-009** For finite radii 0 <= r1 <= r2, a target that occupies a region at r1 occupies it at r2,
  and the runway-axis coordinates do not depend on the radius. Tested, not proved: on 30,000 random runways at
  any heading, positions within 1e9 m and radii up to 1e9 m, through `occupies` itself
  (`a_larger_radius_never_clears_a_region_for_random_runways_positions_and_radii`).
- **OA-RWY-010** A non-finite, negative or beyond-1e9 m position or radius occupies every region and is marked
  unknown, and so does every position and radius on a runway with any non-finite number. Tested, not proved: at
  the edge values, and on 30,000 random runways with a position, a radius or one of the runway's 16 numbers set to
  NaN, infinity or beyond the limit
  (`a_position_radius_or_runway_that_is_not_known_occupies_every_region_for_random_runways`).
- **OA-RWY-011** For every runway of the kind `build_runway` makes and every finite radius of at least zero, a
  target whose coordinates in the runway axes lie inside the rectangle touches it. Tested, not proved: 1e-6 m
  inside the edges and on the edges themselves with a radius above zero
  (`a_target_inside_the_runway_rectangle_touches_it_at_every_radius`), and on 30,000 random runways
  (`a_target_inside_a_random_runway_rectangle_touches_it_at_every_radius`).
- **OA-RWY-012** For every runway of the kind `build_runway` makes (finite geometry within 1e9 m, axis components
  within [-1, 1]) and every position and radius, any `f64` bit pattern included, `occupies` returns without panic,
  and for a known position the coordinates in the runway axes are finite and under 1e10 m. Proved with Kani
  (`occupies_never_panics`). The proof checks panics and those assertions; it does not run a separate float-overflow
  check.
- **OA-RWY-013** The conversion from degrees to the local plane gives no position for a non-finite input, and any
  position it gives is finite, within 20 km on each axis and inside the extent's disc. Tested, not proved: on
  30,000 random origins and points, up to `f64::MAX`, NaN and infinity included
  (`a_point_in_the_local_plane_is_finite_and_inside_the_extent_or_refused_for_random_origins_and_points`).
- **OA-RWY-014** A final-monitor pair can be adapted from two distinct built runway geometries by extending each
  selected runway end's centerline by a bounded configured distance. The resulting midpoint NTZ requires inbound
  courses within 3 degrees, consistent-side geometry, adequate configured minimum separation, and a zone narrower
  than course separation. The centerline extension is an engineering geometry endpoint, not a published FAF or
  procedure fix. The 2,000 ft NTZ and 3,100 ft minimum-separation defaults are configurable profile values, not
  universal minima or facility authorization. Tested by `kord_parallel_pair_uses_public_runway_geometry`,
  `pair_uses_endpoints_and_midway_ntz_default`, and `close_parallel_finals_are_refused`.
- **OA-RWY-015** On a due prediction update, a track inside the configured NTZ and between the outer-fix and
  threshold limits is a warning; a predicted linear
  entry within the configured lookahead is a caution; a track remaining on its own course is clear. The default
  cadence is 1.2 s and default caution horizon is 10 s. These are this monitor's engineering policy. Tested by
  `on_course_clear` and `drift_caution_then_warning`.
- **OA-RWY-016** Prediction evaluation consumes bounded tracker-prediction columns, validates them as a complete
  set before output mutation, and identifies alerts by stable runway/end and track identifiers. At most 256 rows
  are accepted; course length is 1–40 km, NTZ half-width is positive and at most 3,048 m, local origin/positions
  stay within the 20 km frame, speed is at most 4,088 kt (the accepted reported-speed envelope), R95 is 0–20 km,
  and lookahead is at most 60 s. Invalid
  input, over-cap work or insufficient output capacity is
  refused atomically, leaving monitor state and output arrays unchanged. A due call performs four bounded linear
  row passes without allocation; output capacity is checked before the evaluation pass and no row can create more
  than one alert. Millisecond entry times are range-checked and never saturate. A 30-degree inward blunder that
  reaches the NTZ within the prediction horizon is cautioned. Tested by
  `thirty_degree_blunder_caution_within_prediction`,
  `a_750_mps_predicted_track_remains_inside_the_accepted_speed_domain`,
  `finite_extreme_pair_and_prediction_values_refuse_without_false_clear_or_mutation`,
  `excessive_lookahead_cannot_saturate_a_caller_visible_entry_time` and
  `track_and_output_caps_refuse_atomically`.
- **OA-RWY-017** A prediction report at least 4.8 s old sets the degraded output flag; the 4.8 s value is a
  development threshold, not a statement of facility adaptation. Tested by `stale_prediction_sets_degraded`.
- **OA-RWY-018** A parallel-final pair below its configured minimum separation is refused before evaluation.
  Tested by `close_parallel_finals_are_refused`.
- **OA-RWY-019** A track beyond either longitudinal end of the final-monitor segment is not warned for NTZ overlap,
  and a predicted entry that occurs only after passing the threshold end is not cautioned. Tested by
  `warning_is_limited_to_the_final_segment_endpoints` and `predicted_zone_entry_beyond_final_endpoint_is_clear`.
- **OA-RWY-020** The ARV core evaluates caller-supplied predicted track positions against the assigned runway and
  candidate runway/taxiway centerlines, using a configured lateral tolerance and heading tolerance within 5 NM of
  each candidate threshold. A match to the assigned runway is clear; an aligned match to another parallel runway or
  taxiway is reported distinctly. A latched alert clears only when the caller supplies an authenticated go-around
  event. The core neither authenticates that event nor claims an operational controller warning. Tested by
  `assigned_runway_match_is_clear_and_keeps_full_identity`, `parallel_runway_and_taxiway_mismatches_alert`,
  `only_authenticated_go_around_clears_latched_alert` and
  `threshold_range_and_heading_tolerances_bound_candidate_matches`.
- **OA-RWY-021** ARV consumes complete bounded structure-of-arrays inputs for at most 256 unique full flight
  identities and 512 candidate paths with strictly increasing path IDs. It refuses missing assignments, malformed
  columns and invalid geometry before output mutation, and preserves all 36 bytes of identity in output. A checked
  conservative work bound counts flight/path comparisons, identity validation, path ordering and assignment search;
  calls over 200,000 work units are refused atomically. Tested by `assigned_runway_match_is_clear_and_keeps_full_identity`,
  `invalid_late_row_and_duplicate_identity_leave_output_unchanged`,
  `malformed_and_over_capacity_inputs_refuse_before_mutation`,
  `maximum_capacities_fit_checked_work_budget` and `candidate_path_ids_must_be_strictly_increasing`.

## Feed damage check (oa-feed-gaps, P-006 section 6 and P-007)

- **OA-GAP-001** An interval between consecutive times of a feed over 600 s is an excluded interval (P-007), not damage by itself; exactly 600 s is not over.
- **OA-GAP-002** The interval from the window start to the first time, and from the last time to the window end, counts.
- **OA-GAP-003** A window with no message in a feed is damaged.
- **OA-GAP-004** At most 1,000 intervals over 600 s are listed; the rest are counted and make the window damaged.
- **OA-GAP-005** Any reader gzip or frame error count, and any ADS-B damaged-frame count, makes the window damaged.
- **OA-GAP-006** A time earlier than the one before it is counted, left out of the intervals, and makes the window damaged.
- **OA-GAP-007** SFDPS times are the recorder's receive times, sorted, so files that overlap or are named out of order give the true intervals.
- **OA-GAP-008** A frame the reader finds damaged is counted by kind, never skipped silently.
- **OA-GAP-009** ADS-B report times are sorted before the intervals are taken.
- **OA-GAP-010** Exit 0 clean, 1 damaged, 2 usage or read error; the report is deterministic and written by temporary file and rename.
- **OA-GAP-011** Both inputs go through the shared sealed guard (OA-SEAL-001); a refusal is exit 2.
- **OA-GAP-012** `--from` and `--to` are `YYYY-MM-DDTHH:MM:SSZ`, both required, from before to.
- **OA-GAP-013** An excluded interval is the interval over 600 s and the 300 s after it, in either feed, cut at the window end; each merged block is written as `exclude <from ISO ns> <to ISO ns>`.
- **OA-GAP-014** Excluded intervals are merged where they overlap; more than 3600 s in total makes the window damaged.
- **OA-GAP-015** `--clip-log` is required. A refused trace, not exactly one summary line and one `archive:` line, or an archive not reported complete makes the window damaged.
- **OA-GAP-016** The clip log's `kept` must equal the reports decoded from the clip; a mismatch is damage, with both numbers printed.
- **OA-GAP-017** A regular file in the SFDPS directory that is not `.oaf` or `.oaf.gz` is a read error (exit 2) that names it; the report names the files used.
- **OA-GAP-018** An `exclude` line parses back to the same nanosecond window under the whole-second ISO grammar of `oa_adsb::cli::parse_iso` plus a nine-digit fraction. The scorer's own parser of these lines is tested on its side, not here.
- **OA-GAP-019** An existing `--out` is refused (exit 2); the report is written to a new temporary file, synced, then renamed.

## Raw scope replay restart witness (oa-scope-replay)

- **OA-SCP-200** The opt-in raw-only replay CLI writes and verifies a versioned restart witness
  binding exact adaptation and clip bytes, declared receipt basis, effective render/raw settings,
  frame count, and ordered frame indexes, nanosecond timestamps and pixel hashes. Verification
  checks the bounded witness checksum and metadata before creating output, then compares each
  recomputed frame before publishing it; a mismatch refuses further frames and identifies that
  earlier matching frames may already exist. A successful restart reproduces the same ordered
  hashes. The witness is limited to 1,000,000 bytes and 10,000 frames; unknown fields, partial
  lists and incorrect times are refused. Lossy replay cannot produce or pass a witness even with
  `--allow-loss`. Output never overwrites an existing witness or partial: a recovery copy is
  synced under an exclusive partial name before a distinct final file is exclusively created
  and synced. The complete recovery copy remains after success; publication deletes neither
  file. Interrupted final publication may leave an incomplete final name, which strict
  verification refuses; the complete partial can be explicitly selected for verification. No
  hard-link exception weakens the shared sealed-data guard. SHA-256 is integrity evidence, not
  authentication. Replay starts from the beginning;
  this feature neither restores process state nor provides supervision or failover.

## OpenATCA Core (offline surveillance replay)

- **OA-CORE-001** Replaying a recorded ADS-B clip uses measurement time, preserves tracker row times
  and coasting flags, and produces the same update stream for the same clip and settings.
- **OA-CORE-002** A clip above the configured byte or report-count limit is refused. A truncated
  frame counts as damaged while earlier complete frames remain available for replay.
- **OA-CORE-003** Output rows refused by the configured replay buffer are counted in cumulative
  frame health, and each frame returns at most the configured number of rows.
- **OA-CORE-004** Offline replay takes its tracker cadence settings from the measured report
  intervals when available, using the same configuration logic as scope replay.

## Mode S / 1090ES (initial decoder slice)

- **OA-MODE-001** `oa-modes` accepts exactly one 14-octet long frame and supports only DF17 and
  DF18; every other input length or downlink format returns a typed refusal.
- **OA-MODE-002** A DF17/DF18 frame is admitted only when the 24-bit extended-squitter parity
  check over all 112 bits has zero syndrome. The decoder neither corrects bits nor derives an
  address from parity.
- **OA-MODE-003** The returned raw frame is byte-identical to input. The announced address is
  copied from the AA field; DF17 and DF18 CF=0 classify as ICAO-24, DF18 CF=1 as non-ICAO, CF=2,
  3, 4 and 5 as TIS-B, and CF=6/7 as unclassified. CF=6's body uses the DF17 message formats and
  is decoded under OA-MODE-004; its AA field is not promoted to ICAO-24 by this decoder. This is
  not address registration validation.
- **OA-MODE-004** For DF17 and DF18 CF=0 or CF=6, type codes 1–4 decode category and eight
  supported IA-5 subset characters. Eight zero codes mean absent identification; an unsupported
  character code is a typed error. Unimplemented body formats remain available as raw bytes with
  their type code. DF18 CF=4 is identified as reserved TIS-B management, but its body remains
  uninterpreted.
- **OA-MODE-005** The first slice does not demodulate pulses, decode other DFs, perform error
  correction, reconstruct CPR position or join messages. No measurement time or position is
  invented.
- **OA-MODE-006** This implementation is an experimental partial decoder, not a claim of
  DO-260B conformance or certification. Synthetic vectors are independent of recorded/SWIM data.
- **OA-MODE-007** For DF17 and DF18 CF=0 or CF=6 airborne position messages (TC 9–18), an all-zero
  12-bit altitude subfield is reported as unavailable. With Q=1, the decoder extracts N from the
  11 altitude bits and reports the encoded value as `25*N - 1000` feet, including the code range
  endpoints -1000 and 50175 ft. This one-frame API does not establish the altitude's datum/source.
  Q=0 (100-foot Mode C/Gillham encoding) is retained raw and explicitly marked unsupported; no
  altitude is fabricated. TC 20–22 and other CF families remain uninterpreted for altitude.
- **OA-MODE-008** For DF17 and DF18 CF=0 or CF=6 position messages, the decoder returns the raw
  CPR format flag and 17-bit latitude/longitude fields for surface TC 5–8 and airborne TC 9–18.
  These are encoded fields, not coordinates. The decoder does not pair even/odd frames, apply a
  local reference, infer a timestamp, or reconstruct a position.
## Provisional OpenATCA Core bindings

- **OA-FFI-001** The experimental C ABI exports the version query, WGS-84 geodesic/rhumb/separation and
  polygon operations, source-defined scalar math helpers, one-shot SHA-256, bounded status-message copy,
  and bounded one-shot XML, JSON, and CSV operations over existing `oa-*` APIs. Text parser errors retain
  their typed kind and source position; count queries and short capacities do not write output arrays. Message
  queries report the NUL-inclusive required size and never partially overwrite the destination. Fixed-width status/error shapes, argument
  locations (including the raw `location` field when `location_kind` is none), and all-or-nothing scalar outputs match the hand-written header; pointer validity, alignment,
  extent and non-overlap are documented caller preconditions, never arbitrary-address validation claims.
- **OA-FFI-002** Rust, C and pure-Python `ctypes` source tests consume the same committed synthetic
  geodesic, rhumb, polygon, scalar-math, SHA-256, XML, JSON, and CSV vectors. Importing Python loads no library; native loading
  requires an explicit absolute path. The ABI remains Preview and unvalidated until independent review and
  cross-platform gates pass.
- **OA-FFI-003** The C and Python L0 source runners cover every exported L0 operation family against the
  shared committed vectors, including polygon handle lifetime operations, bounded one-shot hashing, and the
  text parser capacity/error contract. Source
  runner presence does not establish that a native build or run succeeded; the coordinator records those gates.

## Reproducible ZAU comparison package (p002 `--export-eram`, `--eram-export`, tools/reproduce_zau.sh)

- **OA-REP-001** `p002 --export-eram FILE --sfdps-dir DIR` writes the ERAM data the scorer reads and nothing else:
  per flight in order the address, address hint, normalised callsign and sample count, and per sample of a flight
  with a key its time, position, altitude, velocity and coast flag, as the versioned text format `oa-eram-export 1`;
  a flight with no key is its sample count only. FILE and its `FILE.sha256` manifest are written together or not at all.
- **OA-REP-002** The reader refuses a file whose own digest, counts or records are not exactly what the writer writes.
- **OA-REP-003** A manifest line is `sha256sum` form; a file is checked against exactly one line naming it, and a
  mismatch, a missing line or a repeated line is refused.
- **OA-REP-004** An export is never written or read with a sample inside a sealed window, and every path the export
  and scoring modes take (recording, export, drop list, output) goes through the sealed guard first.
- **OA-REP-005** `p002 --eram-export FILE` replaces `--sfdps-dir` (exactly one) in a development run only; its output
  files are byte for byte those of the run on the recording. The run's own timings and memory go to standard error, not
  into `p002.txt`.
- **OA-REP-006** `--drop-addresses FILE` (one 24-bit hex address per line) removes every flight whose address or
  address hint is listed, with its samples, and a run that removes any says how many in its loader notes.
- **OA-REP-007** The export's summary counts flights by key, samples listed and counted only, coast samples and
  distinct addresses.
- **OA-REP-008** `tools/reproduce_zau.sh DAY DATA_DIR OUT_DIR` checks every input against the package manifest,
  refuses a tree that is not at the manifest's commit, runs the clip tool and the scorer, and prints PASS only when
  every output hash matches. The format-1 manifest contains one release-summary URL for its first ADS-B part and an
  exact release-download URL bound to each part's validated `v2026.MM.DD-planes-readsb-prod-N.tar.aa` name; all
  part dates must be real calendar dates equal to the package UTC day, while distinct same-day `prodN` tags are allowed.
- **OA-REP-009** The development export/package reader refuses manifests above 64 KiB or 512 lines, duplicate scalar
  or unknown fields, more than 16 archive parts, a part above 8 GiB, more than 64 GiB total archive bytes, and
  compressed expansion above the smaller of the declared size and 512 MiB. The export is limited to 512 MiB,
  8,000,000 rows, 100,000 flights and 16 KiB per line; its sidecar is limited to 4 KiB and drop list to 1 MiB.
  XZ decoding is one stream, with 128 MiB decoder memory, 64 KiB compressed reads and 1 MiB output steps;
  truncation, trailing bytes or a second stream are refused. Reads and parsing remain bounded even when a manifest advertises a larger value.
- **OA-REP-010** `--eram-export` is accepted only with explicit `--development` and without `--damage-report`;
  `--allow-dirty` alone is insufficient. The clip argument manifest accepts only the finite allowlist and cannot
  inject shell syntax or output-changing options. Package names cannot alias the wrapper's files. The reproduction
  instructions require a coordinator-verified canonical release inventory and exact part completeness before use;
  the wrapper's manifest cannot prove that an omitted release part does not exist. Raw exports and outputs remain
  private; the procedure is development-only and makes no runtime safety or complete-traffic claim.

## Empirical ZAU sector occupancy (oa-sector-reconstruct; development-only G3 evidence)

- **OA-G3-001** The tool accepts only the complete pinned `H2-OBSERVED-STATE-1` header with
  `scope=SFDPS-replay-compatibility`, `independent_predictions=not-measured`, and
  `independent_agreement_denominator=not-observable`, plus the complete H2 CONFIG/BUILD/event/
  RESULT/COUNTS/INPUT envelope. Unknown records/fields or schema changes refuse before aggregation.
  It reads no raw SFDPS recordings and never treats a replayed owner, handoff, or flight-data field
  as an independent controller-action label.
- **OA-G3-002** Training and check partitions are fixed to 2026-11-15 and 2026-11-19 UTC,
  respectively. The shared `oa-sealed` path guard runs before each open; the complete bounded
  trace is pre-scanned for source, position/state, operation, and receive timestamps before
  aggregation, and every present timestamp must be outside a sealed window and inside the declared
  UTC partition. ROW source clock fields are required by schema but may explicitly be `none`.
- **OA-G3-003** A spatial label requires positive same-row identity: the exact zero-padded four-byte
  ZAU centre encoding, a non-unknown zero-padded controlling-owner unit/sector value, and a full GUFI. Handoff
  transferring/receiving/accepting units are separately counted observations and never substitute
  for the controlling-owner field. Unknown identity abstains.
- **OA-G3-004** The frozen empirical grid uses 0.05-degree global WGS-84 cells, 2,000-ft slabs
  of reported altitude, and 15-minute bins of position/state time. State time and feed receive time
  remain distinct and both coverage spans are reported. Missing reported altitude is counted and
  remains missing; assigned altitude is not a substitute.
- **OA-G3-005** A cell/altitude/owner label is `supported` only with at least three distinct GUFIs
  and two distinct state-time bins; competing labels meeting the same minima are `contested` and
  all other observed labels are `unknown`. The result is empirical occupancy support, not a
  probability, confidence of official-boundary correctness, official geometry, or autonomous-policy
  agreement. No hull, interpolation, smoothing, carry-forward, or outside-sector inference is
  used; no official coverage mask is claimed.
- **OA-G3-006** Fixed limits bound input bytes, line bytes, total records, row records, unique
  evidence keys, model bytes, and report bytes. Source receipts use the full
  `(stream, decoded_offset, recorder_sequence, row)` identity. Exact repeats, conflicting source
  receipts, conflicting GUFI/state-time owners, unknown identities, missing/invalid fields,
  missing/out-of-range altitude, sparse support, and unmatched check labels are separate named
  dispositions. FRAME identity is unique by `(stream, decoded_offset)` and its sequence must join
  to each ROW/OP source identity. Each OP joins to a ROW, has a unique per-row operation index, and
  per-row declared operation counts must equal contiguous observed indices. Future source/state
  clocks relative to receive time are counted; affected ROWs abstain from occupancy without
  claiming causal synchronization. The row disposition counters conserve every ROW input; any cap
  or conservation failure is `INCOMPLETE` and creates no report.
- **OA-G3-007** Training and check are separate artifacts; the check reader cannot modify the
  model. Artifacts carry input, policy, binary, and training-model SHA-256 values plus frozen
  schema/partition metadata and an optional embedded source revision, contain only
  aggregate cell support and handoff counts, and are written exclusively under an explicit
  canonical private root with mode 0600 and no overwrite. Check requires an identical G3 binary
  digest and source revision to training. Reproducibility is limited to the exact trace and G3
  binary; no H2 source reconstruction claim is made. The output reports coverage denominators and
  sparse abstentions; a zero eligible check denominator is not a pass. Synthetic tests use
  fabricated source rows only.

## OpenATCA Core conformance vectors (`oacv`)

- **OA-VEC-001** Every checked-in vector is a synthetic, spec-derived case with explicit CC0-1.0
  provenance; the corpus is separate from product crates and contains no FAA/SWIM or operational
  data. The current 18-case corpus includes two SHA-256 known answers; WGS-84 geodesic inverse,
  direct and rhumb inverse positives and typed refusals; and selected ASTERIX CAT021 Ed 2.4/2.7,
  CAT048 Ed 1.32 and CAT062 Ed 1.21 decode projections with typed refusal cases. This is a partial,
  synthetic corpus, not complete module coverage.
- **OA-VEC-002** The standalone Rust loader enforces the versioned schema, exact fields, duplicate
  key rejection, safe relative paths, bounded reads/JSON/work, case and member SHA-256/size
  bindings, symlink refusal, and exact closed file inventory for the immutable checked-in corpus.
  It is not a defense against concurrent hostile filesystem mutation. Every refusal stops
  validation; no malformed or unlisted case is skipped.
- **OA-VEC-003** The Rust checker executes two SHA-256 known answers, selected WGS-84 geodesic
  inverse/direct and rhumb inverse cases, and selected ASTERIX CAT021 Ed 2.4/2.7, CAT048 Ed 1.32 and
  CAT062 Ed 1.21 decode projections. It checks typed geodesy and ASTERIX refusals, named geodesy
  decimal references with stated tolerances/units, and only the ASTERIX fields named by each
  projection. Direct coverage is limited to zero distance; ASTERIX and other module coverage is
  partial. This profile makes no C/Python parity, complete module coverage, conformance, or Stable
  claim.

- **OA-VEC-004** CI tests and runs the standalone checker offline with `--locked` on the x86-64 and
  aarch64 jobs already in the workflow, against the committed corpus and checked-in standalone
  path lockfile; no lockfile is generated by a clean-checkout gate. It adds no external crypto,
  JSON, or geodesy dependency and does not copy the checker into a product crate.
## I1/I2 — Read-only Tech Ops evidence core (oa-techops)

This core is a pure bounded comparator for normalized status evidence. It does not collect live
telemetry, authenticate reporters, expose a GUI or control interface, run PVER stimuli, create
certification records, or certify service. Identities and limits are compared as caller-supplied
claims; a match is explicitly unverified.

- **OA-TECHOPS-001** The expected processor inventory is bounded and duplicate expected identities
  are refused. Every expected processor has a result, including absent processors; unexpected
  report identities are counted separately and never added to the expected denominator. Epoch
  history is an explicit input: absent history is `INCOMPLETE`, and a regressed epoch is named as a
  mismatch.
- **OA-TECHOPS-002** Each expected processor and performance window supplies an independent
  half-open sequence interval. The evaluator reports expected, unique, missing and duplicate
  sequence counts, plus out-of-order sequence/time, outside-window, stale, future and telemetry-loss
  findings. It does not infer an expected population from received rows.
- **OA-TECHOPS-003** Source, epoch, clock domain, release-manifest, firmware, running-image, loaded
  adaptation and on-disk adaptation claims are compared exactly. A mismatch remains named and
  counted. Even complete equality yields `ClaimsMatchUnverified`, never `PASS`, because this core
  has no authenticated evidence or profile authority.
- **OA-TECHOPS-004** A metric window retains its profile/metric/processor/epoch/clock identity,
  expected sample denominator, received/unique/missing counts, freshness and loss findings, observed
  maximum and caller limit. Missing or invalid evidence prevents a complete result; a complete
  reported exceedance is labeled unverified.
- **OA-TECHOPS-005** Inventory, report-row, metric-row and per-window sequence caps are checked
  before scanning or bitmap initialization. Invalid time/window contracts, duplicate expected IDs,
  over-capacity inputs and arithmetic overflow return explicit refusal errors without partial PASS.
- **OA-TECHOPS-006** UTC/GPS conversion uses the IERS positive-leap history, represents the
  2016-12-31 leap second distinctly, and refuses invalid leap labels. Daily clock comparison takes
  processor UTC samples, GPS-derived reference samples, the UTC day and alarm limit as explicit
  inputs; it reports missing, duplicate, unexpected, unavailable and invalid evidence and never
  reads a host clock or treats missing reference data as zero offset. Caller-supplied reference age
  is checked against an explicit freshness bound. `parse_ubx_nav_timegps` accepts one bounded
  UBX-NAV-TIMEGPS frame, verifies checksum and validity fields, checks the reported leap value
  against the pinned IERS table, and returns a GPS reference for the same comparison API; it does
  not authenticate the receiver or prove live source provenance. Conversion refuses beyond the
  published leap table's confirmed horizon. Results remain unverified claims about supplied sources.

## L23 I2 — Monitor/control model (oa-techops)

This bounded model consumes plain values prepared by adapters. It does not collect telemetry,
authenticate users or reports, persist an audit log, render a display, or connect to an RMM service.
The L13 role registry is not yet landed; roles below are plain caller-supplied categories only.

- **OA-L23-001** A bounded service/processor snapshot uses `(kind, parent_service_id, resource_id)`
  as the complete identity and preserves each resource's parent,
  service, reported state, reason, timestamp and clock domain. Fresh values retain `Up`, `Degraded`,
  `Down` or `Unknown`; stale, future or incomparable timestamps become `Unknown` with an explicit
  stale/clock reason and are never converted to `Down`. A view reports bounded total, unknown and
  degraded counts. `inventory_resource_inputs` converts an actual `assess_inventory` result into
  these values, preserving the service evaluation time and each processor's newest valid sample
  time in the assessment clock domain; missing or incomparable times remain Unknown. Numeric IDs
  may be shared across resource kinds, while a duplicate full identity is refused before output is
  written. Absence is Unknown.
  The `oa-safety::NetStatus` shell adapter is not integrated because the regional-session →
  oa-techops F1 link is still pending; this crate does not claim that safety-net wiring is complete.
- **OA-L23-002** Alarm instances bind the full resource identity `(kind, parent_service_id,
  resource_id)` and preserve severity, raise time, clear time and acknowledgement time
  in one explicit caller-supplied monotonic clock domain. A repeated active observation for the same
  instance does not raise it again; clear is idempotent, and a cleared instance cannot be re-raised.
  Operate and Configure categories may acknowledge an active alarm instance once. Clear before
  raise, clear before acknowledgement, acknowledgement before raise, or a clock-domain mismatch
  is refused without changing the alarm record.
- **OA-L23-003** The plain role enum is `Monitor`, `Operate`, or `Configure`: all three can view,
  Operate and Configure can acknowledge, and only Configure can validate threshold changes.
  Threshold configuration is bounded to p99 latency limits from 1 ms through 60 s and tracks-per-scan
  limits from 1 through 1,000,000. A plain event-intent value can be produced for adapter-side audit
  wiring; it is not authenticated or durable evidence.
- **OA-L23-004** A B3 latency claim retains caller-supplied p50, p95, p99, maximum, tracks per scan,
  sample time and clock domain. The model rejects unordered percentiles, stale or incomparable
  samples, and claims exceeding configured p99 or tracks-per-scan limits. Claims remain unverified;
  no latency, track count, clock, or authority is synthesized.
- **OA-L23-005** Resource and alarm input/output state are fixed-capacity and refuse excess work
  before partial status. The respective caps are 256 resources and 256 alarm instances. No display or
  Remote Maintenance Monitoring integration is implemented; RMM remains UNMET.

## H2 normalized migration-shadow event comparison (oa-migration-shadow)

- **OA-MIG-401** The pure bounded comparator accepts versioned normalized streams for handoffs, point-outs, sector assignments, flight-data amendments and alerts. It binds matching function, scope and half-open UTC window, preserves source-event and observed/receive clocks independently, requires explicit per-family coverage and source epoch hashes, compares stable caller-supplied business keys without replacing side-local source row identities, and uses the frozen `M4-H2-EXACT-1` policy (0 ns time margin and exact numeric values). `FIELD_PROJECTION_VERSION` 1 requires every family-defined operational field ID on each observed pair; explicit absent is distinct from an omitted field, and omitted projection fields are unobservable even when both sides omit them. Missing/invalid/ambiguous key, missing time, invalid or unsupported records, outside-window rows, duplicate keys, out-of-order event times, unknown/absent fields, mismatches, one-sided records and complete-empty families remain explicit and bounded; output capacity and record/field caps are checked before work. A complete-empty family is not evidence of a matched action.
- **OA-MIG-402** The comparator does not generate an ERAM reference, infer controller intent, or treat SFDPS observed-state replay as independent action ground truth. Missing legacy alert observations are unobservable, never agreement. Exact pairs alone default to equivalent; every mismatch/unobservable result defaults unresolved and blocks. A `better` or `worse` judgment must bind the exact pair/source identities, both source-epoch SHA-256 values, policy, function, scope and window, a recognized reviewer role, and an evidence-artifact SHA-256 reference resolved by the outer evidence-package verifier; such review cannot clear incomplete family coverage or out-of-order streams.
- **OA-MIG-403** The development-only SFDPS source-facts adapter consumes an already decoded `oa_fixm::Events` row and existing replay row identity. It keeps message/source time separate from observed/receive time, preserves parser absent/present/malformed states and typed values for handoff, controlling-unit, and flight-data fields, refuses missing mandatory parser columns, and does not synthesize cross-side matching ordinals. It makes no controller-intent, sector-truth, point-out, legacy-alert, or amendment-delta claim; unsupported families remain outside this projection.
- **OA-MIG-404** The development-only SFDPS-to-M4 projection is fixed-capacity and consumes validated row facts. Explicit SFDPS handoff event codes map to handoff phase fields; a cross-side business key is valid only when supplied by an independent caller. `controllingUnit` and flight-data records remain source snapshots with unknown event state and missing event keys; they do not assert sector reassignment or amendment deltas. Composite/overlength values that lack a lossless normalized representation are visibly unknown or unsupported, never truncated. Point-out and alert records are not synthesized.
- **OA-MIG-405** The development-only candidate projection exports actual `oa-flight` sector, handoff, plan-view, altitude and identity snapshots with field source and availability clocks from their stored stamps. It is fixed-capacity, preserves caller-supplied side-local source identity, and never derives a cross-side business key or event ordinal. Snapshots retain `Unknown` event state and missing key; they are not controller actions or amendment deltas and remain unmatchable under the event comparator until a separately versioned snapshot policy exists. Fields absent from `oa-flight` (including route) are explicitly counted as not modeled; malformed clocks, invalid view counts and insufficient buffers fail before output is written.
- **OA-MIG-406** The regional-session shell adapter projects actual OA conflict/MSAW alert episodes with caller-supplied source identity, optional external business key, source episode time, and separately supplied observed/availability clock. It never derives an event key or availability from row order, pair identity, or cycle time. Alert kind and open/closed state are preserved; acknowledgement, suppression, and shared reason taxonomy remain explicitly unsupported. Missing observed time stays `None`; missing SFDPS legacy-alert coverage is `Coverage::Missing`, never complete-empty or agreement. Invalid key family/clocks and insufficient output capacity refuse before writing; snapshots are not promoted to action events.
- **OA-MIG-408** `M4-H2-SFDPS-PUBLISHED-STATE-V1` compares the actual candidate `oa-flight` state immediately after an SFDPS row with only the values explicitly present in that same decoded row. It matches by the actual GUFI, retains the source row locator as provenance only, and reports source time, recorder receive time, and candidate model-stamp time separately. `controllingUnit` is compared as its unit and sector bytes; flight status/plan, identity, supported altitudes and route omission have explicit per-field outcomes. Exact values are equivalent; differences remain unresolved absent pair-bound review evidence; absent, malformed, unsupported, missing identity, candidate-unavailable, future-stamped, and not-modeled values are not agreements. This is same-input propagation consistency, not an independent controller-decision or legacy-correctness oracle. The registered zero-value margins are engineering screening limits only, not FAA acceptance thresholds.

## H2 sector and flight-data development comparison profiles

- **OA-MIG-409** M4-H2-G3-OCCUPANCY-TRANSFER-V1 compares each evaluation-day SFDPS controllingUnit with the unique supported owner label in a frozen G3 empirical occupancy model trained only on 2026-11-15 and evaluated only on 2026-11-19. The model identity binds its whole-artifact SHA-256, training-trace SHA-256, policy SHA-256, source revision, and binary SHA-256 to independently verified caller receipts. It uses the existing global WGS-84 0.05-degree grid, 2,000-ft altitude slabs, minimum three distinct GUFIs and two state-time bins; no official coverage mask, smoothing, or extrapolation is introduced. Each row requires the exact positive GUFI, candidate-owned address association, candidate state at the SFDPS receive time, source/availability clocks, valid position and altitude, and a known source owner. Supported match/mismatch, contested, unknown support, no training cell, unavailable output, identity mismatch, invalid/future time, stale position, invalid position/altitude, missing altitude, and unknown owner are explicit dispositions whose counts reconcile to all evaluation rows. This is a held-out empirical occupancy transfer proxy, not official sector geometry, controller intent, probability, or physical boundary accuracy. The candidate-vs-input controllingUnit byte comparison in OA-MIG-408 remains a separate propagation diagnostic, never independent sector-assignment evidence.
- **OA-MIG-410** Flight-data values are compared only as same-input SFDPS row facts against the candidate's stored state after that row. The result is an explicit propagation diagnostic, not an independently keyed amendment event. The current input contract has no independent candidate amendment-event business key or legacy alert feed; these families remain unobservable and cannot be converted from snapshots, omitted rows, or zero-difference counts into agreement or qualification.
- **OA-MIG-411** The offline M4/H2 full-day runner uses only a fixed 24-hour evaluation partition and consumes guarded ordered OAF segments without retaining the day's runtime frames. It binds input, executable, airspace, G3 artifact, policy, and run-profile digests; preserves the runtime state continuously; aggregates and drops each output frame; and publishes aggregate counts only. Physical bytes, expanded bytes, decoded frames, normalized events, runtime frames, tracks, flights, per-receipt events, work, and pending-source memory have fixed checked ceilings. Any parser refusal, timestamp gap, out-of-order stream, failed reconciliation, or exhausted capacity prevents a complete result; prefixes are not complete-day evidence. Defaults for existing research/live entrypoints remain unchanged.
- **OA-MIG-412** `M4-H2-G3-OCCUPANCY-TRANSFER-V2` compares each SFDPS row against the latest already-emitted candidate track state at or before its recorded receive time, no more than one second old. Candidate source and input-availability timestamps must not exceed the query time. Comparison occurs before stepping a later issue tick; future states and future availability are explicit non-comparable dispositions. No prediction or ceil/future tick is used. Every source row retains one explicit G3 disposition and contributes to reconciled denominators. This is a causal as-of occupancy transfer proxy, not physical boundary truth or controller intent.
- **OA-MIG-413** The full-day shell compares SFDPS row facts with only fields exported in the regional runtime's immutable `SharedFlightView`. Fields absent from that view, including requested altitude and route, are explicitly `CandidateNotModeled`; assigned-at-fix values are not compared when the output view lacks the fix identifier. The view adapter does not read back hidden table state or turn source snapshots into independent events.
- **OA-MIG-415** The bounded OAF segment adapter accepts explicit ordered source segments, applies per-segment and aggregate byte/frame limits, validates each frame before visiting it, and merges accepted frames by recorded receive time with the declared deterministic source/sequence tie-break. It preserves per-source hashes/counts, distinguishes OAF frame-activity coverage from message coverage, reports physical prefix/suffix frames outside the half-open evaluation window without decoding them, and refuses malformed, non-monotone, incomplete, or capacity-exhausted input rather than claiming a complete stream. The normalized visitor is called one event at a time; the day is not materialized. Synthetic coverage includes receipt-order merge, nonmonotone refusal, half-open prefix/suffix skipping and its counters, normalized receive provenance, and recorder-frame activity semantics in `recordings_stream.rs` tests `bounded_segment_merge_streams_messages_in_receipt_order`, `nonmonotone_segment_refuses_before_claiming_full_stream`, `declared_window_counts_and_skips_physical_prefix_and_suffix`, `normalized_stream_carries_sequences_and_exact_receive_provenance`, and `source_coverage_reports_unobserved_prefix_without_erasing_available_rows`.
- **OA-MIG-416** `M4-H2-AMENDMENT-PROPAGATION-V1` compares source-field transitions across consecutive SFDPS rows for the same explicit GUFI with the actual candidate state observed after each row. It reports nine fixed projections: flight status, flight-plan ID, callsign, departure, arrival, assigned altitude, interim altitude, requested altitude, and route. It reconciles present/absent/malformed/unsupported, identity, clock, and candidate-availability outcomes. This measures same-input propagation only; it does not identify an independent controller amendment decision, infer missing events, or provide an ERAM correctness oracle. Differences remain unresolved without pair-bound review evidence, and absent or unavailable values are never counted as agreement. Synthetic cases are covered by `migration_amendment.rs` tests `source_changes_compare_exact_candidate_before_and_after_values`, `unavailable_identity_and_missing_key_stay_in_explicit_denominators`, `mismatched_candidate_identity_never_becomes_equivalent`, `route_change_is_detected_without_exporting_text_or_claiming_candidate_support`, `source_clock_regression_is_counted_and_never_scored_as_propagated`, `separate_gufi_argument_must_match_decoded_row_identity`, and `source_absent_malformed_and_unsupported_fields_reconcile_separately`.

## M7 per-input quarantine (`oa-quarantine`)

- **OA-QUAR-001** Flight-data, surveillance, and adaptation items carry a source-class partition,
  nonzero source-instance and item identity, and explicit typed refusal-reason bits. Queue identity
  and caller-supplied claims are not authenticated. Each source class has an independent queue so
  one full partition cannot consume another class's capacity.
- **OA-QUAR-002** Each source partition holds at most 128 metadata events. Every reason bit is
  counted, a queued event remains until explicit acknowledgement, and a full partition returns a
  visible `ManualHandlingRequired` event while incrementing its queue-full counter. This queue
  holds no raw source payload; the shell must retain and retrieve the original item by its stable
  identity. It does not itself guarantee durable operator handling or end-to-end fault isolation.
- **OA-QUAR-003** The core never mutates accepted flight, surveillance, or adaptation state. A
  caller may apply an item's state change only after an `Accepted` disposition; a refusal yields
  queued or direct manual handling. Synthetic LA-2014-shaped altitude-block/route and NATS-2023-
  shaped duplicate-waypoint cases exercise the boundary, but are not exact reproductions of either
  historical input or outage.
- **OA-QUAR-004** Every outcome carries a caller-supplied monotonic clock domain, start/completion
  timestamps, elapsed-time limit, work count, and work limit. Clock regression, invalid limits,
  counter overflow, unknown reasons, and invalid identities are explicit errors. Exceeding the
  declared work or elapsed-time limit adds a named refusal reason and is counted; this pure core
  does not measure wall time or verify the caller's work accounting.

## Migration swap-unit lifecycle (`oa-migration`; H1)

- **OA-MIG-101** The pure bounded lifecycle evaluator binds every swap unit, numeric evidence item,
  dependency receipt, approval and audit event to the exact function/scope/window, policy version
  and source-epoch digest. Metric evidence has a distinct observation interval, allowing valid
  prior S0/S1 observations to qualify an S2 window; sample counts cannot exceed denominators.
  For S2–S5 measured exposure, evidence also binds the origin unit, actual stage activation and,
  when continued, exact prior-unit receipt; intervals cannot predate activation or exceed the
  origin window.
  S0→S1 may be prepared before the window;
  S2–S5 activations require the half-open declared unit window. Longer S3/S4 exposure uses a new
  unit window and an embedded dependency receipt from an opaque successful prior transition.
  It advances only adjacent S0–S5 rungs; missing, stale, out-of-scope,
  failed or over-cap evidence and missing/rejected authority refuse without advancing the rung.
  Promotion requires distinct facility-supervisor and independent-assessor principals; S5 also
  requires a separate facility-retirement authority and cannot occur from a timer alone. S3 and
  S4 require unit windows of at least 48 hours and 7 days; leaving S3 and S4 requires evidence of
  at least 48 hours and 7 days, respectively. Each transition policy declares rollback triggers for blocking differences,
  controller requests, and health faults, plus permitted roles and a response-time target; M1
  never asserts that an output switch occurred. The caller supplies deterministic evaluation time
  and retains returned audit events in bounded storage. Handles identify external records; they do not prove their contents
  or authenticate a principal, and the framework never connects to a live system or selects an
  output.


## Migration output selector (H1, H5)

- **OA-MIG-201** `oa-migration-switch` is a zero-dependency, recorded/synthetic-only core. Its swap
  unit is the exact function/scope/half-open-window tuple plus bound policy/source-epoch digests;
  it has no network, file, clock, or live-system interface.
- **OA-MIG-202** The state mirror accepts only bounded, sorted, unique snapshots whose unit, policy,
  side, source epoch, state version, sequence, time basis, family coverage and health validate.
  Partial, missing, unsupported, unknown, stale, duplicate, out-of-order, or gapped evidence refuses.
- **OA-MIG-203** Reconciliation compares sorted full 32-byte business keys and each declared
  consumer projection for tracks, flight data, handoffs and alerts, including active,
  acknowledged and suppressed alert state. Each row's field-presence mask must match the
  schema-bound policy mask and its canonical projection digest must be present. Side-local source
  identities are retained as provenance and are not cross-side matching keys. This compares only
  the declared operational projection; it does not claim identical internal filters or algorithms.
- **OA-MIG-204** A readiness token binds the exact unit/policy, both source epochs, state versions
  and sequences. An accepted opaque M1 S1 Advisory → S2 Window result creates one reusable window
  grant bound to the same unit, policy/source digests and evidence/approval handles. Every candidate
  re-entry requires a new replay-checked M2 intent bound to a current readiness generation; the M1
  lifecycle transition is not repeated for each switch. Rollback requires a
  fresh legacy mirror and typed, role-allowed rollback event with an explicit current legacy-health
  and source-availability observation bound to the mirrored epoch/version/sequence. These supplied
  observations are not authenticated by the core and do not certify live source health; no caller
  Boolean or count alone certifies readiness.
- **OA-MIG-206** `authorize_window` consumes `oa-migration::TransitionResult` directly and refuses a
  refusal, wrong stage edge, wrong unit/window, policy/source/evidence mismatch, or invalid declared
  principal identifiers. A distinct opaque M2 `PromotionIntent` is created for each re-entry from
  the current readiness token, increasing action sequence, event key, and the same supervisor ID
  bound by the M1 grant; role must be allowed by M2 policy. Selection rechecks the exact grant,
  readiness epochs/versions/sequences, policy handles, intent sequence and time. The trace records
  the M2 intent event key/action sequence and the reused M1 window sequence separately.
- **OA-MIG-207** Caller-built M2 promotion receipts are removed. Synthetic regressions exercise
  wrong-window/policy/source M1 results, refused M1 results, wrong or mismatched M2 intents, stale
  readiness epochs, replay after rollback, and the compile-time opacity of M1 `TransitionResult`.
  Numeric principal IDs and external record handles are declared inputs, not cryptographic
  authentication; neither layer proves live approval, evidence contents, or operational authority.
- **OA-MIG-205** The trace records deterministic decision timestamps and selected-output transitions.
  The endpoint class binds the terminal/en-route update interval and the model records consumer
  route identity and sequence/timing observations. Each promotion trace record preserves the M2
  intent sequence/event key and M1 window-grant sequence independently; rollback records have no
  M1 grant sequence, and rollback event keys cannot be reused within one selector. Promotion
  reserves the final trace slot for immediate rollback; once the fixed trace is exhausted, further
  promotion refuses. These are
  recorded inputs; the core does not route payload bytes or prove a downstream system received
  them. This does not prove availability, controller authorization, or operational cutover. H5
  continuity requires separate measured drill evidence from source-backed consumer observations.

- **OA-MIG-208** Before each candidate selection or rollback, the selector appends one bounded plain
  coordination record for every configured interfaced facility, binding the recipient, unit,
  output sequence, transition time, source/destination and reason. The records are appended before
  the selected output changes. Promotion refuses unless capacity remains for both its own records
  and the immediate rollback records for every recipient; coordination-capacity refusal leaves the
  selected output and trace unchanged. This core only produces local records: it does not send
  messages, contact facilities, or prove acknowledgement. Facility IDs are nonzero, unique and
  bounded at selector construction; an empty set means no interfaced facility was configured for
  that unit.

## Migration drill harness (M3, H5)

- **OA-MIG-301** The synthetic selector drill calls the production `oa-migration-switch` API for
  at least 100 independently initialized flip-and-rollback cycles. Each side supplies the declared
  maximum 64 tracks, flights, handoffs and alerts; reconciliation compares all rows, including
  active, acknowledged and suppressed alert fields. Every clean cycle records both ordered
  selector delivery observations and a zero-loss denominator.
- **OA-MIG-302** A sustained single-selector case retains its full trace across 64 distinct M2
  promotion intents and rollback events under one reusable accepted M1 S2-window grant. The 64th
  rollback succeeds into the final trace slot, with the exact `Candidate`→`Legacy` record checked.
  The next promotion reports `TraceFull` while the selected output stays Legacy; the drill reports
  this per-unit trace-capacity limit instead of resetting the ledger.
- **OA-MIG-303** Synthetic fault cases record refusal reasons for an epoch change after
  reconciliation, duplicate/gapped/out-of-order delivery observations, stale legacy health,
  expiry and incomplete family coverage. A rollback during pending promotion is exercised; any
  resulting delivery gap is counted as injected loss, not hidden in a clean-cycle denominator.
- **OA-MIG-304** Decision-path wall samples and selector-API delivery-call samples are separate
  fields. Modeled delivery delay is not a consumer-wire measurement. Until a normalized,
  source-backed recorded H2/M4 input adapter exists, the harness labels recorded replay
  `NOT_BUILT` and qualification `INCOMPLETE`; synthetic output cannot close H5 or authorize
  readiness.
- **OA-MIG-305** An ordinary Cargo test starts two bounded local synthetic producer processes
  and an independent consumer process over a versioned internal test protocol. Selector snapshots
  are constructed from producer-returned side, epoch-family, source-sequence, and state-version
  fields; the selected payload bytes are delivered and echoed by the consumer before M2 records
  delivery. After a candidate producer loss, rollback through the reusable M1 window grant and a
  fresh M2 intent delivers and acknowledges consumer sequences 1, 2, and 3 within the declared
  local target. Separate gap and duplicate negative controls are refused and counted. Producer
  failure and consumer-loss denominators remain distinct; this synthetic process test does not
  establish a public interface, recorded-data result, network property, or operational readiness.


## Migration safety evidence package (`tools/migration_evidence.py`; H3)

- **OA-MIG-601** The standard-library-only offline generator accepts a versioned bounded internal
  manifest for exactly one function, scope, family, half-open window, policy, and pair of source
  epochs. It opens every input with the production sealed guard, verifies descriptor stability,
  declared byte lengths and SHA-256 values, rejects duplicate IDs/schema fields, checks absolute
  timestamps against sealed windows, and reconciles exposure, traffic and per-family shadow
  denominators. Missing or invalid required artifacts, unknown accounting, empty scope evidence,
  and capped inputs remain explicit incomplete gaps. Output is deterministic, private (`0700`
  directory and `0600` file), create-new and no-overwrite; it never connects to an operational
  system or invents an unavailable ERAM/SFDPS observation.
- **OA-MIG-602** The generated package carries source hazard IDs and mitigations, shadow rows with
  every non-equivalent outcome and its review classification/reference, shadow/live exposure,
  quarantine/rollback events, and reported H5 flip/rollback drill metrics. Review must bind the
  exact comparison, source identities, policy, function, scope, window, source epochs, reviewer
  role and evidence hash; unresolved, worse, pending or unverified references block completeness.
  Fewer than 100 drills reported as recorded-traffic drills with a hash-verified artifact remain
  explicit; the label and claim that the artifact represents a performed drill are not authenticated
  by this tool. Unassessed ICD limits, missing/unknown fields, and any unverified source claim also
  remain explicit. The output is evidence organization for human
  assessment only: it is never FAA approval, a safety-acceptance decision, or promotion authority.

### NIST SP 800-53 Rev. 5 HIGH control evidence map

- **OA-E1-001** `docs/security/control-map.json` contains every control and enhancement selected by
  the pinned NIST SP 800-53 Rev. 5 HIGH resolved OSCAL profile. The checked-in source URL, release
  commit, byte length and SHA-256 are recorded in `docs/security/oscal/SOURCES.md`; the inventory
  generator rejects missing, duplicate, extra, or source-mismatched control IDs and titles.
- **OA-E1-002** A control may be `implemented` only with an exact existing Rust test or named CI
  job/step reference, and `oa-trace` verifies those references. `partial`, `planned`, and
  `inherited` records state missing facts; facility/hosting inheritance remains unverified until
  the responsible provider and boundary evidence are identified. These engineering dispositions
  are not compliance findings, an ATO, or FAA authorization.
- **OA-E1-003** The deterministic generator produces Markdown and CSV counts by control family and
  evidence status. CI checks the generated files and rejects incomplete or duplicate inventories.

## Privileged action audit log (E3, I10)

- **OA-AUDIT-001** Each audit record binds a positive principal ID, one declared registry role,
  action code, bounded ASCII target, positive caller-supplied nanosecond timestamp, bounded opaque
  details, sequence number and previous digest. Version 1 uses fixed-width fields, little-endian
  integers, zero-padded target/details, and the domain `OA-AUDIT-CHAIN-V1\0`; the record digest is
  SHA-256 over that full canonical encoding with the digest field zeroed. The first sequence is 1
  with an all-zero previous digest; each following sequence increments by one and binds the prior
  digest. No alternate encoding is accepted.
- **OA-AUDIT-002** The registry allows `monitor` to verify records, `operate` to release quarantine
  items and submit vulnerability intake, and `configure` to issue adaptations, certify, flip or
  roll back migration output, and disposition vulnerability findings. `operate` and `configure`
  may perform runway assignment and alert-acknowledgement actions. Higher roles may perform
  lower-level actions; a lower role is refused before
  the action commit. Principal IDs, role claims and timestamps are caller inputs; this registry
  provides no login, credential check, authenticated origin or authority validation.
- **OA-AUDIT-003** The shell log uses fixed-size records and an OS-held nonblocking exclusive
  lock on the opened log descriptor. Lock contention fails closed; closing the descriptor releases
  the lock, so no stale sidecar lock needs recovery. The shell verifies the supplied count and tail
  before appending, writes only at EOF, syncs the record and containing directory before returning
  a plain receipt value, and fails closed on I/O, lock or sync errors. A privileged mutation must
  consume the matching receipt value before commit; audit failure cannot invoke or commit that
  mutation. The value is not unforgeable or authenticated; principal and role claims remain caller
  inputs.
- **OA-AUDIT-004** Verification checks records in sequence and reports the first malformed,
  reordered, altered, deleted or truncated record. Every verify/read/append operation requires the
  caller's trusted expected record count and tail digest and refuses a shorter or longer log than
  that checkpoint. `oa-audit-verify` reports the first broken sequence and requires the checkpoint
  on every invocation. The log is capped at 65,536 records. This detects changes relative to the
  supplied checkpoint; a hash chain alone does not prevent full history rewriting together with
  replacement of that checkpoint, and it does not authenticate who wrote a record.
- **OA-AUDIT-005** Adaptation issue, certification, quarantine release, migration flip and
  migration rollback append a bounded record before committing their action. If a later action
  fails, its already-written intent remains visible for reconciliation; no failed action is
  reported as committed solely because an audit record exists.
- **OA-AUDIT-006** Every audit-log read, append destination, and parent directory synchronization
  path is checked by the production `oa-sealed` guard before filesystem access; read and append
  descriptors are verified after opening, and no append bytes are written until the destination
  descriptor is verified. On Linux, append opens the destination through the retained verified
  parent descriptor using `/proc/self/fd`, verifies and locks that same descriptor, and syncs that
  same parent descriptor; its leaf open refuses symlinks with `O_NOFOLLOW`. Append refuses on
  platforms without this directory anchoring. A sealed-path refusal or descriptor-verification
  error fails the audit operation before the associated committed action.
- **OA-AUDIT-007** The core principal registry accepts the existing bounded
  `OACERT-LOCAL-PRINCIPALS-V1` two-column grammar and `OACERT-LOCAL-PRINCIPALS-V2` rows with an
  explicit enabled/disabled status. Both require unique positive canonical decimal IDs and one
  `monitor`, `operate`, or `configure` role. The fixed table admits at most 128 rows and the parser
  admits at most 64 KiB; malformed framing, duplicate IDs, unknown roles, invalid status, excess
  rows, and unknown or disabled principals are explicit refusals. Action authorization first
  resolves the principal, then applies the existing three-role action matrix. This is local role
  configuration only; it does not authenticate a login, credential, principal origin, or authority.
- **OA-AUDIT-008** The shell registry loader opens a regular file through the production sealed
  path guard, reads from the verified descriptor under the 64 KiB byte cap, rejects size changes,
  hashes the exact bytes, and compares them with an independently supplied expected SHA-256 pin
  before parsing. The registry bytes do not supply their own trusted pin. Path, descriptor, pin,
  framing, and parse refusals return no registry.
