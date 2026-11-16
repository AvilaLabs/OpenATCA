//! GeoTIFF 1.1 georeferencing: model pixel scale, one tiepoint, the key directory. Core (pure).
//!
//! Only geographic (latitude/longitude) rasters with axis-aligned pixels are accepted. The
//! result is the position of the *centre* of sample (0, 0) and the sample spacing, in degrees.

use crate::tiff::parse_f64_finite;
use crate::TiffError;

const KEY_MODEL_TYPE: u64 = 1024;
const KEY_RASTER_TYPE: u64 = 1025;
const KEY_GEOGRAPHIC_TYPE: u64 = 2048;
const KEY_VERTICAL_CS: u64 = 4096;
const KEY_VERTICAL_UNITS: u64 = 4099;

const MODEL_TYPE_GEOGRAPHIC: u64 = 2;
const RASTER_PIXEL_IS_AREA: u64 = 1;
const RASTER_PIXEL_IS_POINT: u64 = 2;
/// NAD83 and WGS 84 geographic coordinate systems (EPSG codes).
const CRS_NAD83: u64 = 4269;
const CRS_WGS84: u64 = 4326;
/// EPSG unit codes of VerticalUnitsGeoKey: metre, international foot, US survey foot.
const UNIT_METRE: u64 = 9001;
const UNIT_FOOT: u64 = 9002;
const UNIT_US_SURVEY_FOOT: u64 = 9003;

/// The raw georeferencing tags of an IFD. Plain data (A1).
// A7.2: cold struct, size guard waived
pub struct GeoTags {
    /// `ModelPixelScaleTag` (33550): (sx, sy, sz).
    pub pixel_scale: Option<Vec<f64>>,
    /// `ModelTiepointTag` (33922): groups of (i, j, k, x, y, z).
    pub tiepoints: Option<Vec<f64>>,
    /// `ModelTransformationTag` (34264) is present.
    pub has_transformation: bool,
    /// `GeoKeyDirectoryTag` (34735) as unsigned shorts.
    pub geo_keys: Option<Vec<u64>>,
    /// `GeoDoubleParamsTag` (34736) is present (only needed by keys this reader refuses).
    pub has_geo_doubles: bool,
    /// `GeoAsciiParamsTag` (34737) is present.
    pub has_geo_ascii: bool,
    /// `GDAL_NODATA` (42113) text.
    pub nodata: Option<String>,
}

/// The decoded georeferencing. Plain data (A1).
// A7.2: cold struct, size guard waived
pub struct Geo {
    /// Longitude of the centre of sample column 0, degrees.
    pub lon0: f64,
    /// Latitude of the centre of sample row 0 (the top row), degrees.
    pub lat0: f64,
    /// Longitude step per column, degrees, positive.
    pub dlon: f64,
    /// Latitude step per row (rows run south), degrees, positive.
    pub dlat: f64,
    /// Declared nodata value.
    pub nodata: Option<f64>,
    /// The raster was `PixelIsPoint`.
    pub point_registered: bool,
    /// EPSG geographic code, 4269 or 4326.
    pub crs: u16,
    /// EPSG vertical CS key when present (3DEP: 5703, NAVD88 height).
    pub vertical_cs: Option<u16>,
    /// Metres per vertical unit of the samples: 1 for metres or no unit key, 0.3048 for feet,
    /// 1200/3937 for US survey feet.
    pub vertical_unit_m: f64,
}

/// Interprets the tags for an image of `width` by `height` pixels.
pub fn georeference(t: &GeoTags, width: u32, height: u32) -> Result<Geo, TiffError> {
    let _ = (t.has_geo_doubles, t.has_geo_ascii);
    if t.has_transformation {
        return Err(TiffError::GeoRef(
            "ModelTransformation (rotated or sheared) is not supported",
        ));
    }
    let scale = t
        .pixel_scale
        .as_deref()
        .ok_or(TiffError::GeoRef("ModelPixelScale missing"))?;
    let (sx, sy) = match scale {
        [sx, sy, _] => (*sx, *sy),
        _ => return Err(TiffError::GeoRef("ModelPixelScale must hold 3 values")),
    };
    if !(sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0) {
        return Err(TiffError::GeoRef("pixel scale must be finite and positive"));
    }
    let tie = t
        .tiepoints
        .as_deref()
        .ok_or(TiffError::GeoRef("ModelTiepoint missing"))?;
    let (ti, tj, tx, ty) = match tie {
        [i, j, _k, x, y, _z] => (*i, *j, *x, *y),
        _ => return Err(TiffError::GeoRef("exactly one tiepoint is required")),
    };
    if ![ti, tj, tx, ty].iter().all(|v| v.is_finite()) {
        return Err(TiffError::GeoRef("tiepoint is not finite"));
    }
    let keys = t
        .geo_keys
        .as_deref()
        .ok_or(TiffError::GeoRef("GeoKeyDirectory missing"))?;
    let nkeys = match keys {
        [_ver, _rev, _min, n, ..] => usize::try_from(*n).unwrap_or(usize::MAX),
        _ => return Err(TiffError::GeoRef("GeoKeyDirectory header truncated")),
    };
    if nkeys > crate::MAX_GEO_KEYS {
        return Err(TiffError::TooLarge {
            what: "GeoKey count",
            value: nkeys as u64,
            limit: crate::MAX_GEO_KEYS as u64,
        });
    }
    // nkeys is at most MAX_GEO_KEYS here, so the key table length cannot overflow.
    let table = nkeys.saturating_mul(4).saturating_add(4);
    if keys.len() < table {
        return Err(TiffError::GeoRef(
            "GeoKeyDirectory shorter than its key count",
        ));
    }
    let short_key = |id: u64| -> Result<Option<u64>, TiffError> {
        for k in keys.get(4..table).unwrap_or(&[]).as_chunks::<4>().0.iter() {
            let [kid, loc, cnt, val] = *k;
            if kid == id {
                if loc != 0 || cnt != 1 {
                    return Err(TiffError::GeoRef(
                        "a key this reader needs is not an inline short",
                    ));
                }
                return Ok(Some(val));
            }
        }
        Ok(None)
    };
    match short_key(KEY_MODEL_TYPE)? {
        Some(MODEL_TYPE_GEOGRAPHIC) => {}
        Some(_) => return Err(TiffError::GeoRef("raster is not in geographic coordinates")),
        None => return Err(TiffError::GeoRef("GTModelType key missing")),
    }
    // GeoTIFF 1.1: the default raster type is PixelIsArea.
    let point = match short_key(KEY_RASTER_TYPE)? {
        Some(RASTER_PIXEL_IS_AREA) | None => false,
        Some(RASTER_PIXEL_IS_POINT) => true,
        Some(_) => return Err(TiffError::GeoRef("unknown GTRasterType")),
    };
    let crs = match short_key(KEY_GEOGRAPHIC_TYPE)? {
        Some(c @ (CRS_NAD83 | CRS_WGS84)) => u16::try_from(c).unwrap_or(u16::MAX),
        Some(_) => {
            return Err(TiffError::GeoRef(
                "geographic CRS is not NAD83 (4269) or WGS 84 (4326)",
            ))
        }
        None => return Err(TiffError::GeoRef("GeographicType key missing")),
    };
    let vertical_cs = short_key(KEY_VERTICAL_CS)?.and_then(|v| u16::try_from(v).ok());
    // GeoTIFF 1.1: the default vertical unit is the metre.
    let vertical_unit_m = match short_key(KEY_VERTICAL_UNITS)? {
        Some(UNIT_METRE) | None => 1.0,
        Some(UNIT_FOOT) => crate::METRES_PER_FOOT,
        Some(UNIT_US_SURVEY_FOOT) => 1200.0 / 3937.0,
        Some(_) => {
            return Err(TiffError::GeoRef(
                "vertical unit is not metre, foot or US survey foot",
            ))
        }
    };

    // Raster (i, j) maps to model (x, y) = (tx + (i - ti) * sx, ty - (j - tj) * sy). Sample
    // (col, row) is at raster (col + h, row + h), with h = 0.5 for PixelIsArea (raster integer
    // coordinates are pixel corners) and h = 0 for PixelIsPoint (they are the samples).
    let h = if point { 0.0 } else { 0.5 };
    let lon0 = tx + (h - ti) * sx;
    let lat0 = ty - (h - tj) * sy;
    let lon_last = lon0 + f64::from(width.saturating_sub(1)) * sx;
    let lat_last = lat0 - f64::from(height.saturating_sub(1)) * sy;
    if !(lon0 >= -180.0 && lon_last <= 180.0 && lat0 <= 90.0 && lat_last >= -90.0) {
        return Err(TiffError::GeoRef(
            "raster extent is outside the valid latitude/longitude range",
        ));
    }
    let nodata = match t.nodata.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "nan" => Some(f64::NAN),
            _ => Some(
                parse_f64_finite(s)
                    .ok_or(TiffError::GeoRef("GDAL_NODATA is not a finite number"))?,
            ),
        },
    };
    Ok(Geo {
        lon0,
        lat0,
        dlon: sx,
        dlat: sy,
        nodata,
        point_registered: point,
        crs,
        vertical_cs,
        vertical_unit_m,
    })
}
