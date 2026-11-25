# Test capability accreditation records

**Status: DRAFT / NOT ACCREDITED / NOT APPROVED FOR system verification.** These documents
describe proposed uses and evidence gaps. They are not an Accreditation Plan, TEMP, approval,
acceptance of results, or authorization to operate.

## Records

| Capability | Draft record | Proposed use | Current status |
|---|---|---|---|
| Offline replay | [oa-replay](oa-replay.md) | Reproducible software verification from specified recordings | Component evidence only; no accreditation |
| Sensor simulator | [oa-sensorsim](oa-sensorsim.md) | Synthetic sensor scenarios for component tests and sensitivity studies | Model fidelity unvalidated; no accreditation |
| Scoring tool | [scoring capability](p002-scorer.md) | Retrospective comparison under a frozen measurement method | Method and reference limitations remain; no accreditation |
| Performance harness | [oa-perf](oa-perf.md) | Bounded engineering measurements under specified synthetic workloads | A stress qualification criterion remains failed; no accreditation |

## Authority and configuration

The applicable test authority must identify the capability owner, independent technical reviewer,
and accreditation authority in the approved program plan. None is assigned here. Accreditation of
a tool is separate from approval of a system, acceptance of an individual result, authorization to
operate, or approval of a safety claim.

Each credited use needs an immutable source, toolchain, configuration, input, platform, invocation,
limit, expected-output, and discrepancy record. Historical evidence applies only to its recorded
configuration; it does not qualify later source or binaries. No tests were executed in preparing
these drafts.

## Scope

The records state intended use, assumptions, evidence limits, and required approval work. They do
not publish recorded result tables, data identities, private configuration details, or scoring
artifacts. A synthetic or repeatable result is not, by itself, validated operational fidelity.
