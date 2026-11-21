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
| l2: EKF velocity measurement z = R(-w L) v, L = 1.0 s, with the w Jacobian column | 1.7 / 34.8 / 31.7 | 1.7 / 30.4 / 23.4 | KEPT (`VEL_LAG_S`, carried per measurement as `vel_lag_s`, OA-TRK-401) |
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

## Thinning (turn_diag --rescore --thin N, kept change)

Every Nth report of each aircraft goes to the tracker; grid, reference and ERAM unchanged. Gap: median over consecutive input reports of the sample aircraft. Cells: n, then OpenATCA median / p95 / RMS, cross-track m; ERAM on the same samples.

| Day | N | median input gap s | all n | all OA | all ERAM | cruise n | cruise OA | cruise ERAM | steady turn n | steady turn OA | steady turn ERAM |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 15 | 1 | 8.29 | 10755 | 1.7 / 34.8 / 31.7 | 7.5 / 26.9 / 25.4 | 1908 | 2.3 / 89.5 / 61.0 | 9.4 / 33.9 / 27.2 | 1193 | 5.8 / 61.4 / 28.0 | 12.0 / 30.8 / 19.0 |
| 15 | 2 | 17.44 | 10729 | 2.4 / 126.5 / 94.2 | 7.5 / 26.9 / 25.4 | 1904 | 3.6 / 292.2 / 156.8 | 9.4 / 33.9 / 27.2 | 1193 | 15.9 / 299.2 / 147.0 | 12.0 / 30.8 / 19.0 |
| 15 | 3 | 26.99 | 10704 | 3.4 / 321.1 / 181.3 | 7.5 / 26.9 / 25.4 | 1899 | 8.1 / 622.5 / 270.6 | 9.4 / 34.0 / 27.3 | 1192 | 49.6 / 693.8 / 285.4 | 12.0 / 30.8 / 19.0 |
| 19 | 1 | 8.61 | 16275 | 1.7 / 30.4 / 23.4 | 7.7 / 28.2 / 26.4 | 3966 | 1.9 / 63.9 / 34.1 | 8.5 / 33.9 / 31.8 | 1856 | 5.0 / 66.1 / 28.8 | 12.1 / 33.2 / 33.9 |
| 19 | 2 | 18.22 | 16237 | 2.5 / 116.5 / 91.7 | 7.7 / 28.2 / 26.4 | 3961 | 2.7 / 204.0 / 125.4 | 8.5 / 33.9 / 31.8 | 1855 | 13.5 / 289.9 / 145.5 | 12.1 / 33.2 / 33.9 |
| 19 | 3 | 28.43 | 16198 | 3.5 / 288.3 / 173.2 | 7.7 / 28.2 / 26.4 | 3953 | 3.8 / 414.7 / 207.1 | 8.5 / 33.9 / 31.8 | 1852 | 41.7 / 596.4 / 265.7 | 12.1 / 33.2 / 33.9 |

p95 against median input gap, per day (three points each). Linear fit p95 = a g + b, and power-law fit p95 = c g^k (least squares in the logs); residuals in m (linear) and as a fraction of the point (power).

| Day | Series | linear a | linear b | linear residuals (N=1,2,3) | linear at g=1 | power c | power k | power residuals (N=1,2,3) | power at g=1 | power at g=0.5 |
|---|---|---|---|---|---|---|---|---|---|---|
| 15 | all | 15.35 | -108.9 | 16.5, -32.3, 15.8 | -93.6 | 0.65 | 1.87 | +0.027, -0.069, +0.046 | 0.65 | 0.18 |
| 15 | cruise | 28.55 | -166.9 | 19.8, -38.7, 18.9 | -138.4 | 2.78 | 1.64 | +0.009, -0.025, +0.016 | 2.78 | 0.89 |
| 15 | steady turn | 33.87 | -243.8 | 24.4, -47.8, 23.4 | -209.9 | 0.79 | 2.06 | -0.013, +0.037, -0.023 | 0.79 | 0.19 |
| 19 | all | 13.05 | -95.3 | 13.4, -26.0, 12.6 | -82.3 | 0.53 | 1.87 | +0.017, -0.044, +0.028 | 0.53 | 0.14 |
| 19 | cruise | 17.73 | -99.0 | 10.3, -20.0, 9.7 | -81.3 | 2.20 | 1.56 | +0.003, -0.008, +0.005 | 2.20 | 0.74 |
| 19 | steady turn | 26.79 | -176.0 | 11.4, -22.2, 10.8 | -149.2 | 1.25 | 1.86 | -0.024, +0.066, -0.039 | 1.25 | 0.34 |

| Extrapolation to a gap of 0.5 to 1 s | Value |
|---|---|
| Points per series | 3 |
| Median input gaps spanned, s | 8.3 to 28.4 |
| Smallest gap measured, s | 8.3 |
| Linear fit p95 at g = 1 s, m (day 15 / day 19, all samples) | -93.6 / -82.3 |
| Power fit p95 at g = 1 s, m (day 15 / day 19, all samples) | 0.65 / 0.53 |
| Measured p95 on samples within 2 s of a report, m (all, day 15 / day 19) | 6.4 / 6.3 |
| Supported by the data | no |

## Report age (unthinned, turn_diag --rescore --thin 1)

Samples within 2 s of an ADS-B report of the track, and those more than 5 s from one. Cells: n, then median / p95 / RMS of cross-track error, m.

| Day | Age | Phase | n | OpenATCA | ERAM |
|---|---|---|---|---|---|
| 15 | within 2 s | all | 6453 | 1.2 / 6.4 / 4.7 | 7.2 / 25.6 / 24.9 |
| 15 | within 2 s | climb | 1321 | 1.4 / 7.8 / 5.4 | 8.5 / 37.0 / 41.8 |
| 15 | within 2 s | cruise | 1145 | 1.4 / 10.6 / 7.4 | 8.4 / 31.6 / 28.5 |
| 15 | within 2 s | descent | 3987 | 1.1 / 4.8 / 3.3 | 6.3 / 21.0 / 13.8 |
| 15 | within 2 s | steady turn | 628 | 3.0 / 23.0 / 10.7 | 11.9 / 29.2 / 17.1 |
| 15 | more than 5 s | all | 3121 | 4.5 / 103.1 / 57.9 | 8.2 / 29.5 / 28.0 |
| 15 | more than 5 s | climb | 631 | 3.9 / 91.0 / 37.6 | 8.2 / 55.3 / 50.8 |
| 15 | more than 5 s | cruise | 582 | 17.5 / 154.5 / 109.4 | 11.1 / 36.1 / 25.6 |
| 15 | more than 5 s | descent | 1908 | 3.9 / 75.5 / 37.0 | 7.5 / 22.4 / 15.3 |
| 15 | more than 5 s | steady turn | 334 | 22.1 / 97.8 / 48.2 | 12.1 / 30.9 / 20.4 |
| 19 | within 2 s | all | 9802 | 1.2 / 6.3 / 4.2 | 7.5 / 27.4 / 26.8 |
| 19 | within 2 s | climb | 1850 | 1.5 / 6.4 / 3.8 | 8.6 / 44.9 / 35.3 |
| 19 | within 2 s | cruise | 2425 | 1.3 / 8.5 / 5.7 | 8.2 / 33.7 / 33.2 |
| 19 | within 2 s | descent | 5525 | 1.1 / 4.9 / 3.4 | 7.0 / 21.6 / 19.4 |
| 19 | within 2 s | steady turn | 1056 | 2.6 / 20.6 / 9.8 | 12.3 / 34.8 / 38.5 |
| 19 | more than 5 s | all | 4617 | 4.7 / 94.2 / 43.0 | 7.9 / 28.3 / 25.4 |
| 19 | more than 5 s | climb | 856 | 4.2 / 80.1 / 34.4 | 8.0 / 42.1 / 39.9 |
| 19 | more than 5 s | cruise | 1046 | 11.6 / 150.3 / 65.2 | 9.2 / 34.5 / 28.6 |
| 19 | more than 5 s | descent | 2714 | 4.0 / 67.8 / 33.7 | 7.5 / 22.5 / 16.9 |
| 19 | more than 5 s | steady turn | 501 | 21.6 / 109.9 / 51.3 | 11.5 / 31.3 / 25.6 |

## Review round (per-measurement lag, lag-aware velocity gate)

| Check | Day 15 | Day 19 |
|---|---|---|
| Velocity refusals (`velocity_gated`), lag 0 on every measurement (= efd12cb behaviour, bit for bit) | 136 | 174 |
| Velocity refusals, ADS-B lag 1 s with the lag-aware gate (shipped) | 152 | 203 |
| Applied measurements, lag 0 / shipped | 2,167,094 / 2,168,489 | 2,660,786 / 2,663,496 |
| Position jump refusals (`jump_rejected`), lag 0 / shipped | 3,257 / 1,861 | 6,148 / 3,435 |

| Forged report 2 km north after a gap of: | 8 s | 9 s | 10 s | 11 s | 12 s | First accepted gap |
|---|---|---|---|---|---|---|
| Shipped default gate (A5, 5 m/s^2), lag 0 (efd12cb), refused | 1 | 1 | 0 | 0 | 0 | 10 s |
| Shipped default gate (A5, 5 m/s^2), lag 1 s, refused | 1 | 0 | 0 | 0 | 0 | 9 s |

| Test bound | Before | Now |
|---|---|---|
| IMM speed against the single constant-velocity filter, 1.5 deg/s turn at 1 s input, lagged reports | 0 % (no lag) | up to 17 % worse; bound -20 % |

| p002 --development after the review round (per-measurement lag, lag-aware gate) | Day 15 | Day 19 |
|---|---|---|
| M1 all, median / p95 / RMS | 1.70 / 34.67 / 32.42 | 1.74 / 30.34 / 23.36 |
| Difference from the table above | none (every printed value equal) | none (every printed value equal) |
| M5 ground-speed median, OpenATCA | 0.654 | 0.727 |
| M4t within 100 m, OpenATCA | 0.7125 | 0.7100 |
