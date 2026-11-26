# Synthetic demo scenario format (draft)

Status: engineering proposal for F2, not a controller-fidelity claim. The normal ORD push is
specified as 60 generated aircraft over 45 virtual minutes. Aircraft IDs use `SYN-0000` through
`SYN-0059`; there are no real callsigns, addresses, flight plans, SWIM messages, or surveillance
observations in the scenario source. The seeded schedule is a deterministic synthetic workload,
not a reconstruction or measurement of ORD demand.

`SYN-0000` through `SYN-0059` are stable host-side generation keys only. They have no callsigns or
flight-identification labels and must not be presented as real workstation fields. This scenario
does not assert deployed command grammar or exact workstation responses.

## Portable intent

`oa-demo-scenario` is a split Rust crate. `core/` holds pure, bounded intent, traffic and turn
geometry; `shell/` resolves public CIFP input and validates OASD bytes. Its public records represent a versioned
scenario request, exact public procedure references, source provenance, and generated synthetic
flight plans. Time values are integer milliseconds since scenario T0. The fixed seed determines
only deterministic +/-15-second synthetic arrival jitter and route-variant assignment. The
baseline prep source pins seed `0x4F52442D4E505631` (`ORD-NPV1`). It does not add entropy or modify
public geometry or performance inputs. Input event records for the browser host use
`virtual_time_ms: u64`, a typed `input_kind: u8`, and bounded type-specific bytes; JavaScript may
not send aircraft truth. Rust owns scenario events and traffic rules. The host returns canonical
display-list/frame bytes and bounded health counters; hashes apply to those canonical bytes.

`resolve_procedure_route` / `resolve_route_chain` resolve selected CIFP transitions using fix
positions carried by the parsed CIFP. Unsupported or ambiguous fix records refuse the route;
they are not replaced with guessed coordinates. A selected normal arrival is assembled from its
published STAR transition, common route and runway transition, followed by the selected published
approach transition and final route when those records resolve and their fix endpoints join. If
the STAR exit and approach entry are different published fixes, prep requires the explicit
`--synthetic-direct` option and records a separate `SYN/DCT` route reference. That straight
geodesic flight-plan segment uses only the two source fixes; it is a declared synthetic scenario
assumption, not a published CIFP leg or operational route, and must appear in the fidelity delta.
`materialize_traffic` generates scenario-owned fixed-width `SyntheticReport` samples with relative virtual time. The browser
runtime does not depend on the ADS-B parser or wire-format package. The private offline prep
shell can map those samples to explicitly synthetic ADS3 rows, build truth, and generate radar
plots with `oa-sensorsim` using a named profile and explicit site settings. This is simulated
traffic, not a performance model or captured aircraft data.

The private preparation CLI is `tools/oa-demo-scenario-prep`. Its source requires the pinned
public CIFP archive, a caller-pinned public 737 type-certificate PDF, and exact airway, STAR, and
approach route selectors. Published route-component endpoints must match exactly; the separate
synthetic DCT exception is explicit in the CLI, route source, and provenance. Unsupported fixes,
ambiguous transitions, parser errors, hash mismatches, oversized output, or a route that cannot
meet the selected altitude constraints refuse generation. It writes a synthetic ADS3 report payload, a
CAT048 radar payload, and a provenance record under `openatca-private/demo-scenarios` with
create-new semantics. The source does not run as part of tests or builds.

The CLI defaults to `--outputs all`. The ADS3 source rows are split into ordered files of at most
50,000 rows each, with contiguous row ranges, per-file digests, and a digest over the ordered
frame manifest; all rows must be conserved. `--outputs oasd` writes only the bounded OASD asset
and provenance record; it does not construct ADS3, truth, or CAT048 products, so the browser asset
path does not depend on the legacy ADS3 frame limit.

The default scenario assumptions are supplied explicitly by the caller: nominal groundspeed,
an independent synthetic indicated-airspeed proxy, initial altitude, maximum synthetic vertical
rate, and sample cadence. Groundspeed determines synthetic position and timing; the IAS proxy is
checked separately against CIFP speed restrictions and the selected profile VMO. No atmosphere
or wind model converts between them. The generator follows WGS-84 geodesics between CIFP fixes
and currently changes heading instantaneously at fixes. It does not execute course-to-fix,
course-to-altitude, vector/intercept, manual-termination, or holding path-terminator semantics. A
`SYN/DCT` connector, when selected, is an explicit synthetic straight geodesic between two
published endpoint fixes, not a published leg. These paths are source-ordered synthetic geometry,
not operational procedure compliance or a realistic B738 trajectory. It rejects invalid
coordinates, fix-order changes, unsupported altitudes, excessive vertical rate, and out-of-range
timestamps. These assumptions describe the synthetic demo path only; they must remain in scenario
provenance and outside any controller-facing screen.

`build_fly_by_fillet` is a separate pure geometry helper for a single A→B→C waypoint triple. It
uses the existing B-centered AEQD frame, an explicit radius (or `speed / turn_rate` helper), and
bounded arc sampling. It refuses degenerate/reversal turns and legs shorter than the requested
tangent distances; it never reduces the chosen radius. This helper is not yet wired into
`materialize_traffic` and does not validate CIFP flyover semantics, altitude/speed constraints,
adjacent-fillet overlap, or full-route capture. It cannot be treated as evidence that generated
traffic already follows bounded turns.

## OASD v1 prepared asset

The private preparation shell writes `ord-normal-push-v1.oasd` for the native/browser scenario
engine. It is a 352-byte little-endian header followed by 32-byte little-endian records, bounded
to 360,000 records and 16 MiB total. The header binds the scenario identifier, fixed seed, epoch,
duration, report cadence, seven source SHA-256 digests, the digest of the exact record bytes, and
the KORD airport reference point from the pinned CIFP entry rounded to signed E7. Reserved fields
must be zero. The shell `ScenarioArtifact::parse` API validates the complete header, row hash,
record bounds, and strict `(virtual_time_ms, aircraft_index)` ordering before exposing a borrowed
record iterator. The shell parser is exposed as `oa_demo_scenario::shell::ScenarioArtifact` and
re-exported at the crate root. The artifact contains synthetic scenario samples only; it is not raw surveillance
or controller-procedure evidence.

Header offsets are: `0..4` magic, `4..6` version, `6..8` header length, `8..10` record length,
`10..12` flags, `12..16` reserved, `16..48` NUL-padded scenario ID, `48..56` seed, `56..64`
epoch nanoseconds, `64..72` duration milliseconds, `72..76` sample interval milliseconds,
`76..80` row count, then seven raw SHA-256 digests at `80..304` in order (CIFP archive, CIFP
entry, performance source, scenario library, route resolver, traffic materializer, prep tool),
`304..336` row digest, `336..340` KORD ARP latitude E7, `340..344` longitude E7, and zero padding
at `344..352`. Row offsets are aircraft index u16, flags u16 (bit 0 new-leg), virtual time i64,
latitude E7 i32, longitude E7 i32, altitude feet i32, groundspeed deciknots u16, track decidegrees
u16, and vertical rate fpm i32. The parser validates structure and the row digest; source hashes
are provenance values and require a separate trusted source/hash comparison by the loader. The
provenance record also carries the whole-file OASD hash and an ordered source-bundle inventory
with its own digest. The loader must pin/compare the whole-file hash separately; a self-reported
header digest alone does not authenticate the artifact.

## Source record

Each route records the official FAA CIFP archive URL, filename, cycle, expected archive SHA-256,
extracted `FAACIFP18` entry name and computed SHA-256, airport, procedure kind, procedure and
transition identifiers, and ordered fix sequence. These identify public navigation geometry
only. The source archive metadata currently listed in `DATA.md` is cycle 2026-10-29,
`CIFP_261029.zip`, SHA-256
`e3ff2b31a4f6ad3eda1342b83ada1ae399aae7bd8c1e67f3cacd5b2886b5e141d`.
The preparation tool must verify that pin before parsing and report the parser version and
cycle. No raw archive or extracted CIFP bytes are committed.

The first synthetic airframe is the Boeing 737-800 NG. Its public type-certificate upper bounds
are VMO 340 KCAS and MMO Mach 0.82 (FAA Type Certificate Data Sheet A16WE, 737-800 series;
the same values appear in the current Japanese Civil Aviation Bureau 737 Type Certificate Data
Sheet No. 38, revision 12, 2026-09-24). The simple synthetic path uses a constant 220 kt
groundspeed target and an independent 220 kt IAS proxy. CIFP speed restrictions and VMO apply
only to the IAS proxy; there is no atmosphere or wind conversion to groundspeed or Mach. It does
not evaluate MMO, turn performance, or acceleration and is not a complete aircraft performance
envelope model. Published CIFP procedure altitude and speed constraints further restrict route
samples. Below 10,000 ft
MSL, 14 CFR 91.117(a) supplies a 250 KIAS regulatory speed ceiling unless authorized otherwise.
The scenario uses that as a conservative ceiling, not as a claim that a real aircraft would fly
at that speed.

For each source, the prepared scenario records the document title, canonical URL, revision,
SHA-256, aircraft type, and exact model fields used. `oa-adsb::SPEED_ENVELOPE_KT` is a
surveillance payload/category bound, not a commercial-aircraft performance model, and cannot
be substituted. Boeing's official 737NG airport-planning manual D6-58325-7 Rev C (Oct 2025)
provides generic payload/range and takeoff/landing planning data; Boeing says it is general
airport-planning data and labels the performance charts “DO NOT USE FOR DISPATCH.” It is not
being treated as an operational trajectory model. Climb/descent rates, turn smoothing, wind,
and speed targets must remain explicit deterministic scenario assumptions; they are never shown
as actual aircraft-performance predictions. A named `oa-sensorsim` profile and each site
parameter are separately recorded; radar-site coverage, biases and position are explicit
scenario assumptions unless independently sourced. All performance timing and sensor behavior
in this scenario is synthetic.

## Boundaries and next materialization gate

- The initial route manifest will use only real procedure and airway identifiers resolved from
  the pinned public CIFP. It will not invent a route, sector, or controller display element.
- Evaluation sectors must be generated from public boundaries and explicitly labelled as
  evaluation geometry; no adaptation or reconstructed facility sector data is allowed.
- Materialized traffic needs bounded, source-identified performance envelopes and a route
  resolver that proves every generated route follows its selected public fixes/procedure legs.
- Scenario output must be reproducible from format version, generator version, seed, public
  input hashes, and performance-profile hashes. Any unsupported leg or input mismatch is a
  hard refusal, not an approximate-success status.
- Synthetic report quality/emitter fields are generated specifically to exercise the existing
  truth pipeline. They do not assert actual ADS-B transmitter performance or fidelity.
- The WASM host receives only typed user events and clock deltas. It cannot submit synthetic
  positions or override the Rust-owned scenario.

## Sources

- FAA CIFP, official data location: <https://aeronav.faa.gov/Upload_313-d/cifp/CIFP_261029.zip>
- FAA Type Certificate Data Sheet A16WE (737-series): <https://drs.faa.gov/browse/excelExternalWindow/DRSDOCID198878707920250129005139.0001>
- JCAB 737 Type Certificate Data Sheet No. 38 Rev 12: <https://asims.cab.mlit.go.jp/fsdb/a_katashikisyoumei.nsf/0/b98da61b29ae7a89492575850030a18c/%24FILE/19756776.pdf/JCAB_TCDS_No.38_Rev.12_737Series_September_24%2C2026.pdf>
- Boeing 737NG airport-planning document index: <https://www.boeing.com/commercial/airports/plan-manuals>
- 14 CFR 91.117, aircraft speed: <https://www.ecfr.gov/current/title-14/chapter-I/subchapter-F/part-91/subpart-B/section-91.117>
- Scenario requirements: private `DEMO_SPEC.md`, sections 3–7.
- Parser reference: `crates/oa-arinc424`; radar profiles: `crates/oa-sensorsim/src/profile.rs`.
