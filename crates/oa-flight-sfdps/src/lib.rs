#![forbid(unsafe_code)]
//! Drives the per-flight record (`oa-flight`) from recorded SFDPS messages: the adapter that turns
//! a decoded row into operations, and the replay that reads recordings and applies them. Shell.

pub mod adapter;
pub mod replay;

pub use adapter::{row_ops, AdapterStats, MAX_OPS_PER_MESSAGE};
pub use replay::{
    lag_quantile_s, recordings_in, replay_sfdps, replay_sfdps_counted, LagHist, ReplayConfig,
    ReplayReport, LAG_MAX_S, LAG_MIN_S,
};
