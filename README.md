# OpenATCA

**Open Air Traffic Control Automation.** Free, open-source software for the work ERAM and STARS do
today: tracking every aircraft, keeping them apart, and giving controllers the picture and the
tools to run en route and terminal airspace. It is built to be adopted: it speaks the legacy
systems' own interfaces, so a facility can move to it one function at a time and move back if it
needs to.

OpenATCA is an early-stage operational prototype. It runs on recorded and simulated traffic, and on
live public feeds for testing. Nothing here is certified, approved or connected to anything that
affects the national airspace. [docs/STATUS.md](docs/STATUS.md) says what has been checked, how,
and what has not.

## Why

The automation that separates aircraft over the United States is closed, ageing and hard to
change. On 13 November 2026, communications between en route centers failed under load and the
FAA stopped departures across the country for hours. The FAA plans to replace ERAM and STARS with
a Common Automation Platform. The GAO puts that phase at about $10 billion, and the FAA's request
for information asks for open standards, published interfaces, data portability and no vendor
lock-in.

OpenATCA aims to show that the software part does not need billions. The goals:

- **Better on every function we can measure** than the systems it replaces, with the methods and
  the evidence published.
- **Built to the FAA's own rules** for safety, assurance, security, maintenance and testing,
  instead of fitting them on afterwards.
- **Easy to adopt.** It speaks every legacy interface natively, runs in shadow beside the current
  system, and takes over one function or one sector at a time, with the old system ready behind
  it.
- **Free and auditable.** Anyone can read the code, rerun the tests and reproduce the results.
  Fixes reach every user.

## What works today

- **Airspace from the FAA's public data.** NASR and CIFP become a validated model of an en route
  center's airspace, procedures and routes ([Chicago Center report](docs/M0_REPORT.md)).
- **Every public FAA feed.** Decoders and recorders for SWIM SFDPS (en route flight and track
  data), TFMS, TBFM, FNS NOTAMs and STDDS terminal data, plus public ADS-B archives. All of it
  replays bit for bit: the same input gives identical output on x86-64 and aarch64.
- **Surveillance tracking.** A multi-sensor tracker for ADS-B and radar, using interacting
  multiple models, online radar registration and fixed capacities throughout. Radar has been tested
  on simulated plots so far; a first real radar dataset is being sought.
- **Comparison with ERAM.** A harness that scores OpenATCA against ERAM's own published tracks on
  the same recorded traffic. A comparison with a protocol fixed in advance is under way; its
  protocol and results will be published together.
- **Safety nets and flight data.** Conflict alert, minimum safe altitude warning with terrain and
  obstacles, flight data with amendments and handoffs, and explicit degraded states, running
  together in one offline regional session.
- **A controller display foundation.** A deterministic display and command path, used by a first
  workstation prototype.
- **Whole-country compute.** On the busiest hour of 19 November 2026 (15,107 aircraft over the
  contiguous United States), ADS-B tracking with conflict alert used about a quarter of one laptop
  core at two reports per aircraft per second ([measurement and limits](docs/M9_BUDGET.md)). Radar
  fusion at national load is not yet within budget.

## What comes next

- **Behaviour matched to the legacy system:** the same handoffs to the same sectors at the same
  times as ERAM on the same traffic, measured from ERAM's published data.
- **Migration machinery:** per-function shadow, comparison, cutover and rollback, and facility
  adaptation as versioned, checksummed data.
- **Maintenance built in:** service certification from the system's own displays, a
  monitor-and-control position, and performance checks on demand.
- **An independent backup path** for the safety-critical services, written separately so that one
  fault cannot take down both.
- **A controller workstation** that experienced ERAM and STARS controllers can use without
  retraining.
- **Real radar data and continuous live shadow running.**

## How it is built

The design rules are in [docs/RULESET.md](docs/RULESET.md). How far the code meets each one
today is in [docs/STATUS.md](docs/STATUS.md).

- **Migration first.** Every function can run in shadow beside a legacy system, be compared with
  it, be cut over on its own and be rolled back.
- **Overload is designed, not discovered.** Every queue and collection has a stated capacity and
  a written behaviour when it is full. Inconsistent data is refused and counted, never passed on.
- **Deterministic replay.** The core is a pure function of its inputs, with time as an input, so
  any run can be replayed bit for bit.
- **Assurance from the first commit.** Requirements are numbered and traced to tests, toward RTCA
  DO-278A. Byte-level decoders carry bounded proofs (Kani) and are fuzzed, and mutation testing
  strengthens the tests ([verification](docs/VERIFICATION.md), [fuzzing](docs/FUZZING.md)).
- **Better than parity.** Each function is measured against the legacy approach on the same
  traffic and must beat it before it is called finished ([benchmarks](docs/BENCHMARKS.md)).
- **No outside code in the core.** Rust standard library only. Parsers, geodesy, hashing and
  decompression are written here and tested against published references.
- **Data-oriented design.** Plain data, struct-of-arrays, typed indexes and compile-time size
  guards.

## Repository

| Document | What it covers |
|---|---|
| [SCOPE.md](SCOPE.md) | What the project is, what is in and out of scope, the data, the milestones |
| [docs/STATUS.md](docs/STATUS.md) | What has been checked, how, and what has not |
| [docs/CAPABILITIES.md](docs/CAPABILITIES.md) | Every capability, its entry point, and what it still needs |
| [REQUIREMENTS.md](REQUIREMENTS.md) | The requirements register; every requirement has a test or proof |
| [docs/BENCHMARKS.md](docs/BENCHMARKS.md) | How "better than legacy" is measured |
| [docs/LEGACY_SHORTCOMINGS.md](docs/LEGACY_SHORTCOMINGS.md) | Documented ERAM and STARS failures, each turned into a requirement or test |
| [docs/INTERFACES.md](docs/INTERFACES.md) | How OpenATCA talks to systems it does not control |
| [docs/NODE_NETWORK.md](docs/NODE_NETWORK.md) | How facilities compute the national picture, check each other and survive alone |
| [docs/UNKNOWNS.md](docs/UNKNOWNS.md) | Open questions, each with why it matters and the next step |
| [DATA.md](DATA.md) | Every input dataset with its source and SHA-256 |

| Area | Crates and tools |
|---|---|
| Airspace and aeronautical data | `oa-airspace` (and `oa-adapt`), `oa-nasr`, `oa-arinc424`, `oa-mva`, `oa-dof`, `oa-geo` |
| FAA feeds and recording | `oa-feed`, `oa-xml`, `oa-fixm`, `oa-flight-sfdps`, `oa-tfms`, `oa-tbfm`, `oa-notam`, `oa-stdds`, `oa-nam` |
| Surveillance | `oa-track`, `oa-adsb`, `oa-asterix` (CAT021, 048, 062), `oa-sensorsim`, `oa-truth` |
| Flight data and safety nets | `oa-flight`, `oa-ca`, `oa-msaw`, `oa-safety`, `oa-runway`, `oa-encounter` |
| Weather and terrain | `oa-grib2`, `oa-j2k`, `oa-wind`, `oa-awc`, `oa-tiff` |
| Replay and display | `oa-replay`, `oa-display`, `oa-display-present`, `oa-scope-replay` |
| Comparison and measurement | `oa-bench`, `tools/oa-shadow`, `tools/oa-perf`, `tools/oa-compass`, `tools/oa-stdds-ref` |
| Foundations | `oa-math` (bit-identical elementary functions), `oa-sha256`, `oa-deflate`, `oa-zip`, `oa-tar`, `oa-csv`, `oa-json`, `oa-ieee`, `oa-sealed` |
| Assurance | `tools/oa-trace` (requirements traceability), the Kani proof suite, fuzz targets |
| Library | `openatca-core`: replay recorded ADS-B through the tracker from your own code ([guide](crates/openatca-core/README.md)) |

## Building and testing

```
cargo test
cargo run -p oa-trace          # every requirement must have a test
```

Tests that need full FAA datasets read them from paths in environment variables (`OPENATCA_RAW`,
`OPENATCA_DATA`, `OPENATCA_CIFP`, `OPENATCA_NASR`, `OPENATCA_GEODTEST`, `OPENATCA_ADSB_CLIP`) and
skip when they are unset. [DATA.md](DATA.md) lists where to download each one.

## Data

OpenATCA uses the FAA's public aeronautical data (NASR and CIFP). CIFP recipients agree to pass
reported CIFP errors on to their users and to report errors they find to FAA Aeronautical
Information Services.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE)). Unless you state otherwise,
any contribution you submit for inclusion is licensed under the same terms, without additional
terms or conditions.

Copyright 2026 Avila Labs.
