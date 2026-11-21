#![forbid(unsafe_code)]
//! `oa-legacy-traj`: how well the legacy systems predict, measured on recorded traffic (M4 build
//! step 2, `docs/M4_DESIGN.md` section 9). ERAM's predicted times along its converted route,
//! TBFM's meter-fix and runway estimates and TFMS's arrival estimates are each scored against
//! when the aircraft actually got there, from the ADS-B truth of `oa-truth`, by look-ahead.
//!
//! The crate is `split` (B7): the crossing search, the binning, the statistics, the join rule and
//! the report text are core (pure, `core/`); reading the recordings and the clip, building the
//! truth, sampling its path and writing the files are shell (`shell/`).
//!
//! Requirements: OA-LTJ-001 and the following, in REQUIREMENTS.md.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;
