#![forbid(unsafe_code)]
//! oa-feed-gaps: the damage check of P-006 section 6 over one scored window. Class `split` (B7).
//!
//! * `core`: the interval logic (the window, the largest interval, every interval over 600 s, the
//!   edges of the window, the verdict), the ISO time parse and the report text. No I/O.
//! * `shell`: the arguments, the SFDPS recording and the ADS-B clip read through the guarded
//!   readers, the atomic report file, and the `oa-feed-gaps` binary.
//!
//! Requirements: OA-GAP-001 and the following, in REQUIREMENTS.md.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;
