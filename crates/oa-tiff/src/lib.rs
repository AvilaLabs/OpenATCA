#![forbid(unsafe_code)]
#![deny(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
//! oa-tiff: a TIFF 6.0 / GeoTIFF 1.1 elevation reader and the terrain queries the minimum safe
//! altitude warning needs. Core (pure): every function takes bytes or a [`Grid`] and returns
//! data; [`read_file`] is the one shell function and only reads a file into memory.
//!
//! Scope, read from the specifications (Adobe TIFF 6.0; OGC GeoTIFF 1.1; the Cloud Optimized
//! GeoTIFF layout, which is plain TIFF with tiles and the full-resolution image first):
//!
//! * classic TIFF, little and big endian. BigTIFF is refused with [`TiffError::BigTiffUnsupported`].
//! * strips and tiles; compression none, Deflate (zlib, tags 8 and 32946) and LZW (the TIFF 6.0
//!   MSB-first variant with early change); predictor 1, 2 (horizontal differencing, 16-bit
//!   integers) and 3 (floating point, as in libtiff and Adobe Technical Note 3).
//! * one sample per pixel, 32-bit float or 16-bit signed integer.
//! * only the first IFD is read: a Cloud Optimized GeoTIFF stores the full-resolution image
//!   there and its overviews in the IFDs that follow, which are ignored.
//! * GeoTIFF: `ModelPixelScale` with one `ModelTiepoint`, the `GeoKeyDirectory`, geographic
//!   coordinates in NAD83 (4269) or WGS 84 (4326), `PixelIsArea` and `PixelIsPoint`, and
//!   `GDAL_NODATA`. A `ModelTransformation` (rotation or shear) is refused.
//!
//! Half-pixel convention: the tiepoint ties a raster position to a model position. For
//! `PixelIsArea` the raster point (0, 0) is the outer corner of the first pixel, so sample
//! `(col, row)` lies at raster `(col + 0.5, row + 0.5)`; for `PixelIsPoint` raster `(0, 0)` is the
//! sample itself. [`Grid`] always stores the position of the sample *centres*, so the two
//! registrations meet in one representation and queries never think about it again.
//!
//! Elevations are metres, as stored; 3DEP elevations are NAVD88 orthometric heights, which is
//! the reference the FAA's AMSL values use to within the geoid model. NAD83 and WGS 84 differ by
//! about a metre horizontally and are treated as the same datum.
//!
//! Every input is bounded by the constants below and refused with a typed error beyond them.

mod conservative;
mod geotiff;
mod grid;
mod lzw;
mod tiff;

// Test fixtures build files and lattices from loop counters, not parsed input.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests;
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests_conservative;
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod testutil;

pub use conservative::{
    grid_max_in_polygon_conservative, grid_max_in_radius_conservative,
    max_elevation_in_polygon_conservative, max_elevation_in_radius_conservative, TerrainMax,
};
pub use grid::{
    elevation_at, grid_elevation_at, grid_max_in_polygon, grid_max_in_radius, grid_sample,
    max_elevation_in_polygon, max_elevation_in_radius, tileset_covers, Elevation, Grid, MaxQuery,
    Peak, QueryError, TileSet,
};
pub use tiff::read_geotiff;

use std::fmt;
use std::path::Path;

/// Limit: largest file [`read_file`] and [`read_geotiff`] accept, 1 GiB. A 1 arc-second 3DEP tile
/// is about 15 to 55 MB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// Limit: most entries in the image file directory. Real files carry 15 to 30.
pub const MAX_IFD_ENTRIES: usize = 512;
/// Limit: most values in one tag (strip and tile offset arrays are the large ones). A 3612 by
/// 3612 image in 16-row strips is 226 values, in 1-row strips 3612.
pub const MAX_TAG_COUNT: u64 = 1 << 20;
/// Limit: most columns or rows. A 1 degree tile at 1/3 arc-second is 10,812.
pub const MAX_DIMENSION: u32 = 65_536;
/// Limit: most pixels in one image (64 Mi, a 256 MiB elevation array). 3DEP 1 arc-second is 13 M.
pub const MAX_PIXELS: u64 = 1 << 26;
/// Limit: largest uncompressed strip or tile, 64 MiB.
pub const MAX_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
/// Limit: most `GeoKeyDirectory` keys.
pub const MAX_GEO_KEYS: usize = 1024;
/// Limit: longest ASCII tag value (`GDAL_NODATA`, `GeoAsciiParams`), 64 KiB.
pub const MAX_ASCII_BYTES: u64 = 64 * 1024;
/// Limit: most tiles in a [`TileSet`]. The upper Midwest study area is about 90.
pub const MAX_TILES: usize = 1024;
/// Limit: most vertices in a query polygon.
pub const MAX_POLYGON_VERTICES: usize = 10_000;
/// Limit: largest query radius, nautical miles.
pub const MAX_RADIUS_NM: f64 = 500.0;
/// Limit: most samples one query may visit (a 500 nm radius at 1 arc-second is about 2.4e9, so
/// the radius limit is the practical one; this guards a polygon the size of a continent).
pub const MAX_QUERY_SAMPLES: u64 = 1 << 31;
/// Limit: most work one conservative query may do, counted as lattice cells times the larger of 1
/// and the polygon vertices, plus those cells times the tiles that reach the box, plus the tiles. The largest
/// query the tests and the real-data runs make is about 4e6 (a 5 nm radius on a 1/3 arc-second lattice); this allows over 500 times that.
pub const MAX_QUERY_WORK: u64 = 1 << 31;

/// Metres per foot, exact.
pub const METRES_PER_FOOT: f64 = 0.3048;
/// Metres per nautical mile, exact.
pub const METRES_PER_NM: f64 = 1852.0;

/// Reading a TIFF failed. Plain data; every variant names what was wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum TiffError {
    /// The file could not be opened or read (message of the I/O error).
    Io(String),
    /// The file is larger than [`MAX_FILE_BYTES`].
    FileTooLarge(u64),
    /// The first two bytes are neither `II` nor `MM`, or the magic number is not 42 or 43.
    BadHeader,
    /// BigTIFF (magic 43) is not read.
    BigTiffUnsupported,
    /// Input ended inside the named structure.
    Truncated(&'static str),
    /// An offset points outside the file or the count overflows.
    BadOffset(&'static str),
    /// More IFD entries than [`MAX_IFD_ENTRIES`].
    TooManyEntries(u64),
    /// The same tag appears twice in the IFD.
    DuplicateTag(u16),
    /// A tag has a data type the reader cannot take for that tag.
    BadTagType {
        /// The tag number.
        tag: u16,
        /// The TIFF data type code.
        typ: u16,
    },
    /// A tag holds more values than allowed.
    TagTooLarge {
        /// The tag number.
        tag: u16,
        /// The declared value count.
        count: u64,
    },
    /// A required tag is missing.
    MissingTag(&'static str),
    /// A size exceeds a stated limit.
    TooLarge {
        /// What was too large.
        what: &'static str,
        /// The value found.
        value: u64,
        /// The limit.
        limit: u64,
    },
    /// A valid TIFF feature this reader does not implement.
    Unsupported {
        /// The feature.
        what: &'static str,
        /// The value found.
        value: u32,
    },
    /// A value that is inconsistent or out of range.
    BadValue(&'static str),
    /// A strip or tile did not decompress to exactly the size its dimensions require.
    Decompress {
        /// Zero-based strip or tile index.
        segment: usize,
        /// What went wrong.
        reason: &'static str,
    },
    /// The GeoTIFF georeferencing is missing, unsupported or inconsistent.
    GeoRef(&'static str),
}

impl fmt::Display for TiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TiffError::Io(m) => write!(f, "i/o error: {m}"),
            TiffError::FileTooLarge(n) => {
                write!(f, "file is {n} bytes, over the {MAX_FILE_BYTES} limit")
            }
            TiffError::BadHeader => write!(f, "not a TIFF file (bad byte order mark or magic)"),
            TiffError::BigTiffUnsupported => write!(f, "BigTIFF is not supported"),
            TiffError::Truncated(w) => write!(f, "truncated inside {w}"),
            TiffError::BadOffset(w) => write!(f, "offset or size out of range in {w}"),
            TiffError::TooManyEntries(n) => {
                write!(f, "{n} IFD entries, over the {MAX_IFD_ENTRIES} limit")
            }
            TiffError::DuplicateTag(t) => write!(f, "tag {t} appears twice"),
            TiffError::BadTagType { tag, typ } => write!(f, "tag {tag} has unusable type {typ}"),
            TiffError::TagTooLarge { tag, count } => {
                write!(
                    f,
                    "tag {tag} has {count} values, over the {MAX_TAG_COUNT} limit"
                )
            }
            TiffError::MissingTag(t) => write!(f, "missing required tag {t}"),
            TiffError::TooLarge { what, value, limit } => {
                write!(f, "{what} is {value}, over the {limit} limit")
            }
            TiffError::Unsupported { what, value } => write!(f, "unsupported {what}: {value}"),
            TiffError::BadValue(w) => write!(f, "bad value: {w}"),
            TiffError::Decompress { segment, reason } => {
                write!(f, "strip or tile {segment}: {reason}")
            }
            TiffError::GeoRef(w) => write!(f, "georeferencing: {w}"),
        }
    }
}

impl std::error::Error for TiffError {}

/// Shell: reads a whole file into memory, refusing one over [`MAX_FILE_BYTES`].
pub fn read_file(path: &Path) -> Result<Vec<u8>, TiffError> {
    let meta = std::fs::metadata(path).map_err(|e| TiffError::Io(e.to_string()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(TiffError::FileTooLarge(meta.len()));
    }
    std::fs::read(path).map_err(|e| TiffError::Io(e.to_string()))
}
