# Retrospective scoring capability

**Status:** Proposal, under review.
**Accreditation authority:** External; independent accreditation is pending.

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

### Source-backed checks and known-answer evidence

- Test: `crates/oa-track-tools/examples/p002.rs::the_hold_out_gate_counts_only_openatca_rows_matched_to_a_grid_sample`
  (OA-TRK-373) checks that hold-out scoring counts only rows matched to the defined grid sample.
- Test: `crates/oa-track-tools/examples/p002.rs::the_cross_day_rule_is_met_only_on_a_and_b_in_openatcas_favour_and_c_not_against`
  (OA-TRK-393) checks the stated cross-day rule and pins the generated synthetic report SHA-256
  to `cd54b9880a95154f58dc5316b377eb06c69659af398b38bda23ef54572afc288`. The producer is source
  commit `8c4a196d826be9cf6b2a14d2e1c7365c7f44e503`; the input rows and three-day rule evaluation
  in this test are synthetic. Guarded request `openatca-L1-D1-accreditation-20261126-234048-168744`
  ran the example's 58 tests, all passed.
- Test: `crates/oa-track-tools/examples/p002.rs::the_reports_the_tool_prints_are_read_exactly`
  checks exact report parsing.
- Test: `crates/oa-track-tools/examples/p002.rs::export_requires_sidecar_and_checks_its_digest`
  checks export binding to its required sidecar digest.
- Test: `crates/oa-track-tools/tests/score.rs::the_real_clip_tracker_run_has_the_pinned_digest`
  (OA-TRK-171, OA-TRK-330) conditionally pins an upstream tracker output digest to
  `8892186a0d06e90ebbbf2545c6f2af286a46949c255bedf68b1072fff2607e22`. It returns without
  scoring if `OPENATCA_ADSB_CLIP` is unset and is not evidence of a P-002 score/report golden.

The scorer's operational run outputs are configuration- and cohort-specific. The pinned P-002
digest is for a synthetic cross-day-rule report, not an operational cohort. The upstream tracker
digest is distinct from a scorer-result golden.

## Limits and open questions

The reference is retrospectively smoothed and may share correlated sensor error with the input
feed; it is not independent absolute truth. Any time alignment fitted from the scored period is
retrospective and noncausal, not online clock calibration or live-latency evidence. Publication
quantization of OpenATCA positions and velocity components is a scorer experiment, not tracker
behavior or exact wire identity; a run uses it only when its frozen configuration says so.

Historical build metadata does not validate every scoring input, reference assumption, exclusion,
or report calculation. Reference independence, cohort representativeness, and the scope of any
claim remain method-specific questions for the governing protocol and review.
