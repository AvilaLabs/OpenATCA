# Assurance status

What has been checked, how, and what has not. This page states where the project is, not where it
is meant to go; the design rules are in [RULESET.md](RULESET.md) and the goals in
[SCOPE.md](../SCOPE.md). When this page and a design document disagree about what is true today,
this page is the one to believe, and the disagreement is a defect to report.

The [capability and integration register](CAPABILITIES.md) maps implemented entry points to
their remaining integration and qualification boundaries. Design build orders are not status
ledgers.

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
| Unit and integration tests | The frozen `e84d933` baseline passed 1,838 release tests; full checks run before promotion | A test checks the cases it was written for; [candidate validation](VALIDATION.md) also checks the evidence and open claims |
| Real-data suites | FAA NASR and CIFP (2026-10-29 cycle), a one-day Chicago Center ADS-B clip, GeodTest, RAP weather, MVA charts, 3DEP terrain | Skipped when the data is not on the machine; one clip of one day |
| Requirements traceability | `oa-trace`: every requirement in [REQUIREMENTS.md](../REQUIREMENTS.md) has a test or proof | Shows a test exists, not that it is sufficient |
| Bounded proofs (Kani) | 112 registered harnesses, 103 enabled and 9 disabled experiments; see [Rust assurance](RUST_ASSURANCE.md) | Results are revision-, assumption- and bound-specific; push-tier selection is not a full-suite run |
| Model checking (TLA+) | The replay clock; see [tla/README.md](tla/README.md) | Small bounds; the model abstracts the code |
| Fuzzing | 21 libFuzzer targets over the byte-level decoders; see [FUZZING.md](FUZZING.md) | Most targets have had hours, not weeks |
| Mutation testing | Saved numerical and truth cases have strengthened independent-answer tests | Unresolved cases are not automatically equivalent or confirmed defects |
| Allocation instrumentation | Named M3 tracker/fusion routes run under an isolated counting allocator, with positive and injected-allocation controls | Complete C5 coverage remains open; see [candidate validation](VALIDATION.md) |
| Turn-model moments | Closed-form and independent high-precision references for nonlinear prediction and separate discovery-floor regularization; numerical refusals are counted and history replay is atomic | Gaussian, cubature and regularization approximations remain; synthetic accuracy does not establish recorded-traffic truth |
| CI | GitHub Actions: format, clippy with Rust 1.89.0 (the declared minimum), release-mode tests, traceability, scoped M3 allocation check, no external crates | x86-64 and aarch64 Linux |

The [full manual Rust assurance run at `4ad16fe`](https://github.com/AvilaLabs/OpenATCA/actions/runs/45231435245)
passed all four Kani shards and Miri on 2026-11-23. This evidence belongs to that source and the
registered assumptions and bounds; it does not close the benchmark or operational milestones.
The [regional replay increment](REGIONAL_INCREMENT_2026-11-23.md) records the next implemented
flight, recovery and safety-evaluation slices and their limits.

Everything above runs on x86-64 Linux. CI also runs the whole release test suite, golden replay
digests included, on aarch64 Linux (the `arm` job), so bit-identical replay (RULESET B6) is tested
on two architectures and gives the same digests on both. `oa-math` is written so that results do not
depend on the machine's maths library. No other operating system or architecture has been tested.

## The design rules and how far the code meets them

| Rule | Status today |
|---|---|
| Fixed capacities, named, with a written behaviour when full (C1, C2) | Followed in most collections. Buffer addresses and capacities are checked on exercised paths. A separate allocator hook checks named M3 routes; complete C5 coverage remains open in [candidate validation](VALIDATION.md). |
| Bounded work, not only bounded storage (C7) | Not uniform. Radar initiation now shares a checked work allowance across radars, including address lookups and grid queries; exhaustion is counted. Other paths still need work arguments. |
| Invalid input is refused and counted, never passed on or substituted (E5, E6) | Followed in most paths. Some paths have been found to substitute a value or drop data silently; these are being found and fixed. |
| Identity from positive evidence (E7) | Anonymous radar candidates remain separate source-local hypotheses and carry an unconfirmed-identity flag. Proximity and Mode A do not establish duplicate aircraft identity. Input addresses cannot claim the reserved local-key namespace. |
| Arithmetic on untrusted values is checked before use (E8) | Not uniform; being made checkable with clippy's arithmetic and cast lints on the input-handling crates. |
| Deterministic replay (B6) | Tested on one platform; see above. |
| Measurements that cannot flatter the result (G6) | Capacity claims now fail on any lost work and on any second over budget. Benchmark definitions are in [BENCHMARKS.md](BENCHMARKS.md); each change to a metric is a reviewed change. |

## Performance numbers

The [2026-11-23 radar investigation](RADAR_CAPACITY_2026-11-23.md) closes the coverage lane with
62,002/62,002 anonymous gate traversals complete in one 8,192-aircraft stress case, versus
16,089/62,002 on `ce7d4b1`. It remains a failed capacity case: 8/15 measured seconds exceed 1 s,
the peak is 1.545 s, and missing-height losses remain nonzero. The linked report retains the
full counters and timing sequence, including identity errors and the limits of this evidence.

The capacity figures come from `tools/oa-perf`: synthetic traffic, a laptop (Intel i3-N305), no
flight-plan processing. A run passes only if every measured second finishes within real time and no
work is lost (no refused plot, capped batch, overflow or refusal of any kind). They show where the
tracker breaks under load; they are not a like-for-like comparison with a fielded system.
Measured input coverage verifies the offered aircraft count independently; reported track counts
are hypotheses and may include unresolved duplicates.
