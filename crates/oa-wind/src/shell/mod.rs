//! Shell (B1, B3, B7, C6): hour files on disk with a bounded cache (`archive`), the archive-backed
//! comparison with observations (`compare`), and the tests. Also the pure but offline code that
//! consumes decoder output: the hour model built from `oa-grib2` fields (`hour`), the query over
//! it (`sample`), the validation statistics (`check`), the ADS-B comparison over `oa-adsb`
//! reports (`adsb`) and `LoadError`/`QueryError` (`error`). It allocates an hour of grids per
//! input file (about 90 MB), so it is not real-time core (C1). The `oa-wind-check` binary
//! (`src/bin/oa-wind-check/main.rs`) is shell too.

pub mod adsb;
pub mod archive;
pub mod check;
pub mod compare;
pub mod error;
pub mod hour;
pub mod sample;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_index;
