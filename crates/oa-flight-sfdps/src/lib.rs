#![forbid(unsafe_code)]
//! Drives the per-flight record (`oa-flight`) from recorded SFDPS messages: the adapter that turns
//! a decoded row into operations, and the replay that reads recordings and applies them. Shell.

pub mod adapter;
pub mod audit;
pub mod compat;
pub mod migration_projection;
pub mod replay;
pub mod timing;

pub use adapter::{norm_callsign, row_ops, AdapterStats, MAX_OPS_PER_MESSAGE};
pub use replay::replay_sfdps_compat_counted;
pub use replay::{
    decoder_limits, lag_quantile_s, recordings_in, replay_sfdps, replay_sfdps_audited_counted,
    replay_sfdps_counted, replay_sfdps_timed_counted, validate_replay_window, LagHist, Position,
    Positions, ReplayConfig, ReplayReport, LAG_MAX_S, LAG_MIN_S,
};

pub use timing::{ReplayTimingReport, TimingBoundsError, MAX_TIMING_BINS};
