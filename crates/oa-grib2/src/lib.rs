#![forbid(unsafe_code)]
//! oa-grib2: GRIB Edition 2 decoder for NOAA RAP (13 km) and HRRR (3 km) winds and temperatures
//! aloft, written from the WMO Manual on Codes (FM 92 GRIB) and NCEP's public GRIB2
//! documentation. Core (pure): bytes in, plain values out; the caller reads the files. The
//! pages followed are listed in `openatca-data/raw/grib2-docs/SOURCE.md`.
//!
//! NOAA writes RAP (all fields) and many other products (GEFS) with JPEG 2000 packing
//! (template 5.40); those decode through `oa-j2k`, and a field's packed integers can be read
//! before scaling with [`pack::unpack_ints`]. HRRR UGRD/VGRD at 250 mb are template 5.3.
//!
//! * [`message`]: sections 0-8, several messages per file, several fields per message.
//! * [`grid`]: templates 3.30 (Lambert conformal) and 3.0 (latitude/longitude), the projection,
//!   and the grid-relative to earth-relative vector rotation.
//! * [`product`]: templates 4.0 and 4.8, parameter names, fixed surfaces, times.
//! * [`pack`]: templates 5.0, 5.2, 5.3 and 5.40, and the section 6 bitmap. Template 5.41 and
//!   others are refused with [`Grib2Error::UnsupportedPacking`].
//! * [`sample`]: bilinear interpolation and [`wind_at`].
//! * [`idx`]: the `.idx` inventory parser for byte-range requests.
//!
//! Every input is bounded by the constants below and refused with a typed error when it
//! exceeds them or is inconsistent (RULESET E5, OA-GEN-003).

pub mod bits;
pub mod error;
pub mod grid;
pub mod idx;
pub mod message;
pub mod pack;
pub mod product;
pub mod sample;

/// Test encoder for this crate's tests and for downstream crates' (feature `testenc`).
#[cfg(any(test, feature = "testenc"))]
pub mod testenc;
#[cfg(test)]
mod tests;

pub use error::Grib2Error;
pub use grid::{
    components_grid_relative, grid_dims, grid_rotation, ij_to_latlon, lambert_setup, latlon_to_ij,
    parse_grid, to_earth_relative, Grid, LambertGrid, LambertInput, LatLonGrid,
};
pub use idx::{byte_range, find_rows, idx_len, isobaric_label, parse_idx, Idx};
pub use message::{
    decode_message, decode_stream, inspect_stream, message_length, peek_info, scan_messages,
    Decoded, Field, FieldInfo, Inspected, MessageSpan, Skipped,
};
pub use pack::MISSING;
pub use product::{
    forecast_seconds, parameter_name, parameter_of, ref_unix_seconds, Interval, Level, Parameter,
    RefTime, LEVEL_HEIGHT_ABOVE_GROUND, LEVEL_ISOBARIC, LEVEL_SURFACE,
};
pub use sample::{bilinear_ij, sample_bilinear, wind_at, Wind};

/// Limit: largest message accepted, 256 MiB. A full HRRR pressure-level file is about 130 MB
/// and a single field message a few MB, so this is ~50x headroom over one field.
pub const MAX_MESSAGE_BYTES: u64 = 256 * 1024 * 1024;
/// Limit: most grid points in one field, 16 Mi. HRRR is 1799 x 1059 = 1.9 M, GFS 0.25 degree
/// is 1.04 M, so ~8x headroom; a field is at most 64 MiB of f32.
pub const MAX_GRID_POINTS: u64 = 16 * 1024 * 1024;
/// Limit: most fields in one message. Real NCEP messages hold one; this stops a message that
/// repeats sections 4-7 without end from multiplying its decoded size.
pub const MAX_FIELDS_PER_MESSAGE: usize = 16;
/// Limit: grid points summed over all fields of one message, 64 Mi (256 MiB of f32 at most).
pub const MAX_MESSAGE_POINTS: u64 = 64 * 1024 * 1024;
