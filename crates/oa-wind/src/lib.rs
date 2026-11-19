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

pub mod core;
pub mod shell;

pub use crate::core::error::{LoadError, QueryError, Refusal};
pub use crate::core::hour::{
    build_hour, check_claims, head_claim, hour_bytes, hour_levels, index_claims, load_hour,
    load_hour_claimed, needed_ranges, needed_ranges_text, Claim, Hour,
};
pub use crate::core::isa::{altitude_ft_from_pressure_hpa, pressure_hpa_from_altitude_ft};
pub use crate::core::limits::*;
pub use crate::core::sample::{
    bracket, sample_blend, sample_hour_pressure, sample_hour_unrotated, sample_hours, Point, Query,
    Sample,
};
pub use crate::core::{adsb, check, error, hour, isa, sample};
pub use crate::shell::archive;
pub use crate::shell::archive::{
    archive_files, archive_sample, index_files, new_archive, read_file_capped, read_hour_file,
    read_hour_file_report, Archive, HourFile, IndexUse,
};
pub use crate::shell::compare::compare_obs;
