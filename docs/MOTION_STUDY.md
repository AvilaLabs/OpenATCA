# Motion model development study v1

This preregistration freezes a small development experiment for choosing a single `H_Q_CV`
setting. It changes exactly one parameter across three values: 1, 0.1, and 0.01. Training uses
seeds 1–3 on the six frozen v1 scenarios, with 20 aircraft for 300 seconds. The winner is the
lowest pooled nominal post-initialization position RMS among settings that pass every predeclared
gate. Seeds 4–6 are observed once after that selection as a development check; they cannot trigger
retuning or selection of another setting.

The study also includes a diagnostic onset/exit maneuver: 150 m/s straight flight, a 2 degree per
second turn from 60 to 120 seconds, then straight flight at the final 120 degree heading. Analysis
uses fixed 0–30 second response windows around start, turn entry, and turn exit. Turn lag is the
first crossing of 0.5 degree per second sustained for three samples. A crossing not observed by the
window end is right-censored and remains visible.

Every candidate must retain zero lost, pending, or refused inputs; achieve full post-initialization
usable coverage with the same usable truth-grid keys as the frozen default; and stay within the
predeclared 1.02 scenario-level regression limits. Dropout stays diagnostic-only and uses its
paired p99 and coverage comparison as a descriptive gate. Per-scenario covariance checks use
candidate-local ENU errors and the emitted covariance. The study reports undercoverage and
overcoverage; it makes no NIS claim because the public pre-gate API does not expose NIS.

The study file `validation/motion-study-v1.json` is pinned by its SHA-256 in
`tools/motion_study.py`; a changed file stops the study before any run.

This is a developmental model check. It cannot establish physical accuracy, professional tracker
performance, or independent qualification. It does not use qualification or sealed fault seeds,
and results cannot change the thresholds or settings after the registered single check candidate.

Before winner selection, each candidate also must pass the existing
`oa-track core accelerating_flight_is_tracked` regression guard, with its exit status and log hash
recorded in the build evidence. After selection, the full `oa-track core` suite runs for the
selected candidate; it cannot change which setting was selected.

## Preregistration clarification before any output review

The onset/exit maneuver mirrors the turn by numeric flight-ID parity: even IDs turn left
(`s=+1`), odd IDs turn right (`s=-1`), starting from the frozen per-flight v2 origin. Turn rate is
`s * 2 degrees/second`; final heading is `s * 120 degrees`. Onset and exit lag are scored
separately by direction and transition in the fixed 30-second windows. A transition must cross
both the `mu_ct` threshold and the absolute turn-rate threshold for three consecutive samples; a
censored crossing fails the response gate. Candidate lag may exceed the frozen default by at most
one second. Turn-phase position RMS also has a 1.02 regression bound.

Posterior containment is an undercoverage guard. The paired bootstrap samples the three seed
blocks with replacement, then whole flights within each sampled seed; all rows for a selected
flight remain together. It uses seed 173, 2,000 replicates, at least 20 unique `(seed, flight)`
clusters and 200 scored rows per group. The one-sided 95% lower bound is the nearest-rank 5th
percentile. It must meet the 50/90/95% floors for each nominal scenario and every onset-exit
direction/window group. Upper bounds and empirical overcoverage are reported. The single
development check runs the selected setting and frozen-default `H_Q_CV=1` control on seeds 4–6
for all six v1 scenarios and onset-exit; it does not evaluate the other experimental settings.
