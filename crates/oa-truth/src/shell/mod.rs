//! Shell (B1, B7): the clip reader (owns the file), `quality` (the quality rules on an
//! `oa_adsb::Row`) and `build`. `build` holds the whole build
//! algorithm. Its output is a function of its input, but two things keep it out of core (B8):
//! a `thread_local!` counter of same-time distance evaluations (`pair_evals_this_thread`, a
//! test hook that `build_truth` increments), and the `oa_adsb` crate (class shell) it names (`Reports`, the
//! jump and category rules). It can move to `core` only when the hook is replaced by a counter
//! returned in the output and the `oa_adsb` rules it uses are reached through a core
//! module. The `oa-truth-check` binary
//! (`src/bin/oa-truth-check.rs`) is shell too.

pub mod build;
pub mod clip;
pub mod quality;
