# Rust assurance record

This record separates requirement validity, bounded proof, runtime testing, allocation checks, replay conservation, and physical-truth validation. Each answers a different question. Results are scoped to the stated source, harness, bounds, inputs and tools; a passing proof of a mistaken requirement is not evidence of intended behavior.

## Risk and evidence matrix

| Priority | Property / boundary | Suitable evidence | Current result and limit |
|---|---|---|---|
| **P0** | Output table columns stay aligned and preserve row bits; tracker decisions produce one accounted row for eligible confirmed updates/resyncs and none for ineligible events. Boundary: output helpers, decision dispatch, emit and replay. | Requirement traces; ordinary tests for production caller wiring; Kani for bounded table and dispatch properties; replay ledger keyed by event ordinal for end-to-end conservation. | The final Kani suite passes output append/full/clear and seven dispatch/accounting properties. Decision proofs have harness bounds/stubs; they do not prove all lookup/routing, arbitrary history or end-to-end data conservation. Output-column negative control was detected. A focused resync-emission negative control was detected after its paired positive proof passed. |
| **P0** | Numerical refusal restores pre-transaction history/filter/metadata/output and all but its refusal count; late replay does not commit a partial row/state. Boundary: `TrackTxn`/`OutputTxn`, in-order update and late replay. | Transactional runtime tests; Kani on production transaction boundaries with arbitrary retained pre-state, independently dirtied intermediate fields and field-by-field restoration comparison. Include output columns and counters; for late replay, append before the later failure. | The final required Kani suite passed 22/22; historical candidate reevaluation accepted 22/23, with late-replay rollback incomplete. The final 12 GiB late-loop attempt timed out at 600 seconds. Broad all-state rollback and full-64 sensor-note proofs are incomplete. These incomplete/resource-limited runs establish no proof result. |
| **P1** | Ring/anchor invariant and chronological replay across wrap, eviction, late insertion and resync. Boundary: ring helpers, reconstruction and update order. | Kani for production ring bounds and short sequences; differential/property tests against an independently structured chronological reference. | Finite traces cover tested sequences only. If the reference shares filter equations, it cannot independently validate filter math. Report evicted/refused history explicitly. |
| **P1** | Full address and anonymous/local namespaces do not alias; invalid/exhausted keys are refused. Boundary: tracker lookup, `FixedMap`, fusion dispatch, reserved radar namespace and output flags. | Kani for key/range/no-wrap boundary logic; multi-radar integration tests with equal or near kinematics, missing/conflicting codes and ambiguity. | Small map proofs cover their declared slots/operations and stubs only. They do not establish fusion identity end to end. Proximity supports association, not identity. |
| **P1** | Runtime capacity is fixed and excess work/data is visible; prohibited hot routes do not allocate. Boundary: tracker/fusion buffers, output, scratch and allocator hooks. | Counting allocator with positive and injected-allocation controls on declared routes; maximum-capacity integration tests; source review and memory accounting. | Allocator evidence is route-scoped and cannot prove all inputs/cores or observe every OS allocation mechanism. Kani is for small capacity arithmetic, not a full symbolic tracker. |
| **P1** | Input-dependent work is charged before it is performed; budget refusal preserves state and accounts for omitted input. Boundary: candidate, grid/sort, exact-test and retirement loops. | Map every input-sized loop to its pre-charge; exhaustion tests at every work class and across sources; arithmetic proof for budget helper; worst-case latency measurement. | Tests cannot guarantee future loops are charged; counters do not prove WCET. Keep a source-reviewable loop-to-charge map and a separate worst-case deadline result. |
| **P1** | Timestamp split arithmetic and refusal cases are exact and overflow-safe. Boundary: production `mono_split` helper used by replay clock classification. | Kani for bounded symbolic properties; Creusot contracts for the pure helper; stable-toolchain tests for caller wiring. | Kani full-typed monotonic split passes in the accepted suite. Creusot v0.14.0 proves the direct production helper contract (11 goals); see the Creusot record below. Neither proves full clock policy, stream order, scheduling or generated-code equivalence. |
| **P2** | Memory safety / undefined behavior on executed paths. Boundary: unsafe code, FFI, pointer/slice and allocator edges. | Review unsafe contracts; Miri for supported Rust abstract-machine checks on selected executions; sanitizers for exercised error classes on supported targets. | Three exact timestamp/rollback/recovery regressions passed under pinned Miri; no sanitizer result is claimed. These tools do not exhaust inputs or prove tracker semantics, allocation freedom, deadlines or accuracy. |
| **P2** | Estimator behavior, uncertainty and physical accuracy. Boundary: filter/turn math, covariance, truth/reference and scoring. | Independent analytic/high-precision cases; synthetic truth with known noise; predeclared held-out/prospective metrics with full denominators and exclusions. | Formal tools can establish selected arithmetic/state properties, not agreement with aircraft dynamics or physical truth. Correlated split-ADS-B agreement is not independent physical validation; RMS/NIS alone does not establish calibration. |
| **P2** | End-to-end conservation and operational performance. Boundary: ingestion, ordering, fusion, tracker, serialization and deployment. | Frozen input/source/binary manifests; independent conservation checker; complete replay dispositions/output comparisons; worst-case load and latency measurement. | A replay applies only to its data, build, reference method and configuration. It does not establish untouched holdout, live integration, all-NAS capacity or independent truth unless those were tested. |

## Current verification results

**Kani 0.68.0 registry:** all 22 enabled harnesses passed in one fresh sequential run on frozen current production/proof source. The registry contains 25 obligations; three remain disabled and experimental. The accepted subset covers three output-table properties, seven dispatch cases, real-decision/history commit, refusal routing, seven transaction-component restoration properties, full-typed timestamp split, and full-64 live-count/monotonicity. Each raw result is bound to its source snapshot, command, toolchain, declared stubs, and required covers. The local run predates a comment-only requirement-tag correction on the experimental late helper; [the machine-readable record](../validation/rust-proof-status.json) retains both hashes and the exact change. CI reruns the registered suite on each proposed revision.

The earlier candidate suite remains recorded FAIL. Re-evaluation of its unchanged raw results accepted 22/23 after correcting the evaluator's treatment of compiler-cloned cover sites; late replay was incomplete. That historical re-evaluation is distinct from the fresh 22/22 run. The final late-loop attempt at 12 GiB timed out at 600 seconds, so the combined rollback obligation remains open.

The broad all-state rollback and late-replay proofs, plus the full-64 sensor update/note proof, remain experimental or incomplete. Recent resource ceilings were reduced; a timeout or memory exhaustion is not a proof or a counterexample. Do not generalize the accepted bounded harnesses to arbitrary histories, all tracker state, every capacity, or full-system correctness.

**Negative controls:** the output-column mutation was detected by its intended assertion. The wrong timestamp-split result was detected by the Kani control and by Creusot. A focused resync-emission control passed its positive proof and detected the intended missing-row mutation. The earlier larger-model control exhausted memory and remains recorded as inconclusive.

**Creusot v0.14.0:** the assurance package imports the same `mono_split.rs` production helper used by replay; it does not prove a separately copied model. The helper has no preconditions and specifies the exact `Some`/`None` eligibility and representability split and returned values. Eleven proof goals passed: ten with Alt-Ergo and the helper contract with Z3. No custom trusted external specification was added. A wrong-step mutant translated and its `vc_split` obligation failed as expected. This proves the helper contract under the pinned Creusot/Why3/solver model, not the replay clock as a whole or stable Rust machine code.

**Miri:** three exact production regression executions passed under `nightly-2026-08-21` with default interpreter checks: timestamp refusals/signed boundaries, partial late-replay rollback, and invalid-anchor recovery. The CI workflow requires each selected test to execute exactly once. This covers those executions only.

**Runtime checks:** the recorded product check suite reports 1,890 Rust tests passing; final formatting, Clippy, timestamp-helper tests and trace checks pass. The late-loop runtime check suite reports 430 tests passing. These are software regression results, not formal proof or operational performance results.

## Reproduce the checks

Install the pinned tools shown in [.github/workflows/rust-proofs.yml](../.github/workflows/rust-proofs.yml), including product Rust 1.89.0 and Kani 0.68.0. Run the registry sequentially from the repository root, with a new evidence directory:

```sh
python3 tools/test_kani_validation.py
python3 tools/test_rust_proof_suite.py
python3 tools/rust_proof_suite.py --repo "$PWD" \
  --registry "$PWD/tools/rust_proofs.json" \
  --out /tmp/oa-rust-proof-evidence --target-dir /tmp/oa-rust-proof-cache
```

The runner records source and tool identities, exact commands, required branch covers, declared stubs, raw results, and resource outcomes. It refuses partial reports, mismatched bindings, failed assertions/unwind checks, and missing required covers. A compiler-cloned cover site must have a satisfied instance; an unsatisfiable site alone cannot pass. Build caches are namespaced by the resolved checkout path, and solver runs are offline and single-job.

The [Creusot package instructions](../validation/oa-creusot-check/README.md) describe its separate pinned toolchain and direct production-source import. It adds no product dependency. `tools/prepare_rust_proof_controls.py` freezes source and prepares isolated one-site negative-control checkouts; provisioning alone is not a proof or a detected control.

## Requirements and claims

Requirements specify the intended property; tests exercise chosen production executions; bounded proof harnesses quantify over inputs admitted by their assumptions and unwind limits. Keep requirements linked to tests/proofs, and review the oracle independently. If a proof stubs a decision, its conclusion is conditional on that decision. Tests remain necessary for caller wiring and realistic replay. For state restoration, compare against an independent pre-state that uses non-default/arbitrary field values; do not let the expected state share mutations with the transaction under test.

## Tool boundaries

- **Kani**: bounded symbolic checking with explicit assumptions, stubs, loop limits and configured capacities. A passing bounded harness is not induction over arbitrary histories or proof of code it does not reach.
- **Creusot**: deductive contracts for the directly imported production helper. Record toolchain, solver, trusted model and source identity; a helper proof does not verify its caller or system.
- **Miri/sanitizers**: complementary checks on executed paths and supported operations, not exhaustive model checking.
- **Allocator/performance runs**: observations on named routes and hardware/configurations; controls validate the measurement, not unexecuted routes. Refused, dropped or overflowed work must remain visible.
- **Replay and independent truth**: evidence about conservation and empirical behavior. Verifiers cannot establish truth-source independence or model validity.

## Release state

The assurance/release gate remains **OPEN**. Allocation C5 coverage, matched ERAM comparison, and independent holdout/physical truth remain unresolved. The accepted Kani subset, Creusot helper proof, and runtime tests do not close those project-level claims or establish that the entire project is verified.
