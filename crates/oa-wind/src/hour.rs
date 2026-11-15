//! One analysis hour as struct-of-arrays grids, and how it is built from decoded fields or from
//! the bytes of a file. Core (pure): bytes and fields in, plain data out.
//!
//! Layout: every array is level-major. The value of level `k` at column `i`, row `j` is at
//! `k * nx * ny + j * nx + i`, with `(i, j)` in the grid's scan order (the order
//! `oa_grib2::latlon_to_ij` returns). Levels are in ascending pressure (top of the atmosphere
//! first). U and V are as the file stores them (grid-relative on RAP); see the crate docs.

use crate::error::LoadError;
use crate::{MAX_HOUR_VALUES, MAX_LEVELS};
use oa_grib2::{
    byte_range, decode_message, forecast_seconds, grid_dims, inspect_stream, parse_idx,
    ref_unix_seconds, scan_messages, Field, Grid, Idx, Parameter, LEVEL_ISOBARIC,
};
use std::collections::BTreeMap;

/// One analysis hour: U, V, T and geopotential height on isobaric levels.
// A7.2: cold struct (one per loaded hour; the columns are the hot data), size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct Hour {
    /// Valid time, Unix seconds (reference time plus forecast lead).
    pub valid_s: i64,
    pub grid: Grid,
    pub nx: u32,
    pub ny: u32,
    /// Level pressures in Pa, strictly ascending.
    pub levels_pa: Vec<f32>,
    /// Grid-relative or east component per the grid's flags, m/s. NaN where the file has none.
    pub u: Vec<f32>,
    pub v: Vec<f32>,
    /// Temperature, K.
    pub t: Vec<f32>,
    /// Geopotential height, gpm.
    pub z: Vec<f32>,
}

/// Number of levels of an hour.
pub fn hour_levels(h: &Hour) -> usize {
    h.levels_pa.len()
}

/// Bytes held by an hour's four arrays: `levels x points x 4 fields x 4 bytes`.
pub fn hour_bytes(h: &Hour) -> u64 {
    (h.u.len() + h.v.len() + h.t.len() + h.z.len()) as u64 * 4
}

const NAMES: [&str; 4] = ["UGRD", "VGRD", "TMP", "HGT"];

fn slot(p: Parameter) -> Option<usize> {
    match p {
        Parameter::Ugrd => Some(0),
        Parameter::Vgrd => Some(1),
        Parameter::Tmp => Some(2),
        Parameter::Hgt => Some(3),
        _ => None,
    }
}

/// Builds an hour from decoded fields. Takes the fields by value so each field's values are
/// freed as soon as they are copied into the level-major arrays.
///
/// Only isobaric UGRD, VGRD, TMP and HGT fields are used; others are ignored. Refuses: a level
/// that lacks one of the four, a parameter twice on a level, fields on different grids, point
/// counts or valid times, fewer than two levels, more than [`MAX_LEVELS`], a grid under 2 x 2,
/// and an hour over [`MAX_HOUR_VALUES`] values.
pub fn build_hour(fields: Vec<Field>) -> Result<Hour, LoadError> {
    let mut by_level: BTreeMap<u32, [Option<Field>; 4]> = BTreeMap::new();
    for f in fields {
        let Some(s) = slot(f.parameter) else { continue };
        if f.level.kind != LEVEL_ISOBARIC {
            continue;
        }
        let pa = f.level.value;
        if !pa.is_finite() || pa < 1.0 || pa.fract() != 0.0 || pa > 200_000.0 {
            return Err(LoadError::BadLevel);
        }
        let entry = by_level.entry(pa as u32).or_default();
        let dup = LoadError::DuplicateField {
            field: NAMES.get(s).copied().unwrap_or(""),
            level_pa: pa as u32,
        };
        match entry.get_mut(s) {
            Some(cell) if cell.is_none() => *cell = Some(f),
            _ => return Err(dup),
        }
        if by_level.len() > MAX_LEVELS {
            return Err(LoadError::TooManyLevels(by_level.len()));
        }
    }
    for (&level_pa, four) in &by_level {
        for (s, f) in four.iter().enumerate() {
            if f.is_none() {
                return Err(LoadError::MissingField {
                    field: NAMES.get(s).copied().unwrap_or(""),
                    level_pa,
                });
            }
        }
    }
    if by_level.len() < 2 {
        return Err(LoadError::TooFewLevels(by_level.len()));
    }
    let first = by_level
        .values()
        .next()
        .and_then(|four| four.first())
        .and_then(|f| f.as_ref())
        .ok_or(LoadError::TooFewLevels(0))?;
    let grid = first.grid;
    let (nx, ny) = grid_dims(&grid);
    let npts = u64::from(nx) * u64::from(ny);
    if nx < 2 || ny < 2 {
        return Err(LoadError::BadSize(npts));
    }
    let total = npts * by_level.len() as u64 * 4;
    if total > MAX_HOUR_VALUES {
        return Err(LoadError::BadSize(total));
    }
    let valid_s = valid_time(first)?;
    let per = npts as usize * by_level.len();
    let mut h = Hour {
        valid_s,
        grid,
        nx,
        ny,
        levels_pa: Vec::with_capacity(by_level.len()),
        u: Vec::with_capacity(per),
        v: Vec::with_capacity(per),
        t: Vec::with_capacity(per),
        z: Vec::with_capacity(per),
    };
    for (level_pa, four) in by_level {
        h.levels_pa.push(level_pa as f32);
        for (s, f) in four.into_iter().enumerate() {
            let f = f.ok_or(LoadError::TooFewLevels(0))?;
            if f.grid != grid {
                return Err(LoadError::Inconsistent("grid"));
            }
            if f.values.len() as u64 != npts {
                return Err(LoadError::Inconsistent("point count"));
            }
            if valid_time(&f)? != valid_s {
                return Err(LoadError::Inconsistent("valid time"));
            }
            // NaN marks a missing value and is kept; an infinite cell is a corrupt field.
            if f.values.iter().any(|v| v.is_infinite()) {
                return Err(LoadError::NonFinite {
                    field: NAMES.get(s).copied().unwrap_or(""),
                    level_pa,
                });
            }
            let dst = match s {
                0 => &mut h.u,
                1 => &mut h.v,
                2 => &mut h.t,
                _ => &mut h.z,
            };
            dst.extend_from_slice(&f.values);
        }
    }
    Ok(h)
}

fn valid_time(f: &Field) -> Result<i64, LoadError> {
    let lead = forecast_seconds(f.time_unit, f.forecast_time).ok_or(LoadError::BadTime)?;
    ref_unix_seconds(&f.ref_time)
        .checked_add(lead)
        .ok_or(LoadError::BadTime)
}

fn wanted(info_param: Parameter, kind: u8) -> bool {
    slot(info_param).is_some() && kind == LEVEL_ISOBARIC
}

/// Builds an hour from the bytes of a GRIB2 file (several messages back to back). Every message
/// is inspected (sections 1 to 6, no data unpacked); only the messages that hold an isobaric
/// UGRD, VGRD, TMP or HGT are decoded, one message at a time, so the other fields of a full
/// file cost no memory. A message `oa-grib2` cannot read is skipped when inspected; if it was
/// one the hour needed, the hour then lacks that level and is refused as incomplete. A needed
/// message that inspects but fails to decode is refused with its error.
pub fn load_hour(data: &[u8]) -> Result<Hour, LoadError> {
    let spans = scan_messages(data)?;
    let mut keep: Vec<Field> = Vec::new();
    for span in spans {
        let Some(msg) = data.get(span.offset..span.offset + span.length) else {
            continue;
        };
        let info = inspect_stream(msg);
        if !info
            .fields
            .iter()
            .any(|i| wanted(i.parameter, i.level.kind))
        {
            continue;
        }
        for f in decode_message(msg)? {
            if wanted(f.parameter, f.level.kind) {
                keep.push(f);
            }
        }
    }
    build_hour(keep)
}

/// Parses `NNN mb` and returns NNN; layer labels such as `500-1000 mb` give `None`.
fn parse_mb(level: &str) -> Option<u32> {
    let n = level.strip_suffix(" mb")?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

/// Byte ranges (first, last inclusive) of an original NOAA file that hold the isobaric UGRD,
/// VGRD, TMP and HGT messages according to its `.idx`, with adjacent ranges joined. `file_len`
/// closes the last range. Fetching only these ranges, in order, and concatenating them gives a
/// valid multi-message file for [`load_hour`].
pub fn needed_ranges(idx: &Idx, file_len: u64) -> Result<Vec<(u64, u64)>, LoadError> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for k in 0..idx.msg.len() {
        let var = idx.var.get(k).map(String::as_str).unwrap_or("");
        let level = idx.level.get(k).map(String::as_str).unwrap_or("");
        if !NAMES.contains(&var) || parse_mb(level).is_none() {
            continue;
        }
        let (first, last) = byte_range(idx, k, Some(file_len))
            .and_then(|(a, b)| b.map(|b| (a, b)))
            .ok_or(LoadError::Inconsistent("idx range"))?;
        match out.last_mut() {
            Some(prev) if prev.1.checked_add(1) == Some(first) => prev.1 = last,
            _ => out.push((first, last)),
        }
    }
    Ok(out)
}

/// Parses an `.idx` and returns [`needed_ranges`].
pub fn needed_ranges_text(idx_text: &str, file_len: u64) -> Result<Vec<(u64, u64)>, LoadError> {
    let idx = parse_idx(idx_text)?;
    needed_ranges(&idx, file_len)
}
