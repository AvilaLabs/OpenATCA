# Surveillance tracker/fusion API proposal

**Tier:** L3 Experimental; outside the proposed 0.1 release. API names below are review proposals only. Existing function names are identified as current Rust source exports, not frozen or approved public names.

## Current source surface

`crates/oa-track/src/lib.rs` exports the tracker functions/types (`tracker_new`, `tracker_update`, `tracker_advance`, `tracker_flush`, `tracker_predict[_checked]`, `tracker_forecast`, `Tracker`, `TrackerConfig`, `Measurement`, `TrackOutput`, `OutRow`, `TrackerStats`) and separate ADS-B, fusion, radar, score and diagnostic modules. A tracker is created with a caller `TrackerConfig`; `tracker_update` and time-advance functions mutate its bounded state and append to caller-owned output. `tracker_update` does not return a per-measurement `Result`; refusal/velocity-gating and output loss are reflected in counters/logs. Configuration construction can return `TrackError::BadConfig`.

`Measurement` fields are explicitly unit-tagged in source: time of applicability is Unix nanoseconds (not receive time); position is integer E7 degrees; local east/north velocity is m/s; position covariance is m² and velocity covariance is (m/s)²; altitude is feet; vertical rate is feet/minute; altitude quantization is feet. Address/identity and flags have sentinel/encoding rules. A public constructor should preserve those units and require explicit validity for optional fields.

Current nominal defaults are **not** an acceptance target: 4,096 tracks, output capacity 65,536, log capacity 1,048,576, 1 s tick, 10 s out-of-order window, and 60 s drop age. `TrackerConfig` permits a larger per-track bound up to `1<<24`, table capacities up to `1<<24`, and tick interval from 100 ms to a large arithmetic safety ceiling. The configuration includes initialization, coast/drop, filter, velocity gating, move confirmation and clock-step policies; changing them changes semantics. A fusion API has separate capacities and budgets and should not be conflated with the single-sensor tracker.

## Proposed public shape

For review, consider a `Tracker::with_config(TrackerConfig)` plus `push(Measurement)`, `advance(TimeNs)`, `drain(&mut [TrackUpdate])`, `stats()` pattern. It is only a naming/design sketch: today the crate already exposes free functions and a caller-provided `TrackOutput`; no such methods are implied to exist. Preserve the separate applicability and receive times and report per-reason refused input and output loss. Expose initialization/coast/confirmation behavior as named config fields with defaults explicitly labelled development choices.

Radar fusion should be a later separately versioned API, not inferred from the tracker-facing `Measurement`. Current fusion source has radar association, ADS-B/radar identity paths and diagnostics, while its crate dependency graph still includes benchmark, simulator, sealed-data and other internal crates. A public API should accept an abstract measurement stream and explicit adaptation/configuration, without pulling those internal dependencies or benchmark/recording formats into a stable package.

## Units, editions, errors, and determinism

This is an algorithm and sensor-data API rather than a published wire-format edition. Source standard inputs include ADS-B/readsb-derived measurements and Mode C radar values through in-repo adapters; do not claim a fully native CAT048 public interface from the tracker itself. Numeric ranges/refusals are defined in `types.rs` and `params.rs`. Errors distinguish bad configuration and checked-prediction failures; most step-level refusals are counted rather than raised as exceptions. Public docs must define whether a counter overflow saturates, whether logs drop rows, and how a caller distinguishes no update from missing input.

The core is designed as a pure function of ordered measurements and explicit time, with fixed capacities and reproducible output. This is not a cross-platform bit-identity result: no complete x86-64/aarch64 evidence bundle was established here. It also does not make live wall-clock latency, radar accuracy or missed-track risk claims. Source tests, synthetic scorers and development clips are not public truth references.

## Current tier blockers

`oa-track` currently depends on `oa-bench`, `oa-sensorsim`, `oa-sealed`, and other workspace crates, and is not split into a reusable dependency-clean public package. A public package cut, independent sensor adapters, exact capacity refusal contract, complete units/errors docs, published synthetic/licensed vectors, 24-hour fuzzing, applicable Kani proofs, mutation review, no-panic review, cross-architecture bit checks, and Rust/Python examples remain before any Stable designation. Track, fusion, scorer, and replay interfaces require separate tier entries.
