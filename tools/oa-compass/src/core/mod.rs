//! Core (pure, B1, B7): the conversion of one row into the values of one CAT062 record. No I/O,
//! no clock, no randomness.

pub mod addrcheck;
pub mod adsb;
pub mod convert;
pub mod stats;
pub mod time;
pub mod walk;
