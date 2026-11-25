# Geodesy

**Current surface.** `oa-geo` is `no_std` core code (with `alloc` for prepared polygons) and
depends on `oa-math`/`oa-ieee`.

- `LatLon::new(lat_deg, lon_deg)` validates finite latitude in `[-90,90]` and longitude in
  `[-180,180]`; `lat()`/`lon()` return degrees. DMS conversion helpers validate axis, minutes,
  seconds, hemisphere and sign conventions.
- `wgs84()` returns an `Ellipsoid`; `Ellipsoid::new(a, f)` validates a supported equatorial radius
  and flattening. `inverse(p1,p2)` returns `Inverse { distance_m, azimuth1_deg, azimuth2_deg }`;
  azimuths are degrees clockwise from north in `(-180,180]`. `direct(p1, azimuth_deg,
  distance_m)` accepts any finite azimuth and finite signed distance in metres and returns a
  destination and terminal azimuth. `rhumb_inverse`, `rhumb_direct`, `rhumb_point_at_fraction`
  and `geodesic_rhumb_separation` are also present.
- `aeqd` provides a local spherical azimuthal-equidistant `Frame`; projected `x` is east, `y` is
  north, in metres. Frame/projection helpers include `frame_new`, `project`, `unproject`, and
  Jacobian/velocity transforms. Its documented domain excludes a margin around the antipode;
  checked variants return `Option` when the frame transform is unsupported.
- `Polygon::new(&ellipsoid, vertices)` prepares a ring whose edges are geodesics;
  `contains(point)` uses the even-odd rule. Bounds: 3 to 100,000 vertices, no pole vertices, each
  edge shorter than 10,000 km and spanning less than 180 degrees, boundary cannot enclose a pole.
  Self-crossings are not rejected; points exactly on a boundary may be reported either way.

There is currently no public geodesic line/segment intersections API in `oa-geo`; polygon edge
intersection is an internal boundary detail, and polygon construction does not test self-crossing.
“Intersections” in CORE_LIBRARY_SPEC §10 therefore remains a missing L0 capability. It needs a
separate API design for finite segments vs infinite geodesics, coincident/overlapping lines,
multiple/near-antipodal solutions, tolerances, and units before implementation.

**Errors/limits.** Public fallible construction/solvers use `GeoError`: nonfinite values, latitude
or longitude range, DMS ranges/hemisphere, unsupported ellipsoid, bad parameters, solver
nonconvergence, rhumb line reaching a pole, or rejected polygon. Direct geodesic input is finite
but signed distance is otherwise unbounded; output validity is checked. `Polygon::contains` returns
`bool`, including its documented boundary ambiguity. Do not treat boundary classification as a
conservative containment proof without a caller-supplied margin policy.

**Tier/stability proposal.** L0 geodesy, but split current capabilities into separately qualified
contracts: inverse/direct, rhumb, local projection, polygon containment, and future intersections.
The implementation describes Karney geodesics and has geodtest samples, geodesic/polygon/rhumb
tests and purity checks. These are not evidence of the complete §8 criteria. Remaining: identify
the published reference edition and test corpus, add edge/degenerate/antipodal and polygon topology
vectors, establish fuzz/Kani applicability and mutation results, prove no panic, verify
x86-64/aarch64 identical outputs, and publish Rust/Python examples and vectors. Record geodetic
datum, unit, angle convention, local-frame domain, and each unsupported case on every user-facing
operation.

No hidden clock, dataset seal, sensor adaptation or external I/O is consulted; the caller supplies
coordinates, ellipsoid and polygon vertices.
