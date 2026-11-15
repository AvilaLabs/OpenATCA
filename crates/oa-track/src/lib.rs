#![forbid(unsafe_code)]
//! oa-track: the surveillance tracker of OpenATCA (`docs/M3_DESIGN.md` section 3), ADS-B only in
//! this step, with the scoring path of section 2 (split reports). Core (pure, B1): the filters
//! and the track table are pure functions of the measurements and the times given to them;
//! [`adsb`] is the adapter from `oa-adsb` reports; [`score`] and the `oa-track-run` binary are
//! shell (they read a file, read the clock and write files).
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
//! The scoring input is adsb.lol history thinned to about one position per 17 s per aircraft,
//! and the split halves it again. No result here is an ESASSP exit result (that needs full-rate
//! ADS-B, `docs/M3_DESIGN.md`); every scored number is labelled so. GNSS error correlated over
//! seconds is shared between the two halves of a split, so the scored error is a lower bound.

mod linalg;

pub mod adsb;
pub mod assoc;
mod filter;
pub mod fusion;
pub mod fusion_params;
pub mod fusion_score;
pub mod fusion_world;
mod hfilter;
pub mod params;
pub mod radar;
pub mod registration;
pub mod score;
pub mod synth_truth;
mod tracker;
pub mod types;
mod vfilter;

pub use filter::Filter;
pub use hfilter::{HFilter, HModel};
pub use tracker::{
    measurement_check, tracker_advance, tracker_buffers, tracker_bytes, tracker_config,
    tracker_counts, tracker_flush, tracker_live, tracker_log, tracker_new, tracker_predict,
    tracker_stats, tracker_update, Tracker,
};
pub use types::*;
pub use vfilter::{VFilter, VModel};
