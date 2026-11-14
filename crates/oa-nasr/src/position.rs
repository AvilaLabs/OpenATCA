//! Positions computed from degrees/minutes/seconds and compared with the published decimal.

/// A position as published by NASR, with our own decimal computation next to it.
///
/// `lat` and `lon` are computed from the degree/minute/second/hemisphere fields (north and east
/// positive). `published_lat` and `published_lon` are the file's decimal columns. The `*_diff`
/// fields are the absolute differences in degrees. The FAA publishes 8 decimal places and
/// appears to truncate rather than round, so a healthy record differs by less than `1e-8`; a
/// difference beyond that is a data defect for the airspace model to report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    /// Latitude computed from DMS, degrees.
    pub lat: f64,
    /// Longitude computed from DMS, degrees.
    pub lon: f64,
    /// Published decimal latitude, degrees.
    pub published_lat: f64,
    /// Published decimal longitude, degrees.
    pub published_lon: f64,
    /// `|lat - published_lat|`, degrees.
    pub lat_diff: f64,
    /// `|lon - published_lon|`, degrees.
    pub lon_diff: f64,
}

/// Builds a position from computed and published values, filling in the differences.
pub fn from_parts(lat: f64, lon: f64, published_lat: f64, published_lon: f64) -> Position {
    Position {
        lat,
        lon,
        published_lat,
        published_lon,
        lat_diff: (lat - published_lat).abs(),
        lon_diff: (lon - published_lon).abs(),
    }
}

/// The larger of a position's latitude and longitude differences, degrees.
pub fn max_diff(p: &Position) -> f64 {
    p.lat_diff.max(p.lon_diff)
}

/// Converts degrees, minutes, seconds and a hemisphere letter to signed decimal degrees.
///
/// `is_lat` selects the allowed letters (`N`/`S` or `E`/`W`) and the degree range (0..=90 or
/// 0..=180). Minutes and seconds must be in `[0, 60)`; at 90 (or 180) degrees they must be zero.
/// Returns a description of the problem otherwise.
pub fn dms_to_decimal(
    deg: f64,
    min: f64,
    sec: f64,
    hemis: &str,
    is_lat: bool,
) -> Result<f64, String> {
    let (max_deg, pos, neg) = if is_lat {
        (90.0, "N", "S")
    } else {
        (180.0, "E", "W")
    };
    if !(deg.is_finite() && min.is_finite() && sec.is_finite()) {
        return Err("non-finite component".to_string());
    }
    if !(0.0..=max_deg).contains(&deg) {
        return Err(format!("degrees {deg} outside 0..={max_deg}"));
    }
    if !(0.0..60.0).contains(&min) {
        return Err(format!("minutes {min} outside [0, 60)"));
    }
    if !(0.0..60.0).contains(&sec) {
        return Err(format!("seconds {sec} outside [0, 60)"));
    }
    if deg == max_deg && (min != 0.0 || sec != 0.0) {
        return Err(format!("{deg} degrees with non-zero minutes or seconds"));
    }
    let sign = if hemis == pos {
        1.0
    } else if hemis == neg {
        -1.0
    } else {
        return Err(format!("hemisphere {hemis:?} is not {pos} or {neg}"));
    };
    Ok(sign * (deg + min / 60.0 + sec / 3600.0))
}
