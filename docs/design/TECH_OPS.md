# Tech Ops monitoring and certification design

**Status: draft proposal.** This document describes a possible headless interface and evidence model for maintenance monitoring and service certification. It is not an approved design, implemented capability, operational procedure, FAA acceptance finding, or certification claim. It does not define a display, authentication mechanism, cryptographic protocol, recovery behavior, or remote-maintenance wire interface.

## Purpose and boundary

The proposed service would let qualified maintenance personnel inspect service health, verify installed resources, run bounded performance-verification procedures, and retain evidence for a human certification decision. A future monitor-and-control display would render these results; it would not own certification logic.

The evidence must identify the service, complete processor/resource inventory, software release, adaptation release, test profile, target map, and every supporting artifact. A missing processor, stale or unauthenticated report, changed configuration, lost event, timeout, or failed evidence write must result in `INCOMPLETE` or `REFUSED`, never a pass.

This proposal adds no service transition or recovery model. A successful replay, checksum, package decode, or synthetic outage test is not service certification, patch qualification, or proof of field behavior.

## Existing implementation evidence and limits

| Source area | Evidence available today | Limit for Tech Ops |
|---|---|---|
| `oa-feed` health and replay summaries | Bounded per-feed counters for connection state, rates, bytes, latency, sequence gaps/reordering, rejects, frame errors, and freshness. | These describe an input stream, not processor/service health, alarm state, target-processing performance, or M&C status. |
| `oa-feed` recording | Rotated OAF/OAF.GZ recordings, byte bounds, loss/error reporting, and optional sync requests for closed segments. | The default recorder limit is 2 GiB; this is not a 45-day retention service or a guarantee of power-loss durability. |
| `SurveillanceReplay` and `oa-perf` | Offline replay/capacity counters, measured work, load context, and explicit refusal/loss counts. | These are development tools, not a live per-processor monitor or approved full/abbreviated PVER catalog. Replay time is not an operational monitor clock. |
| Airspace snapshot and proposed G2 package verification | The G2 design introduces a bounded package manifest verifier. `manifest::verify_current(root, expected_manifest_sha256)` checks an independently supplied pin, manifest identity, declared/actual snapshot size and digest, facility, and decode. It returns a `VerifiedPackage` whose model is decoded from the exact bytes checked. The verified route path uses that returned model; the legacy raw-snapshot path is labelled unverified. | This is single-process adaptation-package integrity/consistency evidence. It does not authenticate the issuer, identify the deployed processor, prove which executable or adaptation a live service loaded, enumerate all processors, or provide a live configuration epoch. This proposal does not establish operational deployment or fleet-wide integration of that interface. |
| MSAW reporting and replay witnesses | Reports can identify supplied configuration bytes; replay evidence can preserve bounded input/state digests and diagnostic outcomes. | Neither establishes installed MSAW configuration identity, operational alarm behavior, authenticated origin, or an approved live PVER result. |

The reviewed source does not currently provide trusted enrolled processor identity, authenticated processor evidence, live software/adaptation identity for every processor, a monotonic service configuration epoch, full/abbreviated PVER, fixed operational test targets/maps, Tech Ops authorization, certification statements/log, or an RMM adapter.

## Proposed headless interface

Commands would be typed and bounded. They are design proposals only; the control and certification-write commands must not be exposed operationally until the authentication, authorization, and audit gates below are satisfied.

| Command | Proposed result |
|---|---|
| `ReadServiceStatus(service_id)` | One bounded snapshot of service state, expected/responding resource IDs, active alarms, health freshness, configuration epoch, and software/adaptation release identities. Missing data is explicit; silence is not healthy. |
| `ReadPerformanceWindow(service_id, window_id)` | A fixed-window report for each approved metric with units, sampling period, valid/missing counts, observed value, limit/baseline ID, source processor/interface, and `PASS`/`FAIL`/`INCOMPLETE`. A caller cannot loosen a limit or extend a window. |
| `VerifyResources(service_id, manifest_id, scope)` | Per-processor and per-resource expected/observed versions and digests, verification state, and refusal reason. A selected subset cannot be reported as full-system verification. |
| `RunPerformanceVerification(service_id, mode, profile_id, target_map_id)` | Per-target/per-check outcome, runtime, alarms exercised, warnings, and evidence IDs. `mode` is `FULL` or `ABBREVIATED`; immutable reviewed catalogs define all tests, thresholds, targets, and limits. |
| `ReadCertificationLog(service_id, after_id, limit)` | A bounded page and continuation token. Truncation or unavailable records are explicit. |
| `RecordCertification(service_id, statement, exception_refs, verification_refs, idempotency_key)` | Appends a human statement linked to exact completed evidence and configuration epoch. It never changes service configuration or certifies automatically. Duplicate keys return the prior record. |

Result states are `PASS`, `FAIL`, `INCOMPLETE`, and `REFUSED`. Missing/unrun evidence is distinct from a failed measurement. A passing result is only meaningful for the explicitly named check and scope; it does not imply that every acceptance item is met.

### Identity, processor inventory, and adaptation evidence

Every expected processor/resource needs a unique enrolled identity and an authenticated evidence channel supplied by a separately reviewed system. Evidence must include hardware/firmware inventory, running executable/build identity, loaded adaptation identity, and process/service identity. A hostname or caller-supplied processor ID is not sufficient. Duplicate, unexpected, missing, unauthenticated, or mismatched resources refuse or invalidate the aggregate; one resource cannot stand in for another.

For an adaptation package, the future I1/I2 evidence path should call the G2 `manifest::verify_current` verifier and retain the verified identity through its read-only accessor only when the running process consumes the checked model. Do not reopen or independently decode another path after verification. A verified route conversion demonstrates use of that package in that process; it does not prove that a deployed operational processor has loaded it.

SHA-256 can detect byte disagreement with an independently controlled expected digest. It does not authenticate the issuer, prove who approved a manifest, authenticate the reporting processor, or prove that the running service uses those bytes. Manifest and profile provenance must come from a controlled release/configuration process. E4 cryptographic-module work by itself does not provide authorization.

### PVER catalogs and exceptions

Full and abbreviated PVER must be separate, immutable, approved catalogs. Each fixes the resource inventory, target map, stimuli and observations, order, maximum input/work, memory/I/O/deadline, sample requirements, and thresholds. The caller selects a catalog ID; it cannot supply arbitrary targets, scripts, or tolerances.

Abbreviated PVER may omit only checks explicitly allowed by the approved catalog. It must retain mandatory MSAW and conflict-alert tests and critical identity, configuration, software, and adaptation checks. Test alarms require a verified isolated path that cannot inject into operational controllers or feeds. Until the catalog and isolation path exist, these tests must not run against a live operational service and abbreviated PVER cannot pass.

`CERTIFIED_WITH_EXCEPTIONS` is allowed only for a reviewed, versioned exception catalog entry matching the exact service/check and approved disposition. Record the affected component/service level, reason, compensating disposition, authority reference, expiry or recheck trigger, and degraded-service state. Unknown, expired, or mismatched exceptions refuse certification. Exceptions cannot waive incomplete/unknown evidence, mandatory MSAW/conflict-alert checks, invalid identity/configuration, or unverified software/adaptation.

## Consistency and invalidation

The live system needs a trusted, monotonically increasing configuration/release epoch plus a maintenance/update lock or reliable nonrollback event stream. Bind each run to the complete enrolled processor inventory, software release, adaptation package identity, PVER profile, and target map at that epoch. Invalidate on every epoch advance, restart, inventory change, maintenance event, or lost event. Start/end rereads alone cannot detect an A→B→A change.

No current source provides this fleet-wide live epoch/event primitive. Until it exists, matching package hashes do not support live service certification and the result remains `INCOMPLETE`. Any relevant software/adaptation change, maintenance action, processor replacement/restart, clock fault, or resource alarm invalidates prior certification as defined by a future approved procedure. A successful service certification after a patch does not validate the patch itself; software/adaptation qualification must remain a separate record.

## Operational limits and audit

Before any live PVER or full resource scan, an approved service-specific operation window and live-impact envelope must define CPU, memory, bytes/I/O, network, duration, telemetry-age, alert, queue, loss, and refusal abort thresholds. Each operation needs fixed input/work, memory, I/O, and duration caps. Resource scans use a fixed inventory, bounded chunks, and a low-impact schedule; no arbitrary tree walk. Monitor service latency, queues, loss/refusal, and alarms throughout. A budget exceedance, timeout, missing sample, stale telemetry, or lost event ends `INCOMPLETE`.

Keep only one mutating verification or certification-write operation per service/configuration epoch. Bound status responses and log pages. The persistent certification log must have maximum record/page sizes, retention/capacity rules, and explicit full/write-failed/truncated states. A persistence failure blocks control and certification writes; success is not returned before the approved durable acknowledgement. Raw recordings use a separate retention policy.

Record timestamps in named clock domains. If UTC/monotonic conversion, synchronization, leap-second handling, or time validity is unavailable, mark timing evidence uncertain/incomplete. Do not substitute replay timestamps for live monitor time.

## Authorization and human authority

The future role mapping has three levels: logged out, monitor, and control. This is a requirements mapping, not an authentication design. No operational control or certification-write endpoint should be exposed until individual identity, authorization, and authenticated audit are designed, reviewed, and implemented. Never trust a caller-supplied principal, role, qualification, or service identity. Do not simulate authorization with local labels or environment variables.

Certification additionally requires a trusted current roster confirming an individually identified, qualified, site-familiar, non-vendor Tech Ops certifier. A control role alone is insufficient. Denials and control actions need a bounded persistent audit record with identity, action, target, time, request/result IDs, and reason. Unknown/expired authority, stale identity, or failed audit persistence must fail closed.

## Metrics and current gaps

The legacy STARS Gen 4 maintenance handbook documented operational thresholds and checks for its own system. It is a historical reference, not a profile for OpenATCA and not an assertion that the same thresholds apply to a new service. For example, the handbook described 1-minute samples over an hour for processor utilization and virtual memory, network error-rate checks, target processing time against baseline, recording age, and periodic verification. Applicable current limits and approved denominators must be confirmed with the responsible authority before use.

Current OpenATCA development sources do not supply per-processor one-minute/hourly CPU or virtual-memory histories, network-interface error denominators, approved target-processing baselines, a 45-day recording coverage ledger, or a defined operational VSP certification timer. `oa-feed` reports feed-level errors/latency; `oa-perf` reports replay/capacity timing and host/process context. Neither may be substituted for those operational measurements.

Other unresolved gaps include installed registration/QC/IFDT checks and fixed target maps; live in-service resource redundancy/switched-service tests; GPS/UTC and leap-second verification plus daily FS/EFS comparison; approved clock/source identity; training and site-familiarization records; a current RMM ICD/adapter or waiver process; and maintenance handbook, technical instruction, retention, and configuration-control workflows.

## Implementation gate and first useful slice

No standalone wrapper around the G2 verifier that merely reports unsupported operational fields is proposed. The first useful implementation slice can begin only after owners provide (1) enrolled per-processor identity and authenticated evidence transport; (2) controlled software/adaptation/profile provenance and running/loaded identities; (3) monotonic live epoch plus lock or loss-detecting event stream; and (4) real resource evidence producers. That slice would be a read-only per-processor evidence snapshot bound to the epoch and would return only `PASS`, `FAIL`, `INCOMPLETE`, or `REFUSED`; absent or unauthenticated inputs must return `INCOMPLETE`.

PVER and certification writes remain blocked until there are approved full/abbreviated test catalogs, target maps, tolerances, isolated alarm-test paths, measured worst-case resource limits, authenticated individual identities and roles, a qualified-certifier roster, and a durable bounded log policy. No display or control workflow is specified here.

## Source references

The references below provide design context. They do not establish that every legacy-system procedure applies to a new implementation.

- FAA, [JO 6191.5, STARS Maintenance Technical Handbook, Generation 4](https://www.faa.gov/documentLibrary/media/Notice/JO_6191.5_Standard_Terminal_Automation_Replacement_System_%28STARS%29_Maintenance_Technical_Handbook_%28MTHB%29_Generation4_%28G4%29.pdf) (2015). The FAA directive index marks JO 6191.5 cancelled on 2024-08-21; consult the current governing material before adopting any procedure or threshold: [FAA directive status page](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.information/documentid/1028651).
- FAA, [JO 6100.1H Change 2, En Route Stage A maintenance](https://www.faa.gov/documentLibrary/media/Order/6100.1H2.pdf) (2009). This is historical context; the current edition was not reviewed for this draft.
- FAA, [Order 1800.66A, Configuration Management Policy](https://www.faa.gov/documentLibrary/media/Order/1800.66A.pdf) (2022).
- FAA, [JO 6000.53E status page, Remote Maintenance Monitoring and Control](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.information/documentID/1040982) (active, issued 2022-05-02). The order text is FAA-network only and was not reviewed here; no RMM conformance claim is made.
- U.S. DOT Office of Inspector General, [Management Advisory on STARS site-specific deployment requirements and specialist training](https://webarchive.library.unt.edu/web/20161101065351id_/https://www.oig.dot.gov/sites/default/files/STARS%20Management%20Advisory%5E8-14-14.pdf) (2014).

