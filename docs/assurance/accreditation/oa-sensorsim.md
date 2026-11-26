# Sensor simulator capability

**Status:** Proposal, under review.
**Accreditation authority:** External; independent accreditation is pending.

## Intended use and boundary

`oa-sensorsim` in `crates/oa-sensorsim` is proposed to generate deterministic, labeled synthetic
radar and ADS-B scenarios for component tests, fault injection, and sensitivity studies. A result
supports conclusions about the selected software model and frozen inputs only. It does not establish
operational radar detection probability, surveillance accuracy, field coverage, sensor registration,
or fidelity outside the selected model.

## Configuration and admission record

Freeze the truth source and covered time interval, site and coordinates, scan settings, sensor
profile, error and split parameters, random seed, platform, output schema, resource limits, and
expected invariants. Bind these to source revision and clean/dirty state, compiler/toolchain,
command, exact binary hashes, and generated report identities.

The evidence package must prove build closure using an isolated empty target directory, exact build
invocation, toolchain, lockfile and resolved dependency manifest, relevant environment, complete
build log, and produced artifact hashes. A top-level source hash alone does not establish which
dependencies or cached artifacts produced the executable. Retain all inputs, outputs, refusals,
resource records, and discrepancies.

## Evidence and decision criteria

Synthetic tests for random generation, geometry, quantization, and repeatability can support
implementation checks for those properties. They do not validate physical sensor behavior. Before
credited use, the TEMP must identify the intended model claims, fidelity class, known-answer cases,
acceptance bounds, uncertainty treatment, and refusal conditions. An independent reviewer checks
the frozen model/configuration, truth coverage, implementation checks, and comparison to a separately
controlled reference. The authority records accepted, rejected, or restricted use and the evidence
supporting that decision.

Refuse admission if the truth coverage, sensor profile, seed, source/build closure, or input/output
identity is unbound; an acceptance criterion is missing or post hoc; a required comparison is absent;
or outputs are incomplete. A failed bound or unresolved discrepancy stays visible and cannot be
reclassified by changing model settings after observing results.

### Source-backed checks and known-answer evidence

- Test: `crates/oa-sensorsim/tests/sim.rs::same_seed_same_bytes_and_streams_are_per_aircraft`
  (OA-SIM-009), introduced by `43c5657fa77239e981d2a1a49775fb4b80259759`, checks deterministic
  output for a fixed seed, changed output for a different seed, and per-aircraft stream independence.
- Test: `crates/oa-sensorsim/tests/adsb_error.rs::the_output_depends_only_on_the_arguments`
  (OA-SIM-039), introduced by `540210e4084a4208fdf2fe89dd748acc0bb6fe25`, checks repeatability
  from the supplied truth, reports, and error parameters.
- Test: `crates/oa-sensorsim/tests/sim.rs::cat048_decodes_back_to_the_plots` checks a synthetic
  encoder/decoder round trip for simulated plots.
- Test: `crates/oa-sensorsim/tests/sim.rs::same_seed_same_bytes_and_streams_are_per_aircraft`
  emits the complete deterministic CAT048 payload for its fixed synthetic input when test output
  is enabled. Guarded request `openatca-L1-D1-accreditation-20261126-233909-9fc8a8` recorded
  7,491 bytes with SHA-256
  `dd4e138b932e1484bb2eeb3fe3ef29d6710b4e5dcefa56de31e1d516e5c1d5a6` over the full payload.
  The exact full payload is pinned as text hex in
  `crates/oa-sensorsim/tests/fixtures/cat048_seed77.hex` and compared byte-for-byte by the test;
  this assertion was added in `326507c82f43e263aae2ac6227862b7fdd315700`. The output was produced
  by the deterministic synthetic case introduced in source commit
  `8c4a196d826be9cf6b2a14d2e1c7365c7f44e503`. Guarded request
  `openatca-L1-D1-accreditation-20261126-001746-015878` passed formatting, Clippy, and the
  simulator's 19 tests. This golden is distinct from the same-seed comparison and separate
  round-trip test.

The cited cases validate only the encoded synthetic model behavior, not physical sensor fidelity.

## Limits and open questions

The radar model queries a truth trajectory, solves scan crossing times, and applies configured
detection, range and azimuth noise, coverage, altitude quantization, false plots, and split plots.
Pressure altitude is treated as ellipsoid height; truth without altitude uses zero height for the
geometry model. Named profiles contain literature-derived values and assumptions; site biases and
split geometry require explicit values because they are not established by the profile names.
Some outputs use platform math-library functions, so bit-level repeatability can vary by runtime.
The plot-table limit alone does not establish end-to-end memory or runtime bounds.

Unsourced bias and timing distributions, clutter, site adaptation, propagation, receiver processing,
and correlated errors remain material validation gaps. Representative measured data and documented
uncertainty for range, azimuth, time, detection, false/split reports, altitude, and coverage are
required before making claims about operational radar performance. Model owner, independent
reviewer, and approval authority must be assigned in the governing plan.
