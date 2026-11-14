//! Typed refusals. Core (pure).

use std::fmt;

/// Why a simulation or an output conversion refused its input (E5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// A site or profile value is out of range; the name says which.
    BadSite(&'static str),
    /// A spoof parameter is out of range or inconsistent with the reports; the text says which.
    BadSpoof(&'static str),
    /// The reports' columns have different lengths.
    ColumnLength,
    /// The plot count would exceed [`crate::plots::MAX_PLOTS`].
    TooManyPlots,
    /// The plot table's columns have different lengths.
    PlotColumns,
    /// A value does not fit its ASTERIX element (name of the element).
    Encode(&'static str),
    /// The ASTERIX encoder refused a block.
    Asterix(String),
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::BadSite(w) => write!(f, "bad radar site or profile: {w}"),
            SimError::BadSpoof(w) => write!(f, "bad spoof parameter: {w}"),
            SimError::ColumnLength => write!(f, "report columns have different lengths"),
            SimError::TooManyPlots => write!(f, "plot count exceeds the limit"),
            SimError::PlotColumns => write!(f, "plot columns have different lengths"),
            SimError::Encode(w) => write!(f, "value does not fit the ASTERIX element {w}"),
            SimError::Asterix(s) => write!(f, "ASTERIX encoder: {s}"),
        }
    }
}

impl std::error::Error for SimError {}
