# Exit plan

This document says how a facility stops using OpenATCA, or stops depending on its current
maintainers, without losing data, function or time. It answers the FAA's no-lock-in requirement
(SCOPE §2, SEC R-27). Lock-in usually comes from four places: closed source, closed formats, a
single supplier for builds, and records that only the supplier can read. This plan takes each in
turn.

## 1. Source

All OpenATCA source is public under the Apache License 2.0. Anyone may copy, modify, build and
operate it, including a competitor or the FAA itself, with no licence fee and no permission
needed. The licence cannot be withdrawn from a copy already received.

The product depends on no third-party crates (E5). The Software Bill of Materials generated in CI
lists every crate, and all of them are in this repository. Leaving OpenATCA's maintainers does
not mean finding substitutes for anyone else's libraries.

## 2. Formats

Every input OpenATCA reads is a published standard or a published FAA or industry format. These
include ASTERIX, ARINC 424, NASR, FIXM, AIXM, IWXXM, GRIB2 and the NAM ICD. Each one is
documented under `docs/core-api/formats/` or in the README of the crate that decodes it.

Every output and every record OpenATCA writes is in a format whose specification is in this
repository:
- the replay and recording formats;
- the adaptation manifest and its checksums (G2);
- the configuration;
- the frame and trace formats.

A facility's data is readable without OpenATCA's code. A third party can write a reader from the
specification alone.

## 3. Moving out: the portability drill

`tools/j2_portability.py` exports one complete setup: the adaptation, the recording index, the
configuration and the replay profile. Each part carries its SHA-256 digest.

The drill (J2) imports that export into a fresh clone, with an empty build cache and an empty
build directory. It builds offline with the pinned toolchain, replays the setup, and compares
every frame digest with the original. On 26 November 2026, at commit 4793f3d, all 23 frame
digests of a complete 60-second recorded setup matched byte for byte. The public aggregate is in
`validation/portability-drill-v1.json`.

The same export is the hand-over package for any successor: another maintainer, another vendor,
or the facility's own staff. A facility should run the drill on its own setup at each release and
keep the result.

## 4. Builds

Builds need nothing but the repository and the Rust toolchain version pinned in
`rust-toolchain.toml`. The CI provenance record (E5) names the exact compiler, its component
digests and the build host. Reproducible-build hashes let anyone confirm that a binary was built
from the source it claims.

A facility that wants independence from public package hosts keeps its own copy of three things:
- the repository;
- the pinned toolchain archive;
- the provenance record for each release it operates.

## 5. Escrow

Because the source is public, source code escrow in the usual sense is not needed. These still
need to be held by someone the facility trusts:

| Item | Who holds it | Why |
|---|---|---|
| The facility's adaptation, recordings and configuration | The facility | They are the facility's data, in open formats |
| A copy of the repository and toolchain archive for each operated release | The facility, or an escrow agent it chooses | Protects against the public repository disappearing |
| Release signing keys | The facility, under its own custody (E4) | Signing authority must never rest with the supplier |
| The validation evidence behind each release | The facility, or an escrow agent | Lets a successor re-establish the safety case |

Choosing and contracting an escrow agent is the facility's decision. That arrangement is outside
this repository.

## 6. Leaving, step by step

1. Run the portability export for every setup in operation and verify its digests.
2. Hand the export, the repository at the operated release, the toolchain archive and the
   provenance record to the successor.
3. The successor runs the portability drill and confirms identical digests before taking over.
4. If the successor replaces OpenATCA with a different system, the facility migrates through
   `docs/migration/TRANSITION_PROCEDURE.md`, with OpenATCA as the system kept hot behind the new
   one.

## 7. What this plan does not cover

It does not provide an escrow agent, a successor maintainer, or a contract. Those belong to the
facility and its acquisition authority.
