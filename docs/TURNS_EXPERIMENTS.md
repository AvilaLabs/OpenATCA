# Turn handling experiments (lane-turns)

Fitted on the two development days only (2026-11-15 and 2026-11-19), with
`crates/oa-track/examples/turn_diag.rs` (screening, `--rescore`) and `p002 --development`.
Cross-track error in metres, common set: median / p95 / RMS. ERAM on the same sets: day 15
7.5 / 26.9 / 26.6, day 19 7.7 / 28.2 / 26.4.

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
worse in p95 and RMS on both days (47.4 to 50.9 and 40.9 to 47.1 m p95). The lag model may keep
the turn rate too long during rollout. Larger errors are concentrated at longer report ages.

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

## Gap diagnostic (turn_diag, kept change, no tracker change)

Dead reckoning (DR) = last raw report position plus its reported velocity times the age. Cross-track m, median / p95 / RMS, same samples in every column. OA = OpenATCA by the same cross-track definition (equal to p002's to rounding). Level / turning = reference turn rate under / over 0.5 deg/s at the sample.

| Day | Group | Age | n | DR | OA | ERAM |
|---|---|---|---|---|---|---|
| 15 | all | under 5 s | 7528 | 1.5 / 9.4 / 9.6 | 1.3 / 8.9 / 6.4 | 7.3 / 26.3 / 24.4 |
| 15 | all | 5-8 s | 1077 | 4.1 / 117.4 / 50.6 | 3.4 / 75.5 / 29.5 | 8.4 / 28.4 / 18.5 |
| 15 | all | over 8 s | 2014 | 5.7 / 142.7 / 88.7 | 5.2 / 125.7 / 68.7 | 8.1 / 30.6 / 32.1 |
| 15 | all | over 5 s | 3091 | 5.1 / 133.4 / 77.6 | 4.5 / 104.7 / 58.1 | 8.2 / 29.6 / 28.1 |
| 15 | cruise | under 5 s | 1261 | 1.7 / 36.7 / 15.3 | 1.5 / 15.3 / 10.1 | 8.6 / 31.3 / 28.5 |
| 15 | cruise | 5-8 s | 197 | 67.4 / 143.6 / 77.3 | 13.6 / 100.6 / 45.6 | 11.9 / 39.1 / 27.3 |
| 15 | cruise | over 8 s | 367 | 32.5 / 175.9 / 180.7 | 23.0 / 167.7 / 133.2 | 10.8 / 36.1 / 25.1 |
| 15 | cruise | over 5 s | 564 | 43.6 / 164.7 / 152.7 | 18.2 / 157.9 / 110.8 | 11.2 / 36.4 / 25.9 |
| 15 | level | under 5 s | 5183 | 1.3 / 4.2 / 2.5 | 1.0 / 4.2 / 2.8 | 6.4 / 21.1 / 24.9 |
| 15 | level | 5-8 s | 683 | 2.4 / 20.8 / 12.6 | 2.2 / 18.5 / 10.5 | 6.9 / 19.8 / 16.9 |
| 15 | level | over 8 s | 908 | 4.1 / 78.2 / 32.5 | 3.8 / 64.9 / 36.5 | 7.2 / 21.5 / 19.8 |
| 15 | level | over 5 s | 1591 | 3.3 / 45.5 / 25.9 | 3.0 / 41.9 / 28.4 | 7.1 / 20.4 / 18.6 |
| 15 | turning | under 5 s | 2345 | 2.3 / 44.1 / 16.8 | 2.3 / 20.1 / 10.7 | 9.7 / 32.1 / 23.1 |
| 15 | turning | 5-8 s | 394 | 74.1 / 141.9 / 82.1 | 17.0 / 100.6 / 46.7 | 11.4 / 34.2 / 21.0 |
| 15 | turning | over 8 s | 1106 | 8.3 / 155.6 / 116.0 | 6.7 / 147.4 / 86.6 | 8.9 / 34.7 / 39.5 |
| 15 | turning | over 5 s | 1500 | 12.9 / 150.2 / 108.1 | 8.2 / 137.5 / 78.1 | 9.7 / 34.3 / 35.6 |
| 19 | all | under 5 s | 11238 | 1.6 / 8.5 / 8.8 | 1.3 / 8.7 / 5.7 | 7.6 / 28.2 / 27.1 |
| 19 | all | 5-8 s | 1592 | 4.6 / 106.0 / 46.8 | 4.0 / 70.9 / 27.5 | 8.5 / 28.2 / 28.3 |
| 19 | all | over 8 s | 2900 | 5.9 / 147.2 / 50.8 | 5.0 / 115.5 / 50.1 | 7.7 / 28.3 / 24.1 |
| 19 | all | over 5 s | 4492 | 5.5 / 127.9 / 49.4 | 4.6 / 95.7 / 43.5 | 7.9 / 28.3 / 25.7 |
| 19 | cruise | under 5 s | 2650 | 1.7 / 14.1 / 12.5 | 1.4 / 11.9 / 7.9 | 8.2 / 34.0 / 34.1 |
| 19 | cruise | 5-8 s | 368 | 41.3 / 121.0 / 64.2 | 9.1 / 80.4 / 35.5 | 11.1 / 32.2 / 23.6 |
| 19 | cruise | over 8 s | 600 | 15.2 / 177.5 / 81.0 | 13.5 / 170.1 / 81.1 | 8.5 / 36.1 / 32.8 |
| 19 | cruise | over 5 s | 968 | 23.3 / 169.8 / 75.1 | 11.8 / 153.8 / 67.5 | 9.5 / 35.8 / 29.6 |
| 19 | level | under 5 s | 7657 | 1.4 / 4.3 / 2.6 | 1.1 / 4.4 / 2.7 | 6.9 / 21.3 / 23.5 |
| 19 | level | 5-8 s | 971 | 2.7 / 26.2 / 12.6 | 2.6 / 21.9 / 10.5 | 7.2 / 19.2 / 27.8 |
| 19 | level | over 8 s | 1194 | 4.3 / 86.7 / 34.4 | 3.8 / 67.9 / 37.1 | 7.0 / 22.3 / 22.8 |
| 19 | level | over 5 s | 2165 | 3.6 / 48.1 / 26.9 | 3.2 / 38.5 / 28.4 | 7.1 / 20.5 / 25.1 |
| 19 | turning | under 5 s | 3581 | 2.2 / 34.8 / 15.0 | 2.2 / 17.5 / 9.3 | 9.7 / 34.8 / 33.5 |
| 19 | turning | 5-8 s | 621 | 63.3 / 126.8 / 73.2 | 14.6 / 92.1 / 42.1 | 11.7 / 35.6 / 29.2 |
| 19 | turning | over 8 s | 1706 | 7.5 / 159.5 / 59.6 | 6.3 / 140.2 / 57.5 | 8.3 / 30.6 / 25.0 |
| 19 | turning | over 5 s | 2327 | 10.6 / 152.2 / 63.5 | 7.7 / 124.0 / 53.8 | 8.9 / 32.2 / 26.2 |

Level samples more than 5 s from a report: what the track carries.

| Day | Group | n | track heading error at last report, median / p95 deg (n) | raw reported heading error, median / p95 deg | track turn rate abs, median / p95 deg/s | reference rate abs, median / p95 | mu_turn median / p95 |
|---|---|---|---|---|---|---|---|
| 15 | all level samples | 1591 | 0.10 / 0.55 (1484) | 0.10 / 0.60 | 0.00 / 0.31 | 0.02 / 0.34 | 0.086 / 0.661, |
| 15 | OpenATCA error over 50 m | 66 | 0.69 / 1.96 (49) | 0.52 / 1.39 | 0.08 / 1.98 | 0.33 / 0.49 | 0.318 / 0.832, |
| 15 | OpenATCA error 50 m or less | 1525 | 0.10 / 0.44 (1435) | 0.10 / 0.48 | 0.00 / 0.09 | 0.02 / 0.26 | 0.085 / 0.284, |
| 19 | all level samples | 2162 | 0.09 / 0.55 (1982) | 0.09 / 0.54 | 0.00 / 0.23 | 0.02 / 0.36 | 0.087 / 0.548, |
| 19 | OpenATCA error over 50 m | 91 | 0.56 / 3.51 (66) | 0.35 / 1.48 | 0.01 / 1.89 | 0.35 / 0.48 | 0.215 / 0.822, |
| 19 | OpenATCA error 50 m or less | 2071 | 0.09 / 0.47 (1916) | 0.09 / 0.50 | 0.00 / 0.05 | 0.02 / 0.27 | 0.086 / 0.236, |

Level samples by age of the last report: OpenATCA, dead reckoning, ERAM cross-track median / p95 m.

| Day | Age s | n | mean age | OA | DR | ERAM |
|---|---|---|---|---|---|---|
| 15 | 1-2 | 290 | 1.45 | 0.9 / 3.7 | 1.3 / 3.5 | 6.3 / 20.6 |
| 15 | 2-3 | 287 | 2.52 | 1.4 / 5.9 | 1.4 / 5.4 | 7.1 / 27.5 |
| 15 | 3-4 | 284 | 3.48 | 1.7 / 8.9 | 1.7 / 6.9 | 7.8 / 24.4 |
| 15 | 4-6 | 454 | 4.97 | 2.0 / 12.0 | 1.9 / 14.0 | 6.4 / 20.2 |
| 15 | 6-8 | 472 | 6.96 | 2.4 / 21.1 | 2.5 / 30.4 | 7.4 / 19.8 |
| 15 | 8-12 | 701 | 9.66 | 3.4 / 38.8 | 3.8 / 43.6 | 7.1 / 20.2 |
| 15 | 12-20 | 180 | 13.93 | 5.2 / 131.1 | 5.6 / 141.0 | 8.0 / 24.0 |
| 15 | 20-inf | 27 | 24.11 | 7.9 / 124.9 | 13.0 / 89.4 | 8.2 / 17.6 |
| 15 | log-log slope from 3 s | | | median 0.82, p95 1.91 | p95 2.09 | |
| 19 | 1-2 | 587 | 1.45 | 0.9 / 3.1 | 1.2 / 3.6 | 6.9 / 21.7 |
| 19 | 2-3 | 507 | 2.48 | 1.3 / 6.3 | 1.6 / 5.3 | 7.3 / 37.6 |
| 19 | 3-4 | 426 | 3.49 | 1.7 / 9.0 | 1.9 / 8.0 | 7.7 / 31.3 |
| 19 | 4-6 | 726 | 4.97 | 2.1 / 15.3 | 2.2 / 16.5 | 7.4 / 20.0 |
| 19 | 6-8 | 620 | 6.97 | 2.9 / 22.3 | 3.1 / 27.8 | 7.0 / 19.2 |
| 19 | 8-12 | 826 | 9.79 | 3.4 / 46.1 | 3.7 / 65.0 | 7.1 / 20.2 |
| 19 | 12-20 | 311 | 14.06 | 5.0 / 113.5 | 6.1 / 139.1 | 7.0 / 32.5 |
| 19 | 20-inf | 57 | 23.68 | 8.8 / 150.5 | 9.6 / 106.5 | 6.8 / 42.6 |
| 19 | log-log slope from 3 s | | | median 0.85, p95 1.58 | p95 1.51 | |

Day 19, 41,000 ft and above band (n = 149 common samples, 43 flights): nearest-rank p95 is the 8th largest error.

| Quantity | Before | After |
|---|---|---|
| Samples over 70 m | 8 (8 flights) | 8 (8 flights) |
| Largest 7 errors, m | 205.2, 179.2, 176.2, 161.1, 133.5, 129.5, 121.6 | 205.4, 180.1, 176.3, 161.3, 133.5, 129.5, 121.6 |
| 8th largest (the p95), m | 70.3 | 105.8 |
| Flight holding the 8th largest (5 samples in the band) | median 1.4 m, one sample at 70.3 | the same sample at 105.8 |
| Top five flights by sample count (25, 12, 10, 10, 9) | max error 5.7, 7.3, 2.7, 9.5, 2.6 m | unchanged to 0.1 m |

ERAM age: samples.csv has no ERAM input-age or coast column; `eram_row_dt_ms` is 0 on every row (the grid is on ERAM's row times).

How to read the spacing. An ERAM row is ERAM's estimate at the instant it was published, made from
whatever surveillance ERAM had received by then. The spacing is how often SFDPS publishes a row for the
aircraft, not how often ERAM receives data, and the archive does not say what ERAM received. The samples
are taken at ERAM's row times, so ERAM is always scored at the moment of its own estimate. OpenATCA is
scored at the same moment, from input that may be up to one archive gap old (8 to 9 s median). ERAM's
error being flat against the spacing fits that, but does not show ERAM's input rate.

For level samples, dead reckoning (DR) has lower p95 cross-track error in most of the 1-to-4-second
age rows, and in the over-20-second groups on both days (day 15: 89.4 m DR vs 124.9 m OA, n=27;
day 19: 106.5 m vs 150.5 m, n=57). In the aggregate over-5-second group, DR also has lower RMS
on both days (25.9 vs 28.4 m and 26.9 vs 28.4 m); this does not hold in the 5-to-8-second
subgroup, where DR RMS is 12.6 m on both days versus OA's 10.5 m. In that subgroup, OA's p95 is
18.5 m on day 15 and 21.9 m on day 19, against ERAM's 19.8 m and 19.2 m.

The tail includes a heading-error problem. Among level samples more than 5 seconds after a report,
OpenATCA's median heading error at the last report is about 0.1°. For the subset with more than
50 m of cross-track error (n=66 and 91), median heading error rises to 0.69° and 0.56°, and p95
reaches 1.96° (n=49) and 3.51° (n=66). Heading error and residual turn estimates warrant
investigation. The groups in these tables use reference turn rate at the sample time; whether a turn began during the gap was not
measured, because that requires comparing reference heading over the gap. The observed ERAM
publication intervals are row spacing, not its surveillance input rate, which the archive does
not reveal.

| Day | ERAM row spacing (previous grid row of the aircraft) | n | ERAM cross-track median / p95 / RMS m |
|---|---|---|---|
| 15 | under 5 s | 841 | 8.5 / 38.1 / 34.8 |
| 15 | 5-10 s | 1311 | 7.2 / 25.2 / 19.8 |
| 15 | 10-20 s | 5861 | 7.2 / 23.7 / 25.8 |
| 15 | 20 s or more | 2523 | 7.9 / 32.9 / 28.7 |
| 19 | under 5 s | 1244 | 8.3 / 55.0 / 37.7 |
| 19 | 5-10 s | 1430 | 7.5 / 29.4 / 40.5 |
| 19 | 10-20 s | 6760 | 7.4 / 25.3 / 21.8 |
| 19 | 20 s or more | 6327 | 7.7 / 28.5 / 23.5 |

## Straight-line forecast after 12 seconds

The development sweep on 15 November compared the current forecast with four turn-rate decay
settings and three ages at which to switch to a straight-line forecast. The 12-second switch
advanced to the single 19 November forward check. Both runs used the p002 common set for the table below.

| Metric | 15 Nov current | 15 Nov 12-second switch | 19 Nov current | 19 Nov 12-second switch |
|---|---|---|---|---|
| M1 cross-track p95 (m) | 32.473 | 32.143 | 32.561 | 32.327 |
| M1 cross-track RMS (m) | 26.140 | 25.358 | 23.538 | 21.246 |
| M2 raw along-track RMS (m) | 68.430 | 69.113 | 15.852 | 16.424 |
| M2t rounding-tolerant along-track RMS (m) | 66.333 | 67.022 | 7.626 | 8.618 |

The switch improved cross-track p95 and RMS on both days and increased along-track RMS on both
days. The acceptance rule requires improvement on both days without a metric getting worse.
The tracker keeps its current forecast for the hold-out candidate.

