# oa-runway

Runway geometry per airport and the occupancy test: build step 1 of `docs/RUNWAY_SAFETY_DESIGN.md`.
A core crate (`no_std`, no `unsafe`, no I/O), class B1 and B7 of `docs/RULESET.md`.

- `plane`: the airport's local east-north plane (extent 20 km, error bound against the WGS-84
  geodesic stated and tested).
- `runway`: a `Runway` built from plain NASR runway and runway-end values and, where NASR has no
  threshold coordinate, the CIFP threshold (OA-ASP-067). A runway whose thresholds are missing or
  disagree with its published length, alignment or displacement is refused and the reason counted;
  nothing is guessed. Tolerances and the development-value approach and departure dimensions are
  named constants beside their basis.
- `occupy`: `occupies`, which says which of the rectangle, approach areas and departure areas a
  target, widened by its 95 % radius, touches. A larger radius only adds occupancy, and an unknown
  position or radius occupies everything. Five Kani proofs are registered in `tools/rust_proofs.json`.

The shell reads NASR and CIFP and passes the plain values in; this crate reads nothing.
