# Core API and tier proposals

**Status: draft engineering/API proposal.** Package boundaries, facade names, release scope, and
tier assignments are not reviewed or frozen. No API in this directory is approved or Stable.
These pages record source-backed behavior and identify candidate public facades; candidate names
are examples only, not implemented compatibility promises. They do not change source, registry,
requirements, release scope, or tier approval. No build, test, formatter, or script was run for this
documentation draft.

## Proposed 0.1 scope needs reconciliation

[Core library design](CORE_LIBRARY_SPEC.md) §10 names broader capabilities than the current source supports. Before
calling any profile part of a release, reconcile the specification to the exact source-backed
subset or schedule the missing implementation. An unavailable feature is out of scope; it is not
an Experimental feature included in a release.

| Area | Specification wording | Source-backed proposal | Draft tier and open gap |
|---|---|---|---|
| L0 scalar math, geodesy, integrity, compression/containers, XML/JSON/CSV | Broad families | Include only documented current operations and grammars; track Rust/Python bindings separately. | Preview by operation. The §8 Stable evidence bundle is incomplete. |
| L1 ASTERIX | CAT021, CAT034, CAT048, CAT062 | Scope current source to CAT021 Ed 2.4/2.7, CAT048 Ed 1.32, CAT062 Ed 1.21 and ASTERIX Part 1 Ed 3.1 framing. | Preview for implemented categories. CAT034 codec is absent and outside 0.1 unless implemented. |
| L1 ARINC 424 / CIFP | ARINC 424 | Narrow to the implemented FAA `FAACIFP18` / ARINC 424-18 record subset. Unsupported sections are skipped or counted as documented. | Preview. Not a general ARINC 424 parser; no encoder. |
| L1 NASR | NASR | Identify the subscription/schema snapshot and cycle supplied with each input package. | Preview. Do not imply one immutable NASR edition; no encoder. |
| L1 AIXM NOTAM | AIXM NOTAMs | Narrow to the FAA FNS AIXM 5.1 profile and FAA event extension. | Preview decoder. No encoder, publisher, SWIM envelope or transport. |
| L1 FIXM/SFDPS | FIXM/SFDPS messages | Narrow to FIXM 3.0 core plus FAA NAS 3.0 `MessageCollection` profile. | Preview decode. No encoder or ACK/REJ publisher; the current crate depends on internal `oa-sealed`, so a public package boundary is not ready. |
| L1 GRIB2 winds | GRIB2 winds | Name the current decoder subset: grid 3.0/3.30, product 4.0/4.8 and representations 5.0/5.2/5.3/5.40 with documented bitmap reuse. | Preview subset. No general encoder; general GRIB2 and representation 5.41 are unsupported. |
| L0 WGS-84 geodesy | distance, azimuth, intersections, point-in-polygon | Include implemented inverse/direct/rhumb operations and bounded polygon containment only. | Preview by operation. Geodesic line/segment intersection API is absent. |
| Rust/Python | “from Rust and Python” | State binding availability per module and operation. Current reviewed surfaces are Rust; matching Python parser bindings are not established. | Bindings are future work and have their own API and §8 gates. |
| Test vectors | vectors for each listed item | Provide vectors only for implemented, scoped profiles; use synthetic or redistributable material with provenance. | A public vector corpus is not established by these pages. |

## L0 source surfaces

| Module | Draft tier | Source API notes | Principal gap |
|---|---|---|---|
| `oa-ieee` + `oa-math` | Preview | [math-ieee.md](foundations/math-ieee.md) | Per-function special-value contract; `oa-ieee::sqrt(-x)` NaN payload varies by target; aarch64 and §8 evidence remain. |
| `oa-geo` | Preview per operation | [geo.md](foundations/geo.md) | Intersections absent; boundary/topology caveats; examples/vectors and cross-target evidence incomplete. |
| `oa-time-window` | Preview supporting value | [time-window.md](foundations/time-window.md) | Unix-millisecond half-open intervals only; decoder policy is caller-supplied; 64-bit targets only. Supporting dependency, not a separate 0.1 feature commitment. |
| `oa-sha256` | Preview, integrity-only | [integrity.md](foundations/integrity.md) | Digest is not authentication. FIPS 180-4 implementation vectors do not establish FIPS 140 module validation. |
| `oa-deflate`, `oa-zip`, `oa-tar` | Preview by codec | [containers.md](foundations/containers.md) | Keep writer, untrusted-input readers, container subsets and extraction/path policy distinct; `oa-zip` depends on internal `oa-sealed`. |
| `oa-xml`, `oa-json`, `oa-csv` | Preview by grammar | [text.md](foundations/text.md) | Exact grammar, refusal policy, limits and caller-significant duplicate/partial behavior need public vectors and bindings. |

## L1 source surfaces

| Module | Draft tier | Source API notes | Principal gap |
|---|---|---|---|
| `oa-asterix` | Preview CAT021/048/062; CAT034 out of scope | [asterix.md](formats/asterix.md) | CAT034 codec absent; CAT021 edition must be explicit; category-specific encoder coverage. |
| `oa-arinc424` | Preview, FAA CIFP profile | [arinc424.md](formats/arinc424.md) | Unsupported sections skipped; no encoder; identify cycle and complete evidence. |
| `oa-nasr` | Preview, subscription-specific | [nasr.md](formats/nasr.md) | Carry NASR cycle/schema identity with each dataset; no encoder. |
| `oa-notam` | Preview, FAA FNS profile | [aixm-notam.md](formats/aixm-notam.md) | No encoder or publisher; public synthetic vectors and source-position error policy remain open. |
| `oa-fixm` | Preview, decode only | [fixm-sfdps.md](formats/fixm-sfdps.md) | No encoder/publisher; source-location errors and internal dependency cut remain open. |
| `oa-grib2` | Preview, decoder subset | [grib2-winds.md](formats/grib2-winds.md) | Partial templates/packing, no public encoder, `f32` cross-platform identity not established. |

## Later tiers, outside 0.1

These systems appear in the broader library vision, but §10's proposed first release is L0 plus
selected L1 formats. The notes below are future API design proposals, not release commitments.

| Area | Current internal modules | Draft tier | Source/API note |
|---|---|---|---|
| Airspace/adaptation model | `oa-airspace` | L2 Preview | [airspace.md](modules/airspace.md): internal NASR/CIFP-derived ARTCC model, geometry and route conversion; not general worldwide airspace or filed-route semantics. |
| Surveillance tracking/fusion | `oa-track` | L3 Experimental | [tracking.md](engines/tracking.md): internal ADS-B/radar/fusion work; dependency graph includes benchmark, simulator, seal and other workspace crates; no clean public streaming boundary. |
| Safety/common status | `oa-safety`, `oa-ca`, `oa-msaw` | L3 Experimental | [safety.md](engines/safety.md): caller-owned config, explicit limits and degraded/unavailable reports; no FAA policy value should be inferred from synthetic fixtures. |
| Flight state/replay | `oa-flight`, `oa-flight-sfdps`, `oa-replay`, `openatca-core` | L3 Experimental or tool/example | [replay-flight.md](engines/replay-flight.md): distinguish standards-facing codecs from offline replay wrappers; do not present internal recording formats as standards. |

## Cross-cutting proposal requirements

- **Edition and provenance:** name exact standard/profile, accepted editions/templates, source
  schema cycle/hash and unsupported records. Do not infer profile from filename or prefix.
- **Units and time:** name units and reference frame on every public value. Distinguish event,
  applicability, receive and message time; state UTC/day-relative versus Unix time and clock units.
- **Limits and errors:** expose bounds where callers may set them. Document hard ceilings, atomic
  versus prefix decode, count-and-skip behavior, source/byte/record positions and counter overflow.
- **Determinism and panic behavior:** state only evidence actually established. Distinguish shell
  I/O, runtime allocation, floating-point edge cases and cross-architecture evidence.
- **Dependencies:** list allowed internal `oa-*` composition and external/runtime dependencies.
  Remove internal `oa-sealed`, benchmark and simulator dependencies from published package
  boundaries.
- **Tiers:** assign tiers per exact capability/profile. “Core” does not imply Stable. Keep decoder,
  encoder, query, simulator, replay and bindings separate.
- **Vectors and bindings:** publish licensed, language-neutral vectors for each included profile.
  Bindings must pass the same vectors; do not claim C/Python support before implementation and
  verification.

The per-module pages describe current source behavior and provisional public surfaces. Source
inspection alone does not establish the §8 Stable bar: reference tests, at least 24 hours of fuzzing,
Kani where integer/byte logic warrants it, reviewed mutation results, no-panic evidence,
x86-64/aarch64 output identity, Rust/Python examples and published vectors are still required.
