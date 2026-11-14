//! The queries. Core (pure): same input, same output, no I/O.
//!
//! Query coordinates are rounded to the model's 1e-7 degree unit first (about 5 mm), so every
//! containment test is exact integer arithmetic.

use crate::geom::{locate_in_ring, Where};
use crate::model::{
    ring_is_hole, ring_range, ring_vertices, sector_count, Model, SectorIx, SetKind,
};
use crate::{MvaError, E7, MAX_RADIUS_NM, NM_PER_DEG_LAT, NM_PER_DEG_LON_EQUATOR};

/// The answer of [`min_altitude_at`]. Plain data (A1).
// A7.2: cold struct, size guard waived (returned per query, never stored in quantity)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinAlt {
    /// The highest minimum altitude of the sectors containing the point, feet MSL.
    pub min_alt_ft: i32,
    /// How many sectors contain the point (two or more on a shared edge or where facilities
    /// overlap).
    pub sectors: u32,
}

/// Rounds a latitude/longitude in degrees to 1e-7 degrees, refusing a non-finite or out of
/// range value.
fn to_e7(lat: f64, lon: f64) -> Result<(i32, i32), MvaError> {
    if !(lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon))
    {
        return Err(MvaError::BadCoordinate);
    }
    let f = E7 as f64;
    // In range, so the rounded values fit i32 (at most 1.8e9).
    Ok(((lat * f).round() as i32, (lon * f).round() as i32))
}

/// Whether sector `s` contains the point (`lat`, `lon`), 1e-7 degrees. The sector is the closed
/// set: its exterior ring and every hole's ring belong to it. A point strictly inside a hole is
/// outside the sector. A sector that does not exist contains nothing.
pub fn sector_contains(m: &Model, s: SectorIx, lat: i32, lon: i32) -> bool {
    let i = s.0 as usize;
    let (Some(&la0), Some(&la1), Some(&lo0), Some(&lo1)) = (
        m.bbox_lat_min.get(i),
        m.bbox_lat_max.get(i),
        m.bbox_lon_min.get(i),
        m.bbox_lon_max.get(i),
    ) else {
        return false;
    };
    if lat < la0 || lat > la1 || lon < lo0 || lon > lo1 {
        return false;
    }
    for r in ring_range(m, s) {
        let (lats, lons) = ring_vertices(m, r);
        let w = locate_in_ring(lats, lons, lat, lon);
        if ring_is_hole(m, r) {
            match w {
                Where::Inside => return false,
                Where::Boundary => return true,
                Where::Outside => {}
            }
        } else {
            match w {
                Where::Outside => return false,
                Where::Boundary => return true,
                Where::Inside => {}
            }
        }
    }
    true
}

/// Appends to `out` every sector of `set` that contains the point, in model order. `out` is not
/// cleared.
pub fn sectors_containing(
    m: &Model,
    lat: f64,
    lon: f64,
    set: SetKind,
    out: &mut Vec<SectorIx>,
) -> Result<(), MvaError> {
    let (la, lo) = to_e7(lat, lon)?;
    for i in 0..sector_count(m) {
        if m.set.get(i) == Some(&set) {
            let s = SectorIx(i as u32);
            if sector_contains(m, s, la, lo) {
                out.push(s);
            }
        }
    }
    Ok(())
}

/// The highest minimum altitude of every sector of `set` that contains the point (conservative:
/// where charts overlap or meet, the higher value wins). `None` when no sector of the set
/// contains it: the point is outside the charts, and no altitude is invented.
pub fn min_altitude_at(
    m: &Model,
    lat: f64,
    lon: f64,
    set: SetKind,
) -> Result<Option<MinAlt>, MvaError> {
    let (la, lo) = to_e7(lat, lon)?;
    let mut best: Option<MinAlt> = None;
    for i in 0..sector_count(m) {
        if m.set.get(i) != Some(&set) {
            continue;
        }
        if !sector_contains(m, SectorIx(i as u32), la, lo) {
            continue;
        }
        let alt = m.min_alt_ft.get(i).copied().unwrap_or(i32::MIN);
        best = Some(match best {
            Some(b) => MinAlt {
                min_alt_ft: b.min_alt_ft.max(alt),
                sectors: b.sectors + 1,
            },
            None => MinAlt {
                min_alt_ft: alt,
                sectors: 1,
            },
        });
    }
    Ok(best)
}

/// Distance in nautical miles from a point to a segment, in a plane whose axes are scaled by
/// `kx` and `ky` nautical miles per degree.
fn seg_dist_nm(p: (f64, f64), a: (f64, f64), b: (f64, f64), kx: f64, ky: f64) -> f64 {
    let (px, py) = (p.0 * kx, p.1 * ky);
    let (ax, ay) = (a.0 * kx, a.1 * ky);
    let (bx, by) = (b.0 * kx, b.1 * ky);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (ax + t * dx, ay + t * dy);
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

/// Nautical miles per degree of longitude that never exceeds the true value anywhere between
/// latitude 0 and `max_abs_lat_deg`.
fn kx_for(max_abs_lat_deg: f64) -> f64 {
    NM_PER_DEG_LON_EQUATOR * max_abs_lat_deg.min(90.0).to_radians().cos()
}

/// Distance from the point to sector `s`, nautical miles: 0 inside, else the distance to the
/// nearest edge of any ring (a point outside the region is nearest to its boundary, and a hole's
/// edge is part of it). Returns `f64::INFINITY` without scanning edges when the sector's
/// bounding box is farther than `limit_nm`. The metric is planar with
/// [`NM_PER_DEG_LAT`] per degree of latitude and longitude scaled by the cosine of the highest
/// latitude of the point and the sector, so it never exceeds the true distance (the error is a
/// few tenths of a percent in latitude and under 10 percent in longitude at these latitudes):
/// a sector within the true radius is never missed.
fn dist_nm(m: &Model, s: SectorIx, la: i32, lo: i32, limit_nm: f64) -> f64 {
    let i = s.0 as usize;
    let (Some(&la0), Some(&la1), Some(&lo0), Some(&lo1)) = (
        m.bbox_lat_min.get(i),
        m.bbox_lat_max.get(i),
        m.bbox_lon_min.get(i),
        m.bbox_lon_max.get(i),
    ) else {
        return f64::INFINITY;
    };
    let f = E7 as f64;
    let (pla, plo) = (f64::from(la) / f, f64::from(lo) / f);
    let top = pla
        .abs()
        .max((f64::from(la0) / f).abs())
        .max((f64::from(la1) / f).abs());
    let (kx, ky) = (kx_for(top), NM_PER_DEG_LAT);
    // Distance to the bounding box.
    let bx = (f64::from(lo0) / f - plo)
        .max(plo - f64::from(lo1) / f)
        .max(0.0)
        * kx;
    let by = (f64::from(la0) / f - pla)
        .max(pla - f64::from(la1) / f)
        .max(0.0)
        * ky;
    if (bx * bx + by * by).sqrt() > limit_nm {
        return f64::INFINITY;
    }
    if sector_contains(m, s, la, lo) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for r in ring_range(m, s) {
        let (lats, lons) = ring_vertices(m, r);
        let n = lats.len().min(lons.len());
        let mut k = 1;
        while k < n {
            if let (Some(&ay), Some(&ax), Some(&by), Some(&bx)) =
                (lats.get(k - 1), lons.get(k - 1), lats.get(k), lons.get(k))
            {
                let d = seg_dist_nm(
                    (plo, pla),
                    (f64::from(ax) / f, f64::from(ay) / f),
                    (f64::from(bx) / f, f64::from(by) / f),
                    kx,
                    ky,
                );
                best = best.min(d);
            }
            k += 1;
        }
    }
    best
}

/// Distance in nautical miles from the point to sector `s`: 0 when inside or on an edge, else
/// the distance to its nearest edge, by the deliberately small metric of the module (never
/// above the true distance). A sector that does not exist is infinitely far.
pub fn sector_distance_nm(m: &Model, s: SectorIx, lat: f64, lon: f64) -> Result<f64, MvaError> {
    let (la, lo) = to_e7(lat, lon)?;
    Ok(dist_nm(m, s, la, lo, f64::INFINITY))
}

/// Every sector of `set` whose polygon comes within `radius_nm` of the point, in model order
/// (conservative: a sector counts when any part of its edge or interior is within the radius,
/// not only a vertex, and the distance metric never overstates). The radius must be finite,
/// positive and at most [`MAX_RADIUS_NM`].
pub fn sectors_within_radius(
    m: &Model,
    lat: f64,
    lon: f64,
    radius_nm: f64,
    set: SetKind,
) -> Result<Vec<SectorIx>, MvaError> {
    let (la, lo) = to_e7(lat, lon)?;
    if !(radius_nm.is_finite() && radius_nm > 0.0 && radius_nm <= MAX_RADIUS_NM) {
        return Err(MvaError::BadRadius);
    }
    let mut out = Vec::new();
    for i in 0..sector_count(m) {
        if m.set.get(i) != Some(&set) {
            continue;
        }
        let s = SectorIx(i as u32);
        if dist_nm(m, s, la, lo, radius_nm) <= radius_nm {
            out.push(s);
        }
    }
    Ok(out)
}
