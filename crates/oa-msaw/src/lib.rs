#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-msaw: the minimum safe altitude warning, the second of the M5 safety nets
//! (`docs/M5_DESIGN.md` section 3, the five rules). Class: core (RULESET B1, B7).
//!
//! Each cycle the tracker's live tracks go to [`msaw_plan`], which gives the points of every
//! track's straight-line path over the look-ahead; the shell looks up terrain and obstacle floors
//! at each point and reports coverage gaps (plain values, no callback), and [`msaw_evaluate`]
//! judges the paths against them. A track whose predicted altitude is below its floor within the
//! look-ahead is alerted: a caution, and a warning once the time to the violation is within the
//! warning time.
//!
//! * The alert floor is the higher of terrain plus its clearance, obstacle top plus its clearance,
//!   and the applicable procedure surface ([`floor_of`]). MVA is reported separately; it is never
//!   an alert floor. The shell supplies exact terrain and obstacle lookups over each point's query
//!   radius (the track's 95 % radius plus half a step, with the radius growing with prediction
//!   time). Missing terrain or obstacle coverage is reported and degrades the net. Inside an
//!   approach capture box, only an aligned track receives the approach surface and terminal
//!   look-ahead; runway strips and departure corridors have their own direction and floor rules.
//! * Nothing here has a default: the look-ahead, the warning time, every clearance, the budgets
//!   and the procedure areas come from the caller in an [`MsawConfig`]. The tests use made-up
//!   values and say so; no FAA value is in this crate.
//! * A missing per-aircraft altitude is counted; the net degrades only when the configured share
//!   of airborne tracks is not monitored for altitude. Missing terrain or obstacle coverage is
//!   counted and degrades the net; covered points are still evaluated, and nothing is treated as
//!   safe for want of data.
//!   Rule 2: an alert is hidden only by an `oa-safety` inhibition of the net in force at the
//!   track's position and altitude, and every hidden violation is recorded there. Rule 5: the
//!   track budget is named; over it the lowest altitudes are evaluated first and the rest are
//!   counted, listed and the net reads degraded. Aircraft on the ground are excluded and counted.
//! * Limits, stated: the prediction is a straight line from the cycle's position and velocity;
//!   the altitude is on the datum of the floors, which the shell must have corrected (a track
//!   flagged [`F_QNH_MISSING`] had no altimeter setting: counted, listed, the net degraded); a
//!   row beyond latitude 85 degrees is refused as bad.

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

pub mod calc;
mod corridor;
mod cycle;
mod state;
mod types;

pub use calc::{
    aligned, altitude_use, bearing_gap_deg, corridor_hit_for, corridor_hit_ft, corridor_hit_linear,
    extrapolated_alt_ft, floor_of, not_climbing, runway_elev_at, track_deg_of, violates,
    with_airport_area, AltUse, PAVEMENT_MARGIN_M, UNKNOWN_RATE_DESCENT_FPM,
};
pub use corridor::{corridor_hit_at, corridor_index_new, corridor_runway_elev_at, CorridorIndex};
pub use cycle::{
    msaw_evaluate, msaw_finish, msaw_plan, msaw_publish_refused, net_report_of, CycleOut,
};
pub use state::{
    alert_log_clear, alert_log_new, msaw_check, msaw_new, msaw_note_lost_input, msaw_open,
    msaw_open_len, msaw_planned, msaw_points, msaw_totals, skip_log_clear, skip_log_new, MsawState,
};
pub use types::{
    Alert, AlertLog, Corridor, CorridorHit, CycleStats, FloorKind, FloorSample, Grade, MsawConfig,
    MsawTrack, SamplePoint, SkipLog, SkipWhy, Skipped, AF_COAST, AF_COLD_CORRECTED, AF_COLD_HIGH,
    AF_COLD_UNKNOWN, AF_GROUND_UNKNOWN, AF_IDENTITY_UNCONFIRMED, AF_PARTIAL_COVERAGE,
    AF_QNH_MISSING, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_STALE_EXTRAPOLATED, AF_UNCERTAIN,
    AF_WIDE_UNCERTAINTY, AIRBORNE, CK_APPROACH, CK_DEPARTURE, CK_RUNWAY, END_CLEAR,
    END_LOST_ALTITUDE_MISSING, END_LOST_ALTITUDE_STALE, FLOOR_NONE, F_COAST, F_COLD_CORRECTED,
    F_COLD_HIGH, F_COLD_UNKNOWN, F_IDENTITY_UNCONFIRMED, F_QNH_MISSING, F_RADAR_ONLY, F_UNCERTAIN,
    GROUND_UNKNOWN, KB_APPROACH, KB_DEPARTURE, KB_RUNWAY, MAX_ABS_ALT_FT, MAX_ABS_LAT_DEG,
    MAX_ABS_VRATE_FPM, MAX_CLEARANCE_FT, MAX_CORRIDORS, MAX_CORRIDOR_LENGTH_M, MAX_LOOK_AHEAD_S,
    MAX_OPEN_LIMIT, MAX_R95_M, MAX_RADIUS_M, MAX_SAMPLES, MAX_SPEED_LIMIT_MPS, MAX_TRACKS_LIMIT,
    ON_GROUND, WIDE_R95_M,
};
