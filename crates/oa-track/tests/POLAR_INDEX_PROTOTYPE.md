# Polar index prototype report

This is a test-only prototype based on `b45fa6910c43173d9955dac7e54ac24380fe1079`. The test
imports the public `coarse_passes` predicate as its oracle. Nothing in library or fusion code is
changed, and the integration-test file is not reachable from a product target.

## Reference behavior

At the base revision, `coarse_passes` computes `W = window + 8*sigma` and returns false only if
either `abs(plot_rho - rho) > W` or `abs(az_diff(plot_az, az)).to_radians() * ground > W`. In
fusion the window is `COARSE_M = 3,000 m`, the regular grid search uses `COARSE_STD_M = 4,500 m`,
tracks whose own window exceeds that standard are handled on the wide list, and
`WIDE_TRACKS_MAX = 4,096`. This prototype does not model that split or cap; it asks whether a
standalone candidate index is a superset of the exact coarse predicate.

The predicate has no timestamp argument. Therefore negative or positive times cannot affect its
acceptance, and this prototype intentionally does not create a time dimension or claim temporal
coverage.

## Posting and query

Each input track creates exactly one posting. Regular postings key by `(W band, rho cell, track
index)` and retain the track's azimuth and ground range. Width uses 512 m bands for
`1 <= W < 65,536 m`; slant range uses 500 m cells. Only occupied W bands are visited. For each one,
two prefix searches select the widened radial slice and the stored polar values apply a conservative
angular check. The occupancy directory and output bitset each have a fixed 512-track bound.

Tracks outside those finite domains, with non-finite polar values, non-positive or sub-metre ground
range, or with non-finite `W`, get one `ALWAYS` posting. A query outside the finite plot domain,
an incomplete build, or exhausted query work returns `ScanRequired` and an empty output. The caller
must then use its full scan; no partial candidate list is usable.

## Inclusion argument

For a regular posting let `W_hi` be the upper edge of its width band. The bands are half-open, so
`W < W_hi`. If the exact predicate
accepts a track, its range condition gives `|plot_rho-rho| <= W < W_hi`. Thus its 500 m range cell
meets the query interval expanded by `W_hi`. Query endpoints are padded outward by one complete
500 m cell; over the declared 0–16,000,000 m domain this exceeds floating addition, division, and
floor rounding error and preserves equality at both exact range boundaries.

The exact cross-range condition gives `abs(az_diff) * ground <= W < W_hi`. The query evaluates the
same wrapped-angle expression for every posting in the conservative radial slice and rejects it only
when that cross-range exceeds `W_hi`. It therefore cannot reject a regular track accepted by the
reference predicate. Azimuth inputs are restricted to `[-720,720]` degrees for regular postings and
queries; larger finite values use `ALWAYS` or `ScanRequired`, avoiding claims about extreme-value
remainder precision. Ground range is similarly regular only on `[1,2^25)` metres.

For exceptional tracks no narrower bound is assumed: each is included from `ALWAYS`. In
particular, ground zero, negative ground, and NaN or infinite terms can make the reference
predicate's `> W` rejection false; they are not filtered out. Incomplete builds clear all postings
and leave `valid=false`; queries then fail closed. The same applies if query work runs out.

## Bounds and modeled work

- Build storage is allocated at construction and holds at most 512 postings. Build rejects a
  larger declared input before writing. This is a test bound, not a production capacity.
- Build charges a fixed 8 units per input visit for validation and cell classification, charges
  active-band comparisons, then performs an in-place insertion sort charging before every
  comparison and swap. Its modeled worst-case ceiling is 323,008 units and its allowance is
  500,000 units. Any cap clears postings and
  occupancy and leaves the index unusable.
- Query visits at most 128 occupied width bands. It charges 8 units per band setup and 20 units for
  its two prefix searches (at most 10 comparisons each with 512 postings). Because every track has
  one posting in one band, at most 512 regular postings are visited across all radial slices. Each
  posting visit, always-bucket inspection, bitset word, and emitted candidate costs one unit. The
  resulting analytic ceiling is 5,128 units; the test allowance is 6,000. Exhaustion leaves output
  length zero and requests a full scan.
- Candidate output is a fixed 512-entry array; a bitset deduplicates and emits in ascending track
  order. No result order depends on posting sort ties or input traversal.

The insertion sort is intentionally simple for exhaustive small-domain checks. It is quadratic in
the worst case and explicitly capped at 512 tracks; it is not a production strategy for the
65,536-track association capacity. A production design would need a separately reviewed bounded
sort/build strategy before integration.

## Reference cases and limits

The test source covers exhaustive small cross-products, 80 deterministic seeded queries, the
actual adjacent-float acceptance transition of the reference angular predicate, exact and
next-representable range boundaries, wrapped azimuth, every supported 512 m width-band boundary,
power-of-two ground values through the regular-domain cap, singular and non-finite geometry, high
uncertainty, far-range/azimuth exclusion, capacity exhaustion, incomplete build/query, work
accounting, and deterministic ascending output.

This establishes a mathematical inclusion argument for the declared regular domain and explicit
fallback behavior outside it. The coordinator verified all eight integration tests and strict
Clippy with Rust 1.89.0 under the repository memory guard. It does not establish production
performance, a 65,536-track build, or equivalence of the existing wide-list behavior.
