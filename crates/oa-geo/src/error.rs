//! Error type shared by every fallible operation in this crate.

use std::fmt;

/// Why an input was rejected or a computation could not be completed.
#[derive(Debug, Clone, PartialEq)]
pub enum GeoError {
    /// A value was NaN or infinite. The payload names the quantity.
    NonFinite(&'static str),
    /// Latitude outside [-90, 90] degrees.
    LatitudeOutOfRange(f64),
    /// Longitude outside [-180, 180] degrees.
    LongitudeOutOfRange(f64),
    /// Degrees field negative or beyond the axis limit.
    DegreesOutOfRange(f64),
    /// Minutes field outside [0, 60).
    MinutesOutOfRange(f64),
    /// Seconds field outside [0, 60).
    SecondsOutOfRange(f64),
    /// Hemisphere letter other than N, S, E or W.
    BadHemisphere(char),
    /// Ellipsoid parameters outside the supported range.
    BadEllipsoid(&'static str),
    /// A fraction outside [0, 1] or a sample count of zero.
    BadParameter(&'static str),
    /// A solver failed to converge (not expected for any valid input).
    NoConvergence(&'static str),
    /// The requested rhumb line reaches or passes a pole.
    RhumbPole,
    /// A polygon was rejected; the payload says why.
    BadPolygon(&'static str),
}

impl fmt::Display for GeoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeoError::NonFinite(what) => write!(f, "{what} is not a finite number"),
            GeoError::LatitudeOutOfRange(v) => write!(f, "latitude {v} is outside [-90, 90]"),
            GeoError::LongitudeOutOfRange(v) => write!(f, "longitude {v} is outside [-180, 180]"),
            GeoError::DegreesOutOfRange(v) => write!(f, "degrees {v} is out of range"),
            GeoError::MinutesOutOfRange(v) => write!(f, "minutes {v} is outside [0, 60)"),
            GeoError::SecondsOutOfRange(v) => write!(f, "seconds {v} is outside [0, 60)"),
            GeoError::BadHemisphere(c) => write!(f, "hemisphere {c:?} is not one of N, S, E, W"),
            GeoError::BadEllipsoid(why) => write!(f, "unsupported ellipsoid: {why}"),
            GeoError::BadParameter(why) => write!(f, "bad parameter: {why}"),
            GeoError::NoConvergence(what) => write!(f, "{what} did not converge"),
            GeoError::RhumbPole => write!(f, "rhumb line reaches or passes a pole"),
            GeoError::BadPolygon(why) => write!(f, "unsupported polygon: {why}"),
        }
    }
}

impl std::error::Error for GeoError {}
