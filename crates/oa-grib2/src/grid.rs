//! Grid definition (section 3) and map geometry. Core (pure).
//!
//! Two templates are read: 3.30 (Lambert conformal, NCEP RAP and HRRR) and 3.0 (regular
//! latitude/longitude, GFS). Both are held as plain records ([`Grid`]); geometry is in free
//! functions.
//!
//! Earth shape: the spherical codes only (0: 6367470 m, 1: radius in the template, 6: 6371229 m
//! as NCEP uses, 8: 6371200 m). Oblate codes are refused, because the NCEP Lambert grids are
//! defined on the sphere and silently treating an ellipsoid as a sphere would misplace points.
//!
//! Scanning mode: 0x00 (rows west to east, first row northernmost) and 0x40 (rows west to east,
//! first row southernmost) with adjacent points along i. Anything else is refused.
//!
//! Values are stored in scan order: index `j * nx + i`, `i` increasing eastward in the grid and
//! `j` counting rows in scan order. [`latlon_to_ij`] and [`ij_to_latlon`] work in those indexes.

use crate::bits::{be, sign_mag, u16_at, u32_at, u8_at};
use crate::error::Grib2Error;
use crate::MAX_GRID_POINTS;

/// Resolution and component flags bit 5 (0x08): u and v are relative to the grid axes.
pub const FLAG_GRID_RELATIVE: u8 = 0x08;
/// Scanning mode bit 2 (0x40): rows run south to north.
pub const SCAN_PLUS_J: u8 = 0x40;

/// A Lambert conformal grid (template 3.30) with its projection constants precomputed.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct LambertGrid {
    pub nx: u32,
    pub ny: u32,
    /// Latitude and longitude of the first grid point, degrees (longitude as written, 0..360).
    pub la1: f64,
    pub lo1: f64,
    /// Latitude where Dx and Dy are specified, degrees.
    pub lad: f64,
    /// Orientation longitude, degrees.
    pub lov: f64,
    /// Grid length at LaD, metres.
    pub dx: f64,
    pub dy: f64,
    pub latin1: f64,
    pub latin2: f64,
    /// Sphere radius, metres.
    pub radius: f64,
    pub flags: u8,
    pub scan: u8,
    /// Cone constant n.
    pub n: f64,
    /// Projection constant F (not scaled by the radius).
    pub f: f64,
    /// Radial distance of the pole-side reference parallel (LaD), metres.
    pub rho0: f64,
    /// Plane coordinates of the first grid point, metres.
    pub x1: f64,
    pub y1: f64,
    /// Plane spacing in metres, `dx` and `dy` scaled from LaD to the plane.
    pub step_x: f64,
    pub step_y: f64,
}

/// A regular latitude/longitude grid (template 3.0).
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct LatLonGrid {
    pub ni: u32,
    pub nj: u32,
    pub la1: f64,
    pub lo1: f64,
    /// Increments, degrees, positive.
    pub di: f64,
    pub dj: f64,
    /// +1 when row 0 is the southernmost (scan 0x40), -1 when it is the northernmost (0x00).
    pub j_dir: i8,
    /// The columns cover the full circle, so the last column neighbours the first.
    pub global: bool,
    pub flags: u8,
    pub scan: u8,
}

/// Either supported grid.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub enum Grid {
    Lambert(LambertGrid),
    LatLon(LatLonGrid),
}

/// Number of points along i and j.
pub fn grid_dims(g: &Grid) -> (u32, u32) {
    match g {
        Grid::Lambert(l) => (l.nx, l.ny),
        Grid::LatLon(l) => (l.ni, l.nj),
    }
}

fn sphere_radius(shape: u8, sect: &[u8]) -> Result<f64, Grib2Error> {
    match shape {
        0 => Ok(6_367_470.0),
        6 => Ok(6_371_229.0),
        8 => Ok(6_371_200.0),
        1 => {
            let t = Grib2Error::Truncated("grid definition");
            let scale = sign_mag(sect, 15, 1).ok_or_else(|| t.clone())?;
            let value = be(sect, 16, 4).ok_or(t)? as f64;
            let r = value * 10f64.powi(-(scale as i32));
            if r.is_finite() && (1.0e6..=1.0e8).contains(&r) {
                Ok(r)
            } else {
                Err(Grib2Error::BadGrid("spherical radius out of range"))
            }
        }
        other => Err(Grib2Error::UnsupportedEarthShape(other)),
    }
}

fn check_scan(scan: u8) -> Result<(), Grib2Error> {
    if scan == 0x00 || scan == SCAN_PLUS_J {
        Ok(())
    } else {
        Err(Grib2Error::UnsupportedScanMode(scan))
    }
}

/// Result of section 3: the grid and the point count it declares.
pub fn parse_grid(sect: &[u8]) -> Result<Grid, Grib2Error> {
    let t = Grib2Error::Truncated("grid definition");
    let source = u8_at(sect, 5).ok_or_else(|| t.clone())?;
    let npts = u32_at(sect, 6).ok_or_else(|| t.clone())?;
    let optional = u8_at(sect, 10).ok_or_else(|| t.clone())?;
    let template = u16_at(sect, 12).ok_or_else(|| t.clone())?;
    if source != 0 {
        return Err(Grib2Error::BadGrid("grid source is not a template"));
    }
    if optional != 0 {
        return Err(Grib2Error::BadGrid("optional point list not supported"));
    }
    let grid = match template {
        30 => parse_lambert(sect)?,
        0 => parse_latlon(sect)?,
        other => return Err(Grib2Error::UnsupportedGridTemplate(other)),
    };
    let (a, b) = grid_dims(&grid);
    let n = u64::from(a) * u64::from(b);
    if a == 0 || b == 0 || n > MAX_GRID_POINTS {
        return Err(Grib2Error::GridPoints(n));
    }
    if n != u64::from(npts) {
        return Err(Grib2Error::BadGrid(
            "point count disagrees with grid dimensions",
        ));
    }
    Ok(grid)
}

fn parse_lambert(sect: &[u8]) -> Result<Grid, Grib2Error> {
    if sect.len() < 81 {
        return Err(Grib2Error::BadSectionLength {
            section: 3,
            length: sect.len() as u32,
        });
    }
    let t = Grib2Error::Truncated("grid definition");
    let shape = u8_at(sect, 14).ok_or_else(|| t.clone())?;
    let radius = sphere_radius(shape, sect)?;
    let nx = u32_at(sect, 30).ok_or_else(|| t.clone())?;
    let ny = u32_at(sect, 34).ok_or_else(|| t.clone())?;
    let micro = |pos: usize| -> Result<f64, Grib2Error> {
        Ok(sign_mag(sect, pos, 4).ok_or_else(|| t.clone())? as f64 * 1e-6)
    };
    let la1 = micro(38)?;
    let lo1 = micro(42)?;
    let flags = u8_at(sect, 46).ok_or_else(|| t.clone())?;
    let lad = micro(47)?;
    let lov = micro(51)?;
    let dx = u32_at(sect, 55).ok_or_else(|| t.clone())? as f64 * 1e-3;
    let dy = u32_at(sect, 59).ok_or_else(|| t.clone())? as f64 * 1e-3;
    let scan = u8_at(sect, 64).ok_or_else(|| t.clone())?;
    let latin1 = micro(65)?;
    let latin2 = micro(69)?;
    check_scan(scan)?;
    let g = lambert_setup(LambertInput {
        nx,
        ny,
        la1,
        lo1,
        lad,
        lov,
        dx,
        dy,
        latin1,
        latin2,
        radius,
        flags,
        scan,
    })?;
    Ok(Grid::Lambert(g))
}

/// The raw values of a Lambert grid, before the projection constants are derived.
#[derive(Debug, Clone, Copy)]
// A7.2: cold struct, size guard waived
pub struct LambertInput {
    pub nx: u32,
    pub ny: u32,
    pub la1: f64,
    pub lo1: f64,
    pub lad: f64,
    pub lov: f64,
    pub dx: f64,
    pub dy: f64,
    pub latin1: f64,
    pub latin2: f64,
    pub radius: f64,
    pub flags: u8,
    pub scan: u8,
}

/// Derives the cone constants and the first point's plane position. Spherical Lambert conformal
/// conic (Snyder, Map Projections: A Working Manual, eqs. 15-1 to 15-11).
pub fn lambert_setup(p: LambertInput) -> Result<LambertGrid, Grib2Error> {
    let bad = |w| Err(Grib2Error::BadGrid(w));
    let finite = [
        p.la1, p.lo1, p.lad, p.lov, p.dx, p.dy, p.latin1, p.latin2, p.radius,
    ];
    if finite.iter().any(|v| !v.is_finite()) {
        return bad("non-finite value");
    }
    if p.dx <= 0.0 || p.dy <= 0.0 {
        return bad("grid length is not positive");
    }
    for lat in [p.la1, p.lad, p.latin1, p.latin2] {
        if !(-89.0..=89.0).contains(&lat) {
            return bad("latitude outside +-89 degrees");
        }
    }
    if p.latin1 * p.latin2 < 0.0 {
        return bad("standard parallels on opposite sides of the equator");
    }
    if p.latin1 == 0.0 || p.latin2 == 0.0 {
        return bad("standard parallel on the equator");
    }
    let r = p.radius;
    let (p1, p2) = (p.latin1.to_radians(), p.latin2.to_radians());
    let tq = |phi: f64| (std::f64::consts::FRAC_PI_4 + phi / 2.0).tan();
    let n = if (p1 - p2).abs() < 1e-12 {
        p1.sin()
    } else {
        (p1.cos() / p2.cos()).ln() / (tq(p2) / tq(p1)).ln()
    };
    if !n.is_finite() || n.abs() < 1e-6 || n.abs() > 1.0 {
        return bad("cone constant out of range");
    }
    let f = p1.cos() * tq(p1).powf(n) / n;
    let rho_at = |phi: f64| r * f / tq(phi).powf(n);
    let rho0 = rho_at(p.lad.to_radians());
    // Plane distance per ground distance at LaD, so that Dx at LaD maps to the plane.
    let k = |phi: f64| p1.cos() / phi.cos() * (tq(p1) / tq(phi)).powf(n);
    let kd = k(p.lad.to_radians());
    let step_x = p.dx * kd;
    let step_y = p.dy * kd;
    let theta = n * wrap180(p.lo1 - p.lov).to_radians();
    let rho1 = rho_at(p.la1.to_radians());
    let x1 = rho1 * theta.sin();
    let y1 = rho0 - rho1 * theta.cos();
    Ok(LambertGrid {
        nx: p.nx,
        ny: p.ny,
        la1: p.la1,
        lo1: p.lo1,
        lad: p.lad,
        lov: p.lov,
        dx: p.dx,
        dy: p.dy,
        latin1: p.latin1,
        latin2: p.latin2,
        radius: r,
        flags: p.flags,
        scan: p.scan,
        n,
        f,
        rho0,
        x1,
        y1,
        step_x,
        step_y,
    })
}

fn parse_latlon(sect: &[u8]) -> Result<Grid, Grib2Error> {
    if sect.len() < 72 {
        return Err(Grib2Error::BadSectionLength {
            section: 3,
            length: sect.len() as u32,
        });
    }
    let t = Grib2Error::Truncated("grid definition");
    let shape = u8_at(sect, 14).ok_or_else(|| t.clone())?;
    // The earth shape does not move points on a latitude/longitude grid, but an unsupported
    // code still means the producer meant something this decoder was not written for.
    sphere_radius(shape, sect)?;
    let ni = u32_at(sect, 30).ok_or_else(|| t.clone())?;
    let nj = u32_at(sect, 34).ok_or_else(|| t.clone())?;
    let basic = u32_at(sect, 38).ok_or_else(|| t.clone())?;
    let sub = u32_at(sect, 42).ok_or_else(|| t.clone())?;
    let unit = if basic == 0 && (sub == 0 || sub == u32::MAX) {
        1e-6
    } else if basic != 0 && sub != 0 && sub != u32::MAX {
        f64::from(basic) / f64::from(sub)
    } else {
        return Err(Grib2Error::BadGrid("angle unit is not defined"));
    };
    let ang = |pos: usize| -> Result<f64, Grib2Error> {
        Ok(sign_mag(sect, pos, 4).ok_or_else(|| t.clone())? as f64 * unit)
    };
    let la1 = ang(46)?;
    let lo1 = ang(50)?;
    let flags = u8_at(sect, 54).ok_or_else(|| t.clone())?;
    let la2 = ang(55)?;
    let lo2 = ang(59)?;
    let di = u32_at(sect, 63).ok_or_else(|| t.clone())? as f64 * unit;
    let dj = u32_at(sect, 67).ok_or_else(|| t.clone())? as f64 * unit;
    let scan = u8_at(sect, 71).ok_or_else(|| t.clone())?;
    check_scan(scan)?;
    if di <= 0.0 || dj <= 0.0 {
        return Err(Grib2Error::BadGrid("increment is not positive"));
    }
    if !(-90.0..=90.0).contains(&la1) || !(-90.0..=90.0).contains(&la2) {
        return Err(Grib2Error::BadGrid("latitude outside +-90 degrees"));
    }
    let j_dir: i8 = if scan & SCAN_PLUS_J != 0 { 1 } else { -1 };
    // The stated far corner must agree with the first point, count and increments.
    let la2_calc = la1 + f64::from(j_dir) * f64::from(nj.saturating_sub(1)) * dj;
    if (la2_calc - la2).abs() > 0.5 * dj {
        return Err(Grib2Error::BadGrid(
            "last latitude disagrees with Nj and Dj",
        ));
    }
    let span = f64::from(ni.saturating_sub(1)) * di;
    let lo_gap = wrap360(lo2 - lo1 - span);
    if lo_gap.min(360.0 - lo_gap) > 0.5 * di {
        return Err(Grib2Error::BadGrid(
            "last longitude disagrees with Ni and Di",
        ));
    }
    let global = (f64::from(ni) * di - 360.0).abs() < 0.5 * di;
    Ok(Grid::LatLon(LatLonGrid {
        ni,
        nj,
        la1,
        lo1,
        di,
        dj,
        j_dir,
        global,
        flags,
        scan,
    }))
}

/// Degrees into (-180, 180].
pub fn wrap180(d: f64) -> f64 {
    let w = d.rem_euclid(360.0);
    if w > 180.0 {
        w - 360.0
    } else {
        w
    }
}

/// Degrees into [0, 360).
pub fn wrap360(d: f64) -> f64 {
    d.rem_euclid(360.0)
}

/// Fractional grid indexes `(i, j)` of a position, in scan order, or `None` when the position
/// cannot be projected. The result may lie outside the grid; the caller checks the range.
pub fn latlon_to_ij(g: &Grid, lat: f64, lon: f64) -> Option<(f64, f64)> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return None;
    }
    match g {
        Grid::Lambert(l) => {
            if lat.abs() > 89.9 {
                return None;
            }
            let tq = (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0).tan();
            let rho = l.radius * l.f / tq.powf(l.n);
            let theta = l.n * wrap180(lon - l.lov).to_radians();
            let x = rho * theta.sin();
            let y = l.rho0 - rho * theta.cos();
            let i = (x - l.x1) / l.step_x;
            let jy = (y - l.y1) / l.step_y;
            let j = if l.scan & SCAN_PLUS_J != 0 { jy } else { -jy };
            Some((i, j))
        }
        Grid::LatLon(l) => {
            let mut di = wrap360(lon - l.lo1);
            // A longitude just west of the first column belongs to the last column of a global
            // grid, not to a position 359.9 degrees away.
            if l.global && di > f64::from(l.ni) * l.di - 0.5 * l.di {
                di -= 360.0;
            }
            let i = di / l.di;
            let j = f64::from(l.j_dir) * (lat - l.la1) / l.dj;
            Some((i, j))
        }
    }
}

/// Latitude and longitude (degrees, longitude in [-180, 180)) of fractional indexes `(i, j)`.
pub fn ij_to_latlon(g: &Grid, i: f64, j: f64) -> Option<(f64, f64)> {
    match g {
        Grid::Lambert(l) => {
            let x = l.x1 + i * l.step_x;
            let jy = if l.scan & SCAN_PLUS_J != 0 { j } else { -j };
            let y = l.y1 + jy * l.step_y;
            let s = l.n.signum();
            let dy = l.rho0 - y;
            let rho = s * (x * x + dy * dy).sqrt();
            if rho == 0.0 {
                return Some((s * 90.0, l.lov));
            }
            let theta = (s * x).atan2(s * dy);
            let lat =
                2.0 * ((l.radius * l.f / rho).powf(1.0 / l.n)).atan() - std::f64::consts::FRAC_PI_2;
            let lon = l.lov + (theta / l.n).to_degrees();
            Some((lat.to_degrees(), wrap180(lon)))
        }
        Grid::LatLon(l) => {
            let lat = l.la1 + f64::from(l.j_dir) * j * l.dj;
            let lon = l.lo1 + i * l.di;
            Some((lat, wrap180(lon)))
        }
    }
}

/// Angle in radians by which the grid's +y axis is turned counter-clockwise from true north at
/// longitude `lon`. For Lambert conformal this is the convergence angle `n * (lon - LoV)`;
/// for a latitude/longitude grid it is zero.
pub fn grid_rotation(g: &Grid, lon: f64) -> f64 {
    match g {
        Grid::Lambert(l) => l.n * wrap180(lon - l.lov).to_radians(),
        Grid::LatLon(_) => 0.0,
    }
}

/// True when the stored u and v are along the grid axes rather than east and north.
pub fn components_grid_relative(g: &Grid) -> bool {
    match g {
        Grid::Lambert(l) => l.flags & FLAG_GRID_RELATIVE != 0,
        // On a regular latitude/longitude grid the axes are east and north already.
        Grid::LatLon(_) => false,
    }
}

/// Turns a grid-relative vector `(u, v)` at longitude `lon` into `(east, north)`.
///
/// The grid's +y axis points at angle `theta` counter-clockwise from true north (`grid_rotation`),
/// so true east is the grid +x axis turned by `theta` the same way:
/// `east = u cos(theta) + v sin(theta)`, `north = -u sin(theta) + v cos(theta)`.
/// A grid that is already east/north is returned unchanged.
pub fn to_earth_relative(g: &Grid, lon: f64, u: f64, v: f64) -> (f64, f64) {
    if !components_grid_relative(g) {
        return (u, v);
    }
    let th = grid_rotation(g, lon);
    let (s, c) = th.sin_cos();
    (u * c + v * s, -u * s + v * c)
}
