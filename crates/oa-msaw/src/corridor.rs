//! The grid index of the corridors: every corridor is entered under each 0.1-degree cell its
//! bounding box touches, so that a point looks at the corridors of its own cell and not at all of
//! them. A point is only ever inside a corridor whose bounding box holds the point's centre, so the
//! answer is that of looking at every corridor ([`crate::calc::corridor_hit_linear`], tested
//! against). Core (pure).

use alloc::vec::Vec;

use crate::calc::{aligned, corridor_hit_for, hit_union, runway_elev_at, M_PER_DEG, RAD_PER_DEG};
use crate::types::{Corridor, CorridorHit, MsawConfig, SamplePoint};

/// The cell of the index, 1e-7 degree (0.1 degree).
const CELL_E7: i32 = 1_000_000;
/// A bounding box is widened by this, 1e-7 degree (22 m), which covers the rounding of the
/// corner positions and of a point's coordinates.
const MARGIN_E7: f64 = 2_000.0;

/// The entries of the index: (cell latitude, cell longitude, corridor number), sorted.
// A7.2: cold struct (one per run), size guard waived
#[derive(Debug, Default)]
pub struct CorridorIndex {
    entries: Vec<(i32, i32, u16)>,
}

impl CorridorIndex {
    /// How many entries the index holds (a corridor has one for each cell its box touches).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The cells, as (south, north, west, east) inclusive, that the bounding box of the corridor
/// touches. The corners are put where `corridor_hit_ft` puts the frame of the corridor.
// E8: the values are held within the i32 range before the cast.
#[allow(clippy::cast_possible_truncation)]
fn cells_of(c: &Corridor) -> (i32, i32, i32, i32) {
    let thr_lat = f64::from(c.thr_lat_e7) * 1.0e-7;
    let cos_lat = oa_math::cos(thr_lat * RAD_PER_DEG);
    let (sin_b, cos_b) = oa_math::sin_cos(c.out_bearing_deg * RAD_PER_DEG);
    let (mut la0, mut la1) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut lo0, mut lo1) = (f64::INFINITY, f64::NEG_INFINITY);
    for (along, cross) in [
        (0.0, -c.near_half_width_m),
        (0.0, c.near_half_width_m),
        (c.length_m, c.far_half_width_m),
        (c.length_m, -c.far_half_width_m),
    ] {
        let dx = along * sin_b + cross * cos_b;
        let dy = along * cos_b - cross * sin_b;
        let la = f64::from(c.thr_lat_e7) + dy / M_PER_DEG * 1.0e7;
        let lo = f64::from(c.thr_lon_e7) + dx / (M_PER_DEG * cos_lat) * 1.0e7;
        la0 = oa_math::min_f64(la0, la);
        la1 = oa_math::max_f64(la1, la);
        lo0 = oa_math::min_f64(lo0, lo);
        lo1 = oa_math::max_f64(lo1, lo);
    }
    let cell =
        |v: f64| oa_math::clamp_f64(oa_ieee::floor(v / f64::from(CELL_E7)), -2.0e3, 2.0e3) as i32;
    (
        cell(la0 - MARGIN_E7),
        cell(la1 + MARGIN_E7),
        cell(lo0 - MARGIN_E7),
        cell(lo1 + MARGIN_E7),
    )
}

/// The index of the corridors of `cfg`, or the refusal if the allocator has no room.
pub fn corridor_index_new(cfg: &MsawConfig) -> Result<CorridorIndex, &'static str> {
    let mut n = 0u64;
    for c in cfg.corridors.iter() {
        let (a0, a1, b0, b1) = cells_of(c);
        let (h, w) = (i64::from(a1 - a0) + 1, i64::from(b1 - b0) + 1);
        n = n.saturating_add(h.saturating_mul(w).unsigned_abs());
    }
    let mut entries: Vec<(i32, i32, u16)> = Vec::new();
    entries
        .try_reserve_exact(usize::try_from(n).map_err(|_| "corridors: the index is too large")?)
        .map_err(|_| "corridors: the index cannot be allocated")?;
    for (k, c) in cfg.corridors.iter().enumerate() {
        let (a0, a1, b0, b1) = cells_of(c);
        let k = u16::try_from(k).map_err(|_| "corridors")?;
        for a in a0..=a1 {
            for b in b0..=b1 {
                entries.push((a, b, k));
            }
        }
    }
    entries.sort_unstable();
    Ok(CorridorIndex { entries })
}

impl CorridorIndex {
    /// The numbers of the corridors whose cells the box of `radius_m` around a point touches (a
    /// superset of those that can hold or reach a circle of that radius), each once.
    // E8: the values are held within the i32 range before the cast.
    #[allow(clippy::cast_possible_truncation)]
    pub fn near(&self, lat_e7: i32, lon_e7: i32, radius_m: f64) -> Vec<u16> {
        let dlat = radius_m / M_PER_DEG * 1.0e7 + MARGIN_E7;
        // The widest the degree of longitude is: the narrowest, at the latitude of the point.
        let cos_lat =
            oa_math::max_f64(oa_math::cos(f64::from(lat_e7) * 1.0e-7 * RAD_PER_DEG), 0.05);
        let dlon = radius_m / (M_PER_DEG * cos_lat) * 1.0e7 + MARGIN_E7;
        let cell = |v: f64| {
            oa_math::clamp_f64(oa_ieee::floor(v / f64::from(CELL_E7)), -2.0e3, 2.0e3) as i32
        };
        let (a0, a1) = (
            cell(f64::from(lat_e7) - dlat),
            cell(f64::from(lat_e7) + dlat),
        );
        let (b0, b1) = (
            cell(f64::from(lon_e7) - dlon),
            cell(f64::from(lon_e7) + dlon),
        );
        let mut out: Vec<u16> = Vec::new();
        for a in a0..=a1 {
            for b in b0..=b1 {
                let from = self.entries.partition_point(|&(x, y, _)| (x, y) < (a, b));
                for &(x, y, k) in self.entries.get(from..).unwrap_or(&[]) {
                    if (x, y) != (a, b) {
                        break;
                    }
                    if !out.contains(&k) {
                        out.push(k);
                    }
                }
            }
        }
        out
    }
}

/// What the corridors say of a point: the union of the hits of the corridors of the point's cell
/// that hold the point's whole query circle, `None` if none does.
pub fn corridor_hit_at(
    cfg: &MsawConfig,
    ix: &CorridorIndex,
    p: &SamplePoint,
) -> Option<CorridorHit> {
    let cell = (
        i32::div_euclid(p.lat_e7, CELL_E7),
        i32::div_euclid(p.lon_e7, CELL_E7),
    );
    let from = ix.entries.partition_point(|&(a, b, _)| (a, b) < cell);
    let mut best: Option<CorridorHit> = None;
    for &(a, b, k) in ix.entries.get(from..).unwrap_or(&[]) {
        if (a, b) != cell {
            break;
        }
        let Some(c) = cfg.corridors.get(usize::from(k)) else {
            continue;
        };
        if let Some(h) = corridor_hit_for(
            c,
            p.lat_e7,
            p.lon_e7,
            f64::from(p.radius_m),
            p.track_deg,
            cfg,
        ) {
            best = Some(hit_union(best, h));
        }
    }
    best
}

/// The runway elevation at a point if the point itself is over the pavement of a runway strip of the
/// point's cell (the lowest of them where strips overlap), see [`crate::calc::runway_elev_at`].
pub fn corridor_runway_elev_at(
    cfg: &MsawConfig,
    ix: &CorridorIndex,
    p: &SamplePoint,
    radius_m: f64,
) -> Option<i32> {
    let cell = (
        i32::div_euclid(p.lat_e7, CELL_E7),
        i32::div_euclid(p.lon_e7, CELL_E7),
    );
    let from = ix.entries.partition_point(|&(a, b, _)| (a, b) < cell);
    let mut best: Option<i32> = None;
    for &(a, b, k) in ix.entries.get(from..).unwrap_or(&[]) {
        if (a, b) != cell {
            break;
        }
        let Some(c) = cfg.corridors.get(usize::from(k)) else {
            continue;
        };
        if !aligned(c, p.track_deg, cfg.capture_heading_deg) {
            continue;
        }
        if let Some(e) = runway_elev_at(c, p.lat_e7, p.lon_e7, radius_m) {
            best = Some(best.map_or(e, |b| b.min(e)));
        }
    }
    best
}
