#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-msaw: the minimum safe altitude warning, the second of the M5 safety nets
//! (`docs/M5_DESIGN.md` section 3, the five rules). Class: core (RULESET B1, B7).
//!
//! Each cycle the tracker's live tracks go to [`msaw_plan`], which gives the points of every
//! track's straight-line path over the look-ahead; the shell looks up the floor at each point
//! (terrain, obstacle, chart: plain values, no callback) and [`msaw_evaluate`] judges the paths
//! against them. A track whose predicted altitude is below its floor within the look-ahead is
//! alerted: a caution, and a warning once the time to the violation is within the warning time.
//!
//! * The floor is the higher of terrain plus a clearance, an obstacle top plus a clearance and the
//!   minimum vectoring altitude of the sector ([`floor_of`]). Terrain and obstacles are the
//!   shell's conservative maxima over the point's radius (the track's 95 % radius plus half a
//!   step). Inside a procedure area the published altitude replaces the chart altitude when lower,
//!   and the clearances are the procedure clearance: a lower floor in that area only.
//! * Nothing here has a default: the look-ahead, the warning time, every clearance, the budgets
//!   and the procedure areas come from the caller in an [`MsawConfig`]. The tests use made-up
//!   values and say so; no FAA value is in this crate.
//! * Rule 4: a track with no altitude is counted, listed and the net reads degraded. Rule 1: a
//!   point with no terrain or chart coverage is counted, listed and the net reads degraded; the
//!   covered points are still evaluated, and nothing is treated as safe for want of data.
//!   Rule 2: an alert is hidden only by an `oa-safety` inhibition of the net in force at the
//!   track's position and altitude, and every hidden violation is recorded there. Rule 5: the
//!   track budget is named; over it the lowest altitudes are evaluated first and the rest are
//!   counted, listed and the net reads degraded. Aircraft on the ground are excluded and counted.
//! * Limits, stated: the prediction is a straight line from the cycle's position and velocity;
//!   the altitude is the tracker's barometric altitude as given (no altimeter correction yet);
//!   a row beyond latitude 85 degrees is refused as bad.

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

pub mod calc;
mod cycle;
mod state;
mod types;

pub use calc::{floor_of, violates};
pub use cycle::{
    msaw_evaluate, msaw_finish, msaw_plan, msaw_publish_refused, net_report_of, CycleOut,
};
pub use state::{
    alert_log_clear, alert_log_new, msaw_check, msaw_new, msaw_note_lost_input, msaw_open,
    msaw_open_len, msaw_planned, msaw_points, msaw_totals, skip_log_clear, skip_log_new, MsawState,
};
pub use types::{
    Alert, AlertLog, CycleStats, FloorKind, FloorSample, Grade, MsawConfig, MsawTrack, ProcArea,
    SamplePoint, SkipLog, SkipWhy, Skipped, AF_COAST, AF_GROUND_UNKNOWN, AF_IDENTITY_UNCONFIRMED,
    AF_PARTIAL_COVERAGE, AF_RADAR_ONLY, AF_RATE_UNKNOWN, AF_UNCERTAIN, AIRBORNE, FLOOR_NONE,
    F_COAST, F_IDENTITY_UNCONFIRMED, F_RADAR_ONLY, F_UNCERTAIN, GROUND_UNKNOWN, MAX_ABS_ALT_FT,
    MAX_ABS_LAT_DEG, MAX_ABS_VRATE_FPM, MAX_CLEARANCE_FT, MAX_LOOK_AHEAD_S, MAX_OPEN_LIMIT,
    MAX_PROCS, MAX_RADIUS_M, MAX_SAMPLES, MAX_SPEED_LIMIT_MPS, MAX_TRACKS_LIMIT, ON_GROUND,
};
