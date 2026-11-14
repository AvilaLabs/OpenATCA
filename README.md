# OpenATCA

**Open Air Traffic Control Automation.** An open-source platform for the job the FAA's planned
Common Automation Platform is meant to do: one system for en route and terminal air traffic
control automation, replacing ERAM and STARS.

OpenATCA is early. The first milestone, a validated model of the airspace built from the FAA's
public data, is in progress. Nothing here is certified or approved for operational use.

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
| [SCOPE.md](SCOPE.md) | What the project is, how it would be used, what is in and out of scope |
| [REQUIREMENTS.md](REQUIREMENTS.md) | Requirements register; every requirement has a test |
| [docs/RULESET.md](docs/RULESET.md) | The engineering law every change is reviewed against |
| [docs/M0_DESIGN.md](docs/M0_DESIGN.md) | Milestone 0: airspace foundation |
| [docs/UNKNOWNS.md](docs/UNKNOWNS.md) | Open questions, each with why and the next step |
| [DATA.md](DATA.md) | Every input dataset with its source and SHA-256 |
| `crates/oa-csv` | RFC 4180 reader |
| `crates/oa-geo` | WGS-84 geodesy (Karney), rhumb lines, point-in-polygon |
| `crates/oa-nasr` | FAA NASR parsers with validation against the FAA's own schema |
| `crates/oa-arinc424` | FAA CIFP (ARINC 424-18) parser |
| `crates/oa-sha256` | SHA-256, verified against NIST CAVP vectors |
| `crates/oa-zip` | ZIP and DEFLATE reader with hard limits |
| `crates/oa-airspace` | The airspace model and the `oa-adapt` tool |
| `tools/oa-trace` | Requirements traceability checker |

## Building and testing

```
cargo test
cargo run -p oa-trace          # every requirement must have a test
```

Tests that need full FAA datasets read them from paths in environment variables (`OPENATCA_RAW`,
`OPENATCA_DATA`, `OPENATCA_CIFP`, `OPENATCA_NASR`, `OPENATCA_GEODTEST`) and skip when they are
unset. [DATA.md](DATA.md) lists where to download each one.

## Data

OpenATCA uses the FAA's public aeronautical data (NASR and CIFP). CIFP recipients agree to pass
reported CIFP errors on to their users and to report errors they find to FAA Aeronautical
Information Services.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you state otherwise, any contribution you submit for inclusion is dual
licensed as above, without additional terms.

Copyright 2026 Avila Labs.
