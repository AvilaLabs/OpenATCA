//! Core (pure, B1, B7): the standard atmosphere, the query refusal and the limits. Plain values
//! in, plain values out; no files, no clock. The hour model, the query, the validation
//! statistics and the ADS-B comparison consume `oa-grib2` and `oa-adsb` output, one allocated
//! field or report set per input, and so are shell (C6): see `shell`.

pub mod error;
pub mod isa;
pub mod limits;
