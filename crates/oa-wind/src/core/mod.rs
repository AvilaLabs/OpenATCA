//! Core (pure, B1, B7): the hour model built from decoded fields or bytes, the standard
//! atmosphere, the query, the validation statistics and the ADS-B comparison. Bytes and values
//! in, plain data out; no files, no clock.

pub mod adsb;
pub mod check;
pub mod error;
pub mod hour;
pub mod isa;
pub mod limits;
pub mod sample;
