# Airspace model and route-conversion API proposal

**Tier:** L2 Preview; outside the proposed 0.1 release. This is a draft API proposal. Proposed facade names are not implemented promises, approved names, or a Stable contract.

## Current source API

`crates/oa-airspace/src/lib.rs` re-exports `core::{airac, build_info, diff, geom, model, report, route, snapshot, source}` and `shell::{build, cli, load, sources}`. The current path-level entry points are `load::load_files` / `load::load_bytes`, `build::build_model(&Sources, &BuildParams) -> Result<BuildOutput, BuildError>`, model validation/lookups such as `validate_model`, `find_point`, `find_airway`, geometry functions including `build_area`, `area_contains`, `area_edge_distance_m`, `segment_samples`, and `route::convert_route(&Model, &RoutePlan, &mut ConvertedRoute) -> RouteReport`. These are current in-repo paths, not an approved compatibility boundary.

`Model` is a deterministic, flat table collection with typed indices/spans and fixed-point source data. Coordinates in its tables are signed E7 degrees; stored altitudes are integer feet; distances are integer metres; bearings are centidegrees; speeds are knots. The model source says rows are sorted by stable keys. `BuildParams` names an ARTCC and caller-selected metre buffer and can explicitly permit a NASR/CIFP AIRAC mismatch; its implementation cap is 1,852,000 m and ARTCC identifier length 8. AIRAC is a 28-day cycle relative to the source constant `AIRAC_EPOCH=(2026,1,22)`, not an event timestamp.

Inputs are FAA NASR subscription files/structure tables and FAA CIFP `FAACIFP18` / ARINC 424-18 data as consumed by the current loader. The build chooses CIFP position when both sources contribute an object, records disagreements, retains unresolved references, and marks unsupported/unknown CIFP fields. It is not a generic worldwide airspace database or a complete ARINC procedure model.

## Bounds, errors, and observable refusals

- Current route conversion has at most 32 steps, 256 output points, and 1,048,576 charged record visits. It refuses malformed identity/continuity/unsupported constraints and returns no partial new route.
- Geometry bounds include a maximum of 1,048,576 sampled boundary points and a default area-touch work ceiling of `1<<34`; these are implementation ceilings, not a recommended public default without review.
- The current custom OATCA-ADAPT snapshot is capped at 512 MiB. It is an internal snapshot format, not a standard boundary format or an obligation for public 0.1.
- `LoadError` distinguishes bad/unused expected digests, mismatch, ZIP/entry failures and NASR/CIFP parse failures. `BuildError` distinguishes bad parameters, missing ARTCC/cycle mismatch, bad geometry, unstorable/bad source records and invalid assembled models. `ModelError`, `RouteFailure`, `GeoError`, and `SnapshotError` are separate current errors. A public facade should preserve source file/record context and error categories.
- `load_files` is shell I/O and currently hashes the two archives against caller-supplied SHA-256 expectations before parsing. A digest is an integrity comparison only; no signature or provenance authenticator is implied.

## Provisional public shape for review

Prefer a narrow `AirspaceBuildInput`/`AirspaceBuilder` façade over exposing shell parsing record types. Keep an explicitly versioned `NasrSchema`/`CifpProfile` or data-bundle metadata that records the exact NASR effective date, its structure-file identity, CIFP cycle and accepted record subset. A possible Rust-only first design is `build_artcc(sources, BuildOptions) -> Result<AirspaceModel, BuildError>` plus read-only identity-qualified query functions; these names are illustrative only. Do not export the project-internal `oa-sealed` crate, ZIP path policy, `HashMap` implementation, or CLI grammar as public semantics. Keep route normalization separate from route expansion, and identify unresolved/unsupported path terminators rather than guessing geometry.

The model does not implement geodesic line/segment intersections. It also does not reject self-crossing selection polygons universally. Point/area boundary inclusion and buffer semantics need explicit tests and a stated conservative policy before a Stable public query contract.

## Tier evidence status

The crate has source-level geometry, snapshot, route and real-data tests plus a deterministic build design. That is not the §8 bundle. Standards/profile coverage, source-edition matrix, 24-hour fuzz, Kani applicability, mutation review, no-panic review, x86-64/aarch64 output comparison, Rust/Python examples and redistributable vectors remain unverified here. The crate currently depends on `oa-sealed`, `oa-nasr`, `oa-arinc424`, `oa-zip`, `oa-geo`, and internal workspace crates; `oa-sealed` must be removed or isolated before establishing a public package boundary. Preview only, no 0.1 commitment.
