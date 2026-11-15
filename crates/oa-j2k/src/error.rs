//! The one error type of this crate. Every refusal names what was refused.

use std::fmt;

/// Why a codestream was refused. Nothing is ever passed on after a refusal (RULESET E5).
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum J2kError {
    /// Input ended inside the named structure.
    Truncated(&'static str),
    /// The input does not start with the SOC marker (0xFF4F).
    NotCodestream,
    /// A marker segment this decoder does not handle (POC, PPM, PPT, RGN, ...), by code.
    UnsupportedMarker(u16),
    /// A required marker segment never appeared.
    MissingMarker(&'static str),
    /// A marker segment is malformed or its values are impossible.
    BadMarker(&'static str),
    /// More samples or a longer side than [`crate::MAX_SAMPLES`] / [`crate::MAX_DIM`].
    ImageTooLarge { width: u32, height: u32 },
    /// Bit depth outside 1 to [`crate::MAX_BITS`].
    BadBitDepth(u8),
    /// A feature outside the supported profile (several tiles or components, subsampling,
    /// irreversible wavelet, precincts, SOP/EPH markers, ...); the payload says which.
    Unsupported(&'static str),
    /// Progression order other than 0 (LRCP).
    UnsupportedProgression(u8),
    /// Code-block style other than 0 (no bypass, reset, termination, vertically causal,
    /// predictable termination or segmentation symbols).
    UnsupportedBlockStyle(u8),
    /// Quantization style other than 0 (reversible, no quantization).
    UnsupportedQuantization(u8),
    /// More code blocks than [`crate::MAX_CODE_BLOCKS`].
    TooManyCodeBlocks(u64),
    /// A packet header or body is inconsistent with the codestream.
    BadPacket(&'static str),
    /// A code block's arithmetic-coded data or pass count is impossible.
    BadBlock(&'static str),
    /// The coding passes asked for exceed the work budget ([`crate::work_budget`]).
    WorkBudget { work: u64, budget: u64 },
    /// Bytes remain where the codestream should have ended.
    TrailingBytes(usize),
}

impl fmt::Display for J2kError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            J2kError::Truncated(what) => write!(f, "codestream ends inside {what}"),
            J2kError::NotCodestream => write!(f, "no SOC marker at the start"),
            J2kError::UnsupportedMarker(m) => write!(f, "marker {m:#06x} is not supported"),
            J2kError::MissingMarker(what) => write!(f, "missing {what} marker"),
            J2kError::BadMarker(why) => write!(f, "bad marker segment: {why}"),
            J2kError::ImageTooLarge { width, height } => {
                write!(f, "image of {width} x {height} is out of bounds")
            }
            J2kError::BadBitDepth(b) => write!(f, "bit depth {b} is out of bounds"),
            J2kError::Unsupported(what) => write!(f, "{what} is not supported"),
            J2kError::UnsupportedProgression(p) => {
                write!(f, "progression order {p} is not supported")
            }
            J2kError::UnsupportedBlockStyle(s) => {
                write!(f, "code-block style {s:#04x} is not supported")
            }
            J2kError::UnsupportedQuantization(s) => {
                write!(f, "quantization style {s} is not supported")
            }
            J2kError::TooManyCodeBlocks(n) => write!(f, "{n} code blocks is out of bounds"),
            J2kError::BadPacket(why) => write!(f, "bad packet: {why}"),
            J2kError::BadBlock(why) => write!(f, "bad code block: {why}"),
            J2kError::WorkBudget { work, budget } => {
                write!(
                    f,
                    "tier-1 work of {work} sample visits exceeds the budget {budget}"
                )
            }
            J2kError::TrailingBytes(n) => write!(f, "{n} bytes after the end of the codestream"),
        }
    }
}

impl std::error::Error for J2kError {}
