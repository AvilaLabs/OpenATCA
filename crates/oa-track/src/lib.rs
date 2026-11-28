#![forbid(unsafe_code)]
//! oa-track: the reusable surveillance tracker of OpenATCA (`docs/M3_DESIGN.md` section 3).
//! Core (pure, B1): filters, association, fusion and the track table depend on measurements and
//! times supplied by the caller; [`adsb`] adapts `oa-adsb` reports. Offline scoring and simulation
//! commands live in the separate `oa-track-tools` shell package.
//!
//! * [`Measurement`] is the interface every sensor plugs into: time of applicability, sensor id,
//!   position with covariance, optional velocity with covariance, optional barometric or
//!   geometric altitude with quantisation, optional address and identity.
//! * [`tracker_update`], [`tracker_advance`] and [`tracker_flush`] are the whole interface. The
//!   engine implementation lives in `oa-track-core` and owns time ordering, initiation, coasting,
//!   dropping and fixed-rate output.
//! * The core crate holds the horizontal and vertical interacting-multiple-model filters and
//!   joins them to the local frame (`oa_geo::aeqd`, moved from `oa-truth` so both share it).
//! * Every parameter is a named constant in [`params`] with what will set it from data.
//!
//! # Honest limits of this step
//!
//! Qualification and comparison claims belong to the private `oa-track-tools` harness; they are
//! not guarantees of this reusable tracking API.

pub mod adsb;
pub mod assoc;
pub mod canon;
pub mod canon_spill;
pub mod coarse;
mod coarse_polar;
pub mod fixed_map;
pub mod fusion;
pub mod fusion_params;
pub mod gate_trace;
pub mod latency;
pub mod output_hash;
pub mod params {
    pub use oa_track_core::params::{
        cv_only_q, ALT_BASE_SIGMA_FT, ALT_GATE_SIGMAS, BARO_QUANT_FT, CHI2_2_95,
        CLOCK_STEP_CONFIRM, CLOCK_STEP_LIMIT_NS, COAST_AFTER_INTERVALS, COAST_MAX_R95_M,
        CONFIRM_AGREE_M, CONFIRM_MIN_SEP_NS, CONFIRM_MOVE_M, CONFIRM_POOL, CONFIRM_REPORTS,
        CONFIRM_WINDOW_INTERVALS, CONFIRM_WINDOW_NS, CT_OMEGA_STD_FLOOR, DEFAULT_NACV,
        DROP_AFTER_NS, EXPECTED_INTERVAL_NS, FAIL_CLOSED_R95_M, GATE_CHI2, GATE_MANOEUVRE_MPS2,
        H_MU0_CV, H_PRIOR_VEL_STD, H_Q_CT, H_Q_CV, H_Q_OMEGA, H_TAU_CT_S, H_TAU_CV_S, INIT_M,
        INIT_N, LOG_CAPACITY, MAX_TRACKS, MU_FLOOR, NACV_BOUND_VEL_SIGMA_MPS,
        NACV_VEL_SIGMA_ALL_CONDITIONS_MPS, NACV_VEL_SIGMA_MPS, NS_PER_S, OMEGA_MAX, OOO_WINDOW_NS,
        OUT_CAPACITY, PLAUSIBLE_SLACK_M, POS_SIGMA_SCALE, REACQUIRE_GAP_NS, RESYNC_AFTER,
        RING_DEPTH, TICK_NS, VEL_GATE_CHI2, VEL_GATE_CHI2_STRICT, VEL_LAG_MAX_S, VEL_LAG_S,
        VRATE_SIGMA_FPS, V_MU0_LEVEL, V_PRIOR_RATE_STD_FPS, V_Q_CLIMB, V_Q_LEVEL, V_TAU_CLIMB_S,
        V_TAU_LEVEL_S,
    };
}
pub mod radar;
pub mod realtime;
pub mod registration;
pub mod spill;
pub mod synth_truth;
pub mod types {
    pub use crate::output_hash::{output_hash_into, OUTPUT_HASH_TAG};
    pub use oa_track_core::types::{
        check_config, log_new, output_clear, output_len, output_new, output_push, to_e7,
        Measurement, OutRow, PredictError, TrackError, TrackLog, TrackOutput, TrackerConfig,
        TrackerStats, DEFAULT_CONFIG, END_DROPPED, END_FLUSHED, END_KINDS, END_NAMES,
        END_TENTATIVE, MAX_AIRCRAFT_ADDRESS, MAX_TABLE_ROWS, MAX_TICK_NS, MIN_TICK_NS,
        M_ALT_GEOMETRIC, M_HAS_VEL, NONE_I32, NO_ADDR, NO_IDENT, NO_IDENT_NS, O_COAST, O_HAS_ALT,
        O_IDENTITY_UNCONFIRMED, O_MOVE_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK,
        O_UNCERTAIN, RADAR_KEY_BASE, REFUSE_KINDS, REFUSE_NAMES, REF_ALTITUDE, REF_IDENTITY,
        REF_NO_ADDRESS, REF_POSITION, REF_POS_COV, REF_TIME, REF_VELOCITY, TIME_LIMIT_NS,
    };
}
mod tracker;

pub use oa_track_core::types::{
    check_config, log_new, output_clear, output_len, output_new, output_push, to_e7, Measurement,
    OutRow, PredictError, TrackError, TrackLog, TrackOutput, TrackerConfig, TrackerStats,
    DEFAULT_CONFIG, END_DROPPED, END_FLUSHED, END_KINDS, END_NAMES, END_TENTATIVE,
    MAX_AIRCRAFT_ADDRESS, MAX_TABLE_ROWS, MAX_TICK_NS, MIN_TICK_NS, M_ALT_GEOMETRIC, M_HAS_VEL,
    NONE_I32, NO_ADDR, NO_IDENT, NO_IDENT_NS, O_COAST, O_HAS_ALT, O_IDENTITY_UNCONFIRMED,
    O_MOVE_UNCONFIRMED, O_RADAR_DISAGREES, O_REACQUIRED, O_TICK, O_UNCERTAIN, RADAR_KEY_BASE,
    REFUSE_KINDS, REFUSE_NAMES, REF_ALTITUDE, REF_IDENTITY, REF_NO_ADDRESS, REF_POSITION,
    REF_POS_COV, REF_TIME, REF_VELOCITY, TIME_LIMIT_NS,
};
pub use oa_track_core::{
    measurement_check, r95_m_of, tracker_advance, tracker_config, tracker_counts, tracker_flush,
    tracker_forecast, tracker_live, tracker_live_iter, tracker_log, tracker_moves_active,
    tracker_new, tracker_newest_update_ns, tracker_note_radar_disagreement, tracker_predict,
    tracker_predict_checked, tracker_set_expected_interval, tracker_snapshot_at, tracker_stats,
    tracker_update, Filter, HFilter, HModel, Tracker, VFilter, VModel,
};
pub use output_hash::{output_hash_into, OUTPUT_HASH_TAG};
