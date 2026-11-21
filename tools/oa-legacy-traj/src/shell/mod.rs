//! Shell (B3, B7): arguments, the recordings and the clip, the truth path sampling, files. The
//! core modules do the arithmetic; nothing here decides a number.

pub mod args;
pub(crate) mod arrival;
pub mod guard;
pub mod places;
pub mod run;
pub mod write;
