# OpenATCA Configuration Management Plan

Status: draft 1, 2026-11-26. Owner: Avila Labs (maintainer). Applies to the repository
`AvilaLabs/OpenATCA` and to every release made from it.

This plan says how OpenATCA names, controls, builds, releases and audits what it ships. It is
written against FAA Order 1800.66A, Configuration Management Policy (effective 10 August 2022),
which sets the configuration management requirements for NAS products: software development library,
reproducible builds, version description, baselines, NAS Change Proposals, configuration control
board decisions, site adaptation, modification records and configuration audits). Where a step
belongs to the FAA rather than to us, the plan says so and says what we hand over.

## 1. Principles

1. **One source of truth.** Everything that defines a release lives in one git repository:
   source, toolchain pin, lock file, requirements, interface documents, the hazard log and the
   tools that check them. Nothing a release depends on lives only on someone's machine.
2. **Nothing reaches `main` unchecked.** Every change lands through a train branch whose exact
   commit has passed both CI workflows. `main` only ever moves forward to a commit that was
   checked green.
3. **Software and adaptation are separate items.** Site adaptation is data with its own
   versions and digests. Changing adaptation is not a software change, and the reverse.
4. **Any release can be rebuilt by someone else.** The toolchain is pinned, the lock file is
   committed, the build needs no network, and the shipped workspace has no third-party crates.
   The target is that a rebuild on another machine gives the same outputs; so far that has been
   shown on one host (section 7).
5. **Records are generated, not typed.** Version descriptions, the SBOM, safety tables and
   traceability reports come from tools run in CI, so they cannot drift from the code.

## 2. Configuration items

| Item | What it is | Where it lives | How it is identified |
|---|---|---|---|
| Software | The Rust workspace: core, split and shell crates, tools and adapters | `crates/`, `tools/`, `adapters/`, `Cargo.toml`, `Cargo.lock` | Release version and git commit; programs that link `tools/oa-build-info` report the commit they were built from |
| Python binding | The `openatca` package over the core FFI | `python/`, `crates/openatca-core-ffi`, `include/` | Package version and the core commit it wraps |
| Toolchain | Rust 1.89.0 with clippy and rustfmt; named nightlies for proof and fuzz jobs only | `rust-toolchain.toml`, `.github/workflows/` | Exact toolchain version; changing it is a reviewed change made together with the workflows and `rust-version` |
| Site adaptation | Airspace, fixes, sectors, maps and parameters for one facility | Issued adaptation sets, built and checked by `tools/oa-adapt` | Adaptation version, SHA-256 digest, issuer and approver IDs, reason |
| Requirements and design | Requirements, scope, interface and design documents | `REQUIREMENTS.md`, `SCOPE.md`, `docs/` | Git commit; requirement IDs `OA-XXX-NNN` |
| Safety records | Hazard log and the tables generated from it | `docs/safety/hazard-log.json`; tables built by `tools/generate_safety_tables.py` | Git commit of the hazard log; tables attached to each release |
| Security records | Control map and OSCAL output | `docs/security/` | Git commit |
| Rules | The engineering ruleset every change is checked against | `docs/RULESET.md` | Git commit; rule IDs |
| Verification evidence | Test, proof, fuzz, replay and drill results | CI run logs, release attachments, `validation/` | CI run ID and the commit it ran on |

A **controlled baseline** is any set of these items at one commit that we have released or
handed to someone else. Work in progress on a train or wip branch is not a baseline.

## 3. Identification and versioning

- **Software release version.** `MAJOR.MINOR.PATCH`. MAJOR changes when a behaviour a user or an
  interface depends on changes incompatibly; MINOR when function is added without breaking
  anything; PATCH for corrections only. Before 1.0.0, MINOR may break and says so in the
  version description.
- **Every software item can be named** by title, version, release date and commit, as the FAA
  requires of a NAS product. The release tag `vX.Y.Z` points at that commit.
- **Programs say where they came from.** A program that links `oa-build-info` embeds
  `commit <hash>`, and marks `(tracked files modified)` if a tracked file under `crates/` or
  `tools/`, or the workspace manifest or lock file, differed from that commit at build time. A
  release build must never carry that mark. Every shipped program is to link it; not all do yet.
- **Adaptation versions** are independent of software versions. A software release lists the
  adaptation format versions it accepts; an adaptation set records the software versions it was
  checked against.

## 4. Software development library

The software development library is the GitHub repository `AvilaLabs/OpenATCA` together with
its releases.

- `main` is the only line that releases are made from.
- Release attachments hold what is generated rather than committed: the SPDX SBOM and build
  provenance (`.github/scripts/supply_chain_evidence.py`) and the safety tables, attached by the
  `Release evidence` workflow when a release is published.
- The build environment is recorded with each release: the toolchain version, the host and the
  build provenance written by the supply-chain evidence step.
- The software is Apache-2.0 and the shipped workspace has no third-party crates, so the
  library is complete in itself. The one exception is outside the shipped workspace: the
  separate fuzzing workspace (`fuzz/`) uses the `libfuzzer-sys` crate to drive fuzz targets. It
  is test tooling and is never part of a release build. A copy of the repository and its releases is everything needed to rebuild. The exit
  plan (`docs/EXIT_PLAN.md`) covers escrow and what a party needs to take the code over.
- Outside code is controlled by RULESET F1: no dependency on code from outside the repository
  without a written justification and the maintainer's approval. Links between workspace crates
  follow F1a: a one-line reason in `Cargo.toml` and the layering check in CI
  (`tools/check_f1a_dependency_graph.py`), which refuses cycles and core crates that depend on
  shell or tool crates.

## 5. Change control

### 5.1 How a change lands

1. The change is made on its own branch on top of the current `main`. Commits are authored by
   the maintainer.
2. It is checked against `docs/RULESET.md`. Changes to safety-related code get an independent
   review by someone who did not write it.
3. Work-in-progress branches (`wip/**`) run the CI check job. A change ready to land goes on a
   train branch (`train/**`).
4. Both workflows, `ci` and `Rust assurance`, must succeed on the train's exact commit. CI checks
   formatting, clippy, tests on x86-64 and AArch64, debug assertions, a fuzz smoke run,
   requirements traceability (`oa-trace`), the dependency layering check, allocation checks and
   the hazard-to-mitigation links. Rust assurance runs the proof harnesses.
5. `main` is fast-forwarded to that commit and nothing else. If `main` moved in the meantime,
   the train is rebased and checked again.

### 5.2 What may not change without a reviewed record

- The sealed hold-out data list (`crates/oa-sealed/sealed_paths.txt`) and its guard.
- Thresholds, caps and limits used to judge a result. A failing measurement is reported; the
  limit is not raised to make it pass.
- The toolchain pin.
- Any outside dependency (F1).
- A test, to make it pass. A test that fails to catch a deliberate break is strengthened, never
  removed.

### 5.3 Baselines and the FAA's change process

The FAA controls changes to a NAS baseline through a NAS Change Proposal (FAA Form 1800-2) and
a Configuration Control Board decision. The FAA owns that process. Our part is to make each
release ready to go through it.

| FAA baseline | What we supply | Status |
|---|---|---|
| Functional | `REQUIREMENTS.md`, `SCOPE.md`, the interface requirements in `docs/interfaces/` | Exists; requirement IDs are traced to tests by `oa-trace` |
| Allocated | Interface control and core API documents (`docs/core-api/`, `docs/INTERFACES.md`), design documents in `docs/design/` | Exists in part; the controlling legacy ICDs that are not public are listed as unknowns |
| Product | A tagged release: version description document, SBOM, safety tables, verification evidence | Release mechanism exists; first release not yet tagged |
| Operational | Set by the FAA at the in-service decision, with the technical instruction books and maintenance handbook | FAA step; we supply the manuals (rows I-series) |
| Facility | Set by the FAA from the joint acceptance inspection | FAA step |

For each release proposed for the NAS we hand the FAA sponsor a **change package** for the NCP:

- the version description document;
- the list of changes since the last baselined release, by commit;
- the safety impact: hazards whose mitigations changed, from the hazard log and generated tables;
- verification evidence: CI run IDs on the release commit and the replay and drill results;
- the SBOM and build provenance.

**Emergency changes.** The FAA requires an NCP within two calendar days of an emergency change.
Our hotfix path is the normal train path with the same checks; it is fast because the checks run
in CI in parallel, not because steps are skipped. The change package is produced the same day.

## 6. Site adaptation and patches outside the NCP

The FAA does not require an NCP for site adaptation updates or security patches, but requires a
structured method for issuing them. Ours:

- **Adaptation** is issued with `tools/oa-adapt`. Each issued set records its version, digests,
  issuer and approver IDs and the reason. A local check verifies an installed adaptation package
  at startup and on demand. The `oa-adapt` fleet comparison reads one report per processor and
  flags any processor that is stale, differs from the issued set or is missing. Those reports
  are supplied by the processors and are not yet authenticated, and checks on every processor
  of facility hardware have not been demonstrated; both are deployment work.
  Quick field changes go through the same issue step, which verifies the changed body.
- **Spares** are loaded with the current release and adaptation before they enter service, and
  are included in the fleet comparison like any other processor.
- **Security patches.** With no third-party crates in the shipped workspace, a security patch is a change to our own code
  or to the toolchain. Both go through the change path in 5.1. A toolchain update is a reviewed
  change to `rust-toolchain.toml` and the workflows together.

## 7. Reproducible builds

- The toolchain is pinned (`rust-toolchain.toml`, 1.89.0); `Cargo.lock` is committed; the build
  runs offline.
- The portability drill (`tools/j2_portability.py`) rebuilds from a fresh clone with empty
  build and cache directories, offline, and compares outputs with the source checkout. At commit
  `4793f3d` all 23 frame digests matched (`validation/portability-drill-v1.json`). That drill ran
  on one host; the same drill on a second machine is planned.
- Each release states the commit, toolchain and build host, so a third party can repeat the
  build and compare digests with the release attachments.

## 8. Releases and the version description document

Each release has a version description document (VDD) generated from the release commit. It
contains:

1. Title, version, release date and commit.
2. The configuration items it contains and their versions (software, Python package, toolchain,
   accepted adaptation format versions).
3. Changes since the previous release, by commit, grouped by requirement.
4. Requirements traced to tests and proofs at this commit (`oa-trace` report).
5. Hazard log commit and the generated safety tables.
6. Known problems and limits: open problem reports, unmet requirements, and rows of the
   acceptance list still partial.
7. Build instructions and environment, and the digests of the built outputs.
8. Installation and adaptation compatibility notes.

The VDD, SBOM and safety tables are attached to the GitHub release by the release workflow.

## 9. Records

- **Change records** are the git history: each commit on `main` with its train, CI run IDs and
  review records.
- **Problem reports** are tracked with the requirement or row they affect and listed in each
  VDD until closed.
- **Adaptation issue records** are written by `oa-adapt` at each issue.
- **Modification records** at a facility (FAA Form 6032-1, in the Maintenance Management
  System) are kept by the FAA. Each release's VDD gives the facility what it needs to fill one
  in.

## 10. Configuration audits

- **Functional configuration audit**, before a product baseline: every requirement in
  `REQUIREMENTS.md` is traced to a passing test or proof at the release commit, or listed as
  not met in the VDD. `oa-trace` produces the trace.
- **Physical configuration audit**, before a product baseline: the release attachments match a
  rebuild from the release commit (digests), the SBOM matches `Cargo.lock`, and the VDD's item
  list matches the repository.
- **CM process review** twice a year, matching the NAS CCB's own review cycle: this plan against
  what was actually done, with findings recorded and fixed through the normal change path.

## 11. Roles

| Role | Holder | Decides |
|---|---|---|
| Maintainer | Avila Labs | What lands, what is released, outside dependencies, changes to this plan |
| Independent reviewer | Someone who did not write the change | Whether a safety-related change meets RULESET |
| FAA sponsor | FAA | Whether a release goes forward as an NCP |
| Configuration control board | FAA | Whether a baseline changes |

## 12. Limits of this plan today

- No release has been tagged yet, so no product baseline exists. The first tagged release is
  the first baseline.
- The VDD generator (`tools/generate_vdd.py`) exists; no VDD has yet been issued with a
  tagged release.
- The product baseline index the FAA expects for software lists: specifications, IRD/ICD,
  software and system design descriptions, a software development plan, a software requirements
  specification, a database design description, the VDD and a software user manual. Today the
  requirements, interface and design documents exist in the forms named in section 5.3; the
  software development plan, database design description and user manual are not yet written.
- Release artifacts are not yet signed. Signing waits on the cryptography decision (row E4).
- There is one maintainer. Independent review is done today by reviewers who did not write the
  change; a second organisation with commit authority is part of the long-term home question.
- Which FAA CM records apply to a third-party product entering by the CAP route (Master
  Configuration Index entries, CCB charter appendices) is not stated publicly and is listed as
  unknown.
