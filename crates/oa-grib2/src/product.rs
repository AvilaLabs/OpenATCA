//! Product definition (section 4): what a field is, at what level, for what time. Core (pure).
//!
//! Templates 4.0 (analysis or forecast at a point in time) and 4.8 (statistical interval) are
//! read, and so are their ensemble forms 4.1 and 4.11 (GEFS); all four share the same first 34
//! octets, which is all that is kept. What follows (ensemble member, interval description) is
//! checked for length and ignored.

use crate::bits::{be, sign_mag, u16_at, u8_at};
use crate::error::Grib2Error;

/// Code table 4.5 type of fixed surface: isobaric surface, value in Pa.
pub const LEVEL_ISOBARIC: u8 = 100;
/// Code table 4.5: specified height above ground, value in m.
pub const LEVEL_HEIGHT_ABOVE_GROUND: u8 = 103;
/// Code table 4.5: ground or water surface.
pub const LEVEL_SURFACE: u8 = 1;
/// Code table 4.5: missing surface (octet value 255).
pub const LEVEL_NONE: u8 = 255;

/// Parameters this crate names. Everything else is [`Parameter::Other`], and the raw
/// discipline, category and number stay on the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum Parameter {
    /// 0-0-0 temperature, K.
    Tmp,
    /// 0-1-1 relative humidity, %.
    Rh,
    /// 0-2-2 u-component of wind, m/s.
    Ugrd,
    /// 0-2-3 v-component of wind, m/s.
    Vgrd,
    /// 0-2-8 vertical velocity (pressure), Pa/s.
    Vvel,
    /// 0-3-5 geopotential height, gpm.
    Hgt,
    /// 0-3-0 pressure, Pa.
    Pres,
    Other,
}

/// Name a (discipline, category, number) triple. Only discipline 0 (meteorology) is named.
pub fn parameter_of(discipline: u8, category: u8, number: u8) -> Parameter {
    match (discipline, category, number) {
        (0, 0, 0) => Parameter::Tmp,
        (0, 1, 1) => Parameter::Rh,
        (0, 2, 2) => Parameter::Ugrd,
        (0, 2, 3) => Parameter::Vgrd,
        (0, 2, 8) => Parameter::Vvel,
        (0, 3, 5) => Parameter::Hgt,
        (0, 3, 0) => Parameter::Pres,
        _ => Parameter::Other,
    }
}

/// The wgrib2 / `.idx` name of a parameter, `None` for [`Parameter::Other`].
pub fn parameter_name(p: Parameter) -> Option<&'static str> {
    match p {
        Parameter::Tmp => Some("TMP"),
        Parameter::Rh => Some("RH"),
        Parameter::Ugrd => Some("UGRD"),
        Parameter::Vgrd => Some("VGRD"),
        Parameter::Vvel => Some("VVEL"),
        Parameter::Hgt => Some("HGT"),
        Parameter::Pres => Some("PRES"),
        Parameter::Other => None,
    }
}

/// A fixed surface: its code (table 4.5) and its value in the code's unit (Pa for isobaric,
/// m for height above ground), already scaled.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Level {
    pub kind: u8,
    pub value: f32,
}

/// Reference time of the data (section 1), as written, validated for calendar ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RefTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// What section 4 says, before the data are attached.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Product {
    pub template: u16,
    pub category: u8,
    pub number: u8,
    /// Code table 4.4 unit of the forecast time.
    pub time_unit: u8,
    pub forecast_time: u32,
    pub level: Level,
}

/// Octets of template 4.0 including the 9-octet section header.
const T40_LEN: u32 = 34;
/// Octets of template 4.8 up to and including one time range (octets 35-58).
const T48_MIN_LEN: u32 = 58;
/// Template 4.1 adds 3 octets (ensemble type, member, member count) after octet 34.
const T41_MIN_LEN: u32 = 37;
/// Template 4.11 is 4.1 followed by the interval description of 4.8.
const T411_MIN_LEN: u32 = 61;

/// Reads a section 4 (`sect` is the whole section including its length and number octets).
pub fn parse_product(sect: &[u8]) -> Result<Product, Grib2Error> {
    let t = Grib2Error::Truncated("product definition");
    let nv = u16_at(sect, 5).ok_or_else(|| t.clone())?;
    if nv != 0 {
        return Err(Grib2Error::BadProduct(
            "vertical coordinate list not supported",
        ));
    }
    let template = u16_at(sect, 7).ok_or_else(|| t.clone())?;
    let min = match template {
        0 => T40_LEN,
        8 => T48_MIN_LEN,
        1 => T41_MIN_LEN,
        11 => T411_MIN_LEN,
        other => return Err(Grib2Error::UnsupportedProductTemplate(other)),
    };
    if (sect.len() as u64) < u64::from(min) {
        return Err(Grib2Error::BadSectionLength {
            section: 4,
            length: sect.len() as u32,
        });
    }
    let category = u8_at(sect, 9).ok_or_else(|| t.clone())?;
    let number = u8_at(sect, 10).ok_or_else(|| t.clone())?;
    let time_unit = u8_at(sect, 17).ok_or_else(|| t.clone())?;
    let forecast_time = be(sect, 18, 4).ok_or_else(|| t.clone())? as u32;
    let kind = u8_at(sect, 22).ok_or_else(|| t.clone())?;
    let level = if kind == LEVEL_NONE {
        Level { kind, value: 0.0 }
    } else {
        let scale_raw = u8_at(sect, 23).ok_or_else(|| t.clone())?;
        let scale = sign_mag(sect, 23, 1).ok_or_else(|| t.clone())?;
        let scaled_raw = be(sect, 24, 4).ok_or_else(|| t.clone())?;
        if scale_raw == 0xFF && scaled_raw == 0xFFFF_FFFF {
            // Both octets all ones is "missing": a surface that has no value (ground, MSL).
            Level { kind, value: 0.0 }
        } else {
            // Scale factor is a power of ten: value = scaled * 10^-scale.
            let v = (scaled_raw as f64 * 10f64.powi(-(scale as i32))) as f32;
            if !v.is_finite() {
                return Err(Grib2Error::BadProduct("level value is not finite"));
            }
            Level { kind, value: v }
        }
    };
    Ok(Product {
        template,
        category,
        number,
        time_unit,
        forecast_time,
        level,
    })
}

/// Reads section 1 (whole section) into a discipline-free reference time.
pub fn parse_ref_time(sect: &[u8]) -> Result<RefTime, Grib2Error> {
    let t = Grib2Error::Truncated("identification section");
    if sect.len() < 21 {
        return Err(Grib2Error::BadSectionLength {
            section: 1,
            length: sect.len() as u32,
        });
    }
    let year = u16_at(sect, 12).ok_or_else(|| t.clone())?;
    let month = u8_at(sect, 14).ok_or_else(|| t.clone())?;
    let day = u8_at(sect, 15).ok_or_else(|| t.clone())?;
    let hour = u8_at(sect, 16).ok_or_else(|| t.clone())?;
    let minute = u8_at(sect, 17).ok_or_else(|| t.clone())?;
    let second = u8_at(sect, 18).ok_or_else(|| t.clone())?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(Grib2Error::BadProduct(
            "reference time is not a calendar time",
        ));
    }
    Ok(RefTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

/// Reference time as Unix seconds (proleptic Gregorian, no leap seconds).
pub fn ref_unix_seconds(t: &RefTime) -> i64 {
    let y = i64::from(t.year) - i64::from(t.month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(t.month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(t.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.minute) * 60 + i64::from(t.second)
}

/// Forecast lead in seconds for the time units used by NCEP (code table 4.4: minute, hour, day,
/// 3, 6 and 12 hours, second); `None` for any other unit.
pub fn forecast_seconds(unit: u8, value: u32) -> Option<i64> {
    let per: i64 = match unit {
        0 => 60,
        1 => 3600,
        2 => 86_400,
        10 => 3 * 3600,
        11 => 6 * 3600,
        12 => 12 * 3600,
        13 => 1,
        _ => return None,
    };
    Some(i64::from(value) * per)
}
