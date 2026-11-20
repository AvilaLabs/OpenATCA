# Turn handling experiments (lane-turns)

Fitted on the two development days only (2026-11-15 and 2026-11-19), with
`crates/oa-track/examples/turn_diag.rs` (screening, `--rescore`) and `p002 --development`.
Cross-track error in metres, common set: median / p95 / RMS. ERAM on the same sets: day 15
7.5 / 26.9 / 26.6, day 19 7.7 / 28.2 / 26.4. The lead will rewrite this file.

Finding: the reported ADS-B velocity lags the position-derived reference by about 1 s, and the
tracker trusted it tightly, so in a turn it under-turned; the error grows with the time since
the last report (day 19 p95 by report age: 12.0 m under 1 s, 14.2, 58.1, 78.9, 115.3 m at 10 s or
more).

| Experiment | Day 15 | Day 19 | Verdict |
|---|---|---|---|
| baseline | 1.9 / 45.4 / 33.1 | 1.9 / 41.2 / 22.8 | |
| a1: turn rate from consecutive reported velocities (candidate a) | p95 45.9, RMS 33.2 | 41.2 / 22.8 | no effect, reverted |
| l1: plug-in lag correction (rotate the report by the predicted turn rate) | 1.7 / 35.6 / 33.0 | 1.7 / 29.2 / 24.2 | outliers, replaced |
| l2: EKF velocity measurement z = R(-w L) v, L = 1.0 s, with the w Jacobian column | 1.7 / 34.8 / 31.7 | 1.7 / 30.4 / 23.4 | KEPT (`VEL_LAG_S`) |
| b9, b4: inflate velocity R when the normalised innovation exceeds 9 or 4 (candidate b) | identical to l2 | identical to l2 | no effect, reverted |
| q10: `H_Q_OMEGA` 3e-5 (on l2) | p95 38.1, RMS 39.5 | p95 31.5, RMS 31.4 | rejected |
| q03: `H_Q_OMEGA` 1e-6 | p95 34.9, RMS 30.8 | p95 30.3, RMS 22.4 | marginal, not kept |
| t60: `H_TAU_CT_S` 60 | p95 34.2, RMS 29.7 | p95 30.7, RMS 24.8 | rejected |
| t30: `H_TAU_CT_S` 30 | p95 35.1, RMS 33.3 | p95 30.3, RMS 22.4 | rejected |
| f06: `CT_OMEGA_STD_FLOOR` 0.06 | p95 37.4, RMS 31.6 | not read | rejected |
| v15: `VEL_LAG_S` 1.5 | p95 35.1, RMS 31.9 | not read | rejected |

## Final p002 --development (kept change)

| | Day 15 before | Day 15 after | Day 19 before | Day 19 after |
|---|---|---|---|---|
| M1 all (median / p95 / RMS) | 1.86 / 45.3 / 33.9 | 1.70 / 34.7 / 32.4 | 1.92 / 41.1 / 22.8 | 1.74 / 30.3 / 23.4 |
| climb | 2.23 / 40.6 / 21.1 | 1.96 / 23.7 / 19.9 | 2.23 / 31.6 / 18.8 | 1.91 / 22.6 / 18.0 |
| cruise | 2.97 / 91.7 / 65.8 | 2.26 / 89.5 / 60.9 | 2.20 / 71.0 / 33.0 | 1.91 / 63.9 / 34.1 |
| descent | 1.58 / 27.2 / 19.5 | 1.51 / 20.6 / 20.6 | 1.74 / 28.6 / 18.1 | 1.62 / 19.2 / 18.9 |
| M5 ground-speed median | 0.655 | 0.654 | 0.727 | 0.727 |
| M4t within 100 m | 0.7128 | 0.7125 | 0.7099 | 0.7100 |

Not reached: OpenATCA p95 above ERAM's remains in cruise (day 15 89.5 vs 34.1, day 19 63.9 vs
33.9), in the 18000 ft and above bands on both days, and the 41000 ft and above band on day 19
(n = 149) got worse, 70.3 to 105.8 m p95. Rollout (the first seconds after a turn) is slightly
worse in p95 and RMS on both days (47.4 to 50.9 and 40.9 to 47.1 m p95): the lag model keeps
the turn rate a little too long. The remaining tail is mostly unobserved manoeuvres inside
long report gaps (the tracker input is thinned, median gap about 7 s).
