#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-ca: conflict alert, the first of the M5 safety nets (`docs/M5_DESIGN.md` section 2, the
//! five rules). Class: core (RULESET B1, B7).
//!
//! At each cycle the tracker's live tracks are given to [`ca_cycle`]. Pairs that could come
//! within the horizontal and vertical minima inside the look-ahead are found with a spatial
//! grid, predicted in a straight line ([`solve`]) and kept as one [`Alert`] per pair from the
//! first cycle in conflict to the last (with a hysteresis, as `oa-encounter` keeps encounters).
//!
//! * Nothing here has a default: the look-ahead, the minima of every volume (a list of areas
//!   with an altitude band, and a required default volume for a pair outside them all), the
//!   hysteresis and every budget come from the caller in a [`CaConfig`]. The tests use made-up
//!   values and say so; no ERAM, STARS or 7110.65 number is in this crate. The one constant,
//!   [`COAST_V_ALLOWANCE_FT`], is this crate's own widening of the vertical minimum for a poor
//!   track.
//! * A poor track widens the alert and never hides it (rule 4): the horizontal minimum grows by
//!   both 95 % radii, the vertical by [`COAST_V_ALLOWANCE_FT`] per coasting or uncertain track.
//!   A pair with an altitude unknown on either track is tested horizontally only and carries
//!   [`AF_ALT_UNKNOWN`]; a track that loses a known altitude raises an [`AltLost`] event.
//! * Work is bounded (RULESET C7, rule 5): the grid scan has a budget of band steps, cell probes
//!   and tracks visited, the exact evaluation a budget of pairs per cycle. Over budget, pairs
//!   are evaluated in order of smallest current horizontal separation (ties by address), the
//!   rest are counted, and an open alert whose pair was not evaluated is kept alive and marked,
//!   never ended by the gap.
//! * Every cycle publishes the net's state to the `oa-safety` table (rule 1): running when
//!   everything was evaluated, degraded with the count otherwise ([`net_report_of`]), and
//!   unavailable when the cycle was refused.
//!
//! Limits, stated: the grid test uses a local flat approximation (east-west scale at the first
//! track's latitude), a superset test up to the latitude `atan(0.02 / dphi)`, `dphi` being the
//! cycle's largest radius over 6,356,752 m; rows above it are used and counted
//! ([`CycleStats::rows_high_lat`]) and the net reports degraded. Prediction is a straight line
//! from the cycle's position and velocity; a turning aircraft is not yet predicted (build order
//! step 6).

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

mod cycle;
mod grid;
pub mod solve;
mod state;
mod types;
mod volume;

pub use cycle::{ca_cycle, ca_finish, ca_publish_refused, net_report_of, CycleOut};
pub use state::{
    alert_log_clear, alert_log_new, ca_check, ca_new, ca_note_lost_input, ca_open, ca_open_len,
    ca_totals, event_log_clear, event_log_new, CaState,
};
pub use types::{
    Alert, AlertLog, AltLost, Area, CaConfig, CaTrack, CycleStats, EventLog, Volume,
    AF_ALT_UNKNOWN, AF_COAST, AF_IDENTITY_UNCONFIRMED, AF_ONE_GROUND, AF_RADAR_ONLY,
    AF_RATE_UNKNOWN, AF_UNCERTAIN, AIRBORNE, COAST_V_ALLOWANCE_FT, F_COAST, F_IDENTITY_UNCONFIRMED,
    F_RADAR_ONLY, F_UNCERTAIN, GROUND_UNKNOWN, MAX_ABS_ALT_FT, MAX_ABS_VRATE_FPM, MAX_VOLUMES,
    ON_GROUND,
};
pub use volume::{volume_maxima, volume_minima};
