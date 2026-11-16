# Assurance status

What has been checked, how, and what has not. This page states where the project is, not where it
is meant to go; the design rules are in [RULESET.md](RULESET.md) and the goals in
[SCOPE.md](../SCOPE.md). When this page and a design document disagree about what is true today,
this page is the one to believe, and the disagreement is a defect to report.

## What OpenATCA is today

A research prototype. Nothing here is certified, approved or fit for operational use, and nothing
may be connected to anything that affects the national airspace. The tracker, the benchmark
harness and the airspace model run on recorded and simulated data. The comparison against ERAM on
recorded traffic is being built; no result of it is published yet.

Independent review of the source keeps finding defects, including serious ones in new code. Each
fixed defect gets a test that fails without the fix. Open findings are tracked privately until they
are fixed. A finding is fixed when its fix is merged and the full test suite and CI pass, not
before.

## How the code is checked

| Check | What it covers | Limits |
|---|---|---|
| Unit and integration tests | About 1,600 tests over the workspace, run in full before every push | A test checks the cases it was written for |
| Real-data suites | FAA NASR and CIFP (2026-10-29 cycle), a one-day Chicago Center ADS-B clip, GeodTest, RAP weather, MVA charts, 3DEP terrain | Skipped when the data is not on the machine; one clip of one day |
| Requirements traceability | `oa-trace`: every requirement in [REQUIREMENTS.md](../REQUIREMENTS.md) has a test or proof | Shows a test exists, not that it is sufficient |
| Bounded proofs (Kani) | 40 harnesses, each with its bound; see [VERIFICATION.md](VERIFICATION.md) | True only up to the stated bound; some harnesses did not finish and are listed there |
| Model checking (TLA+) | The replay clock; see [tla/README.md](tla/README.md) | Small bounds; the model abstracts the code |
| Fuzzing | 21 libFuzzer targets over the byte-level decoders; see [FUZZING.md](FUZZING.md) | Most targets have had hours, not weeks |
| Mutation testing | Being run over the core crates to measure how many deliberate code changes the tests miss | No published result yet |
| CI | GitHub Actions: format, clippy with Rust 1.89 (the declared minimum), release-mode tests, traceability, no external crates | One platform |

Everything above runs on x86-64 Linux. Bit-identical replay (RULESET B6) is tested on that platform.
`oa-math` is written so that results do not depend on the machine's maths library, but no other
architecture has been tested.

## The design rules and how far the code meets them

| Rule | Status today |
|---|---|
| Fixed capacities, named, with a written behaviour when full (C1, C2) | Followed in most collections. Not yet enforced by a test: the counting-allocator check (C5) needs an exception to the no-unsafe rule (A0) that is not decided, so "no allocation after startup" is a design rule, not a verified property. |
| Bounded work, not only bounded storage (C7) | Not uniform. Several decoders and the tracker have had limits that capped memory but not the work done; these are being fixed and the rule is being made checkable. |
| Invalid input is refused and counted, never passed on or substituted (E5, E6) | Followed in most paths. Some paths have been found to substitute a value or drop data silently; these are being found and fixed. |
| Identity from positive evidence (E7) | Written as a rule on 2026-11-16. Matching by local name, by raw text, or by position alone has been found and fixed in several places. |
| Arithmetic on untrusted values is checked before use (E8) | Not uniform; being made checkable with clippy's arithmetic and cast lints on the input-handling crates. |
| Deterministic replay (B6) | Tested on one platform; see above. |
| Measurements that cannot flatter the result (G6) | Capacity claims now fail on any lost work and on any second over budget. Benchmark definitions are in [BENCHMARKS.md](BENCHMARKS.md); each change to a metric is a reviewed change. |

## Performance numbers

The capacity figures come from `tools/oa-perf`: synthetic traffic, a laptop (Intel i3-N305), no
flight-plan processing. A run passes only if every measured second finishes within real time and no
work is lost (no refused plot, capped batch, overflow or refusal of any kind). They show where the
tracker breaks under load; they are not a like-for-like comparison with a fielded system.
