//! Validation statistics for `oa-wind-check`: the model against `oa-grib2`'s own decoded values,
//! against HRRR, and against pilot-reported winds. Core (pure): the binary reads the files.

use crate::archive::{archive_sample, Archive};
use crate::error::{LoadError, QueryError, Refusal};
use crate::hour::Hour;
use crate::sample::{sample_hour_pressure, sample_hour_unrotated, Query};
use oa_grib2::{ij_to_latlon, sample_bilinear, wind_at, Field, Parameter, LEVEL_ISOBARIC};

/// Median, 95th percentile (linear between ranks), mean and largest of a list of values.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub median: f64,
    pub p95: f64,
    pub mean: f64,
    pub max: f64,
}

/// Percentile `q` in 0..=1 of an ascending slice, linear between ranks; `None` when empty or `q` is NaN.
pub fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 0 || q.is_nan() {
        return None;
    }
    let pos = q.clamp(0.0, 1.0) * (n - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let (a, b) = (sorted.get(lo)?, sorted.get(hi)?);
    Some(a + (b - a) * (pos - lo as f64))
}

/// Summary of values (sorted in place); `None` when empty. NaN values are dropped.
pub fn summarize(values: &mut Vec<f64>) -> Option<Summary> {
    values.retain(|v| !v.is_nan());
    values.sort_by(|a, b| a.total_cmp(b));
    let n = values.len();
    Some(Summary {
        n,
        median: percentile(values, 0.5)?,
        p95: percentile(values, 0.95)?,
        mean: values.iter().sum::<f64>() / n as f64,
        max: values.last().copied()?,
    })
}

fn find(fields: &[Field], p: Parameter, pa: f32) -> Option<&Field> {
    fields
        .iter()
        .find(|f| f.parameter == p && f.level.kind == LEVEL_ISOBARIC && f.level.value == pa)
}

/// Result of [`check_grid_points`].
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPointCheck {
    /// Grid points compared (per level, summed over levels).
    pub n: usize,
    /// Largest absolute difference between a stored array value and the decoded field value.
    pub raw_max: f64,
    /// Largest differences of the query (at the level's own pressure) from `oa-grib2`'s
    /// `wind_at` (u and v, m/s), `sample_bilinear` of TMP (K) and of HGT (gpm).
    pub wind_max: f64,
    pub temp_max: f64,
    pub height_max: f64,
}

/// Compares an hour with the fields it was built from, decoded by `oa-grib2` independently, at
/// every `stride`-th column and row of every level: the stored values (exact) and the query at
/// the grid point and the level's pressure (rounding only).
pub fn check_grid_points(h: &Hour, fields: &[Field], stride: usize) -> GridPointCheck {
    let mut r = GridPointCheck {
        n: 0,
        raw_max: 0.0,
        wind_max: 0.0,
        temp_max: 0.0,
        height_max: 0.0,
    };
    let stride = stride.max(1);
    let npts = h.nx as usize * h.ny as usize;
    for (k, &pa) in h.levels_pa.iter().enumerate() {
        let (Some(fu), Some(fv), Some(ft), Some(fz)) = (
            find(fields, Parameter::Ugrd, pa),
            find(fields, Parameter::Vgrd, pa),
            find(fields, Parameter::Tmp, pa),
            find(fields, Parameter::Hgt, pa),
        ) else {
            continue;
        };
        for (arr, f) in [(&h.u, fu), (&h.v, fv), (&h.t, ft), (&h.z, fz)] {
            for (m, &x) in f.values.iter().enumerate() {
                let mine = arr.get(k * npts + m).copied().unwrap_or(f32::NAN);
                if !(x.is_nan() && mine.is_nan()) {
                    r.raw_max = r.raw_max.max(f64::from((x - mine).abs()));
                }
            }
        }
        for j in (0..h.ny).step_by(stride) {
            for i in (0..h.nx).step_by(stride) {
                let Some((lat, lon)) = ij_to_latlon(&h.grid, f64::from(i), f64::from(j)) else {
                    continue;
                };
                let Ok(p) = sample_hour_pressure(h, lat, lon, f64::from(pa) / 100.0) else {
                    continue;
                };
                let Ok(Some(w)) = wind_at(fu, fv, lat, lon) else {
                    continue;
                };
                let (Some(t), Some(z)) =
                    (sample_bilinear(ft, lat, lon), sample_bilinear(fz, lat, lon))
                else {
                    continue;
                };
                r.n += 1;
                r.wind_max = r
                    .wind_max
                    .max((p.u - f64::from(w.east)).abs())
                    .max((p.v - f64::from(w.north)).abs());
                r.temp_max = r.temp_max.max((p.t - f64::from(t)).abs());
                r.height_max = r.height_max.max((p.z - f64::from(z)).abs());
            }
        }
    }
    r
}

/// Result of a vector comparison: differences of the model from a reference.
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, PartialEq)]
pub struct VectorCompare {
    /// Points compared.
    pub n: usize,
    /// Points the model refused, by reason name.
    pub refused: Vec<(&'static str, usize)>,
    /// Vector difference (model minus reference) magnitude, m/s; `None` when no point compared.
    pub diff: Option<Summary>,
    /// The same with the model's U and V left grid-relative (HRRR comparison only).
    pub diff_unrotated: Option<Summary>,
    /// Mean model-minus-reference eastward and northward difference, m/s.
    pub bias_u: f64,
    pub bias_v: f64,
    /// Speed difference (model minus reference), summary of absolute values, m/s.
    pub speed_diff: Option<Summary>,
}

/// Short stable name of a refusal for counting.
pub fn refusal_name(r: &Refusal) -> &'static str {
    match r {
        Refusal::BadInput(_) => "bad-input",
        Refusal::AltitudeOutsideIsa { .. } => "altitude-outside-isa",
        Refusal::OutsideGrid => "outside-grid",
        Refusal::BelowLevels { .. } => "below-levels",
        Refusal::AboveLevels { .. } => "above-levels",
        Refusal::OutsideTime { .. } => "outside-time",
        Refusal::NoHours => "no-hours",
        Refusal::MissingValue => "missing-value",
        Refusal::NonFinite => "non-finite",
    }
}

fn count(list: &mut Vec<(&'static str, usize)>, name: &'static str) {
    match list.iter_mut().find(|(n, _)| *n == name) {
        Some(e) => e.1 += 1,
        None => list.push((name, 1)),
    }
}

/// Compares an hour with an HRRR (or any other) pair of U and V fields on one isobaric level at
/// the reference grid's every `stride`-th point, at the pressure of the reference level. The
/// reference's own earth-relative wind (`oa_grib2::wind_at`) is the truth here.
pub fn compare_to_fields(h: &Hour, u: &Field, v: &Field, stride: usize) -> VectorCompare {
    let (nx, ny) = oa_grib2::grid_dims(&u.grid);
    let p_hpa = f64::from(u.level.value) / 100.0;
    let (mut d, mut du, mut sp) = (Vec::new(), Vec::new(), Vec::new());
    let (mut bu, mut bv) = (0.0, 0.0);
    let mut refused = Vec::new();
    for j in (0..ny).step_by(stride.max(1)) {
        for i in (0..nx).step_by(stride.max(1)) {
            let Some((lat, lon)) = ij_to_latlon(&u.grid, f64::from(i), f64::from(j)) else {
                continue;
            };
            let Ok(Some(w)) = wind_at(u, v, lat, lon) else {
                continue;
            };
            let (re, rn) = (f64::from(w.east), f64::from(w.north));
            match (
                sample_hour_pressure(h, lat, lon, p_hpa),
                sample_hour_unrotated(h, lat, lon, p_hpa),
            ) {
                (Ok(m), Ok(raw)) => {
                    d.push((m.u - re).hypot(m.v - rn));
                    du.push((raw.u - re).hypot(raw.v - rn));
                    sp.push((m.u.hypot(m.v) - re.hypot(rn)).abs());
                    bu += m.u - re;
                    bv += m.v - rn;
                }
                (Err(e), _) | (_, Err(e)) => count(&mut refused, refusal_name(&e)),
            }
        }
    }
    let n = d.len();
    let nf = n.max(1) as f64;
    VectorCompare {
        n,
        refused,
        diff: summarize(&mut d),
        diff_unrotated: summarize(&mut du),
        bias_u: bu / nf,
        bias_v: bv / nf,
        speed_diff: summarize(&mut sp),
    }
}

/// A wind observation (a pilot report).
// A7.2: cold struct, size guard waived
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObsWind {
    pub lat: f64,
    pub lon: f64,
    /// Pressure altitude, feet.
    pub alt_ft: f64,
    pub time_s: f64,
    /// Direction the wind blows from, degrees true; speed in knots.
    pub wdir_deg: f64,
    pub wspd_kt: f64,
}

/// Eastward and northward wind (m/s) of a "from" direction in degrees and a speed in knots.
pub fn obs_components(wdir_deg: f64, wspd_kt: f64) -> (f64, f64) {
    let ms = wspd_kt * 1852.0 / 3600.0;
    let (s, c) = wdir_deg.to_radians().sin_cos();
    (-ms * s, -ms * c)
}

/// Compares the archive's query with observations, in the order given (time order keeps the
/// cache from reloading). A refused query is counted by reason and left out of the statistics;
/// a failed file load stops with its error.
pub fn compare_obs(a: &mut Archive, obs: &[ObsWind]) -> Result<VectorCompare, LoadError> {
    let (mut d, mut sp) = (Vec::new(), Vec::new());
    let (mut bu, mut bv) = (0.0, 0.0);
    let mut refused = Vec::new();
    for o in obs {
        let q = Query {
            lat: o.lat,
            lon: o.lon,
            alt_ft: o.alt_ft,
            time_s: o.time_s,
        };
        match archive_sample(a, &q) {
            Ok(s) => {
                let (ou, ov) = obs_components(o.wdir_deg, o.wspd_kt);
                d.push((s.u_ms - ou).hypot(s.v_ms - ov));
                sp.push((s.u_ms.hypot(s.v_ms) - ou.hypot(ov)).abs());
                bu += s.u_ms - ou;
                bv += s.v_ms - ov;
            }
            Err(QueryError::Refused(r)) => count(&mut refused, refusal_name(&r)),
            Err(QueryError::Load(e)) => return Err(e),
        }
    }
    let n = d.len();
    let nf = n.max(1) as f64;
    Ok(VectorCompare {
        n,
        refused,
        diff: summarize(&mut d),
        diff_unrotated: None,
        bias_u: bu / nf,
        bias_v: bv / nf,
        speed_diff: summarize(&mut sp),
    })
}
