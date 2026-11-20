# oa-runway

Runway geometry per airport, and the test of whether a target occupies a runway or its approach
and departure areas. This is build step 1 of `docs/RUNWAY_SAFETY_DESIGN.md`. It is a core crate
(`no_std`, no `unsafe`, no I/O), classes B1 and B7 of `docs/RULESET.md`. The shell reads NASR and
CIFP and passes the plain values in; this crate reads nothing.

- `plane` is the airport's local east-north plane: a disc of 20 km about the reference point, for
  an origin up to 80 degrees of latitude. A point's distance from the origin is within 1.5 m of
  the WGS-84 geodesic on every case tested (worst 1.05 m), and the distance between two points is
  within 3 m (worst 1.90 m over 2,800 pairs). Those bounds are measured, not proved.
- `runway` builds a `Runway` from NASR runway and runway-end values. Where NASR has no displaced
  threshold coordinate, it uses the CIFP threshold (OA-ASP-067). A runway whose values are missing,
  or disagree with each other by more than the stated tolerances, is refused, and the reason is
  counted under one of 16 names. Nothing is guessed: when NASR and CIFP place a threshold more than
  30 m apart, the runway is refused rather than one source being chosen.
- `occupy` provides `occupies`. It reports which of five regions a target touches: the rectangle,
  the two approach areas and the two departure areas. The target's position is widened by its
  95 % radius plus the plane error bound. An unknown position or radius, or a runway with any
  non-finite number, occupies everything. A NaN anywhere reads as touching, never as clear.

The approach and departure dimensions are development values, not FAA values.

## What is proved and what is tested

Six Kani proofs are registered in `tools/rust_proofs.json`:

| Proof | Shows | Time |
|---|---|---|
| `non_finite_input_occupies` | an unknown position or radius occupies every region | 6 s |
| `non_finite_runway_occupies` | a runway with a non-finite number occupies every region | 6 s |
| `point_in_rectangle_occupies` | a point inside the rectangle, in runway axes, touches it at every radius | 5 s |
| `occupies_never_panics` | no panic for any input; the runway-axis coordinates are finite | 43 s |
| `local_plane_is_bounded` | the plane gives a finite position within 20 km on each axis, or none | 16 s |
| `occupancy_is_monotone_in_r95` | in the one-region test, a larger radius never removes a region | 341 s |

The times are single runs on the development laptop and include compilation.

Three things are tested, not proved:

- monotonicity through the whole of `occupies` (a proof with a symbolic runway did not finish);
- the disc limit of the plane;
- the rotation from the plane into the runway axes.

The tests run on real runways: KMDW 04L/22R, which is displaced at both ends, KAUS 18L/36R and
KSFO 10L/28R, all from the NASR 2026-10-29 values.
