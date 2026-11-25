#![forbid(unsafe_code)]
//! oa-track: the reusable surveillance tracker of OpenATCA (`docs/M3_DESIGN.md` section 3).
//! Core (pure, B1): filters, association, fusion and the track table depend on measurements and
//! times supplied by the caller; [`adsb`] adapts `oa-adsb` reports. Offline scoring and simulation
//! commands live in the separate `oa-track-tools` shell package.
//!
//! * [`Measurement`] is the interface every sensor plugs into: time of applicability, sensor id,
//!   position with covariance, optional velocity with covariance, optional barometric or
//!   geometric altitude with quantisation, optional address and identity.
//! * [`tracker_update`], [`tracker_advance`] and [`tracker_flush`] are the whole interface. See
//!   `tracker.rs` for time ordering, initiation, coasting, dropping and the fixed-rate output.
//! * `hfilter.rs` and `vfilter.rs` are the horizontal and vertical interacting-multiple-model
//!   filters, `filter.rs` joins them to the local frame (`oa_geo::aeqd`, moved from `oa-truth`
//!   so both share it).
//! * Every parameter is a named constant in [`params`] with what will set it from data.
//!
//! # Honest limits of this step
//!
//! Qualification and comparison claims belong to the private `oa-track-tools` harness; they are
//! not guarantees of this reusable tracking API.

mod linalg;

pub mod adsb;
pub mod assoc;
pub mod canon;
pub mod canon_spill;
pub mod coarse;
mod coarse_polar;
mod confirm;
mod filter;
pub mod fixed_map;
pub mod fusion;
pub mod fusion_params;
pub mod gate_trace;
mod hfilter;
pub mod latency;
pub mod params;
pub mod radar;
pub mod realtime;
pub mod registration;
pub mod spill;
pub mod synth_truth;
mod time_gate;
mod tracker;
pub mod types;
mod vfilter;

pub use filter::Filter;
pub use hfilter::{HFilter, HModel};
pub use tracker::{
    measurement_check, r95_m_of, tracker_advance, tracker_buffers, tracker_bytes, tracker_config,
    tracker_counts, tracker_flush, tracker_forecast, tracker_live, tracker_live_iter, tracker_log,
    tracker_moves_active, tracker_new, tracker_newest_update_ns, tracker_note_radar_disagreement,
    tracker_predict, tracker_predict_checked, tracker_set_expected_interval, tracker_stats,
    tracker_update, Tracker,
};
pub use types::*;
pub use vfilter::{VFilter, VModel};
