# Proposed accreditation record: sensor simulator

**Status: DRAFT / NOT ACCREDITED / NOT APPROVED FOR system verification.** This record does not
validate the simulator's model against operational sensors.

## Identification and intended use

- **Capability:** `oa-sensorsim`, implemented in `crates/oa-sensorsim`.
- **Proposed use:** deterministic, labeled synthetic radar and ADS-B scenarios for component tests,
  fault injection, and sensitivity studies.
- **Not intended to establish:** operational radar detection probability, surveillance accuracy,
  field coverage, sensor registration, or fidelity outside the selected model.
- **Configuration:** bind source/build, truth source and coverage, time window, site, scan settings,
  sensor profile, explicitly chosen error and split parameters, random seed, platform, and all
  output limits for each use.

## Model and assumptions

The radar model queries a truth trajectory, solves scan crossing times, and applies configured
detection, range and azimuth noise, coverage, altitude quantization, false plots, and split plots.
Pressure altitude is treated as ellipsoid height; a truth record without altitude uses zero height
for the geometry model. Named profiles contain literature-derived values and assumptions; the code
requires explicit site biases and split geometry because their values are not established. Some
model outputs use platform math-library functions, so bit-level repeatability can vary by runtime.
The table has a finite plot limit, which does not alone establish end-to-end memory or runtime
bounds.

## Evidence and gaps

The source contains synthetic tests for random generation, geometry, quantization, and repeatability.
They support implementation checks only. No approved execution package or independent comparison
against measured sensor truth is identified for this record. Unsourced bias and timing distributions,
clutter, site adaptation, propagation, receiver processing, and correlated errors remain material
fidelity gaps. A deterministic synthetic result does not demonstrate physical accuracy.

## Required entrance and exit evidence

The approved TEMP must define use-specific fidelity classes and acceptance criteria. Freeze and
independently review the simulator, truth source, site, profiles, seed, toolchain, and output schema.
Validation requires representative measured data and documented uncertainty for range, azimuth,
time, detection, false/split reports, altitude, and coverage. Preserve refusals and all sensitivity
cases; do not tune parameters to obtain a desired tracker outcome. Until independent model
validation and formal approval, results must be labeled synthetic and cannot support claims about
operational radar performance. Model owner, independent reviewer, and approval authority remain
unassigned.
