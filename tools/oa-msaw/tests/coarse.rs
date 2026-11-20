//! The terrain grid and the chart raster against the lookups they replace: over dense bounded
//! grids and polygons the coarse answer is never below the fine one, and it is never an answer
//! where the fine lookup has none. Every value is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_msaw_run::chart_raster::ChartRaster;
use oa_msaw_run::coarse::{Coarse, CoarseTerrain};
use oa_msaw_run::floors::ceil_ft;
use oa_mva::{min_altitude_at, new_model, Model, SetKind};
use oa_tiff::{max_elevation_in_radius_conservative, Grid, TerrainMax, TileSet};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

fn grid(rng: &mut Lcg, w: u32, h: u32, lat0: f64, lon0: f64, dlat: f64, dlon: f64) -> Grid {
    let elev = (0..w * h)
        .map(|_| {
            let r = rng.next();
            if r < 0.0003 {
                f32::NAN
            } else {
                (r * 800.0) as f32
            }
        })
        .collect::<Vec<_>>();
    Grid {
        width: w,
        height: h,
        lon0,
        lat0,
        dlon,
        dlat,
        nodata_count: elev.iter().filter(|v| v.is_nan()).count() as u64,
        elev,
        point_registered: true,
        crs: 4269,
        vertical_cs: None,
    }
}

// REQ: OA-MSAW-051
#[test]
fn the_coarse_terrain_lookup_is_never_below_the_fine_lookup() {
    let mut rng = Lcg(7);
    let (mut both, mut complete_fine, mut refused_by_coarse_only) = (0u32, 0u32, 0u32);
    // Lattices of different spacing in the two axes, the largest of 3DEP's and finer ones.
    for (dlat, dlon) in [
        (0.000_277_8, 0.000_277_8),
        (0.0005, 0.0007),
        (0.0013, 0.0009),
    ] {
        let (w, h) = (360u32, 360u32);
        let (lat0, lon0) = (42.0 + 0.0123, -88.0 + 0.0071);
        let mut g = grid(&mut rng, w, h, lat0, lon0, dlat, dlon);
        // No nodata in the first grid of each lattice for the "complete" comparisons to dominate.
        if dlat > 0.001 {
            g.elev.iter_mut().for_each(|v| {
                if v.is_nan() {
                    *v = 10.0;
                }
            });
        }
        let mut c = CoarseTerrain::default();
        c.add_grid(&g);
        let set = TileSet { tiles: vec![g] };
        for _ in 0..3000 {
            let lat = lat0 - 0.002 - rng.next() * (f64::from(h) * dlat - 0.004);
            let lon = lon0 + 0.002 + rng.next() * (f64::from(w) * dlon - 0.004);
            let radius_m = 30.0 + rng.next() * 900.0;
            let fine = max_elevation_in_radius_conservative(&set, lat, lon, radius_m / 1852.0);
            let coarse = c.lookup(lat, lon, radius_m);
            match (fine, coarse) {
                (Ok(TerrainMax::Complete { peak, .. }), Coarse::Peak(ft)) => {
                    complete_fine += 1;
                    both += 1;
                    assert!(
                        ft >= ceil_ft(f64::from(peak.metres)),
                        "coarse {ft} below fine at {lat} {lon} r {radius_m}"
                    );
                }
                (Ok(TerrainMax::Complete { .. }), _) => {
                    // The coarse lookup may refuse near the grid's edge; never a lower number.
                    complete_fine += 1;
                }
                (Ok(TerrainMax::Incomplete { .. }), Coarse::Peak(_)) => {
                    refused_by_coarse_only += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        refused_by_coarse_only, 0,
        "an answer where the fine one is absent"
    );
    assert!(both > 3000, "{both} comparisons of {complete_fine}");
}

#[test]
fn a_cell_that_no_grid_holds_whole_is_missing_and_one_on_a_nodata_sample_is_nodata() {
    let mut rng = Lcg(3);
    let mut g = grid(&mut rng, 200, 200, 42.1, -88.1, 0.0005, 0.0005);
    g.elev.iter_mut().for_each(|v| *v = 100.0);
    g.elev[100 * 200 + 100] = f32::NAN;
    let mut c = CoarseTerrain::default();
    c.add_grid(&g);
    // Inside, away from the hole: the value, 100 m is 328.08 ft, up to 329.
    assert_eq!(c.lookup(42.075, -88.075, 100.0), Coarse::Peak(329));
    // Around the nodata sample.
    assert_eq!(c.lookup(42.1 - 0.05, -88.1 + 0.05, 100.0), Coarse::Nodata);
    // Reaching past the grid.
    assert_eq!(c.lookup(42.1 - 0.001, -88.0, 100.0), Coarse::Missing);
    assert_eq!(c.lookup(10.0, -88.0, 100.0), Coarse::Missing);
    // Too large a circle is refused, not answered.
    assert_eq!(c.lookup(42.05, -88.05, 1.0e6), Coarse::TooLarge);
}

fn add_sector(m: &mut Model, set: SetKind, alt: i32, ring: &[(f64, f64)]) {
    let e7 = |x: f64| (x * 1e7).round() as i32;
    let first = m.vert_lat_e7.len() as u32;
    let (mut a0, mut a1, mut b0, mut b1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(la, lo) in ring {
        m.vert_lat_e7.push(e7(la));
        m.vert_lon_e7.push(e7(lo));
        a0 = a0.min(e7(la));
        a1 = a1.max(e7(la));
        b0 = b0.min(e7(lo));
        b1 = b1.max(e7(lo));
    }
    let ring_ix = m.ring_vert_first.len() as u32;
    m.facility.push(*b"TST\0");
    m.set.push(set);
    m.sub.push([0; 4]);
    m.sector_id.push(*b"A\0\0\0\0\0\0\0");
    m.min_alt_ft.push(alt);
    m.ring_first.push(ring_ix);
    m.ring_count.push(1);
    m.bbox_lat_min.push(a0);
    m.bbox_lat_max.push(a1);
    m.bbox_lon_min.push(b0);
    m.bbox_lon_max.push(b1);
    m.ring_vert_first.push(first);
    m.ring_vert_count.push(ring.len() as u32);
    if m.ring_hole.len() * 64 <= ring_ix as usize {
        m.ring_hole.push(0);
    }
    let s = m.chart_facility.len() as u32;
    m.chart_facility.push(*b"TST\0");
    m.chart_set.push(set);
    m.chart_sub.push([0; 4]);
    m.chart_first_sector.push(s);
    m.chart_sector_count.push(1);
}

fn star(rng: &mut Lcg, clat: f64, clon: f64, rad: f64, n: usize) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    for k in 0..n {
        let a = std::f64::consts::TAU * (k as f64) / (n as f64);
        let r = rad * (0.4 + 0.6 * rng.next());
        v.push((clat + r * a.sin(), clon + r * a.cos() * 1.3));
    }
    v.push(v[0]);
    v
}

// REQ: OA-MSAW-052
#[test]
fn the_chart_raster_is_never_below_the_sector_lookup_and_is_empty_where_there_is_no_sector() {
    let mut rng = Lcg(11);
    let mut m = new_model();
    for k in 0..12 {
        let set = [SetKind::Fus3, SetKind::Fus5, SetKind::Tav][k % 3];
        let (clat, clon) = (41.2 + rng.next() * 0.6, -88.8 + rng.next() * 0.6);
        let rad = 0.03 + rng.next() * 0.12;
        let ring = star(&mut rng, clat, clon, rad, 5 + k);
        add_sector(&mut m, set, 2000 + 100 * (k as i32), &ring);
    }
    let mut r = ChartRaster::new(m);
    let (mut some, mut none) = (0u32, 0u32);
    for _ in 0..6000 {
        let (lat, lon) = (41.0 + rng.next(), -89.0 + rng.next());
        let mut best = None::<i32>;
        for set in [SetKind::Fus3, SetKind::Fus5, SetKind::Tav] {
            if let Ok(Some(a)) = min_altitude_at(r.model(), lat, lon, set) {
                best = Some(best.map_or(a.min_alt_ft, |b| b.max(a.min_alt_ft)));
            }
        }
        let got = r.at(lat, lon);
        match (best, got) {
            (Some(b), Some(g)) => {
                some += 1;
                assert!(g >= b, "raster {g} below the sectors {b} at {lat} {lon}");
            }
            (Some(b), None) => panic!("no chart in the raster where a sector gives {b}"),
            (None, Some(_)) | (None, None) => none += 1,
        }
    }
    assert!(some > 500 && none > 500, "{some} inside, {none} outside");
    assert_eq!(r.blocks(), 1);
}

#[test]
fn a_cell_on_a_sectors_edge_has_its_altitude_and_a_cell_beyond_it_has_none() {
    let mut m = new_model();
    let ring = [
        (41.5, -88.5),
        (41.5, -88.4),
        (41.6, -88.4),
        (41.6, -88.5),
        (41.5, -88.5),
    ];
    add_sector(&mut m, SetKind::Fus3, 3100, &ring);
    let mut r = ChartRaster::new(m);
    assert_eq!(r.at(41.55, -88.45), Some(3100));
    // 0.0005 degree outside the south edge is in the edge's own cell (0.002 degree): conservative.
    assert_eq!(r.at(41.4995, -88.45), Some(3100));
    assert_eq!(r.at(41.490, -88.45), None);
    assert_eq!(r.at(41.55, -88.39), None);
}

// REQ: OA-MSAW-051
#[test]
fn a_sweep_of_every_radius_and_cell_offset_over_small_grids_never_finds_the_coarse_below_the_fine()
{
    let mut rng = Lcg(99);
    let (mut compared, mut worst_gap_ft) = (0u32, i32::MAX);
    for (dlat, dlon) in [(0.000_277_8, 0.000_277_8), (0.0004, 0.0003)] {
        let (w, h) = (160u32, 160u32);
        let (lat0, lon0) = (42.0 + 0.0003, -88.0 + 0.0001);
        let mut g = grid(&mut rng, w, h, lat0, lon0, dlat, dlon);
        g.elev.iter_mut().for_each(|v| {
            if v.is_nan() {
                *v = 5.0;
            }
        });
        let mut c = CoarseTerrain::default();
        c.add_grid(&g);
        let set = TileSet { tiles: vec![g] };
        // Every radius from 25 m to 800 m in steps of 25, and a 12 by 12 lattice of positions
        // across one cell (0.002 degree), well inside the grid.
        for r in (1..=32).map(|k| f64::from(k) * 25.0) {
            for a in 0..12 {
                for b in 0..12 {
                    let lat = 42.0 - 0.012 + 0.002 * f64::from(a) / 12.0;
                    let lon = -88.0 + 0.012 + 0.002 * f64::from(b) / 12.0;
                    let fine = max_elevation_in_radius_conservative(&set, lat, lon, r / 1852.0);
                    let coarse = c.lookup(lat, lon, r);
                    if let (Ok(TerrainMax::Complete { peak, .. }), Coarse::Peak(ft)) =
                        (fine, coarse)
                    {
                        let f = ceil_ft(f64::from(peak.metres));
                        assert!(ft >= f, "coarse {ft} below fine {f} at {lat} {lon} r {r}");
                        compared += 1;
                        worst_gap_ft = worst_gap_ft.min(ft - f);
                    }
                }
            }
        }
    }
    assert!(compared > 4000, "{compared}");
    assert!(worst_gap_ft >= 0);
}
