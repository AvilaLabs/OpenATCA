//! Validated geographic coordinates and degrees/minutes/seconds conversion.

use crate::error::GeoError;

/// A position on the ellipsoid, in decimal degrees. Latitude is in [-90, 90] and longitude in
/// [-180, 180]; both are finite. The only way to build one is a validating constructor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    lat: f64,
    lon: f64,
}
// Budget: two f64, no padding. Held in quantity (every polygon vertex, every sample point).
// Raising it needs a recorded justification (A7.1).
const _: () = assert!(core::mem::size_of::<LatLon>() == 16);

impl LatLon {
    /// OA-GEO-001: rejects non-finite values and values outside the geographic ranges.
    pub fn new(lat_deg: f64, lon_deg: f64) -> Result<LatLon, GeoError> {
        if !lat_deg.is_finite() {
            return Err(GeoError::NonFinite("latitude"));
        }
        if !lon_deg.is_finite() {
            return Err(GeoError::NonFinite("longitude"));
        }
        if !(-90.0..=90.0).contains(&lat_deg) {
            return Err(GeoError::LatitudeOutOfRange(lat_deg));
        }
        if !(-180.0..=180.0).contains(&lon_deg) {
            return Err(GeoError::LongitudeOutOfRange(lon_deg));
        }
        Ok(LatLon {
            lat: lat_deg,
            lon: lon_deg,
        })
    }

    /// Builds a position from degrees/minutes/seconds/hemisphere for each axis.
    pub fn from_dms(lat: Dms, lon: Dms) -> Result<LatLon, GeoError> {
        let lat_deg = dms_to_decimal(lat.degrees, lat.minutes, lat.seconds, lat.hemisphere)?;
        let lon_deg = dms_to_decimal(lon.degrees, lon.minutes, lon.seconds, lon.hemisphere)?;
        LatLon::new(lat_deg, lon_deg)
    }

    /// Latitude in degrees, positive north.
    pub fn lat(&self) -> f64 {
        self.lat
    }

    /// Longitude in degrees, positive east.
    pub fn lon(&self) -> f64 {
        self.lon
    }
}

/// A hemisphere letter. N and E are positive, S and W negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hemisphere {
    North,
    South,
    East,
    West,
}

/// Parses `N`, `S`, `E` or `W` (upper or lower case).
pub fn hemisphere_from_char(c: char) -> Result<Hemisphere, GeoError> {
    match c {
        'N' | 'n' => Ok(Hemisphere::North),
        'S' | 's' => Ok(Hemisphere::South),
        'E' | 'e' => Ok(Hemisphere::East),
        'W' | 'w' => Ok(Hemisphere::West),
        other => Err(GeoError::BadHemisphere(other)),
    }
}

fn hemisphere_sign(h: Hemisphere) -> f64 {
    match h {
        Hemisphere::North | Hemisphere::East => 1.0,
        Hemisphere::South | Hemisphere::West => -1.0,
    }
}

/// Largest magnitude of the axis a hemisphere belongs to.
fn hemisphere_axis_limit(h: Hemisphere) -> f64 {
    match h {
        Hemisphere::North | Hemisphere::South => 90.0,
        Hemisphere::East | Hemisphere::West => 180.0,
    }
}

/// One coordinate in degrees, minutes, seconds and hemisphere, as the FAA publishes it.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Dms {
    pub degrees: f64,
    pub minutes: f64,
    pub seconds: f64,
    pub hemisphere: Hemisphere,
}

/// OA-GEO-002: converts to signed decimal degrees. Minutes and seconds must be in [0, 60);
/// degrees must be non-negative and within the axis limit (90 for N/S, 180 for E/W), and
/// the total must not exceed that limit. Non-finite values are rejected.
pub fn dms_to_decimal(
    degrees: f64,
    minutes: f64,
    seconds: f64,
    hemisphere: Hemisphere,
) -> Result<f64, GeoError> {
    if !degrees.is_finite() {
        return Err(GeoError::NonFinite("degrees"));
    }
    if !minutes.is_finite() {
        return Err(GeoError::NonFinite("minutes"));
    }
    if !seconds.is_finite() {
        return Err(GeoError::NonFinite("seconds"));
    }
    let limit = hemisphere_axis_limit(hemisphere);
    if !(0.0..=limit).contains(&degrees) {
        return Err(GeoError::DegreesOutOfRange(degrees));
    }
    if !(0.0..60.0).contains(&minutes) {
        return Err(GeoError::MinutesOutOfRange(minutes));
    }
    if !(0.0..60.0).contains(&seconds) {
        return Err(GeoError::SecondsOutOfRange(seconds));
    }
    let magnitude = degrees + minutes / 60.0 + seconds / 3600.0;
    if magnitude > limit {
        return Err(GeoError::DegreesOutOfRange(magnitude));
    }
    Ok(hemisphere_sign(hemisphere) * magnitude)
}

/// Convenience form of [`dms_to_decimal`] taking the hemisphere as a letter.
pub fn dms_char_to_decimal(
    degrees: f64,
    minutes: f64,
    seconds: f64,
    hemisphere: char,
) -> Result<f64, GeoError> {
    dms_to_decimal(degrees, minutes, seconds, hemisphere_from_char(hemisphere)?)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    // REQ: OA-GEO-001
    #[test]
    fn latlon_accepts_the_closed_ranges() {
        for (lat, lon) in [
            (0.0, 0.0),
            (90.0, 180.0),
            (-90.0, -180.0),
            (41.9786, -87.9048),
        ] {
            let p = LatLon::new(lat, lon).expect("valid");
            assert_eq!((p.lat(), p.lon()), (lat, lon));
        }
    }

    // REQ: OA-GEO-001
    #[test]
    fn latlon_rejects_out_of_range_and_non_finite() {
        assert_eq!(
            LatLon::new(90.000_000_1, 0.0),
            Err(GeoError::LatitudeOutOfRange(90.000_000_1))
        );
        assert!(matches!(
            LatLon::new(-91.0, 0.0),
            Err(GeoError::LatitudeOutOfRange(_))
        ));
        assert!(matches!(
            LatLon::new(0.0, 180.000_001),
            Err(GeoError::LongitudeOutOfRange(_))
        ));
        assert!(matches!(
            LatLon::new(0.0, -181.0),
            Err(GeoError::LongitudeOutOfRange(_))
        ));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(LatLon::new(bad, 0.0), Err(GeoError::NonFinite(_))));
            assert!(matches!(LatLon::new(0.0, bad), Err(GeoError::NonFinite(_))));
        }
    }

    // REQ: OA-GEO-002
    #[test]
    fn dms_converts_with_hemisphere_sign() {
        let v = dms_char_to_decimal(41.0, 58.0, 43.0, 'N').expect("valid");
        assert!((v - (41.0 + 58.0 / 60.0 + 43.0 / 3600.0)).abs() < 1e-12);
        let v = dms_char_to_decimal(87.0, 54.0, 17.0, 'W').expect("valid");
        assert!((v + (87.0 + 54.0 / 60.0 + 17.0 / 3600.0)).abs() < 1e-12);
        assert_eq!(dms_char_to_decimal(10.0, 30.0, 0.0, 'S'), Ok(-10.5));
        assert_eq!(dms_char_to_decimal(10.0, 30.0, 0.0, 'e'), Ok(10.5));
        assert_eq!(dms_char_to_decimal(90.0, 0.0, 0.0, 'N'), Ok(90.0));
        assert_eq!(dms_char_to_decimal(180.0, 0.0, 0.0, 'W'), Ok(-180.0));
    }

    // REQ: OA-GEO-002
    #[test]
    fn dms_rejects_minutes_and_seconds_outside_zero_to_sixty() {
        assert_eq!(
            dms_char_to_decimal(10.0, 60.0, 0.0, 'N'),
            Err(GeoError::MinutesOutOfRange(60.0))
        );
        assert_eq!(
            dms_char_to_decimal(10.0, 0.0, 60.0, 'N'),
            Err(GeoError::SecondsOutOfRange(60.0))
        );
        assert!(matches!(
            dms_char_to_decimal(10.0, -1.0, 0.0, 'N'),
            Err(GeoError::MinutesOutOfRange(_))
        ));
        assert!(matches!(
            dms_char_to_decimal(10.0, 0.0, -0.001, 'N'),
            Err(GeoError::SecondsOutOfRange(_))
        ));
        assert!(dms_char_to_decimal(10.0, 59.0, 59.999_999, 'N').is_ok());
    }

    // REQ: OA-GEO-002
    #[test]
    fn dms_rejects_bad_degrees_hemispheres_and_non_finite() {
        assert!(matches!(
            dms_char_to_decimal(91.0, 0.0, 0.0, 'N'),
            Err(GeoError::DegreesOutOfRange(_))
        ));
        assert!(matches!(
            dms_char_to_decimal(90.0, 0.0, 1.0, 'S'),
            Err(GeoError::DegreesOutOfRange(_))
        ));
        assert!(matches!(
            dms_char_to_decimal(180.0, 0.0, 1.0, 'E'),
            Err(GeoError::DegreesOutOfRange(_))
        ));
        assert!(matches!(
            dms_char_to_decimal(-1.0, 0.0, 0.0, 'E'),
            Err(GeoError::DegreesOutOfRange(_))
        ));
        assert_eq!(
            dms_char_to_decimal(1.0, 0.0, 0.0, 'X'),
            Err(GeoError::BadHemisphere('X'))
        );
        for bad in [f64::NAN, f64::INFINITY] {
            assert!(dms_char_to_decimal(bad, 0.0, 0.0, 'N').is_err());
            assert!(dms_char_to_decimal(1.0, bad, 0.0, 'N').is_err());
            assert!(dms_char_to_decimal(1.0, 0.0, bad, 'N').is_err());
        }
    }

    // REQ: OA-GEO-002
    #[test]
    fn latlon_from_dms_builds_a_position() {
        let lat = Dms {
            degrees: 41.0,
            minutes: 58.0,
            seconds: 43.0,
            hemisphere: Hemisphere::North,
        };
        let lon = Dms {
            degrees: 87.0,
            minutes: 54.0,
            seconds: 17.0,
            hemisphere: Hemisphere::West,
        };
        let p = LatLon::from_dms(lat, lon).expect("valid");
        assert!((p.lat() - 41.978_611_111).abs() < 1e-8);
        assert!((p.lon() + 87.904_722_222).abs() < 1e-8);
    }
}
