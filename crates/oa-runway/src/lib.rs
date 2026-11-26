#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-runway: runway geometry per airport and the occupancy test (docs/RUNWAY_SAFETY_DESIGN.md,
//! sections 1, 2 and 4, build step 1). Class: core (RULESET B1, B7).
//!
//! * [`plane`]: the airport's local east-north plane, its stated extent and error bound, and the
//!   conversion from degrees ([`plane::to_local`]).
//! * [`runway`]: a [`runway::Runway`] built from plain runway and runway-end values (the shell
//!   copies them out of NASR `APT_RWY` and `APT_RWY_END` and the CIFP runway record), or refused
//!   with a counted reason.
//! * [`occupy`]: [`occupy::occupies`], the conservative occupancy test, and its one Kani proof (no panic on any input).
//!
//! No function allocates or panics on any input, and nothing here reads a file, a clock or the
//! network: the shell reads the records and passes plain values in (B3, B5).

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

pub mod arv;
pub mod final_monitor;
pub mod occupy;
pub mod plane;
pub mod runway;
