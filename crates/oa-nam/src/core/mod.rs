//! Core (pure, B1, B7): the session state machine and the plain-value identity types it uses.
//! No I/O, no clock, no allocation after construction (C1, C2); time is an argument (B4).

pub mod identity;
pub mod session;
