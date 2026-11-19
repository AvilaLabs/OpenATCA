//! Core (B1, B7): pure functions and plain data. No I/O, no clock, no randomness, no hash-map
//! (the default hasher is seeded from the OS), and nothing here names the shell. The tables of
//! `model` are allocated once per build, offline (C6).

pub mod airac;
pub mod build_info;
pub mod diff;
pub mod geom;
pub mod model;
pub mod report;
pub mod snapshot;
pub mod source;
