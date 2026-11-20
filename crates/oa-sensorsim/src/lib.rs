#![forbid(unsafe_code)]
//! oa-sensorsim: simulated radar plots and injected ADS-B spoofs, generated from `oa-truth`
//! (docs/M3_DESIGN.md sections 1, 2 and 4). Core (pure): every function is a function of its
//! arguments, the same seed and inputs give the same bytes; the `oa-sensorsim` binary is the
//! shell (it reads a clip and writes files).
//!
//! * [`profile`]: named error profiles, each constant citing its row of docs/BENCHMARKS.md 3.1
//!   "Simulated sensor inputs". No other number is used. What the table does not give (range
//!   and azimuth bias, split geometry, the earth-radius factor) is a field of
//!   [`radar::RadarSite`] with no default.
//! * [`radar`]: the rotating-beam simulation: crossing time solved against the moving target,
//!   range and azimuth noise and bias, coverage and 4/3-earth line of sight, altitude
//!   quantisation, Mode A and Mode S identity, detection probability, false and split plots.
//! * [`plots`]: the plot table (struct-of-arrays, with scoring labels) and CAT048 output through
//!   `oa-asterix`.
//! * [`spoof`]: ghost, drift, replay and duplicate-address transforms with per-row labels and a
//!   confusion matrix.
//! * [`adsb_error`]: per-aircraft position bias and latency applied to reports regenerated from an
//!   exact truth, with per-row labels.
//! * [`prng`]: xoshiro256** seeded by SplitMix64, with the published test vectors.
//! * [`geom`]: WGS-84 radar frame and line of sight.

pub mod adsb_error;
pub mod clip;
pub mod error;
pub mod geom;
pub mod plots;
pub mod prng;
pub mod profile;
pub mod radar;
pub mod spoof;

pub use error::SimError;
