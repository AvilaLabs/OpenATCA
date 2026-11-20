//! The two references: the tracker's one-second rows (interpolated here) and the ADS-B truth
//! (interpolated by `oa-truth`, which never extrapolates).

use std::collections::BTreeMap;

use oa_bench::truth::NoTruth;

use crate::model::{Fix, Miss, Reference, EXTRAP_MS, GAP_MS};

/// One tracker output row.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub t_ms: i64,
    pub lat: f64,
    pub lon: f64,
    pub alt_ft: Option<f64>,
    pub vel: Option<(f64, f64)>,
}

fn lerp(a: f64, b: f64, f: f64) -> f64 {
    a + (b - a) * f
}

/// The fix at `t_ms` from `rows` (sorted by time, one row per time).
///
/// Between two rows: linear in position, altitude and velocity, refused when the rows are more
/// than [`GAP_MS`] apart. Before the first row or after the last: the end row advanced by its
/// velocity, when `t_ms` is within [`EXTRAP_MS`] of it and the row has a velocity (the fix is
/// flagged `extrapolated`); further out, or without a velocity, it is [`Miss::OutsideSpan`].
pub fn interpolate(rows: &[Row], t_ms: i64) -> Result<Fix, Miss> {
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Err(Miss::NoAircraft);
    };
    if t_ms < first.t_ms {
        return extrapolate(first, t_ms);
    }
    if t_ms > last.t_ms {
        return extrapolate(last, t_ms);
    }
    let i = rows.partition_point(|r| r.t_ms < t_ms);
    let hi = rows.get(i).ok_or(Miss::OutsideSpan)?;
    let fix_of = |r: &Row| Fix {
        lat: r.lat,
        lon: r.lon,
        alt_ft: r.alt_ft,
        vel: r.vel,
        extrapolated: false,
    };
    if hi.t_ms == t_ms {
        return Ok(fix_of(hi));
    }
    let lo = i
        .checked_sub(1)
        .and_then(|j| rows.get(j))
        .ok_or(Miss::OutsideSpan)?;
    let span = hi.t_ms - lo.t_ms;
    if span > GAP_MS {
        return Err(Miss::Gap);
    }
    let f = (t_ms - lo.t_ms) as f64 / span as f64;
    let alt_ft = match (lo.alt_ft, hi.alt_ft) {
        (Some(a), Some(b)) => Some(lerp(a, b, f)),
        _ => None,
    };
    let vel = match (lo.vel, hi.vel) {
        (Some(a), Some(b)) => Some((lerp(a.0, b.0, f), lerp(a.1, b.1, f))),
        _ => None,
    };
    Ok(Fix {
        lat: lerp(lo.lat, hi.lat, f),
        lon: lerp(lo.lon, hi.lon, f),
        alt_ft,
        vel,
        extrapolated: false,
    })
}

fn extrapolate(end: &Row, t_ms: i64) -> Result<Fix, Miss> {
    let dt_ms = t_ms - end.t_ms;
    if dt_ms.abs() > EXTRAP_MS {
        return Err(Miss::OutsideSpan);
    }
    let Some((ve, vn)) = end.vel else {
        return Err(Miss::OutsideSpan);
    };
    let dt = dt_ms as f64 / 1000.0;
    let frame = oa_geo::aeqd::frame_new(end.lat, end.lon);
    let (lat, lon) = oa_geo::aeqd::unproject(&frame, ve * dt, vn * dt).ok_or(Miss::NotFinite)?;
    Ok(Fix {
        lat,
        lon,
        alt_ft: end.alt_ft,
        vel: end.vel,
        extrapolated: true,
    })
}

/// The tracker's rows per aircraft address.
#[derive(Default)]
// A7.2: cold struct, size guard waived
pub struct TrackerSource {
    rows: BTreeMap<u32, Vec<Row>>,
}

impl TrackerSource {
    /// Builds the source from `(address, row)` pairs in any order; a time given twice for one
    /// address keeps the first.
    pub fn from_rows<I: IntoIterator<Item = (u32, Row)>>(rows: I) -> TrackerSource {
        let mut m: BTreeMap<u32, Vec<Row>> = BTreeMap::new();
        for (a, r) in rows {
            m.entry(a).or_default().push(r);
        }
        for v in m.values_mut() {
            v.sort_by_key(|r| r.t_ms);
            v.dedup_by_key(|r| r.t_ms);
        }
        TrackerSource { rows: m }
    }

    /// Number of rows held.
    pub fn len(&self) -> usize {
        self.rows.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Reference for TrackerSource {
    fn at(&self, addr: u32, t_ms: i64) -> Result<Fix, Miss> {
        interpolate(self.rows.get(&addr).ok_or(Miss::NoAircraft)?, t_ms)
    }
}

/// The ADS-B truth of `oa-truth` through the benchmark adapter.
// A7.2: cold struct, size guard waived
pub struct TruthSource<'a> {
    pub truth: &'a oa_truth::Truth,
}

impl Reference for TruthSource<'_> {
    fn at(&self, addr: u32, t_ms: i64) -> Result<Fix, Miss> {
        match oa_bench::adsb_truth::query(self.truth, addr, t_ms) {
            Ok(s) => Ok(Fix {
                lat: s.lat,
                lon: s.lon,
                alt_ft: s.baro_alt_ft.map(f64::from),
                vel: Some((s.vel_east_mps, s.vel_north_mps)),
                extrapolated: false,
            }),
            Err(NoTruth::UnknownAircraft) => Err(Miss::NoAircraft),
            Err(NoTruth::OutsideLegs) => Err(Miss::OutsideSpan),
            Err(NoTruth::NoNearbyReport) => Err(Miss::NoNearbyReport),
            Err(NoTruth::TooUncertain) => Err(Miss::TooUncertain),
            Err(NoTruth::NotFinite) => Err(Miss::NotFinite),
        }
    }
}
