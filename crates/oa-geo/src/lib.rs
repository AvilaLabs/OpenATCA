#![forbid(unsafe_code)]
#![cfg_attr(not(test), no_std)]
//! oa-geo: WGS-84 geodesy for OpenATCA (see docs/M0_DESIGN.md). Class: core (RULESET B1, B7).
//!
//! * [`LatLon`], [`Dms`]: validated coordinates and degrees/minutes/seconds conversion.
//! * [`Ellipsoid`]: geodesic inverse and direct problems (Karney 2013), rhumb lines, the
//!   geodesic-versus-rhumb separation, and point-in-polygon on the ellipsoid via [`Polygon`].
//!
//! * [`aeqd`]: the local east-north frame (spherical azimuthal equidistant projection about an
//!   origin) used by the truth builder and the tracker; [`small`] has the small-angle series it
//!   uses so its common case needs no library call.
//!
//! Depends only on the core crates `oa-math` and `oa-ieee`; no operation panics on any input
//! (invalid input is an error).

// The width of `usize` is fixed by construction: this crate does not build for a target whose
// pointers are not 64 bits (RULESET B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("the core crates support 64-bit targets only, so that usize is 64 bits everywhere");

extern crate alloc;

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
