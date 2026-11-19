//! [`Refusal`]: why a query cannot be answered without extrapolating or guessing. Core (pure,
//! B1, B7): a plain value with a `Display`. The errors that wrap a GRIB2 decoder error
//! (`LoadError`, `QueryError`) are shell (`shell::error`). Nothing is ever passed on after a
//! refusal (RULESET E5).

use std::fmt;

/// Why a query was refused. The aircraft is never given a number the model does not support.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub enum Refusal {
    /// A query value is not finite or is outside its definition (latitude beyond a pole).
    BadInput(&'static str),
    /// The pressure altitude is outside the range the ISA formulas here cover
    /// (-5000 m to 20 000 m geopotential).
    AltitudeOutsideIsa { alt_m: f64 },
    /// The position is outside the grid.
    OutsideGrid,
    /// The pressure is higher than the lowest level (below the level range), hPa.
    BelowLevels { pressure_hpa: f64, limit_hpa: f64 },
    /// The pressure is lower than the highest level (above the level range), hPa.
    AboveLevels { pressure_hpa: f64, limit_hpa: f64 },
    /// The time is before the first or after the last analysis hour (Unix seconds).
    OutsideTime {
        time_s: f64,
        first_s: i64,
        last_s: i64,
    },
    /// No analysis hour is available.
    NoHours,
    /// A value needed at a grid corner is missing in the file.
    MissingValue,
    /// A grid corner holds an infinite value, or the interpolated answer would not be finite.
    NonFinite,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::BadInput(w) => write!(f, "bad input: {w}"),
            Refusal::AltitudeOutsideIsa { alt_m } => {
                write!(f, "pressure altitude {alt_m:.0} m outside the ISA range")
            }
            Refusal::OutsideGrid => write!(f, "position outside the grid"),
            Refusal::BelowLevels {
                pressure_hpa,
                limit_hpa,
            } => write!(
                f,
                "{pressure_hpa:.1} hPa is below the lowest level {limit_hpa:.1} hPa"
            ),
            Refusal::AboveLevels {
                pressure_hpa,
                limit_hpa,
            } => write!(
                f,
                "{pressure_hpa:.1} hPa is above the highest level {limit_hpa:.1} hPa"
            ),
            Refusal::OutsideTime {
                time_s,
                first_s,
                last_s,
            } => write!(f, "time {time_s:.0} outside {first_s}..{last_s}"),
            Refusal::NoHours => write!(f, "no analysis hours"),
            Refusal::MissingValue => write!(f, "missing value at a grid corner"),
            Refusal::NonFinite => write!(f, "a value in the answer is not finite"),
        }
    }
}

impl std::error::Error for Refusal {}
