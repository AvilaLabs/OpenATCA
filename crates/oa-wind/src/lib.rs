#![forbid(unsafe_code)]
//! oa-wind: wind and temperature aloft at any (latitude, longitude, pressure altitude, time)
//! from NOAA RAP analyses decoded by `oa-grib2`. The trajectory predictor's weather input and
//! the benchmark's along-track context.
//!
//! * [`hour`]: one analysis hour as struct-of-arrays grids of U, V, T and geopotential height per
//!   isobaric level (level-major, pressure ascending), built from decoded fields or from the
//!   bytes of a file, and the `.idx` byte ranges that hold the needed fields. Core (pure).
//! * [`isa`]: the ICAO standard atmosphere, pressure altitude to pressure and back. Core.
//! * [`sample`]: the query. Core.
//! * [`archive`]: files on disk, a bounded cache of hours ([`MAX_HOURS_HELD`]) and the query
//!   over a set of hours. Shell (owns the files).
//! * [`adsb`]: RAP against the winds and temperatures aircraft report over ADS-B. Core.
//! * [`check`]: the validation statistics used by `oa-wind-check`. Core.
//!
//! Model of the query:
//!
//! * Horizontal: bilinear in the grid's own coordinates (fractional `i`, `j` from the Lambert
//!   projection). U and V are stored grid-relative (the resolution-and-component flags of the
//!   grid say so, `oa_grib2::components_grid_relative`); they are interpolated as stored and
//!   then turned to east and north by the convergence angle at the query longitude, the same
//!   as `oa_grib2::wind_at`. Interpolating before rotating uses the exact angle of the query
//!   point instead of an average of four corner angles.
//! * Vertical: linear in the logarithm of pressure between the two bracketing isobaric levels.
//!   The aircraft's pressure altitude becomes pressure through the ISA. That is exact, not an
//!   approximation: pressure altitude is by definition the altitude in the standard atmosphere
//!   that has the measured static pressure, so it is a relabelling of pressure and carries no
//!   dependence on the real temperature profile.
//! * Time: linear between the two bracketing analysis hours.
//! * Never extrapolated: a position outside the grid, a pressure outside the level range, a time
//!   outside the span of the hours, and a missing value at any corner used are refused with a
//!   typed [`Refusal`].
//!
//! Memory: an hour costs `levels x grid points x 4 fields x 4 bytes`. RAP 13 km (451 x 337 =
//! 151 987 points) at its 37 isobaric levels is about 90 MB (see [`hour::hour_bytes`]); at most
//! [`MAX_HOURS_HELD`] hours are held, so a query stream costs under 270 MB. Only UGRD, VGRD,
//! TMP and HGT on isobaric levels are decoded; every other message of a file is only inspected.
//!
//! Every input is bounded by the constants below (RULESET E5).

pub mod adsb;
pub mod archive;
pub mod check;
pub mod error;
pub mod hour;
pub mod isa;
pub mod sample;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_index;

pub use archive::{
    archive_files, archive_sample, index_files, new_archive, read_file_capped, read_hour_file,
    read_hour_file_report, Archive, HourFile, IndexUse,
};
pub use error::{LoadError, QueryError, Refusal};
pub use hour::{
    build_hour, check_claims, head_claim, hour_bytes, hour_levels, index_claims, load_hour,
    load_hour_claimed, needed_ranges, needed_ranges_text, Claim, Hour,
};
pub use isa::{altitude_ft_from_pressure_hpa, pressure_hpa_from_altitude_ft};
pub use sample::{
    bracket, sample_blend, sample_hour_pressure, sample_hour_unrotated, sample_hours, Point, Query,
    Sample,
};

/// Limit: most analysis hours held in memory at once. Three covers a query between two hours
/// plus the next one a time-ordered stream needs (about 270 MB for RAP 13 km).
pub const MAX_HOURS_HELD: usize = 3;
/// Limit: most isobaric levels in one hour. RAP has 37 (1013.2 to 50 hPa at 25 hPa steps is 40);
/// 64 is headroom, and bounds the arrays.
pub const MAX_LEVELS: usize = 64;
/// Limit: most values in one hour's arrays (`levels x points x 4`), 128 Mi (512 MiB of f32).
pub const MAX_HOUR_VALUES: u64 = 128 * 1024 * 1024;
/// Limit: largest file read whole, 1 GiB. A full RAP pressure-level file is about 100 MB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// Limit: most hour files in one archive.
pub const MAX_ARCHIVE_FILES: usize = 1024;
/// Limit: largest `.idx` sidecar read, 1 MiB. The largest real one (HRRR, 7 messages per level
/// of a subset) is 32 596 bytes; a full RAP file's is 16 497.
pub const MAX_IDX_BYTES: u64 = 1024 * 1024;
