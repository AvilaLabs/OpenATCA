# Assignment-regret prototype (test only)

`assignment_regret_prototype.rs` characterizes a possible supplement to the current local
competitor-margin ambiguity rule. It is not linked into `oa-track` and does not change tracker
behavior.

For one exact GNN component, let `M[r]` be the column selected for padded row `r`. Contract each
matched row/column pair into one vertex. The arc from `r` to `k` has weight

```text
C[r, M[k]] - C[r, M[r]].
```

A directed cycle is an alternate perfect matching. The cheapest cycle through a selected real
edge's row is therefore the increase in the complete padded assignment objective when that edge
is forbidden. Floyd-Warshall finds every such value in `O(n^3)` time and `O(n^2)` scratch for the
same `n <= 160` bound as the exact Hungarian solve. The prototype charges every arc inspection,
Floyd-Warshall cell visit, optimality check, and selected-row scan; exhaustion returns no result.

The tests compare the calculation with an independent enumeration of every padded perfect
matching for hand-built and deterministic small graphs. The enumerator shares the documented
padded-matrix construction with the prototype but does not share its alternating-cycle algorithm.
The reconstructed padded objective is also checked against `oa_track::assoc::objective`, a
separate implementation of the assigned-cost plus unmatched-penalty contract. The cases cover
ties, duplicate and reordered pairs, unmatched penalties, disconnected graphs, ignored
NaNs/out-of-range pairs, rerouting, an exact work allowance, a one-unit-short refusal, and
explicit refusal above the exact-component bound.

## Limits before production use

- The harness assembles a whole small problem for oracle convenience. A production implementation
  must run component by component and use fixed, preallocated scratch.
- Components handled by the greedy fallback have no certified exclusion regret.
- Production adds a tiny row-index tie-break surcharge. The prototype proves raw-cost cases where
  that perturbation does not move the selected result away from a raw optimum; production code
  needs an explicit numerical contract and fail-closed handling before relying on this property.
- Current association costs are squared Mahalanobis distances, not established calibrated
  negative log likelihoods across tracks. No probability, odds ratio, or safety threshold follows
  from the regret value. A threshold must be justified independently and cannot be tuned from
  hold-out truth labels.
- This prototype does not decide whether global uncertainty supplements the current local rule,
  changes forced-after-two behavior, or creates a persistent unresolved-identity state. Those are
  safety-policy and requirements decisions.
