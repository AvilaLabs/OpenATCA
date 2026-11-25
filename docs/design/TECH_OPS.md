# Tech Ops monitoring and service certification

**Status:** Proposal, under review.

This design defines a headless maintenance workflow and its evidence contract. It does not authorize operational use until the implementation gates and approvals below are met.

## Decision and operating model

Separate service certification from software or adaptation release qualification. A passing PVER supports a qualified human’s service decision; it does not certify a software patch, establish system safety, or substitute for authority acceptance.

The workflow has four stages:

1. **Observe.** A monitor reads a bounded status snapshot, current alarms, freshness, processor inventory, software/adaptation identities, performance window and latest certification state. Missing or stale data is visible and never treated as healthy.
2. **Verify.** An authorized control-role user selects a fixed resource verification or an immutable `FULL` or `ABBREVIATED` PVER catalog. The command is bound to one service configuration epoch and its complete expected inventory.
3. **Review.** The system stores per-resource and per-check results with evidence links, losses, timestamps and the exact profile/target-map identities. It does not make a certification decision.
4. **Certify.** A qualified, current, site-familiar, non-vendor Tech Ops certifier reviews the evidence and records `CERTIFIED` or `CERTIFIED_WITH_EXCEPTIONS`. The statement is append-only and applies only to the named service, epoch and verification scope.

All results use `PASS`, `FAIL`, `INCOMPLETE` or `REFUSED`. `PASS` means only that the named check met its versioned profile for its complete declared scope. Missing evidence, a changed epoch, absent processor, stale telemetry, failed evidence write, timeout or unaccounted loss cannot produce `PASS`.

## Commands and role/action matrix

Commands are typed, fixed-scope operations. No command accepts a caller-defined script, target list, threshold, or expanded time window.

| Action | Logged out | Monitor | Control |
|---|---:|---:|---:|
| Read public service availability | Allow, minimal | Allow | Allow |
| Read detailed status, alarms, metrics and evidence references | Deny | Allow | Allow |
| Read certification history | Deny | Allow | Allow |
| Start fixed resource verification | Deny | Deny | Allow |
| Start approved full/abbreviated PVER | Deny | Deny | Allow |
| Abort a running verification | Deny | Deny | Allow, audited |
| Record certification statement | Deny | Deny | Allow only for roster-verified certifier |
| Change service, software, adaptation, profile or target map | Deny | Deny | Outside this interface |

“Control” is an authorization level, not proof of qualification. Certification additionally requires a separate current certifier roster lookup tied to individual identity, service/facility endorsement and non-vendor status. No caller-supplied principal, role, qualification or service identity is trusted. Until authenticated identity, authorization and durable audit are implemented and reviewed, operational control and certification-write endpoints stay disabled.

Proposed command shapes:

- `ReadServiceStatus(service_id)` returns one bounded snapshot with service state, complete expected/responding processor IDs, alarms, telemetry freshness, configuration epoch and observed release identities.
- `ReadPerformanceWindow(service_id, window_id)` returns fixed-window measurements, denominators, limits, sample gaps and status.
- `VerifyResources(service_id, manifest_id)` checks the complete pinned inventory. A subset can be used for diagnosis but is labelled partial and cannot support service certification.
- `RunPerformanceVerification(service_id, mode, profile_id)` accepts only an approved `FULL` or `ABBREVIATED` profile ID. That profile fixes target map, stimuli, order, tolerances, resource caps and timeout.
- `ReadCertificationLog(service_id, cursor, limit)` returns a bounded page with explicit truncation and continuation state.
- `RecordCertification(service_id, statement, verification_ids, exception_ids, idempotency_key)` appends a certifier’s decision; repeated idempotency keys return the original record.

## Verification profiles, triggers and checks

Profiles are immutable, versioned artifacts controlled separately from the service under test. Their manifest pins the processor/resource inventory, software release, adaptation release, target map, test stimuli, expected observations, thresholds, minimum sample counts, work/memory/I/O limits, timeout, abort envelope and evidence schema. The command selects a profile; it cannot alter it.

### Full PVER

Run a `FULL` PVER:

- at initial service acceptance and before operational return after major service, software, adaptation or processor changes;
- after a failed or incomplete verification, after any unexplained identity/configuration mismatch, and when the service epoch or event history cannot be trusted;
- at least every 30 days while the service is operating.

The full catalog checks every enrolled processor and required resource, loaded and on-disk software/adaptation identities, registration/QC/IFDT checks, fixed target response across the approved target map, mandatory MSAW and conflict-alert functions on an isolated path, approved redundancy/switchover behavior, all performance windows below, and recording continuity/retention evidence. Any catalog entry not implementable or not applicable must have a reviewed disposition; omission does not silently pass.

### Abbreviated PVER

Run an `ABBREVIATED` PVER before each operational shift, at least every four hours during continuous service, after a transient service/resource alarm, and on a Tech Ops request. It checks the complete processor inventory and identity/configuration pins, current alarms and health freshness, mandatory isolated MSAW and conflict-alert stimuli, fixed abbreviated target set, short performance window, and current recording status. It may omit only full-catalog checks explicitly marked deferrable in the approved profile. It cannot omit identity, loaded configuration, mandatory alarm checks, or a failed check’s required follow-up.

The proposed 30-day, four-hour and per-shift cadences are OpenATCA design targets, not FAA-prescribed intervals. The responsible authority can change them through profile approval. A missed due time changes service status to `VERIFICATION_DUE`; it does not extend the last certification’s validity.

### Safe operation and concrete bounds

PVER must use a validated isolated stimulus path that cannot inject test alerts or targets into operational controllers or feeds. If isolation is unavailable, alarm checks are `REFUSED` and the PVER cannot pass. Before any operational run, a service-specific impact envelope must set firm CPU, memory, storage, network, queue, alert, duration and telemetry-age abort limits. The initial OpenATCA design target is: no more than 5 percentage points additional CPU averaged over one-minute samples, no more than 2 percentage points additional memory use, no more than 1% additional network bytes, and no increase in controller-visible alert count. Establish those deltas from an approved matched-load rehearsal baseline, not a caller-selected live baseline. Any bound breach, missing sample, stale telemetry, timeout, queue/loss increase above the profile limit, or loss of the epoch event stream ends `INCOMPLETE` and stops further stimuli. The service must remain within its separate approved operating envelope. These are design targets pending measured impact and safety review, not FAA acceptance thresholds.

Every check is bounded by maximum input bytes, records/targets, CPU time, memory, I/O and wall duration declared in the signed profile. Resource hashing visits a fixed manifest list in bounded chunks; it never walks arbitrary directories. The monitor samples health throughout the operation. A cancellation or abort preserves partial evidence and reports `INCOMPLETE`.

## Processor and adaptation identity

The expected inventory comes from a controlled service release manifest with a unique enrolled identity for every processor and required resource. Each authenticated report includes hardware/firmware identity, boot/service instance, running executable digest and build identity, loaded adaptation identity, on-disk package identity, configuration epoch, and report freshness. Duplicate, unexpected, missing, unauthenticated, stale or mismatched processors invalidate the aggregate. Hostname and self-reported processor labels are not identity evidence.

Expected release pins are obtained independently from the controlled release/configuration authority and are bound to a versioned release manifest. For adaptation packages, the intended path is the G2 `manifest::verify_current(root, expected_manifest_sha256)` verifier and its `VerifiedPackage.identity`; the running process must consume the exact `VerifiedPackage.model` whose bytes were verified. Do not reopen or decode another path after verification. Compare the loaded runtime identity with the verified package identity and the independent expected pin. This verifier is not yet present in the origin/main source used for this proposal and must be integrated/reviewed before reuse. SHA-256 provides byte-integrity comparison, not issuer authentication or approval. A package hash alone cannot establish which executable is running or which processors reported.

One monotonically increasing service configuration epoch binds inventory, running software, loaded adaptation, profile, target map and all evidence in a verification. A maintenance/update lock or loss-detecting, nonrollback event stream must cover the whole run. An epoch advance, restart, inventory change, maintenance event or missing event invalidates the run. Start/end rereads alone are insufficient because they can miss an A→B→A change.

## Performance reporting and initial OpenATCA targets

Every metric record carries metric ID, value and units, source processor/interface, clock domain, interval, fixed window, sample count, missing count, denominator, applicable profile and limit IDs, observed value, baseline revision, freshness, result, and refusal reason. Report per-processor results and the aggregate; never average away a single failed processor. Missing, invalid or stale samples make the window `INCOMPLETE`. A value outside a target is `FAIL`; a target change requires profile review and cannot be made by a command caller.

The initial OpenATCA service-profile targets below make the design testable. They are engineering targets, not copied FAA requirements and not operational acceptance limits until measured, reviewed and approved for a service profile.

| Metric | Initial OpenATCA design target | Window / denominator | Decision |
|---|---|---|---|
| Processor CPU | ≤70% one-minute mean; no one-minute sample >85% | Per processor, 60 consecutive one-minute samples | Missing sample is `INCOMPLETE`; exceedance is `FAIL` and triggers service impact review. |
| Memory | ≤80% of the configured physical-memory or worker budget used; zero OOM/restart events | Per processor, 60 one-minute samples plus event counters | Unknown accounting source is `INCOMPLETE`; OOM/restart is `FAIL`. |
| Interface errors/drops | ≤0.01% of received+transmitted packets | Per interface, each 5-minute interval and full hour; retain packet denominator | Zero/unknown denominator is `INCOMPLETE`; any interval over target is `FAIL`. |
| Target processing | 99.9% of fixed-profile target updates emit output within 1 second of acceptance at process ingress; zero unaccounted target rows | Per target and processor, minimum 10,000 updates per full profile; abbreviated profile minimum 1,000 | Report p50/p95/p99/max and late/missing/refused counts. Under-sample is `INCOMPLETE`; profile limit breach is `FAIL`. |
| Recording continuity | At least 45 consecutive UTC days of validated required-feed coverage with no unexplained gap | Daily coverage ledger per required feed and storage resource | For an operating service, shorter/incomplete coverage is `INCOMPLETE`; known lost required interval is `FAIL`. At initial acceptance, verify retention configuration/capacity separately and do not claim observed 45-day coverage. |
| Clock health | UTC source identified; offset ≤100 ms to approved reference; no unresolved leap-second, step or sync fault during evidence window | Per processor, 1-minute samples over the PVER window plus event log | Unknown source or a gap is `INCOMPLETE`; limit/fault is `FAIL`. |

The 1-second processing deadline is an initial OpenATCA service objective, not a claim that current development replay timing represents production target latency. `oa-perf` replay timings and `oa-feed` latency/gap counters are supporting development evidence only; they cannot fill a missing live processor or interface metric. Before operational use, establish calibrated live producers, sampling behavior under load, denominators, uncertainty, clock source and worst-case impact. If measurements show an unsafe or unattainable target, revise the profile through approval before use rather than suppressing the failure.

## Certification, exceptions and invalidation

Certification is a human decision after a passing required verification at the current epoch. The certifier reviews the complete processor inventory, all required profile outcomes, active alarms, loss/deferred counts, performance windows, current exception state, and evidence-write status. The exact statement records service and facility, epoch, verification/profile/target-map IDs, software and adaptation pins, timestamp and clock quality, certifier identity/qualification reference, decision, exception references and supporting evidence digests.

`CERTIFIED_WITH_EXCEPTIONS` is permitted only when every exception matches a reviewed, versioned exception catalog entry for the exact service/check and has an authorized disposition. It records affected component/service level, reason, compensating disposition, approving authority/reference, degraded-service state, expiry (maximum 24 hours) and required recheck trigger. It cannot waive an incomplete/unknown result, missing processor, invalid identity or configuration, unverified software/adaptation, failed mandatory MSAW/conflict-alert check, safety alarm, lost evidence, failed performance target or failed audit write. Expiry automatically changes status to `VERIFICATION_DUE` and then `UNCERTIFIED`; only a new review and record can restore certification.

Certification expires at the earliest of 24 hours, a missed required abbreviated/full PVER, an epoch advance, software/adaptation or inventory change, processor restart/replacement, relevant maintenance event, alarm invalidating a required check, clock fault, or lost event/audit history. The proposed 24-hour interval and exception maximum are OpenATCA design targets pending authority review. A software/adaptation patch requires its own release qualification; PVER after the patch is not patch validation.

Certification records are append-only, bounded, durable before success is returned, and linked to an authenticated audit chain. Each record includes request/result IDs, caller and certifier identities, authorization decision, service/epoch, command/profile, evidence references, result, reasons, timestamps and prior-record reference. Duplicate idempotency keys return the original record. Full storage, truncation, write failure, clock uncertainty, or audit-chain verification failure blocks certification and control writes. No stale `PASS` or certification badge is retained after invalidation.

## Failure, refusal and degraded behavior

| Condition | Verification result | Service/certification effect |
|---|---|---|
| Missing/unexpected/duplicate processor; authentication, identity or pin mismatch | `REFUSED` for resource verification; `INCOMPLETE` if expected evidence never arrives | No certification; mark `UNCERTIFIED` or `IDENTITY_FAULT`; alert Tech Ops. |
| Stale/missing telemetry, missing sample, unknown denominator, clock uncertainty, lost epoch event | `INCOMPLETE` | Do not infer healthy status; retain last evidence as stale, not current. |
| Measured target outside approved limit or required check fails | `FAIL` | Enter profile-defined degraded state; prohibit certification until corrective action and required rerun. |
| Unsafe live-impact envelope, no isolated alarm path, exceeded resource cap, timeout or cancellation | `REFUSED` before stimulus, otherwise `INCOMPLETE` | Stop remaining stimuli; preserve partial diagnostics; no certification. |
| Evidence/log write failure, audit integrity failure, expired authority or role denial | `REFUSED` for the requested control/write operation | No state-changing action and no certification record; emit bounded failure evidence if durable audit remains available. |
| Exception missing, expired, mismatched, or prohibited by policy | `REFUSED` certification write | Keep service `UNCERTIFIED` or `DEGRADED` as determined by the failure profile. |

The exact service failover, traffic shedding and controller notification response is specified by the service’s approved safety plan, not improvised by this monitor. A reporting failure never changes the service configuration.

## Implementation and approval sequence

1. **Define authority and evidence ownership.** Approve the service inventory, release manifest/pin source, processor enrollment, authentication transport, metric owners, profile approval authority, clock reference and certifier roster. No live `PASS` exists before authenticated evidence producers are available.
2. **Land read-only evidence collection.** Implement bounded per-processor status, running/loaded identities, epoch binding, freshness, alarms and metric windows. Inject missing, duplicate, stale, mismatched, future-clock and event-loss cases. Preserve partial diagnostics and typed statuses.
3. **Approve profiles and safe test paths.** Measure target latency and PVER impact; approve full/abbreviated catalogs, immutable targets, thresholds, limits, isolated MSAW/conflict-alert path, recording ledger and abort rules. Verify caps and worst-case resource use under maximum declared workload.
4. **Enable fixed PVER.** Add only catalog-selected checks and test the complete fault matrix, including safe stop, incomplete evidence, epoch change and audit loss. Keep certification writes disabled.
5. **Enable human certification and audit.** After role/authentication and durable log review, enable control actions and certifier-only statements; test exception expiry, denial, replay, idempotency and invalidation.
6. **Review operational readiness.** The responsible authority accepts the service-specific evidence, operating envelope and current governing requirements before operational use. M&C display work renders the accepted headless result contract and cannot create or upgrade a certification state.

This order makes read-only authenticated evidence the first implementation slice. Without steps 1–2, existing feed summaries, replay witnesses, synthetic outage drills or package checks remain development evidence and cannot certify a deployed service.

## SUP R1–R13 traceability

The supportability research labels are retained for traceability. “Target” below denotes this OpenATCA design, not an FAA criterion.

| SUP row | Source evidence/gap | Design response / proof needed before claim |
|---|---|---|
| R1 — qualified specialist certification statement/log | No certifier identity or durable certification API in the reviewed service source. | Certifier-only append record bound to exact service, epoch and verification; qualification roster and durable audit required. |
| R2 — repeatable reports, targets, registration/QC/IFDT | `tools/oa-adapt` builds package reports; `crates/oa-sensorsim` emits synthetic plots. Neither provides deployed registration/QC/IFDT checks or approved targets. | Full PVER uses approved immutable target map and distinct registration/QC/IFDT checks; needs owner-approved targets/tolerances. |
| R3 — on-demand full/abbreviated PVER including alert tests | `openatca-core::SurveillanceReplay` and `tools/oa-perf` are offline replay/development tools; no operational PVER catalog exists. | Fixed profiles and isolated mandatory MSAW/conflict-alert stimuli; validate impact and all refusal paths. |
| R4 — software/adaptation verification on every processor/resource | `oa-sha256` and G2 package verification establish artifact checks; no complete live processor inventory/attestation is present in origin/main. | Independently pinned release manifests, authenticated per-processor running/loaded identities and epoch-bound full inventory. |
| R5 — in-service redundancy/switchover | `crates/oa-scope-replay` outage drills exercise synthetic processes, not deployed service resources. | Approved full-PVER switchover check with service-specific targets and safe operating envelope. |
| R6 — event recertification and exceptions | No maintenance-triggered invalidation/certification workflow is implemented. | Nonrollback epoch/event stream; catalog-limited exceptions with authority, degraded state, ≤24 h expiry and recheck. |
| R7 — certification distinct from patch validation | No integrated release qualification workflow. | Separate release qualification record; service certification cannot imply patch acceptance. |
| R8 — GPS UTC, leap seconds, daily FS/EFS comparison | `crates/oa-feed` carries receive timestamps; no live GPS/reference identity, leap-second check or daily node comparison source was found. | Clock health metric and approved reference/comparison producer before time evidence is accepted. |
| R9 — qualified, site-familiar, non-vendor certifier | No account/qualification/site-familiarity roster in source. | Current external certifier roster lookup; control role alone cannot certify. |
| R10 — M&C status/alarms and three access levels | `crates/oa-feed/src/core/health.rs` and `shell/summary.rs` provide feed health, not system-wide M&C or authorization. | Headless status contract and logged-out/monitor/control matrix; authenticated audit is a prerequisite. |
| R11 — self-reported performance metrics and limits | `tools/oa-perf/src/sys.rs` reports host/process context; feed summaries report stream health, not live processor/interface SLOs. | Per-processor metric schema and initial OpenATCA targets above; calibrate producers, denominators and profile before use. |
| R12 — 45-day recording and saved reports | `crates/oa-feed/src/shell/record.rs` bounds recording by bytes; no 45-day retention/coverage service exists. | Per-feed UTC coverage ledger and durable retention/report retrieval; target 45 consecutive validated days. |
| R13 — Remote Maintenance Monitoring interface | No current RMM adapter or reviewed current ICD/waiver workflow was found. | Keep a future adapter boundary; obtain current applicable interface/authority requirements before claiming support. |

## References

These sources inform the design and do not establish that historical procedures apply to OpenATCA.

- FAA, [JO 6191.5, STARS Maintenance Technical Handbook, Generation 4](https://www.faa.gov/documentLibrary/media/Notice/JO_6191.5_Standard_Terminal_Automation_Replacement_System_%28STARS%29_Maintenance_Technical_Handbook_%28MTHB%29_Generation4_%28G4%29.pdf) (2015). FAA directive index marks it cancelled 2024-08-21: [status page](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.information/documentid/1028651).
- FAA, [JO 6100.1H Change 2, En Route Stage A maintenance](https://www.faa.gov/documentLibrary/media/Order/6100.1H2.pdf) (historical edition; current edition not reviewed for this proposal).
- FAA, [Order 1800.66A, Configuration Management Policy](https://www.faa.gov/documentLibrary/media/Order/1800.66A.pdf) (2022).
- FAA, [JO 6000.53E status page, Remote Maintenance Monitoring and Control](https://www.faa.gov/regulations_policies/orders_notices/index.cfm/go/document.information/documentID/1040982) (issued 2022-05-02; order text not reviewed here).
- U.S. DOT Office of Inspector General, [Management Advisory on STARS site-specific deployment requirements and specialist training](https://webarchive.library.unt.edu/web/20161101065351id_/https://www.oig.dot.gov/sites/default/files/STARS%20Management%20Advisory%5E8-14-14.pdf) (2014).

## Limits and open questions

- Which authority approves the service inventory, exception catalog, certification statement, profiles, and OpenATCA target changes? **Next step:** assign owners and approval records before implementing live evidence.
- What enrolled hardware identity and authenticated evidence transport are available for every deployed processor, including firmware, boot and loaded-data attestation? **Next step:** select and review the evidence producer; without it, resource verification remains `INCOMPLETE`.
- What are the actual service processors, resource directories, fixed targets, registration/QC/IFDT checks and target-processing deadlines? **Next step:** baseline each supported service and validate the proposed target map/profile under representative maximum load.
- Can MSAW and conflict-alert stimuli be isolated from operational controller/feed paths? **Next step:** demonstrate isolation and fail-closed behavior before enabling either PVER profile.
- Are the proposed CPU, memory, network, latency, clock, cadence, retention and exception limits safe and achievable? **Next step:** measure live producers and worst-case check impact, then approve or revise each value in a versioned profile; they are not FAA-required values.
- What current governing documents specify GPS/leap-second verification, remote maintenance monitoring, retention, in-service redundancy, and certifier qualification? **Next step:** review the current authoritative editions and reconcile this trace before operational adoption.
- What storage, key management, audit-chain and disaster-recovery policy makes certification records durable and independently reviewable? **Next step:** design and review the authenticated log service before enabling certification writes.
