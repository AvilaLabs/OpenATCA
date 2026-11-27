**Status: draft engineering design. API names and release tiers require review before they are frozen.**

# OpenATCA Core: the library spec

Private draft, written 2026-11-25 by the reviewer for Connor and for the coordinator to build. It replaces the
narrow 0.1.0 replay wrapper (`crates/openatca-core`, 22 Nov) as the definition of "OpenATCA Core".
The public documentation is written from this spec when the first release is ready.

## 1. What it is for

Connor's aim is to help start an open-source ecosystem in aviation, which barely exists today.
The model is what Bitcoin Core's libraries are to Bitcoin developers. Even developers who dislike
Bitcoin Core build their wallets on its libraries. That is because those libraries hold the parts
where a subtle mistake is dangerous: consensus rules and signature cryptography. They are also the
most-reviewed code in the space, so writing your own is risky and pointless. The Lightning Dev
Kit does the same for Lightning: a whole node as a library, where the developer brings their own
storage, networking and wallet.

**OpenATCA Core is that for aviation.** It holds the parts every aviation developer needs, gets
wrong and keeps rewriting: the data formats, geodesy, airspace and procedures, tracking, trajectory
prediction and the safety algorithms. Each is correct against published references, has stated
limits and is tested harder than anyone would bother to test their own copy.

It is **the same code OpenATCA runs on, not a fork.** OpenATCA's operational system is built on
Core, just as Bitcoin Core's node is built on its own libraries. Every improvement reaches both.

## 2. Who uses it, and for what

| User | What they do with Core |
|---|---|
| A researcher in Python | Decodes ASTERIX recordings, ADS-B, SWIM flight data and CIFP procedures without writing a parser |
| A drone or UTM developer | Airspace volumes and procedures as geometry; conflict detection with proven properties |
| An airport or a small air navigation provider | A tracker and safety nets fed by their own sensors, without a vendor contract |
| A flight simulation developer (VATSIM, BlueSky) | Real procedures, airspace and the controller-facing data formats |
| A system vendor, including a CAP finalist | Verified components inside their own platform |
| Another tracker's developer | Published test vectors to check their implementation, and a reference to compare against |
| The FAA or a laboratory | An independent reference implementation of the formats and algorithms in their systems |

## 3. Design rules

1. **Correct before broad.** Every module is tested against published references: standards'
   own examples, NIST vectors, GeodTest and the like. Decoders are fuzzed and refuse bad input
   with a typed error and its position; they never panic or guess. Proofs where they apply.
2. **Standard formats at every boundary.** Inputs and outputs are the industry's formats
   (ASTERIX, FIXM, AIXM, ARINC 424, GRIB2, IWXXM) or plain documented types. OpenATCA-only
   formats such as OAF may be offered, but never required.
3. **No dependencies.** The Rust standard library only, as now. The one exception is the
   FIPS-validated cryptographic module (FAA list row E4). It stays out of the format and
   algorithm layers.
4. **Deterministic.** The same input gives the same bits on every supported platform. `oa-math`
   already makes that possible.
5. **Bounded.** Every collection has a stated capacity and a written behaviour when full,
   following RULESET.
6. **Usable from Rust, C and Python.** Most aviation data work is done in Python. Rust only would
   cut out most of the people the ecosystem is for.
7. **Stable, with tiers.** Every module is marked Stable, Preview or Experimental. Stable means
   semantic versioning, a deprecation period before any break, and the minimum Rust version only
   rising in a minor release.
8. **Nothing project-internal in the public API.** The hold-out seal (`oa-sealed`), the benchmark
   harness and the simulator are OpenATCA's own tools, not library dependencies.

## 4. Layers

Most of this already exists as workspace crates (line counts as of 2026-11-25). The work is to
shape it into a product, not to write it again.

| Layer | Modules | Exists today | Missing |
|---|---|---|---|
| L0 Foundations | Bit-identical maths, WGS-84 geodesy (Karney), hashing for integrity, DEFLATE, ZIP and tar, XML, JSON and CSV | `oa-math`, `oa-ieee`, `oa-geo`, `oa-sha256`, `oa-deflate`, `oa-zip`, `oa-tar`, `oa-xml`, `oa-json`, `oa-csv` | Public API review; `oa-zip` depends on `oa-sealed` |
| L1 Formats | ASTERIX; Mode S and 1090ES; FIXM/SFDPS, TFMS and TBFM; AIXM (NOTAMs, MVA charts); ARINC 424 (CIFP); NASR; GRIB2 and JPEG 2000; METAR/TAF; IWXXM; GeoTIFF terrain; the Digital Obstacle File; STDDS; NAM | `oa-asterix` (021, 048, 062), `oa-fixm`, `oa-tfms`, `oa-tbfm`, `oa-notam`, `oa-iwxxm`, `oa-mva`, `oa-arinc424`, `oa-nasr`, `oa-grib2`, `oa-j2k`, `oa-awc`, `oa-tiff`, `oa-dof`, `oa-stdds`, `oa-nam`, `oa-adsb` (readsb traces) | **Raw Mode S/1090ES decoding (DO-260B)**, ASTERIX 001/002/008/010/019/020/023/033/034/063/065, and encoders where we only decode |
| L2 Models | Airspace and procedures as geometry; terrain and obstacles; wind and temperature aloft | `oa-airspace`, `oa-mva`, `oa-wind` | Decouple from `oa-sealed`; a public query API |
| L3 Engines | Tracking and fusion; trajectory prediction; conflict detection; terrain alert (MSAW); flight data state | `oa-track`, `oa-ca`, `oa-msaw`, `oa-flight`, the shared trajectory baseline | `oa-track` depends on `oa-bench` and `oa-sensorsim`, so split those out. A public, streaming API for each engine |

Each module is published as its own package (`openatca-asterix`, `openatca-geo` and so on), so a
user takes only what they need. An umbrella `openatca` package re-exports the Stable modules.

## 5. Names

- **OpenATCA Core** is the library as a whole. Packages are `openatca-<module>` on crates.io, the
  `openatca` umbrella, `openatca` on PyPI and `openatca.h` for C. Name availability must be checked when a qualifying release is prepared; no names are reserved by this proposal.
- The present `openatca-core` replay wrapper becomes a module of the umbrella (`openatca::replay`)
  and an example, not the library's identity.
- Internal crate names (`oa-*`) may stay inside the workspace if renaming them causes too much
  churn. The published names are what users see.

## 6. Bindings

- **C:** a hand-written `extern "C"` layer with no dependencies. It uses opaque handles, caller-
  provided buffers, no ownership crossing the boundary and integer error codes with a message
  accessor. The header is hand-written and checked by a test. No cbindgen.
- **Python:** a pure-Python package over the C library through `ctypes`, with no compiled
  extension and no PyO3, so the no-dependency rule holds on both sides. Wheels are built for Linux
  x86-64 and aarch64 first, macOS and Windows later. The Python API is idiomatic, with iterators,
  dataclasses and exceptions, rather than a transliteration of the C one.
- Every binding has the same test vectors run through it (section 7). A binding passes only if it
  gives the same answers as Rust.

## 7. Test vectors

The equivalent of Bitcoin's BIP test vectors or Wycheproof, and the most valuable thing Core can
give the ecosystem.

- **Language-neutral files:** binary samples plus JSON with the expected decode, for every format
  and edition, including malformed inputs and the expected refusal. For the algorithms: geodesy
  cases, conflict-detection geometries with expected alerts, and tracker scenarios with known
  truth.
- **Anyone can check against them:** pyModeS, jASTERIX, traffic, BlueSky or a vendor's own code.
  When we disagree with another implementation, we find out who is right and report it upstream.
- **Licence and provenance:** the code is Apache-2.0 and the vectors are CC0. Vectors are
  synthetic or from data whose licence allows it. **Nothing derived from FAA SWIM data goes in**;
  write SFDPS and FIXM samples from the schema. adsb.lol excerpts wait for their permission.

## 8. What "Stable" requires

A module is marked Stable only when it has all of the following:
- reference tests against the published standard;
- at least 24 hours of fuzzing with no findings open;
- Kani proofs where the module is byte-level or integer logic;
- reviewed mutation results;
- no panic on any input;
- documented limits and editions;
- identical output on x86-64 and aarch64;
- API documentation with a runnable example in Rust and Python;
- its test vectors published.

## 9. Documentation

- A guide per layer, plus a cookbook of short, complete examples:
  - decode an ASTERIX recording;
  - read SFDPS flight data;
  - load a CIFP approach as geometry;
  - check two trajectories for conflict;
  - track aircraft from CAT048 radar plots;
  - get winds aloft at a point.
- Every function documents its units, time base, limits and errors.
- Plain language, written for a developer who is new to aviation as well as an expert.

## 10. First release (0.1)

Scope: L0 plus the most-requested L1 formats, from Rust and Python.
- Formats: ASTERIX 021, 034, 048 and 062; ARINC 424 (CIFP); NASR; AIXM NOTAMs; FIXM/SFDPS
  messages; IWXXM 2025-2 METAR/SPECI/TAF subset; GRIB2 winds.
- Geodesy: WGS-84 distance, azimuth, intersections and point-in-polygon.
- Test vectors for each of these.

Package publication is authorized when the module meets the Stable bar, CI is green on the exact commit, and its API document has been reviewed. Existing accounts are used; new PyPI packages require the owner to configure their first trusted publisher. Empty placeholder packages are not published.

**Later releases:**
- Raw Mode S and 1090ES decoding, the remaining ASTERIX categories.
- Then the engines with streaming APIs: tracker, trajectory, conflict detection and MSAW. Each one
  has proofs on its geometric core, in the style of NASA's DAIDALUS.

## 11. Being a good ecosystem member

- Work with the projects that exist rather than against them: pyModeS, traffic, OpenAP, OpenSky,
  jASTERIX and COMPASS (OpenATS), BlueSky (TU Delft), readsb and tar1090.
- Cross-check against them, send fixes upstream, and point to them where they are the better tool.
- Make it easy to contribute: a CONTRIBUTING guide, the review rules, and a list of good first
  modules.

## 12. How it serves the FAA list

- **C1:** an independent second implementation needs public formats and test vectors.
- **J2:** no lock-in, by construction.
- **G1:** every interface codec is reusable and checkable by others.
- **J4:** a CAP finalist or another provider can adopt our components before adopting OpenATCA
  whole.
- **D1:** published test vectors are the substance of tool accreditation.
