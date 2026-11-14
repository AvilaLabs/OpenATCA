//! Interpolation and earth-relative wind. Core (pure).

use crate::error::Grib2Error;
use crate::grid::{grid_dims, latlon_to_ij, to_earth_relative, Grid};
use crate::message::Field;

/// Earth-relative wind, m/s.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Wind {
    /// Towards the east.
    pub east: f32,
    /// Towards the north.
    pub north: f32,
}

fn value_at(f: &Field, i: u32, j: u32, nx: u32) -> Option<f32> {
    let v = *f.values.get(j as usize * nx as usize + i as usize)?;
    if v.is_nan() {
        None
    } else {
        Some(v)
    }
}

/// Bilinear interpolation of `f` at a position. `None` when the position is outside the grid,
/// cannot be projected, or any of the four surrounding points is missing. Longitude wraps on a
/// global latitude/longitude grid.
pub fn sample_bilinear(f: &Field, lat: f64, lon: f64) -> Option<f32> {
    let (i, j) = latlon_to_ij(&f.grid, lat, lon)?;
    bilinear_ij(f, i, j)
}

/// As [`sample_bilinear`] at fractional grid indexes.
pub fn bilinear_ij(f: &Field, i: f64, j: f64) -> Option<f32> {
    let (nx, ny) = grid_dims(&f.grid);
    if f.values.len() != nx as usize * ny as usize || !i.is_finite() || !j.is_finite() {
        return None;
    }
    let wrap = matches!(&f.grid, Grid::LatLon(l) if l.global);
    let (mut i, tol) = (i, 1e-9);
    let imax = f64::from(nx - 1);
    let jmax = f64::from(ny - 1);
    if wrap {
        // Columns repeat every nx; the last column's neighbour is column 0.
        i = i.rem_euclid(f64::from(nx));
    } else if i < -tol || i > imax + tol {
        return None;
    }
    if j < -tol || j > jmax + tol {
        return None;
    }
    let i = if wrap { i } else { i.clamp(0.0, imax) };
    let j = j.clamp(0.0, jmax);
    let i0 = (i.floor() as u32).min(nx - 1);
    let mut i1 = i0 + 1;
    if i1 >= nx {
        i1 = if wrap { 0 } else { nx - 1 };
    }
    let j0 = (j.floor() as u32).min(ny.saturating_sub(2));
    let j1 = (j0 + 1).min(ny - 1);
    let fx = i - f64::from(i0);
    let fy = j - f64::from(j0);
    let v00 = f64::from(value_at(f, i0, j0, nx)?);
    let v10 = f64::from(value_at(f, i1, j0, nx)?);
    let v01 = f64::from(value_at(f, i0, j1, nx)?);
    let v11 = f64::from(value_at(f, i1, j1, nx)?);
    let top = v00 * (1.0 - fx) + v10 * fx;
    let bot = v01 * (1.0 - fx) + v11 * fx;
    Some((top * (1.0 - fy) + bot * fy) as f32)
}

/// Earth-relative wind at a position from the u and v fields on one grid. The two components
/// are interpolated in the grid's own axes, then turned at the position by the grid's
/// convergence angle when the grid says they are grid-relative. Errors when the fields do not
/// share a grid; `Ok(None)` when the position has no data.
pub fn wind_at(u: &Field, v: &Field, lat: f64, lon: f64) -> Result<Option<Wind>, Grib2Error> {
    if u.grid != v.grid || u.values.len() != v.values.len() {
        return Err(Grib2Error::GridMismatch);
    }
    let (i, j) = match latlon_to_ij(&u.grid, lat, lon) {
        Some(p) => p,
        None => return Ok(None),
    };
    let (gu, gv) = match (bilinear_ij(u, i, j), bilinear_ij(v, i, j)) {
        (Some(a), Some(b)) => (f64::from(a), f64::from(b)),
        _ => return Ok(None),
    };
    let (e, n) = to_earth_relative(&u.grid, lon, gu, gv);
    Ok(Some(Wind {
        east: e as f32,
        north: n as f32,
    }))
}
