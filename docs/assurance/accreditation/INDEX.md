# Test capability accreditation records

**Status:** Proposal, under review.

These records define candidate scopes and evidence gates for engineering test capabilities. They
do not record completed accreditation or approve a system, result, operational use, or safety claim.
Each capability requires its own approved test plan and decision.

## Capability records

| Capability | Record | Proposed scope | Current evidence state |
|---|---|---|---|
| Offline replay | [oa-replay](oa-replay.md) | Deterministic offline processing of identified recorded feeds | Source-level evidence described; admitted execution package not established |
| Sensor simulator | [oa-sensorsim](oa-sensorsim.md) | Synthetic component tests, fault injection, and sensitivity studies | Model-validation evidence not established |
| P-002 scorer | [p002-scorer](p002-scorer.md) | Retrospective comparison under a frozen measurement protocol | Method limits documented; run-specific admission is required |
| Performance harness | [oa-perf](oa-perf.md) | Bounded engineering measurements on specified workloads | A recorded stress qualification criterion failed |

## Roles and decisions

Before a capability is used for credited evidence, the program plan must identify:

- a capability owner responsible for implementation, configuration, and the evidence package;
- a technical reviewer independent of the run who checks method, source/build closure, and results;
- a test director or TEMP authority who approves the intended claim, procedure, and criteria before
  execution; and
- an accreditation authority who accepts, rejects, or limits the capability for that stated use.

The data custodian and configuration manager must also be named where applicable. These roles are
not assigned by this proposal. The decision record must state the exact scope accepted, conditions,
limitations, reviewer findings, unresolved discrepancies, signatures, and effective revision.
Acceptance for one use does not transfer to another source, binary, input, platform, or claim.

## Common admission and decision workflow

1. **Define the use.** State the question, claim, system boundary, operational relevance, and what
   the capability cannot answer. Establish measurable acceptance criteria and refusal conditions
   before seeing results.
2. **Freeze the configuration.** Record source revision and clean/dirty state; exact binary hashes;
   compiler and toolchain; command and environment; configuration and seed; input and reference
   identities; platform; resource limits; schema; and expected outputs.
3. **Prove build closure.** A top-level source hash alone is insufficient. Build in an isolated,
   empty target directory and retain the exact build invocation, toolchain identity, lockfile,
   resolved dependency manifest/graph, relevant build environment, complete build log, and hashes
   of produced artifacts. The execution package must bind the selected binary to that closure and
   demonstrate that no stale or unbound artifact was used.
4. **Execute and retain.** Keep the complete command, inputs, standard output/error, exit status,
   resource record, outputs, and discrepancy/refusal details. Missing, truncated, or identity-mismatched
   evidence is an incomplete run, not a pass.
5. **Review and decide.** The independent reviewer checks the frozen bindings, expected-result
   method, criteria, deviations, and full outputs. The authority records accepted, rejected, or
   restricted use with reasons. A failed criterion remains a failure; any changed method needs a
   prospective amendment and a new decision.

Admission is refused when the use is outside scope; criteria were absent or changed after results;
source, binary, dependency closure, inputs, or reference cannot be bound; the build may have reused
unverified artifacts; limits or outputs are incomplete; a required independent review is absent; or
an unresolved discrepancy affects the claim. Refusal preserves the evidence and states the missing
or failed gate.

## Limits and open questions

These software capabilities can provide implementation evidence for specified configurations.
They do not, by themselves, establish live-interface behavior, independent truth, operational
performance, availability, facility readiness, or safety. Model validation, representative inputs,
field testing, and approval remain distinct evidence tasks and must be planned where a claim needs
them. The next step is for the governing program plan to assign a capability owner, independent
reviewer, TEMP authority, and accreditation authority for each proposed use, then approve the
use-specific claims and criteria before any evidence is admitted.
