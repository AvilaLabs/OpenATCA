//! The one error type of this crate. Every refusal names what was refused and where.

use std::fmt;

/// Why an input was refused. Nothing is ever passed on after a refusal (RULESET E5).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum Grib2Error {
    /// Input ended inside the named structure.
    Truncated(&'static str),
    /// The bytes at the given offset are not the "GRIB" indicator.
    NotGrib { offset: usize },
    /// Edition other than 2.
    UnsupportedEdition(u8),
    /// A message declares more bytes than [`crate::MAX_MESSAGE_BYTES`], or fewer than its own
    /// minimum.
    BadMessageLength(u64),
    /// The message does not end in "7777" where its length says it does.
    BadEndMarker,
    /// A section length is impossible (too short, or runs past the message).
    BadSectionLength { section: u8, length: u32 },
    /// A section appeared where the section order does not allow it.
    SectionOrder { section: u8 },
    /// Something required (the named section) never appeared.
    MissingSection(&'static str),
    /// More points than [`crate::MAX_GRID_POINTS`] (or a point count that does not match the grid).
    GridPoints(u64),
    /// More fields in one message than [`crate::MAX_FIELDS_PER_MESSAGE`].
    TooManyFields,
    /// Grid definition template other than 3.0 and 3.30.
    UnsupportedGridTemplate(u16),
    /// Scanning mode other than 0x00 (+i, -j) and 0x40 (+i, +j).
    UnsupportedScanMode(u8),
    /// Earth shape other than the spherical codes 0, 1, 6 and 8.
    UnsupportedEarthShape(u8),
    /// A grid definition value is impossible; the payload says which.
    BadGrid(&'static str),
    /// Product definition template other than 4.0 and 4.8.
    UnsupportedProductTemplate(u16),
    /// A product definition value is impossible.
    BadProduct(&'static str),
    /// Data representation template other than 5.0, 5.2, 5.3 and 5.40. Template 5.41 (PNG) and
    /// others are named so the report can say what the file used.
    UnsupportedPacking(u16),
    /// A data representation or data section value is impossible.
    BadPacking(&'static str),
    /// The JPEG 2000 codestream of a template 5.40 field was refused.
    Jpeg2000(oa_j2k::J2kError),
    /// A decoded value is not finite: the packed integer scaled by `R`, `E` and `D` leaves the
    /// `f32` range. The payload is the index of the first such point.
    NonFinite { index: u32 },
    /// A complex-packed field would need more scratch memory than its size section justifies
    /// (see [`crate::pack::check_scratch`]); `needed` and `budget` are in bytes.
    ScratchBudget { needed: u64, budget: u64 },
    /// A bitmap is impossible or disagrees with the point count.
    BadBitmap(&'static str),
    /// The two fields given to a wind query do not share one grid.
    GridMismatch,
    /// An `.idx` line was refused.
    BadIdx { line: usize, why: &'static str },
}

impl fmt::Display for Grib2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Grib2Error::Truncated(what) => write!(f, "input ends inside {what}"),
            Grib2Error::NotGrib { offset } => write!(f, "no GRIB indicator at byte {offset}"),
            Grib2Error::UnsupportedEdition(e) => write!(f, "GRIB edition {e} is not supported"),
            Grib2Error::BadMessageLength(n) => write!(f, "message length {n} is out of bounds"),
            Grib2Error::BadEndMarker => write!(f, "message does not end in 7777"),
            Grib2Error::BadSectionLength { section, length } => {
                write!(f, "section {section} has impossible length {length}")
            }
            Grib2Error::SectionOrder { section } => {
                write!(f, "section {section} is out of order")
            }
            Grib2Error::MissingSection(what) => write!(f, "missing {what}"),
            Grib2Error::GridPoints(n) => write!(f, "{n} grid points is out of bounds"),
            Grib2Error::TooManyFields => write!(f, "too many fields in one message"),
            Grib2Error::UnsupportedGridTemplate(t) => {
                write!(f, "grid template 3.{t} is not supported")
            }
            Grib2Error::UnsupportedScanMode(m) => {
                write!(f, "scanning mode {m:#04x} is not supported")
            }
            Grib2Error::UnsupportedEarthShape(s) => {
                write!(f, "earth shape code {s} is not supported")
            }
            Grib2Error::BadGrid(why) => write!(f, "bad grid definition: {why}"),
            Grib2Error::UnsupportedProductTemplate(t) => {
                write!(f, "product template 4.{t} is not supported")
            }
            Grib2Error::BadProduct(why) => write!(f, "bad product definition: {why}"),
            Grib2Error::UnsupportedPacking(t) => {
                write!(f, "data representation template 5.{t} is not supported")
            }
            Grib2Error::BadPacking(why) => write!(f, "bad data packing: {why}"),
            Grib2Error::Jpeg2000(e) => write!(f, "JPEG 2000 data: {e}"),
            Grib2Error::NonFinite { index } => {
                write!(f, "decoded value at point {index} is not finite")
            }
            Grib2Error::ScratchBudget { needed, budget } => {
                write!(
                    f,
                    "field needs {needed} bytes of scratch, budget is {budget}"
                )
            }
            Grib2Error::BadBitmap(why) => write!(f, "bad bitmap: {why}"),
            Grib2Error::GridMismatch => write!(f, "fields are on different grids"),
            Grib2Error::BadIdx { line, why } => write!(f, "idx line {line}: {why}"),
        }
    }
}

impl std::error::Error for Grib2Error {}
