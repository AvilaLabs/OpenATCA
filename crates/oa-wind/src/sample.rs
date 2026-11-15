//! The query: horizontal bilinear, vertical linear in log-pressure, linear in time. Core (pure).
//! See the crate docs for the model; every refusal is a [`Refusal`], nothing is extrapolated.

use crate::error::Refusal;
use crate::hour::Hour;
use crate::isa::pressure_hpa_from_altitude_ft;
use oa_grib2::{latlon_to_ij, to_earth_relative};

/// A query: position, pressure altitude and time.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Query {
    pub lat: f64,
    /// Degrees east, any equivalent value (the projection wraps it).
    pub lon: f64,
    /// Pressure altitude, feet (the ISA altitude of the static pressure).
    pub alt_ft: f64,
    /// Unix seconds.
    pub time_s: f64,
}

/// Values at a point of one hour, earth-relative.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Point {
    /// Eastward and northward wind, m/s.
    pub u: f64,
    pub v: f64,
    /// Temperature, K.
    pub t: f64,
    /// Geopotential height of the pressure surface, gpm.
    pub z: f64,
}

/// The answer to a query.
#[derive(Debug, Clone, Copy, PartialEq)]
// A7.2: cold struct, size guard waived
pub struct Sample {
    /// Eastward and northward wind, m/s (earth-relative).
    pub u_ms: f64,
    pub v_ms: f64,
    /// Temperature, K.
    pub temp_k: f64,
    /// Geopotential height of the pressure surface, gpm.
    pub height_gpm: f64,
    /// ISA pressure of the query altitude, hPa.
    pub pressure_hpa: f64,
    /// Valid times of the two analysis hours used (Unix seconds); equal for a single hour.
    pub hour0_s: i64,
    pub hour1_s: i64,
    /// Weight of `hour1`: 0 at `hour0`, 1 at `hour1`.
    pub time_weight: f64,
}

/// Which two of the sorted valid times bracket `t`, and the weight of the later one. A time
/// equal to a valid time gets weight 0 with the next hour (or weight 1 with the previous hour
/// for the last). One hour answers only at its own time. Refuses a time outside the span
/// and an empty list.
pub fn bracket(times: &[i64], t: f64) -> Result<(usize, usize, f64), Refusal> {
    if !t.is_finite() {
        return Err(Refusal::BadInput("time is not finite"));
    }
    let (Some(&first), Some(&last)) = (times.first(), times.last()) else {
        return Err(Refusal::NoHours);
    };
    // Strictly increasing: a duplicate would make the weight 0 / 0, and an unsorted list has no
    // bracket.
    if times.windows(2).any(|w| matches!(w, [a, b] if a >= b)) {
        return Err(Refusal::BadInput("valid times are not strictly increasing"));
    }
    if t < first as f64 || t > last as f64 {
        return Err(Refusal::OutsideTime {
            time_s: t,
            first_s: first,
            last_s: last,
        });
    }
    if times.len() == 1 {
        return Ok((0, 0, 0.0));
    }
    // Largest k with times[k] <= t, kept below the last index.
    let k = times
        .iter()
        .rposition(|&x| x as f64 <= t)
        .unwrap_or(0)
        .min(times.len() - 2);
    let (a, b) = (
        times.get(k).copied().unwrap_or(first) as f64,
        times.get(k + 1).copied().unwrap_or(last) as f64,
    );
    Ok((k, k + 1, ((t - a) / (b - a)).clamp(0.0, 1.0)))
}

/// A corner value: `Ok(None)` where the file has none (NaN), an error for an infinite one.
fn at(a: &[f32], k: usize, npts: usize, off: usize) -> Result<Option<f64>, Refusal> {
    let Some(&v) = a.get(k * npts + off) else {
        return Ok(None);
    };
    if v.is_nan() {
        Ok(None)
    } else if v.is_infinite() {
        Err(Refusal::NonFinite)
    } else {
        Ok(Some(f64::from(v)))
    }
}

/// Fractional column and row and the four corner offsets of a position, or `OutsideGrid`.
struct Cell {
    o00: usize,
    o10: usize,
    o01: usize,
    o11: usize,
    fx: f64,
    fy: f64,
}

fn locate(h: &Hour, lat: f64, lon: f64) -> Result<Cell, Refusal> {
    if !lat.is_finite() || !lon.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return Err(Refusal::BadInput(
            "position is not a latitude and longitude",
        ));
    }
    let (i, j) = latlon_to_ij(&h.grid, lat, lon).ok_or(Refusal::OutsideGrid)?;
    let (imax, jmax) = (f64::from(h.nx - 1), f64::from(h.ny - 1));
    let tol = 1e-9;
    if !i.is_finite() || !j.is_finite() || i < -tol || i > imax + tol || j < -tol || j > jmax + tol
    {
        return Err(Refusal::OutsideGrid);
    }
    let (i, j) = (i.clamp(0.0, imax), j.clamp(0.0, jmax));
    let i0 = (i.floor() as u32).min(h.nx - 2);
    let j0 = (j.floor() as u32).min(h.ny - 2);
    let nx = h.nx as usize;
    let base = j0 as usize * nx + i0 as usize;
    Ok(Cell {
        o00: base,
        o10: base + 1,
        o01: base + nx,
        o11: base + nx + 1,
        fx: i - f64::from(i0),
        fy: j - f64::from(j0),
    })
}

fn bilinear(a: &[f32], k: usize, npts: usize, c: &Cell) -> Result<f64, Refusal> {
    let g = |o: usize| at(a, k, npts, o)?.ok_or(Refusal::MissingValue);
    let top = g(c.o00)? * (1.0 - c.fx) + g(c.o10)? * c.fx;
    let bot = g(c.o01)? * (1.0 - c.fx) + g(c.o11)? * c.fx;
    Ok(top * (1.0 - c.fy) + bot * c.fy)
}

/// Index of the lower-pressure of the two bracketing levels and the weight (in ln p) of the
/// higher-pressure one.
fn vertical(h: &Hour, p_hpa: f64) -> Result<(usize, f64), Refusal> {
    if !p_hpa.is_finite() || p_hpa <= 0.0 {
        return Err(Refusal::BadInput("pressure is not positive"));
    }
    let (Some(&top), Some(&bottom)) = (h.levels_pa.first(), h.levels_pa.last()) else {
        return Err(Refusal::NoHours);
    };
    let (top_hpa, bottom_hpa) = (f64::from(top) / 100.0, f64::from(bottom) / 100.0);
    // A pressure within rounding of a limit (a level's own pressure round-tripped through the
    // ISA altitude) is that limit; anything beyond is refused.
    let tol = 1e-9;
    if p_hpa < top_hpa * (1.0 - tol) {
        return Err(Refusal::AboveLevels {
            pressure_hpa: p_hpa,
            limit_hpa: top_hpa,
        });
    }
    if p_hpa > bottom_hpa * (1.0 + tol) {
        return Err(Refusal::BelowLevels {
            pressure_hpa: p_hpa,
            limit_hpa: bottom_hpa,
        });
    }
    let p_hpa = p_hpa.clamp(top_hpa, bottom_hpa);
    let n = h.levels_pa.len();
    let k = h
        .levels_pa
        .iter()
        .rposition(|&l| f64::from(l) / 100.0 <= p_hpa)
        .unwrap_or(0)
        .min(n.saturating_sub(2));
    let a = f64::from(h.levels_pa.get(k).copied().unwrap_or(top)) / 100.0;
    let b = f64::from(h.levels_pa.get(k + 1).copied().unwrap_or(bottom)) / 100.0;
    let w = ((p_hpa.ln() - a.ln()) / (b.ln() - a.ln())).clamp(0.0, 1.0);
    Ok((k, w))
}

fn sample_inner(h: &Hour, lat: f64, lon: f64, p_hpa: f64, rotate: bool) -> Result<Point, Refusal> {
    let c = locate(h, lat, lon)?;
    let (k, w) = vertical(h, p_hpa)?;
    let npts = h.nx as usize * h.ny as usize;
    let two = |a: &[f32]| -> Result<f64, Refusal> {
        let lo = bilinear(a, k, npts, &c)?;
        // At w == 0 the upper level is not needed, so a missing value there is not a refusal.
        if w == 0.0 {
            return Ok(lo);
        }
        let hi = bilinear(a, k + 1, npts, &c)?;
        Ok(lo * (1.0 - w) + hi * w)
    };
    let (gu, gv) = (two(&h.u)?, two(&h.v)?);
    let (u, v) = if rotate {
        to_earth_relative(&h.grid, lon, gu, gv)
    } else {
        (gu, gv)
    };
    finite_point(Point {
        u,
        v,
        t: two(&h.t)?,
        z: two(&h.z)?,
    })
}

/// The point if every number in it is finite; no answer leaves this module otherwise.
fn finite_point(p: Point) -> Result<Point, Refusal> {
    if [p.u, p.v, p.t, p.z].iter().all(|x| x.is_finite()) {
        Ok(p)
    } else {
        Err(Refusal::NonFinite)
    }
}

/// Earth-relative wind, temperature and height at a position and a pressure (hPa) in one hour.
pub fn sample_hour_pressure(h: &Hour, lat: f64, lon: f64, p_hpa: f64) -> Result<Point, Refusal> {
    sample_inner(h, lat, lon, p_hpa, true)
}

/// As [`sample_hour_pressure`] but with U and V left as stored (grid-relative on RAP). A
/// diagnostic for the validation: it shows what the rotation is worth.
pub fn sample_hour_unrotated(h: &Hour, lat: f64, lon: f64, p_hpa: f64) -> Result<Point, Refusal> {
    sample_inner(h, lat, lon, p_hpa, false)
}

/// Blends two hours' points with `w` the weight of `b`, at a position and pressure.
pub fn sample_blend(
    a: &Hour,
    b: &Hour,
    w: f64,
    lat: f64,
    lon: f64,
    p_hpa: f64,
) -> Result<Point, Refusal> {
    let pa = sample_hour_pressure(a, lat, lon, p_hpa)?;
    if w == 0.0 {
        return Ok(pa);
    }
    let pb = sample_hour_pressure(b, lat, lon, p_hpa)?;
    let mix = |x: f64, y: f64| x * (1.0 - w) + y * w;
    finite_point(Point {
        u: mix(pa.u, pb.u),
        v: mix(pa.v, pb.v),
        t: mix(pa.t, pb.t),
        z: mix(pa.z, pb.z),
    })
}

/// The full query over hours sorted by valid time: ISA pressure from the pressure altitude,
/// bracketing hours, blend. Hours must be sorted ascending and distinct (the archive and
/// [`crate::archive::archive_files`] guarantee it).
pub fn sample_hours(hours: &[&Hour], q: &Query) -> Result<Sample, Refusal> {
    let p = pressure_hpa_from_altitude_ft(q.alt_ft)?;
    let times: Vec<i64> = hours.iter().map(|h| h.valid_s).collect();
    let (i0, i1, w) = bracket(&times, q.time_s)?;
    let (Some(&a), Some(&b)) = (hours.get(i0), hours.get(i1)) else {
        return Err(Refusal::NoHours);
    };
    let pt = sample_blend(a, b, w, q.lat, q.lon, p)?;
    // sample_blend checks the point; the weight and pressure are checked here with the rest.
    if !(w.is_finite() && p.is_finite()) {
        return Err(Refusal::NonFinite);
    }
    Ok(Sample {
        u_ms: pt.u,
        v_ms: pt.v,
        temp_k: pt.t,
        height_gpm: pt.z,
        pressure_hpa: p,
        hour0_s: a.valid_s,
        hour1_s: b.valid_s,
        time_weight: w,
    })
}
