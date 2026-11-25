# Retrospective scoring capability

**Status:** Proposal, under review.

## Intended use and boundary

The P-002 scoring example in `crates/oa-track/examples/p002.rs`, together with its tracker, truth,
and scoring helpers, is proposed for retrospective comparisons under one frozen protocol, dataset
cohort, configuration, metrics, exclusions, and uncertainty method. Results apply to that cohort
and method. They do not establish online accuracy, absolute truth, real-radar-only performance, live
latency, operational suitability, or superiority beyond the reviewed scope.

## Configuration and admission record

Freeze the exact source and binary, protocol and amendments, input identities, reference
construction, sample grid, exclusions, quantization, bootstrap settings, and report-generation
options. Record the time treatment and any fitted alignment, metrics and undefined-value rules,
resource limits, command, and expected report schema before execution.

Bind the binary to an isolated empty-target build, exact invocation, compiler/toolchain, lockfile,
resolved dependency manifest, relevant environment, complete build log, and artifact hashes. A
source hash or historical build record alone does not establish the dependency closure or the
binary used for a particular run. Retain complete report inputs, outputs, discrepancies, and
identity bindings for independent review.

## Evidence and decision criteria

Source tests cover pairing, timing, quantization, uncertainty, and report bindings. For credited
use, the reviewer must check run-specific evidence and the scorer against known-answer synthetic
cases plus a separately controlled reference. Review pairing, interpolation, exclusions,
uncertainty, report arithmetic, and all source/input/reference bindings. Acceptance criteria and
uncertainty bounds must be frozen in the approved protocol before the scored run. The decision
records accepted, rejected, or restricted use with reasons; failed and undefined criteria remain
visible as specified by that protocol.

Refuse admission if source, binary/build closure, protocol, inputs, reference, or report settings
cannot be bound; the cohort or exclusions changed after results; expected behavior is not specified;
or outputs are incomplete. Preserve every discrepancy and failed criterion. A method or claim
change requires a reviewed prospective amendment and a new decision.

## Limits and open questions

The reference is retrospectively smoothed and may share correlated sensor error with the input
feed; it is not independent absolute truth. Any time alignment fitted from the scored period is
retrospective and noncausal, not online clock calibration or live-latency evidence. Publication
quantization of OpenATCA positions and velocity components is a scorer experiment, not tracker
behavior or exact wire identity; a run uses it only when its frozen configuration says so.

Historical build metadata does not validate every scoring input, reference assumption, exclusion,
or report calculation. Reference independence, cohort representativeness, and the scope of any
claim remain method-specific questions for the governing protocol and review.
