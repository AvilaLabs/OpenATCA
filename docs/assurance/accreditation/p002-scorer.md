# Proposed accreditation record: retrospective scoring capability

**Status: DRAFT / NOT ACCREDITED / NOT APPROVED FOR system verification.** The scorer and recorded
comparisons are not an independent truth source or an accreditation decision.

## Identification and intended use

- **Capability:** the P-002 scoring example in `crates/oa-track/examples/p002.rs` and its tracker,
  truth, and scoring helpers.
- **Proposed use:** retrospective comparison of tracker and reference outputs under one frozen
  protocol, dataset cohort, configuration, metrics, exclusions, and uncertainty method.
- **Not intended to establish:** online accuracy, absolute truth, real-radar-only performance, live
  latency, operational suitability, or superiority outside the reviewed cohort and method.
- **Configuration:** a credited run must bind exact source and binary, toolchain, protocol and
  amendments, input identities, reference construction, sample grid, exclusions, quantization,
  bootstrap settings, and report-generation options.

## Reference and scoring assumptions

The reference is retrospectively smoothed and may share correlated sensor error with the input
feed; it is not independent absolute truth. Any time alignment fitted from the scored period is
retrospective and noncausal. It is not an online clock calibration or evidence of live latency.
Rounding or publication quantization can affect measurement and must be treated as part of the
scorer configuration, not as zero-error data.

The development source contains an optional publication-quantization mode for OpenATCA positions
and velocity components. It is a scoring experiment, not tracker behavior or exact wire identity.
Do not infer that a run used this mode unless its frozen run configuration explicitly records it.

## Evidence and gaps

Source tests cover pairing, timing, quantization, uncertainty, and report bindings; no tests were run
for this draft. Historical build metadata records one frozen executable and build environment, but
build provenance alone does not validate every scoring input, reference assumption, exclusion, or
report calculation. Any failed or undefined acceptance criteria must remain visible in the applicable run record and
cannot be reclassified by this draft. No result tables or data identities are included here.

## Required entrance and exit evidence

The approved TEMP must specify the intended claims, reference limits, scoring method, sample and
exclusion rules, and approval authority. Before a credited run, freeze and hash the exact source,
binary, protocol, inputs, configuration, time treatment, quantization, bootstrap, and report
settings. Independently check the scorer on known-answer synthetic cases and a separate reference,
including pairing, interpolation, exclusions, uncertainty, and report arithmetic. Preserve all
discrepancies and failed criteria. Any method or claim change requires a reviewed amendment and new
approval. Custodian, independent reviewer, and accreditation authority remain unassigned.
