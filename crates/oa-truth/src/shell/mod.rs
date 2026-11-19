//! Shell (B1, B7): the clip reader (owns the file) and `build`. `build` holds the whole build
//! algorithm, which is pure but for one thing: a `thread_local!` counter of same-time distance
//! evaluations (`pair_evals_this_thread`, a test hook that `build_truth` increments). A global
//! mutated from the build keeps it out of core (B8); it can move to `core` when the hook is
//! replaced by a counter returned in the output. The `oa-truth-check` binary (`src/main.rs`) is
//! shell too.

pub mod build;
pub mod clip;
