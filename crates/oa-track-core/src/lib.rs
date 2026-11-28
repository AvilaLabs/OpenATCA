#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! Pure fixed-width tracker engine. Sensor parsing, truth selection, sealed inputs and
//! operational commands stay in the `oa-track` façade and tool packages.

#[cfg(not(any(
    target_pointer_width = "64",
    all(
        target_arch = "wasm32",
        target_pointer_width = "32",
        target_os = "unknown",
        target_vendor = "unknown"
    )
)))]
compile_error!("core supports native 64-bit targets and wasm32-unknown-unknown only");

extern crate alloc;

mod confirm;
mod filter;
mod hfilter;
mod linalg;
pub mod params;
mod time_gate;
mod tracker;
pub mod types;
mod vfilter;

pub use filter::Filter;
pub use hfilter::{HFilter, HModel};
pub use tracker::{
    check_advance_time, measurement_check, r95_m_of, tracker_advance, tracker_altitude_forecast,
    tracker_config, tracker_counts, tracker_flush, tracker_forecast, tracker_has_key,
    tracker_is_confirmed, tracker_live, tracker_live_iter, tracker_log, tracker_moves_active,
    tracker_new, tracker_newest_update_ns, tracker_note_radar_disagreement, tracker_predict,
    tracker_predict_checked, tracker_predict_for_association, tracker_set_expected_interval,
    tracker_snapshot_at, tracker_stats, tracker_update, AltitudeForecast, Tracker,
};
pub use types::{
    check_config, log_new, output_clear, output_len, output_new, output_push, to_e7, Measurement,
    OutRow, PredictError, TrackError, TrackLog, TrackOutput, TrackerConfig, TrackerStats,
    DEFAULT_CONFIG, END_DROPPED, END_FLUSHED, END_KINDS, END_NAMES, END_TENTATIVE,
    MAX_AIRCRAFT_ADDRESS, MAX_TABLE_ROWS, MAX_TICK_NS, MIN_TICK_NS, M_ALT_GEOMETRIC, M_HAS_VEL,
    NONE_I32, NO_ADDR, NO_IDENT, NO_IDENT_NS, O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED,
    O_MOVE_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE,
    REFUSE_KINDS, REFUSE_NAMES, REF_ALTITUDE, REF_IDENTITY, REF_NO_ADDRESS, REF_POSITION,
    REF_POS_COV, REF_TIME, REF_VELOCITY, TIME_LIMIT_NS,
};
pub use vfilter::{VFilter, VModel};
