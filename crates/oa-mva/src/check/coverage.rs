//! Coverage of one chart: gaps between sectors and overlaps of sectors.
//!
//! The charts state no area of their own (no boundary feature in the AIXM files), so the area
//! checked is the union of the chart's sectors. A *gap* is a region inside that union's outer
//! boundary that no sector covers (a hole in the union: an unclaimed pocket between adjacent
//! sectors, or an enclave that belongs to someone else), found by rasterising every sector on a
//! grid of [`COVERAGE_CELL_DEG`] and flood-filling the uncovered cells that cannot reach the
//! grid's edge. An *overlap* is a cell covered by two sectors. A cell counts for a sector when
//! its centre is inside the sector (holes excluded), with a half-open rule on edges so two
//! sectors sharing an edge never both claim, or both miss, a cell. Anything narrower than a cell
//! (about 220 m by 165 m) can be missed: the report is a resolution-limited finding, not proof.
//! Areas are approximate (nautical miles from the lower-bound constants of the crate, so low by
//! under 1 percent).

use crate::model::{
    chart_count, facility_text, ring_range, ring_vertices, sector_range_of_chart, Model, SectorIx,
};
use crate::query::sectors_containing;
use std::collections::HashMap;

/// Raster cell size, degrees (about 220 m north-south, 165 m east-west at 42 N).
pub const COVERAGE_CELL_DEG: f64 = 0.002;
/// Most raster cells for one chart (the largest real chart needs about 20 million).
pub const MAX_RASTER_CELLS: usize = 64_000_000;

/// An uncovered pocket inside the union of a chart. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub cells: usize,
    pub area_nm2: f64,
    /// Centre of one cell of the gap, degrees.
    pub lat: f64,
    pub lon: f64,
    /// A sector of another facility's chart of the same set covers that point (an enclave
    /// belonging to a neighbour, not an unclaimed hole).
    pub covered_by_other_facility: bool,
}

/// Two sectors of one chart that share area. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct Overlap {
    pub a: String,
    pub b: String,
    pub alt_a: i32,
    pub alt_b: i32,
    pub cells: usize,
    pub area_nm2: f64,
    /// Longest side of the bounding box of the overlapped cells, nautical miles (the area
    /// divided by this is the mean width of a strip).
    pub extent_nm: f64,
    /// Centre of one overlapped cell, degrees.
    pub lat: f64,
    pub lon: f64,
}

/// The coverage findings of one chart. Plain data (A1).
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq)]
pub struct CoverageReport {
    pub facility: String,
    pub set: &'static str,
    pub sectors: usize,
    pub raster_cells: usize,
    pub covered_area_nm2: f64,
    pub gaps: Vec<Gap>,
    pub overlaps: Vec<Overlap>,
}

/// One sector's edges, as (x0, y0, x1, y1) in degrees with y0 < y1 (horizontal edges dropped).
fn edges(m: &Model, s: SectorIx) -> Vec<(f64, f64, f64, f64)> {
    let f = crate::E7 as f64;
    let mut out = Vec::new();
    for r in ring_range(m, s) {
        let (lats, lons) = ring_vertices(m, r);
        let n = lats.len().min(lons.len());
        let mut i = 1;
        while i < n {
            if let (Some(&ay), Some(&ax), Some(&by), Some(&bx)) =
                (lats.get(i - 1), lons.get(i - 1), lats.get(i), lons.get(i))
            {
                let (ax, ay, bx, by) = (
                    f64::from(ax) / f,
                    f64::from(ay) / f,
                    f64::from(bx) / f,
                    f64::from(by) / f,
                );
                // Order the endpoints by latitude so a shared edge gives the same crossing
                // whichever sector's ring direction it came from.
                if ay < by {
                    out.push((ax, ay, bx, by));
                } else if by < ay {
                    out.push((bx, by, ax, ay));
                }
            }
            i += 1;
        }
    }
    out
}

/// Running totals of the cells two sectors share.
struct Pair {
    cells: usize,
    area: f64,
    i: (usize, usize),
    j: (usize, usize),
}

struct Raster {
    lon0: f64,
    lat0: f64,
    nx: usize,
    ny: usize,
}

/// Calls `paint(i, j)` for every cell of the raster whose centre is inside sector `s`.
fn fill_sector(m: &Model, s: SectorIx, g: &Raster, paint: &mut dyn FnMut(usize, usize)) {
    let c = COVERAGE_CELL_DEG;
    let es = edges(m, s);
    let (ymin, ymax) = es.iter().fold((f64::MAX, f64::MIN), |(lo, hi), e| {
        (lo.min(e.1), hi.max(e.3))
    });
    if es.is_empty() {
        return;
    }
    // Rows whose centre latitude is in [ymin, ymax).
    let jmin = ((ymin - g.lat0) / c - 0.5).ceil().max(0.0) as usize;
    let jmax = (((ymax - g.lat0) / c - 0.5).ceil().max(0.0) as usize).min(g.ny);
    if jmin >= jmax {
        return;
    }
    let mut rows: Vec<Vec<f64>> = vec![Vec::new(); jmax - jmin];
    for &(x0, y0, x1, y1) in &es {
        let lo = (((y0 - g.lat0) / c - 0.5).ceil().max(jmin as f64)) as usize;
        let hi = ((((y1 - g.lat0) / c - 0.5).ceil()).min(jmax as f64)) as usize;
        for j in lo..hi {
            let yc = g.lat0 + (j as f64 + 0.5) * c;
            let x = x0 + (yc - y0) * (x1 - x0) / (y1 - y0);
            if let Some(row) = rows.get_mut(j - jmin) {
                row.push(x);
            }
        }
    }
    for (k, xs) in rows.iter_mut().enumerate() {
        xs.sort_by(f64::total_cmp);
        for &[xa, xb] in xs.as_chunks::<2>().0 {
            let ia = (((xa - g.lon0) / c - 0.5).ceil().max(0.0)) as usize;
            let ib = ((((xb - g.lon0) / c - 0.5).ceil()).min(g.nx as f64).max(0.0)) as usize;
            for i in ia..ib {
                paint(i, jmin + k);
            }
        }
    }
}

/// Nautical miles per degree of latitude and of longitude at `lat_deg` on WGS-84: the meridian
/// radius `M` and the prime-vertical radius `N` times the cosine of the latitude, per degree.
/// (Constants local to the coverage report, which must not understate an area; the crate-level
/// per-degree constants are deliberately conservative for the radius query and stay as they are.)
fn nm_per_deg(lat_deg: f64) -> (f64, f64) {
    const A_M: f64 = 6_378_137.0;
    const E2: f64 = 0.006_694_379_990_141_316;
    let s = lat_deg.to_radians().sin();
    let w = 1.0 - E2 * s * s;
    let (m, n) = (A_M * (1.0 - E2) / (w * w.sqrt()), A_M / w.sqrt());
    let per_deg = std::f64::consts::PI / 180.0 / 1852.0;
    (m * per_deg, n * lat_deg.to_radians().cos() * per_deg)
}

/// Area in square nautical miles of a small patch `dlat_deg` by `dlon_deg` centred at `lat_deg`:
/// `M N cos(lat) dlat dlon`.
fn patch_area_nm2(lat_deg: f64, dlat_deg: f64, dlon_deg: f64) -> f64 {
    let (a, b) = nm_per_deg(lat_deg);
    dlat_deg * a * dlon_deg * b
}

fn cell_area_nm2(g: &Raster, j: usize) -> f64 {
    let lat = g.lat0 + (j as f64 + 0.5) * COVERAGE_CELL_DEG;
    patch_area_nm2(lat, COVERAGE_CELL_DEG, COVERAGE_CELL_DEG)
}

/// Coverage report of chart row `chart`. `Err` names why a chart was not rasterised (too many
/// cells, or no such chart).
pub fn chart_coverage(m: &Model, chart: usize) -> Result<CoverageReport, String> {
    if chart >= chart_count(m) {
        return Err(format!("no chart {chart}"));
    }
    let secs = sector_range_of_chart(m, chart);
    let f = crate::E7 as f64;
    let (mut la0, mut la1, mut lo0, mut lo1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for i in secs.clone() {
        la0 = la0.min(m.bbox_lat_min.get(i).copied().unwrap_or(la0));
        la1 = la1.max(m.bbox_lat_max.get(i).copied().unwrap_or(la1));
        lo0 = lo0.min(m.bbox_lon_min.get(i).copied().unwrap_or(lo0));
        lo1 = lo1.max(m.bbox_lon_max.get(i).copied().unwrap_or(lo1));
    }
    if secs.is_empty() {
        return Err("empty chart".to_string());
    }
    let c = COVERAGE_CELL_DEG;
    // One cell of margin on every side, so the outside is a connected border.
    let g = Raster {
        lon0: f64::from(lo0) / f - c,
        lat0: f64::from(la0) / f - c,
        nx: ((f64::from(lo1 - lo0) / f) / c).ceil() as usize + 2,
        ny: ((f64::from(la1 - la0) / f) / c).ceil() as usize + 2,
    };
    let cells = g.nx.saturating_mul(g.ny);
    if cells > MAX_RASTER_CELLS {
        return Err(format!(
            "{cells} raster cells exceed the limit of {MAX_RASTER_CELLS}"
        ));
    }
    let mut owner = vec![0u16; cells];
    let mut pairs: HashMap<(u16, u16), Pair> = HashMap::new();
    let mut covered = 0.0;
    for (k, si) in secs.clone().enumerate() {
        let id = (k + 1) as u16;
        let nx = g.nx;
        fill_sector(m, SectorIx(si as u32), &g, &mut |i, j| {
            if let Some(o) = owner.get_mut(j * nx + i) {
                if *o == 0 {
                    *o = id;
                    covered += cell_area_nm2(&g, j);
                } else if *o != id {
                    let e = pairs.entry((*o, id)).or_insert(Pair {
                        cells: 0,
                        area: 0.0,
                        i: (i, i),
                        j: (j, j),
                    });
                    e.cells += 1;
                    e.area += cell_area_nm2(&g, j);
                    e.i = (e.i.0.min(i), e.i.1.max(i));
                    e.j = (e.j.0.min(j), e.j.1.max(j));
                }
            }
        });
    }
    // Flood the uncovered cells reachable from the border; what is left uncovered is a gap.
    let mut seen = vec![false; cells];
    let mut stack: Vec<usize> = Vec::new();
    let mut flood = |start: usize, seen: &mut Vec<bool>, area: &mut f64, n: &mut usize| {
        stack.clear();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(p) = stack.pop() {
            *n += 1;
            *area += cell_area_nm2(&g, p / g.nx);
            let (i, j) = (p % g.nx, p / g.nx);
            let mut nb = [None; 4];
            if i > 0 {
                nb[0] = Some(p - 1);
            }
            if i + 1 < g.nx {
                nb[1] = Some(p + 1);
            }
            if j > 0 {
                nb[2] = Some(p - g.nx);
            }
            if j + 1 < g.ny {
                nb[3] = Some(p + g.nx);
            }
            for q in nb.into_iter().flatten() {
                if owner.get(q) == Some(&0) && seen.get(q) == Some(&false) {
                    if let Some(s) = seen.get_mut(q) {
                        *s = true;
                    }
                    stack.push(q);
                }
            }
        }
    };
    let (mut a, mut n) = (0.0, 0usize);
    for i in 0..g.nx {
        for p in [i, (g.ny - 1) * g.nx + i] {
            if owner.get(p) == Some(&0) && seen.get(p) == Some(&false) {
                flood(p, &mut seen, &mut a, &mut n);
            }
        }
    }
    for j in 0..g.ny {
        for p in [j * g.nx, j * g.nx + g.nx - 1] {
            if owner.get(p) == Some(&0) && seen.get(p) == Some(&false) {
                flood(p, &mut seen, &mut a, &mut n);
            }
        }
    }
    let set = m
        .chart_set
        .get(chart)
        .copied()
        .unwrap_or(crate::SetKind::Fus3);
    let fac = facility_text(m, SectorIx(secs.start as u32));
    let mut gaps = Vec::new();
    for p in 0..cells {
        if owner.get(p) == Some(&0) && seen.get(p) == Some(&false) {
            let (mut area, mut cnt) = (0.0, 0usize);
            flood(p, &mut seen, &mut area, &mut cnt);
            let lat = g.lat0 + ((p / g.nx) as f64 + 0.5) * c;
            let lon = g.lon0 + ((p % g.nx) as f64 + 0.5) * c;
            let mut hit = Vec::new();
            let other = sectors_containing(m, lat, lon, set, &mut hit).is_ok()
                && hit.iter().any(|&h| facility_text(m, h) != fac);
            gaps.push(Gap {
                cells: cnt,
                area_nm2: area,
                lat,
                lon,
                covered_by_other_facility: other,
            });
        }
    }
    gaps.sort_by_key(|g| std::cmp::Reverse(g.cells));
    let name = |k: u16| -> (String, i32) {
        let ix = secs.start + usize::from(k).saturating_sub(1);
        (
            crate::sector_name_text(m, SectorIx(ix as u32)),
            m.min_alt_ft.get(ix).copied().unwrap_or(0),
        )
    };
    let mut overlaps: Vec<Overlap> = pairs
        .into_iter()
        .map(|((x, y), p)| {
            let (a, alt_a) = name(x);
            let (b, alt_b) = name(y);
            let lat_mid = g.lat0 + ((p.j.0 + p.j.1) as f64 / 2.0 + 0.5) * c;
            let (nm_lat, nm_lon) = nm_per_deg(lat_mid);
            let dy = (p.j.1 - p.j.0 + 1) as f64 * c * nm_lat;
            let dx = (p.i.1 - p.i.0 + 1) as f64 * c * nm_lon;
            Overlap {
                a,
                b,
                alt_a,
                alt_b,
                cells: p.cells,
                area_nm2: p.area,
                extent_nm: dx.max(dy),
                lat: lat_mid,
                lon: g.lon0 + ((p.i.0 + p.i.1) as f64 / 2.0 + 0.5) * c,
            }
        })
        .collect();
    overlaps.sort_by(|x, y| {
        y.cells
            .cmp(&x.cells)
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    Ok(CoverageReport {
        facility: fac,
        set: set.name(),
        sectors: secs.len(),
        raster_cells: cells,
        covered_area_nm2: covered,
        gaps,
        overlaps,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // REQ: OA-MVA-077
    #[test]
    fn a_small_cell_area_matches_the_exact_wgs84_zone_area() {
        // 0.1 by 0.1 degrees at 45 N. Independent value: the authalic zone area of WGS-84,
        // A = b^2 dlon / 2 [ s/(1 - e^2 s^2) + ln((1 + e s)/(1 - e s)) / (2 e) ] from 44.95 to
        // 45.05 N, with a = 6378137 m, f = 1/298.257223563, in 40-digit arithmetic, divided by
        // 1852^2 m^2 per square nautical mile.
        let exact = 25.547_035_094_100_846;
        let got = patch_area_nm2(45.0, 0.1, 0.1);
        assert!(
            ((got - exact) / exact).abs() < 1e-4,
            "{got} against {exact}"
        );
        // The cell of the raster is the same formula at its row's centre latitude.
        let g = Raster {
            lon0: 0.0,
            lat0: 44.0,
            nx: 1,
            ny: 1000,
        };
        let j = ((45.0 - 44.0) / COVERAGE_CELL_DEG) as usize;
        let want = patch_area_nm2(g.lat0 + (j as f64 + 0.5) * COVERAGE_CELL_DEG, 0.002, 0.002);
        assert!((cell_area_nm2(&g, j) - want).abs() < 1e-12);
    }
}
