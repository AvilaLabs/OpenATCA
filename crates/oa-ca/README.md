# oa-ca

Conflict alert: the first of the M5 safety nets (`docs/M5_DESIGN.md` §2). A core crate (`no_std`,
no `unsafe`), class B1 and B7 of `RULESET.md`. The command-line tool that runs it on a recorded
clip is `tools/oa-ca`.

## Use

Create the state once with `ca_new(&CaConfig)`. Each cycle, pass the tracker's live tracks to
`ca_cycle`. It appends started and ended alerts to an `AlertLog` and altitude losses to an
`EventLog`, and it publishes the net's state to the `oa-safety` table. If the cycle's input is
refused, call `ca_publish_refused` so the table shows the net as unavailable rather than stale.
Call `ca_finish` at the end to close the alerts still open.

Nothing has a default. The look-ahead, the minima of every volume, the hysteresis, the
capacities and the budgets all come from the caller in a `CaConfig`, and `ca_check` refuses a
configuration with a value out of range, naming the value. The one constant is `COAST_V_ALLOWANCE_FT` (100 ft), this
crate's own widening of the vertical minimum for a poor track. No ERAM, STARS or 7110.65 value is
in the crate; the tests use made-up values.

## How a cycle works

1. **Candidates.** A grid of horizontal cells finds every pair that could come within the
   largest minimum in the look-ahead, given each track's speed and 95 % radius. The test is a
   superset: it may pass pairs that cannot conflict, never miss one that can.
2. **Exact test.** Each candidate is predicted in a straight line. The horizontal loss interval
   is the solution of a quadratic, the vertical one of a linear inequality, both with strict
   edges and exact handling of zero relative speed (`solve`). A pair is in conflict when the
   two intervals overlap inside the look-ahead.
3. **Minima.** The volume a pair falls in gives its minima, with a required default volume for a
   pair outside every listed one. A poor track widens them: both 95 % radii are added to the
   horizontal minimum, and `COAST_V_ALLOWANCE_FT` to the vertical for each coasting or uncertain
   track. A pair with an altitude unknown on either track is tested horizontally only and
   carries `AF_ALT_UNKNOWN`.
4. **Alerts.** One `Alert` per pair, from its first cycle in conflict to its last, held open for
   the hysteresis.
5. **State.** The net publishes `running` when every candidate was evaluated, `degraded` with
   the count when a budget ran out or input was missing, and `unavailable` when the cycle was
   refused.

## Bounded work

The grid scan has a budget of band steps, cell probes and tracks visited, and the exact test a
budget of pairs per cycle. Over budget, pairs are evaluated in order of smallest current
horizontal separation, ties broken by address. The rest are counted, and the net reports
degraded. An open alert whose pair was not reached stays open and is marked, never ended by the
gap.

## Stated limits

- Prediction is a straight line from the cycle's position and velocity. A turning aircraft is not
  yet predicted; that is build order step 6.
- The grid uses a local flat approximation, with the east-west scale taken at the first track's
  latitude. The superset holds up to a latitude set by the cycle's largest radius (about
  `atan(0.02 / dphi)`, `dphi` being that radius over 6,356,752 m). Rows above it are still used,
  counted in `CycleStats::rows_high_lat`, and the net reports degraded.

## Verification

Requirements OA-CA-001 to OA-CA-030 in `REQUIREMENTS.md`, each with its tests. The solvers have
Kani harnesses (`solve`), registered with the proof suite (`tools/rust_proof_suite.py`). Their
results go into `VERIFICATION.md` when they have been run.
