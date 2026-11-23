# Radar coverage investigation closure, 2026-11-23

The polar candidate index completes the required anonymous-plot gate traversal in the measured
8,192-aircraft case. This is a prototype source improvement, not a passing capacity qualification.
Strict real-time performance, missing-height evidence and identity quality remain open.

## Measured case

Intel i3-N305, Rust 1.89.0, ordinary portable release profile, one serial processing lane. Synthetic
stress traffic in a 100 km square: 8,192 aircraft all sending ADS-B at 1 Hz, one en-route radar and
two terminal radars. Seed 20261123, 60 s warm-up and 15 s measured; Mode C is removed from every
measured radar plot. Assignment logging, truth scoring and association/initiation diagnostics are
enabled. No FAA recording or sealed hold-out is used.

```sh
cargo +1.89.0 run --release --locked -p oa-perf -- one \
  --n 8192 --layout stress --radars e,t,t --adsb-frac 1 --seed 20261123 \
  --warmup 60 --duration 15 --no-early-stop --max-tracks 16384 --max-wall 120 \
  --truth-score --association-diagnostics --initiation-diagnostics \
  --radar-no-altitude --out /path/to/new-evidence
```

Builds and runs used the project's 6 GiB memory guard, zero swap and two Cargo jobs. Timing is
preliminary on a loaded development machine. The baseline is source `ce7d4b136224302975bed4cd2f8b30edc169d66b`;
the final measurement is an exploratory working-tree build based on that commit, before final
review cleanup and tests. It is not evidence from a clean frozen acceptance revision. The measured
binary SHA-256 was `9506a5831fc92832c66d555f216ca7d15237d4f7ab4988cbffeff64a6c003e12`.

| Measured-window result | Baseline | Polar working build |
|---|---:|---:|
| Anonymous plots offered | 62,002 | 62,002 |
| Gate traversal complete | 16,089 (25.95%) | 62,002 (100%) |
| Gate traversal incomplete | 45,913 | 0 |
| Capped radar batches | 37 | 0 |
| Plots deferred by incomplete graph | 60,420 | 0 |
| Mean processing per traffic second | 339.4 ms | 982.8 ms |
| Worst measured second | 383.2 ms | 1,544.5 ms |
| Seconds at or above 1 s | 0 / 15 | 8 / 15 |
| Preallocated buffers stable | yes | yes |

The baseline ran quickly because it abandoned substantial work; its time is not a passing
capacity result. Gate completion measures traversal of each offered plot's eligible candidate
set, not successful assignment, aircraft identification, or the availability of every measurement.

The polar run performed 2,384,444 exact pair checks and retained 77,454 accepted edges. Each radar
completed every offered plot: 10,153/10,153, 25,459/25,459 and 26,390/26,390. Polar work was
138,801,852 charged units: 18,425,453 build and 120,376,399 query, over 62,002 queries and 737,280
track-slot visits across classification and posting construction. Charged units are not CPU time.

The zero-loss criterion still fails: `no_height_pairs=12121` and `cand_no_height=7`. These remain
classified as losses. The final snapshot matches all 8,192 offered aircraft because ADS-B is also
present. It cannot establish radar-only recovery. Measured assignments include 72 wrong-aircraft
plots and 135 swaps in 57,431 assignment rows. The baseline reports 1 wrong plot and 1 swap in
only 1,536 assignment rows; those counts have different processed cohorts and cannot establish
an accuracy improvement or regression rate by themselves.

Raw synthetic evidence is retained in [baseline KV](evidence/radar-polar-20261123/baseline.kv),
[final KV](evidence/radar-polar-20261123/final.kv), [baseline seconds](evidence/radar-polar-20261123/baseline-seconds.csv)
and [final seconds](evidence/radar-polar-20261123/final-seconds.csv). These preserve the failed
verdict, all exported counters and the complete per-second timing sequence.

## Source change and review scope

One fixed polar posting per standard-width track replaces the geographic coarse search. Width
bands and outward-padded radial intervals narrow candidates; the exact coarse predicate remains
authoritative. Exceptional geometry uses an ALWAYS posting or full scan. Wide tracks keep their
separate fixed list. Index construction, lookup and ordering share a named bounded allowance;
exhaustion clears partial output and defers the whole anonymous graph. Hard-address decisions
retain their independent path.

Exact forecast and conversion reuse removes redundant math while retaining gate inputs and
numerical refusals. Position-only prediction uses the original accumulation order, full mapped
state/mean validation and a conservative overflow certificate; extreme inputs use the full path.
Independent review found the prepared-conversion API could pair stale terms with another plot;
it is now crate-private and used only with the current immutable plot, site and bias. Boundary,
brute-force parity, bit-equivalence and capped-graph regressions accompany the change.

## Remaining work, deliberately outside this closure

1. Profile and redesign the dominant candidate work before further arithmetic tuning. Linux perf
   and BPF sampling were unavailable under this host's kernel permissions. Keep wall-time sampling
   in the shell and preserve the core's purity and the deterministic shared-budget scheduler.
2. Qualify strict latency on a clean frozen revision, controlled machine, longer windows and
   multiple seeds, with every offered plot accounted for and every loss counter retained. Run
   ordinary-altitude controls and radar-only cold-start/source-outage cases separately.
3. Investigate missing-height evidence and wrong-aircraft assignments using event-level truth;
   100% gate traversal alone does not close either problem.

No further micro-optimization is required to publish this corrected research source. Broader
resilience and freeze qualification resume alongside this explicitly open capacity limitation.
