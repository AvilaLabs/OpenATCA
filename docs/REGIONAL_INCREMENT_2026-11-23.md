# Regional replay development increment, 2026-11-23

This increment builds on surveillance source `4ad16fe`; it does not change the surveillance
tracker, its settings or its capacity verdict. [CAPABILITIES.md](CAPABILITIES.md) records the
whole-system boundary and the integrated regional session still to be built.

## Route evidence at forecast issue time

The optional arrival experiment selects a route revision received by the issue cutoff for the
full flight GUFI, retains its provenance, and refuses conflicting evidence. A disqualifying
amendment cannot disappear in favor of an older qualifying route. Forecast geometry is bounded
before resolution. The experiment retains its first eligible issue per flight; later amendments
do not update that frozen forecast. It still consumes received expanded geometry rather than
implementing an independent route converter or shared operational trajectory.

## Raw replay restart evidence

The raw-only scope CLI can persist and later verify exact inputs, adaptation, receipt semantics,
settings and ordered frame hashes. Interrupted publication preserves a synced, independently
verifiable partial copy; an incomplete final file is refused. Publication never overwrites an
existing witness and introduces no exception to the sealed-data guard. Verification replays from
the beginning, compares each frame before publishing it, and refuses counted loss. This is a
building block for recovery, not a supervisor or state restoration mechanism. See the
[format and recovery boundary](../crates/oa-scope-replay/tests/RAW_WITNESS.md).

## MSAW encounter measurement

The [offline evaluator](../tools/oa-msaw/EVALUATION.md) compares complete alert evidence with
independent synthetic hazard windows. It conserves timely/late/missed warning-onset counts,
reports nuisance records and terminal loss observations separately, and distinguishes absent
evidence from an empty log. Compressed episodes cannot establish continuous warning or every
protection transition. Ambiguous pre-window warning evidence is refused. Regressions drive the
real net through altitude loss and return, in addition to independent boundary/oracle fixtures.

## Review and qualification boundary

The coordinating review and cross-reviews covered RULESET A–H, the actual alert-record contract,
cutoff safety, source identity, output collisions, interrupted publication, comparison budgets
and negative controls. The software gate must pass at the published source: format, strict
workspace Clippy, release tests, Python tooling suites, traceability and the scoped allocator
check. A passing software gate permits publication of these research components; it does not
qualify a facility, close M4/M5 or establish operational safety.

Surveillance qualification is separate and retains a named source and its own immutable input,
binary and output evidence. Development/rehearsal data may be replayed under the preregistered
rules; sealed acceptance data must not be consumed for feature development. No FAA-derived
recording or aircraft history is included in this increment.
