#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-safety: the state of the safety nets and the inhibition objects (docs/M5_DESIGN.md, the
//! five rules, rules 1 and 2). Class: core (RULESET B1, B7).
//!
//! * [`net`]: every net publishes its state each cycle; a state table turns a missing or old
//!   report into "unavailable", never into "running" (rule 1).
//! * [`inhibit`]: an inhibition is an object with an area, an altitude band, a start, an end no
//!   later than the configured maximum, a reason and an author; it is logged, it expires, and
//!   every alert it suppresses is recorded (rule 2).
//! * [`geom`]: the fixed-size polygon an inhibition covers.
//! * [`monitor`]: a bounded offline checker for independently paired observations and track
//!   claims (no live wiring or matching).
//!
//! No function allocates: every table is a fixed array whose capacity is a named constant with
//! its reasoning beside it (C1, C2), and no function panics on any input. Times are signed
//! nanoseconds (the unit of `oa-replay`'s clock) and altitudes are in feet. Inhibitions use
//! positions in 1e-7 degrees. The monitor takes local-plane positions in millimetres.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

pub mod geom;
pub mod inhibit;
pub mod monitor;
pub mod net;
