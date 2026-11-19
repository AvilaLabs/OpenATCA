#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-encounter: the record of close encounters (`docs/M5_DESIGN.md` section 5, rule 5). Class:
//! core (RULESET B1, B7).
//!
//! The NTSB found after the DCA collision that the FAA had proximity data but no usable record of
//! close encounters (AIR-26-02, findings 63 to 66, recommendation A-26-37). A facility that runs
//! OpenATCA keeps that record from its own tracks: at each tick the tracker's rows are given to
//! [`encounter_cycle`], every pair inside the record volume is found with a spatial grid, and one
//! [`Encounter`] is kept per pair from the time it enters the volume to the time it has been out of
//! it for the hysteresis. Whether the pair would alert plays no part; this is the geometry only.
//!
//! * The record volume, the classes (tightest first), the hysteresis and every budget come from
//!   the caller in an [`EncounterConfig`]; nothing here has a default (the tool states them).
//! * A pair whose altitude is unknown on either track is kept, by horizontal distance, and an
//!   encounter that never had a known altitude is class [`CLASS_ALT_UNKNOWN`], never dropped.
//! * Two aircraft both on the ground are not an encounter; they are counted.
//! * Work is bounded (RULESET C7, M5 rule 5): the grid scan has a budget of probes and tests, the
//!   exact evaluation a budget of pairs per cycle. Over budget, pairs are evaluated in order of
//!   smallest separation and the rest are counted as not evaluated in [`CycleStats`]; an open
//!   encounter whose pair was not evaluated is kept alive and marked, never ended by the gap.
//!
//! Limits, stated: the grid test uses a local flat approximation. Its east-west scale is taken at
//! the first track's latitude, so it differs from the true separation by at most `tan(lat) * dphi`,
//! `dphi` being the radius as an angle (radius over 6,356,752 m); it accepts 2 % over the radius, so
//! it finds every pair inside the radius up to latitude `atan(0.02 / dphi)`, 85.8 degrees for the
//! 5 NM record ([`encounter_lat_limit_deg`]). Rows above it are used and counted
//! ([`CycleStats::rows_high_lat`]). The geometry is sampled at the ticks given and the true minimum
//! is interpolated between consecutive ticks; velocity is not used.

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

mod cycle;
mod geom;
mod grid;
mod state;
mod types;

pub use cycle::{encounter_cycle, encounter_finish};
pub use geom::{inside_volume, pair_class};
pub use state::{encounter_check, encounter_log_clear, encounter_log_new, encounter_new};
pub use state::{encounter_lat_limit_deg, encounter_open_len, encounter_totals, EncounterState};
pub use types::{
    ClassLimit, CycleStats, Encounter, EncounterConfig, EncounterLog, Obs, AIRBORNE,
    CLASS_ALT_UNKNOWN, CLASS_VOLUME_ONLY, GROUND_UNKNOWN, MAX_CLASSES, M_PER_FT, ON_GROUND,
};
