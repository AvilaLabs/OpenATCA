#![forbid(unsafe_code)]
//! oa-scope-replay: tracker output on the scope, one image per step.
//!
//! The display crate (`oa-display`) knows traffic only as plain [`oa_display::traffic::Target`]s
//! and never the tracker (SCOPE principle 7, RULESET D3, F6). This crate is the join, so the
//! tracker is linked here and not into the display:
//!
//! * [`rows`] (core, B1): tracker output rows kept per address, and the pure conversion of the
//!   rows at a time into a target set (ground track and speed from the velocity, altitude, flags,
//!   95% radius, callsign, history, how long ago the track was reacquired).
//! * [`frame`] (core): the per-frame state to display list to pixels chain over the map and the
//!   traffic, with every table allocated once.
//! * [`cli`] (shell): files, the tracker run over a clip, the clock and the `oa-scope-replay`
//!   command.
//!
//! The tracker is run in this shell from an ADS-B clip (there is no tracker rows file), the
//! simpler route that keeps both cores pure: the rows exist only in memory, for the window asked.

pub mod cli;
pub mod frame;
pub mod rows;
