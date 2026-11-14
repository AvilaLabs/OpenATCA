# OpenATCA

**Open Air Traffic Control Automation.** An open-source platform for the job the FAA's planned
Common Automation Platform is meant to do: one system for en route and terminal air traffic
control automation, replacing ERAM and STARS.

OpenATCA is early. The first milestone, a validated model of Chicago Center's airspace built
from the FAA's public data, is complete ([report](docs/M0_REPORT.md)); recording and replay of
live FAA data is in progress. Nothing here is certified or approved for operational use.

## Why

On 2026-11-13 the FAA stopped every departure in the United States for hours after
communications between en route centers failed under load. The software that controls the
national airspace is closed, hard to change, and expensive to replace. The FAA's own request for
information on its replacement asks for open standards, published interfaces, data portability
and freedom from vendor lock-in. An open-source platform provides those by construction.

## Principles

- **Migration first.** Every function can run in shadow beside a legacy system, be compared with
  it, be cut over on its own and be rolled back.
- **Overload is designed, not discovered.** Every queue and collection has a stated capacity and
  a written behaviour when it is full. Inconsistent data is refused and flagged, never passed on.
- **Deterministic replay.** The core is a pure function of its inputs, with time as an input, so
  any run can be replayed bit for bit.
- **Assurance built in.** Requirements are numbered and traced to tests from the first commit,
  toward RTCA DO-278A.
- **Better than parity.** Each function is measured against the legacy approach and must beat it.
- **Zero dependencies in the core.** Rust standard library only. Parsers, geodesy, hashing and
  decompression are written here and tested against published references.
- **Data-oriented design.** Plain data, struct-of-arrays, typed indexes, compile-time size
  guards. See [docs/RULESET.md](docs/RULESET.md).

## Layout

| Path | What |
|---|---|
| [SCOPE.md](SCOPE.md) | What the project is, how it would be used, what is in and out of scope, data inventory, milestones |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | How "best in class" is measured: published floor, legacy measured on the same traffic, best published result |
| [docs/LEGACY_SHORTCOMINGS.md](docs/LEGACY_SHORTCOMINGS.md) | Documented ERAM and STARS failures, each turned into a requirement or test scenario |
| [REQUIREMENTS.md](REQUIREMENTS.md) | Requirements register; every requirement has a test |
| [docs/RULESET.md](docs/RULESET.md) | The engineering law every change is reviewed against |
| [docs/M0_DESIGN.md](docs/M0_DESIGN.md) | Milestone 0: airspace foundation |
| [docs/M0_REPORT.md](docs/M0_REPORT.md) | Milestone 0 result: the ZAU model, checks, defects found, limits |
| [docs/UNKNOWNS.md](docs/UNKNOWNS.md) | Open questions, each with why and the next step |
| [docs/ADSB_PAYLOAD.md](docs/ADSB_PAYLOAD.md) | ADS-B history reports carried in OAF1 frames |
| [docs/INTERFACES.md](docs/INTERFACES.md) | How we talk to systems we do not control, so we never cause or amplify their failures |
| [DATA.md](DATA.md) | Every input dataset with its source and SHA-256 |
| `crates/oa-csv` | RFC 4180 reader |
| `crates/oa-geo` | WGS-84 geodesy (Karney), rhumb lines, point-in-polygon |
| `crates/oa-nasr` | FAA NASR parsers with validation against the FAA's own schema |
| `crates/oa-arinc424` | FAA CIFP (ARINC 424-18) parser |
| `crates/oa-sha256` | SHA-256, verified against NIST CAVP vectors |
| `crates/oa-zip` | ZIP and DEFLATE reader with hard limits |
| `crates/oa-airspace` | The airspace model and the `oa-adapt` tool |
| `crates/oa-xml` | Bounded XML 1.0 parser for FAA SWIM messages |
| `crates/oa-json` | Strict bounded RFC 8259 JSON parser |
| `crates/oa-tar` | Streaming ustar/GNU tar reader, split archives, typed truncation |
| `crates/oa-adsb` | adsb.lol / readsb trace decoder and the `oa-adsb-clip` tool |
| `crates/oa-truth` | Reference trajectories from ADS-B for the M2 benchmark: quality selection, legs, RTS smoother, no-extrapolation queries, leave-one-out accuracy, and the `oa-truth-check` tool |
| `crates/oa-replay` | Replay of recorded SWIM feeds (SFDPS, TBFM, TFMS, FNS) and ADS-B clips (`--adsb`, `--adsb-dir`) in one time-ordered stream; flight and aircraft state with a deterministic hash; the `oa-replay` tool |
| `crates/oa-display` | Controller display foundation: display list, projection, stroke font, software rasterizer, `oa-scope` |
| `crates/oa-tiff` | TIFF 6.0 / GeoTIFF elevation reader (strips, tiles, Deflate, LZW, predictors 2 and 3) and terrain queries |
| `crates/oa-dof` | FAA Digital Obstacle File parser |
| `crates/oa-grib2` | GRIB2 decoder for NOAA RAP and HRRR winds and temperatures aloft |
| `crates/oa-j2k` | JPEG 2000 decoder for the profile of GRIB2 template 5.40 (NOAA RAP packing) |
| `crates/oa-mva` | FAA Minimum Vectoring Altitude and center MIA charts (AIXM 5.1) as a queryable model, and the `oa-mva-check` validation tool |
| `crates/oa-awc` | Aviation Weather Center JSON (METAR altimeter lookup, PIREP, SIGMET/AIRMET/G-AIRMET, TAF) and `oa-awc-summary` |
| `crates/oa-bench` | M2 benchmark harness: legacy sample extraction, association, metrics T1 to T9, F1 to F3, R1, H1, synthetic self-test, truth and airspace wiring (`oa-truth`, M0 model), report writer, and the `oa-bench` binary |
| `tools/oa-trace` | Requirements traceability checker |

## Building and testing

```
cargo test
cargo run -p oa-trace          # every requirement must have a test
```

Tests that need full FAA datasets read them from paths in environment variables (`OPENATCA_RAW`,
`OPENATCA_DATA`, `OPENATCA_CIFP`, `OPENATCA_NASR`, `OPENATCA_GEODTEST`, `OPENATCA_ADSB_CLIP`) and skip when they are
unset. [DATA.md](DATA.md) lists where to download each one.

## Data

OpenATCA uses the FAA's public aeronautical data (NASR and CIFP). CIFP recipients agree to pass
reported CIFP errors on to their users and to report errors they find to FAA Aeronautical
Information Services.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)). Unless you state
otherwise, any contribution you submit for inclusion is licensed under the same terms, without
additional terms or conditions.

Copyright 2026 Avila Labs.
