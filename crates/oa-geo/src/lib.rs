#![forbid(unsafe_code)]
//! oa-geo: WGS-84 geodesy for OpenATCA (see docs/M0_DESIGN.md).
//!
//! * [`LatLon`], [`Dms`]: validated coordinates and degrees/minutes/seconds conversion.
//! * [`Ellipsoid`]: geodesic inverse and direct problems (Karney 2013), rhumb lines, the
//!   geodesic-versus-rhumb separation, and point-in-polygon on the ellipsoid via [`Polygon`].
//!
//! * [`aeqd`]: the local east-north frame (spherical azimuthal equidistant projection about an
//!   origin) used by the truth builder and the tracker; [`small`] has the small-angle series it
//!   uses so its common case needs no library call.
//!
//! No dependencies; no operation panics on any input (invalid input is an error).

pub mod aeqd;
mod coef;
mod error;
mod geodesic;
mod latlon;
mod polygon;
mod rhumb;
pub mod small;

pub use error::GeoError;
pub use geodesic::{wgs84, Direct, Ellipsoid, Inverse};
pub use latlon::{
    dms_char_to_decimal, dms_to_decimal, hemisphere_from_char, Dms, Hemisphere, LatLon,
};
pub use polygon::Polygon;
pub use rhumb::{RhumbInverse, Separation};
