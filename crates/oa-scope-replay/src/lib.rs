#![forbid(unsafe_code)]
//! oa-scope-replay: tracker output on the scope, one image per step.
//!
//! The display crate (`oa-display`) knows traffic only as plain [`oa_display::traffic::Target`]s
//! and never the tracker (SCOPE principle 7, RULESET D3, F6). This crate is the join, so the
//! tracker is linked here and not into the display. It is split into a core and a shell (B1):
//!
//! * `core::rows` (core): tracker output rows kept per address as plain columns, and the pure
//!   conversion of the rows at a time into a target set (ground track and speed from the
//!   velocity, altitude, flags, 95% radius, callsign, history, how long ago the track was
//!   reacquired). It sees no tracker or decoder type: only slices.
//! * `core::frame` (core): the per-frame state to display list to pixels chain over the map and
//!   the traffic, with every table allocated once.
//! * `shell::adapt` (shell): the tracker's output and the decoder's reports as plain columns.
//! * `shell::cli` (shell): files, the tracker run over a clip, the clock and the
//!   `oa-scope-replay` command, and the end-of-run report of everything excluded or lost.
//!
//! The tracker is run in this shell from an ADS-B clip (there is no tracker rows file), the
//! simpler route that keeps both cores pure: the rows exist only in memory, for the window asked.
//!
//! The modules are re-exported here: `oa_scope_replay::rows` is `oa_scope_replay::core::rows`.

pub mod core;
pub mod shell;

pub use crate::core::{frame, rows};
pub use crate::shell::{adapt, cli};
