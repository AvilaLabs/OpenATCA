#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//! Tests of the warning-grade terrain maxima.

use crate::conservative::{collect, collect_counted};
use crate::grid::check_polygon;
use crate::*;

const D: f64 = 0.01;

/// A flat grid of `base` metres, `w` by `h` samples, north-west sample centre at (40.0, -90.0).
fn flat(w: u32, h: u32, base: f32) -> Grid {
    Grid {
        width: w,
        height: h,
        lon0: -90.0,
        lat0: 40.0,
        dlon: D,
        dlat: D,
        elev: vec![base; (w * h) as usize],
        nodata_count: 0,
        point_registered: false,
        crs: 4269,
        vertical_cs: None,
    }
}

fn set_at(g: &mut Grid, col: u32, row: u32, v: f32) {
    g.elev[(row * g.width + col) as usize] = v;
}

fn lat_of(g: &Grid, row: u32) -> f64 {
    g.lat0 - f64::from(row) * g.dlat
}

fn lon_of(g: &Grid, col: u32) -> f64 {
    g.lon0 + f64::from(col) * g.dlon
}

fn complete_metres(t: &Result<TerrainMax, QueryError>) -> f32 {
    match t {
        Ok(TerrainMax::Complete { peak, .. }) => peak.metres,
        other => panic!("not complete: {other:?}"),
    }
}

/// WGS 84 geodesic distance by Vincenty's inverse formula, metres (test reference).
fn vincenty(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let a = 6_378_137.0_f64;
    let f = 1.0 / 298.257_223_563;
    let b = a * (1.0 - f);
    let u1 = ((1.0 - f) * lat1.to_radians().tan()).atan();
    let u2 = ((1.0 - f) * lat2.to_radians().tan()).atan();
    let l = (lon2 - lon1).to_radians();
    let (su1, cu1, su2, cu2) = (u1.sin(), u1.cos(), u2.sin(), u2.cos());
    let mut lam = l;
    let (mut ss, mut cs, mut sig, mut c2a, mut c2sm) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for _ in 0..200 {
        let (sl, cl) = (lam.sin(), lam.cos());
        ss = ((cu2 * sl).powi(2) + (cu1 * su2 - su1 * cu2 * cl).powi(2)).sqrt();
        if ss == 0.0 {
            return 0.0;
        }
        cs = su1 * su2 + cu1 * cu2 * cl;
        sig = ss.atan2(cs);
        let sa = cu1 * cu2 * sl / ss;
        c2a = 1.0 - sa * sa;
        c2sm = if c2a != 0.0 {
            cs - 2.0 * su1 * su2 / c2a
        } else {
            0.0
        };
        let c = f / 16.0 * c2a * (4.0 + f * (4.0 - 3.0 * c2a));
        let next =
            l + (1.0 - c) * f * sa * (sig + c * ss * (c2sm + c * cs * (-1.0 + 2.0 * c2sm * c2sm)));
        let done = (next - lam).abs() < 1e-12;
        lam = next;
        if done {
            break;
        }
    }
    let u2 = c2a * (a * a - b * b) / (b * b);
    let ka = 1.0 + u2 / 16384.0 * (4096.0 + u2 * (-768.0 + u2 * (320.0 - 175.0 * u2)));
    let kb = u2 / 1024.0 * (256.0 + u2 * (-128.0 + u2 * (74.0 - 47.0 * u2)));
    let ds = kb
        * ss
        * (c2sm
            + kb / 4.0
                * (cs * (-1.0 + 2.0 * c2sm * c2sm)
                    - kb / 6.0 * c2sm * (-3.0 + 4.0 * ss * ss) * (-3.0 + 4.0 * c2sm * c2sm)));
    b * ka * (sig - ds)
}

#[test]
fn vincenty_reference_is_right() {
    // Flinders Peak to Buninyong (Vincenty's own worked example): 54972.271 m.
    let d = vincenty(
        -(37.0 + 57.0 / 60.0 + 3.72030 / 3600.0),
        144.0 + 25.0 / 60.0 + 29.52440 / 3600.0,
        -(37.0 + 39.0 / 60.0 + 10.15610 / 3600.0),
        143.0 + 55.0 / 60.0 + 35.38390 / 3600.0,
    );
    assert!((d - 54_972.271).abs() < 0.01, "{d}");
}

// REQ: OA-TIFF-020
#[test]
fn clipped_cell_and_subcell_queries() {
    // A high sample whose centre is outside the circle but whose cell the circle clips.
    let mut g = flat(21, 21, 100.0);
    set_at(&mut g, 10, 10, 900.0);
    let (slat, slon) = (lat_of(&g, 10), lon_of(&g, 10));
    // Centre 0.03 degrees (2.5 km) west of the spike, radius 1.2 nm (2.2 km): the spike centre is
    // outside, the nearest point of its cell (one step, 0.8 km, nearer) is inside.
    let (clat, clon, rnm) = (slat, slon - 0.03, 1.2);
    assert!(vincenty(clat, clon, slat, slon) > rnm * METRES_PER_NM);
    let old = grid_max_in_radius(&g, clat, clon, rnm).unwrap();
    assert_eq!(old.peak.unwrap().metres, 100.0);
    let new = grid_max_in_radius_conservative(&g, clat, clon, rnm);
    assert_eq!(complete_metres(&new), 900.0);
    // The same spike two steps further out is not counted: over-inclusion stays bounded.
    let (clat, clon) = (slat, slon - 0.06);
    let new = grid_max_in_radius_conservative(&g, clat, clon, rnm);
    assert_eq!(complete_metres(&new), 100.0);

    // A query smaller than one cell: the old query finds nothing, the new one the four corners.
    let mut g = flat(9, 9, 10.0);
    set_at(&mut g, 4, 4, 70.0);
    set_at(&mut g, 5, 4, 20.0);
    set_at(&mut g, 4, 5, 30.0);
    set_at(&mut g, 5, 5, 40.0);
    let (clat, clon) = (lat_of(&g, 4) - 0.004, lon_of(&g, 4) + 0.004);
    let old = grid_max_in_radius(&g, clat, clon, 0.01).unwrap();
    assert_eq!(old.peak, None);
    let new = grid_max_in_radius_conservative(&g, clat, clon, 0.01);
    assert_eq!(complete_metres(&new), 70.0);
    // Only that quad: a corner that is lower is not what is reported when the spike is elsewhere.
    set_at(&mut g, 4, 4, 5.0);
    let new = grid_max_in_radius_conservative(&g, clat, clon, 0.01);
    assert_eq!(complete_metres(&new), 40.0);
}

// REQ: OA-TIFF-021
#[test]
fn radius_on_the_ellipsoid() {
    // Never under-include: a lone high sample exactly inside the WGS 84 radius is found, for
    // bearings from every side and at latitudes from the equator to 60 degrees.
    for &lat in &[0.5, 20.0, 41.0, 60.0] {
        let mut worst_ratio = 0.0_f64;
        for k in 0..16 {
            let brg = f64::from(k) * 22.5_f64.to_radians();
            let mut g = flat(41, 41, 0.0);
            g.lat0 = lat + 0.2;
            g.lon0 = -90.2;
            // The spike at the sample nearest the point 4.9 nm from the centre on this bearing.
            let (clat, clon) = (lat, -90.0);
            let r_m = 5.0 * METRES_PER_NM;
            let want_d = 0.98 * r_m;
            let dlat = want_d * brg.cos() / 111_000.0;
            let dlon = want_d * brg.sin() / (111_000.0 * lat.to_radians().cos());
            let col = ((clon + dlon - g.lon0) / D).round() as u32;
            let row = ((g.lat0 - (clat + dlat)) / D).round() as u32;
            set_at(&mut g, col, row, 1000.0);
            let d = vincenty(clat, clon, lat_of(&g, row), lon_of(&g, col));
            let new = grid_max_in_radius_conservative(&g, clat, clon, 5.0);
            let m = match new {
                Ok(TerrainMax::Complete { peak, .. }) => peak.metres,
                other => panic!("{other:?}"),
            };
            if d <= r_m {
                assert_eq!(
                    m, 1000.0,
                    "lat {lat} brg {k}: spike at {d} m inside {r_m} m"
                );
            }
            worst_ratio = worst_ratio.max(d / r_m);
        }
        assert!(worst_ratio > 0.9);
    }
    // Over-inclusion bound at 41 degrees, 5 nm: a spike more than the radius plus the diagonal of
    // one step plus 0.1 percent away is never counted.
    let mut g = flat(41, 41, 0.0);
    g.lat0 = 41.2;
    g.lon0 = -90.2;
    let (clat, clon) = (41.0, -90.0);
    let r_m = 5.0 * METRES_PER_NM;
    for row in 0..41 {
        for col in 0..41 {
            let d = vincenty(clat, clon, lat_of(&g, row), lon_of(&g, col));
            let mut h = g.clone();
            set_at(&mut h, col, row, 500.0);
            let m = complete_metres(&grid_max_in_radius_conservative(&h, clat, clon, 5.0));
            if d > r_m * 1.001 + 1500.0 {
                assert_eq!(m, 0.0, "spike at {d} m counted");
            }
        }
    }
    // The antimeridian and the poles are refused.
    let g = flat(3, 3, 0.0);
    assert_eq!(
        grid_max_in_radius_conservative(&g, 40.0, 179.99, 5.0),
        Err(QueryError::LongitudeWraps)
    );
    assert_eq!(
        grid_max_in_radius_conservative(&g, 40.0, -179.99, 5.0),
        Err(QueryError::LongitudeWraps)
    );
    assert_eq!(
        grid_max_in_radius_conservative(&g, 89.99, 0.0, 5.0),
        Err(QueryError::LongitudeWraps)
    );
}

// REQ: OA-TIFF-022
#[test]
fn polygon_edge_clipping() {
    let mut g = flat(21, 21, 100.0);
    set_at(&mut g, 10, 10, 800.0);
    let (slat, slon) = (lat_of(&g, 10), lon_of(&g, 10));
    // A box whose east edge stops half a step short of the spike's centre: the centre is outside.
    let boxp = [
        (slat + 0.02, slon - 0.03),
        (slat + 0.02, slon - 0.005),
        (slat - 0.02, slon - 0.005),
        (slat - 0.02, slon - 0.03),
    ];
    let old = grid_max_in_polygon(&g, &boxp).unwrap();
    assert_eq!(old.peak.unwrap().metres, 100.0);
    assert_eq!(
        complete_metres(&grid_max_in_polygon_conservative(&g, &boxp)),
        800.0
    );
    // Two steps short: not counted.
    let far = boxp.map(|(la, lo)| (la, lo - 0.015));
    assert_eq!(
        complete_metres(&grid_max_in_polygon_conservative(&g, &far)),
        100.0
    );
    // A triangle smaller than a cell, wholly inside one quad (no edge reaches a sample).
    let tri = [
        (slat - 0.002, slon + 0.002),
        (slat - 0.002, slon + 0.004),
        (slat - 0.004, slon + 0.003),
    ];
    assert_eq!(grid_max_in_polygon(&g, &tri).unwrap().peak, None);
    assert_eq!(
        complete_metres(&grid_max_in_polygon_conservative(&g, &tri)),
        800.0
    );
    // A concave polygon (a U) whose notch holds the spike is still conservative at the notch:
    // the notch walls are within a step of the spike centre.
    let u = [
        (slat + 0.03, slon - 0.03),
        (slat + 0.03, slon + 0.03),
        (slat - 0.03, slon + 0.03),
        (slat - 0.03, slon + 0.006),
        (slat + 0.01, slon + 0.006),
        (slat + 0.01, slon - 0.006),
        (slat - 0.03, slon - 0.006),
        (slat - 0.03, slon - 0.03),
    ];
    assert_eq!(
        complete_metres(&grid_max_in_polygon_conservative(&g, &u)),
        800.0
    );
    // Wholly inside a large polygon that has no edge near a quad: counted through the interior.
    let big = [
        (slat + 0.07, slon - 0.07),
        (slat + 0.07, slon + 0.07),
        (slat - 0.07, slon + 0.07),
        (slat - 0.07, slon - 0.07),
    ];
    assert_eq!(
        complete_metres(&grid_max_in_polygon_conservative(&g, &big)),
        800.0
    );
}

/// Cuts columns `c0..=c1` out of a grid.
fn cut(whole: &Grid, c0: u32, c1: u32) -> Grid {
    let mut elev = Vec::new();
    for r in 0..whole.height {
        for c in c0..=c1 {
            elev.push(whole.elev[(r * whole.width + c) as usize]);
        }
    }
    Grid {
        width: c1 - c0 + 1,
        lon0: whole.lon0 + f64::from(c0) * whole.dlon,
        elev,
        ..whole.clone()
    }
}

// REQ: OA-TIFF-023
#[test]
fn seam_between_two_tiles() {
    let mut whole = flat(21, 11, 50.0);
    // Spikes on either side of the seam between columns 9 and 10, and one at a tile edge.
    set_at(&mut whole, 10, 5, 400.0);
    set_at(&mut whole, 9, 2, 300.0);
    let abut = TileSet {
        tiles: vec![cut(&whole, 0, 9), cut(&whole, 10, 20)],
    };
    let overlap = TileSet {
        tiles: vec![cut(&whole, 0, 12), cut(&whole, 8, 20)],
    };
    // Centres whose circle reaches the spikes only through their footprint, on both sides.
    for &(clat, clon, rnm) in &[
        (lat_of(&whole, 5), lon_of(&whole, 10) - 0.025, 1.0),
        (lat_of(&whole, 5), lon_of(&whole, 10) + 0.025, 1.2),
        (lat_of(&whole, 2), lon_of(&whole, 9) + 0.03, 1.1),
        (lat_of(&whole, 4), lon_of(&whole, 10) - 0.005, 0.3),
    ] {
        let want = grid_max_in_radius_conservative(&whole, clat, clon, rnm);
        for set in [&abut, &overlap] {
            let got = max_elevation_in_radius_conservative(set, clat, clon, rnm);
            let (
                Ok(TerrainMax::Complete { peak: a, .. }),
                Ok(TerrainMax::Complete { peak: b, .. }),
            ) = (&want, &got)
            else {
                panic!("{want:?} {got:?}");
            };
            assert_eq!(a.metres, b.metres);
        }
    }
    // A circle sitting on the east side sees the spike in the west tile's last column.
    let m = max_elevation_in_radius_conservative(
        &abut,
        lat_of(&whole, 2),
        lon_of(&whole, 9) + 0.03,
        1.1,
    );
    assert_eq!(complete_metres(&m), 300.0);
    // A polygon across the seam.
    let boxp = [
        (lat_of(&whole, 4), lon_of(&whole, 8)),
        (lat_of(&whole, 4), lon_of(&whole, 12)),
        (lat_of(&whole, 6), lon_of(&whole, 12)),
        (lat_of(&whole, 6), lon_of(&whole, 8)),
    ];
    let want = grid_max_in_polygon_conservative(&whole, &boxp);
    for set in [&abut, &overlap] {
        assert_eq!(max_elevation_in_polygon_conservative(set, &boxp), want);
    }
    // Overlapping tiles where one holds nodata and the other a value: the value wins.
    let mut holed = cut(&whole, 0, 12);
    set_at(&mut holed, 10, 5, f32::NAN);
    let set = TileSet {
        tiles: vec![holed, cut(&whole, 8, 20)],
    };
    let m = max_elevation_in_radius_conservative(&set, lat_of(&whole, 5), lon_of(&whole, 10), 1.0);
    assert_eq!(complete_metres(&m), 400.0);
}

// REQ: OA-TIFF-024
#[test]
fn incomplete_answers_are_typed() {
    let mut whole = flat(21, 11, 50.0);
    set_at(&mut whole, 9, 5, 123.0);
    let west = cut(&whole, 0, 9);
    let set = TileSet { tiles: vec![west] };
    // A circle over the seam with only the west tile loaded: east samples are missing.
    let (clat, clon) = (lat_of(&whole, 5), lon_of(&whole, 9) + 0.005);
    match max_elevation_in_radius_conservative(&set, clat, clon, 2.0) {
        Ok(TerrainMax::Incomplete {
            peak,
            missing_nodes,
            missing_squares,
            nodata_nodes,
            ..
        }) => {
            assert!(missing_nodes > 0);
            assert_eq!(missing_squares, 1);
            assert_eq!(nodata_nodes, 0);
            assert_eq!(peak.unwrap().metres, 123.0);
        }
        other => panic!("{other:?}"),
    }
    // Missing squares are counted by whole-degree square: a grid straddling latitude 40 with only
    // its northern rows loaded misses the 39 square.
    let mut tall = flat(11, 21, 10.0);
    tall.lat0 = 40.1;
    let north = Grid {
        height: 11,
        elev: tall.elev[..(11 * 11)].to_vec(),
        ..tall.clone()
    };
    let r =
        max_elevation_in_radius_conservative(&TileSet { tiles: vec![north] }, 39.97, -89.95, 1.0);
    assert!(matches!(
        r,
        Ok(TerrainMax::Incomplete {
            missing_squares: 1,
            ..
        })
    ));
    // Data edge: the area reaching beyond the outermost samples is incomplete.
    let r = grid_max_in_radius_conservative(&whole, lat_of(&whole, 5), lon_of(&whole, 0), 1.0);
    assert!(matches!(r, Ok(TerrainMax::Incomplete { missing_nodes, .. }) if missing_nodes > 0));
    // A tile set that holds nothing near the query.
    let r = grid_max_in_radius_conservative(&whole, 10.0, 10.0, 1.0);
    assert!(matches!(
        r,
        Ok(TerrainMax::Incomplete {
            peak: None,
            nodes: 0,
            ..
        })
    ));
    // Nodata inside the area is reported, not skipped, even when higher samples exist.
    let mut holed = flat(11, 11, 80.0);
    set_at(&mut holed, 5, 5, f32::NAN);
    set_at(&mut holed, 3, 3, 90.0);
    let r = grid_max_in_radius_conservative(&holed, lat_of(&holed, 5), lon_of(&holed, 5), 1.0);
    match r {
        Ok(TerrainMax::Incomplete {
            peak,
            nodata_nodes,
            missing_nodes,
            ..
        }) => {
            assert_eq!(nodata_nodes, 1);
            assert_eq!(missing_nodes, 0);
            assert_eq!(peak.unwrap().metres, 90.0);
        }
        other => panic!("{other:?}"),
    }
    let poly = [
        (lat_of(&holed, 4), lon_of(&holed, 4)),
        (lat_of(&holed, 4), lon_of(&holed, 6)),
        (lat_of(&holed, 6), lon_of(&holed, 6)),
    ];
    assert!(matches!(
        grid_max_in_polygon_conservative(&holed, &poly),
        Ok(TerrainMax::Incomplete {
            nodata_nodes: 1,
            ..
        })
    ));
}

// REQ: OA-TIFF-025
#[test]
fn refusals() {
    let g = flat(5, 5, 1.0);
    let empty = TileSet::default();
    assert_eq!(
        max_elevation_in_radius_conservative(&empty, 39.98, -89.98, 1.0),
        Err(QueryError::NoTiles)
    );
    assert_eq!(
        max_elevation_in_polygon_conservative(
            &empty,
            &[(39.98, -89.98), (39.98, -89.97), (39.97, -89.97)]
        ),
        Err(QueryError::NoTiles)
    );
    for r in [0.0, -1.0, f64::NAN, f64::INFINITY, MAX_RADIUS_NM + 1.0] {
        assert_eq!(
            grid_max_in_radius_conservative(&g, 39.98, -89.98, r),
            Err(QueryError::BadRadius)
        );
    }
    assert_eq!(
        grid_max_in_radius_conservative(&g, f64::NAN, -89.98, 1.0),
        Err(QueryError::BadCoordinate)
    );
    assert_eq!(
        grid_max_in_radius_conservative(&g, 91.0, -89.98, 1.0),
        Err(QueryError::BadCoordinate)
    );
    let two = [(39.98, -89.98), (39.98, -89.97)];
    assert_eq!(
        grid_max_in_polygon_conservative(&g, &two),
        Err(QueryError::BadPolygon)
    );
    let wide = [(0.0, -100.0), (0.0, 100.0), (1.0, 0.0)];
    assert_eq!(
        grid_max_in_polygon_conservative(&g, &wide),
        Err(QueryError::BadPolygon)
    );
    let nan = [(39.98, -89.98), (39.98, f64::NAN), (39.97, -89.97)];
    assert_eq!(
        grid_max_in_polygon_conservative(&g, &nan),
        Err(QueryError::BadCoordinate)
    );
    // Tiles that do not share a lattice are refused, not guessed at.
    let mut shifted = g.clone();
    shifted.lat0 += 0.004;
    let bad = TileSet {
        tiles: vec![g.clone(), shifted],
    };
    assert_eq!(
        max_elevation_in_radius_conservative(&bad, 39.98, -89.98, 1.0),
        Err(QueryError::TilesNotAligned)
    );
    let mut other_step = g.clone();
    other_step.dlon = 0.02;
    let bad = TileSet {
        tiles: vec![g.clone(), other_step],
    };
    assert_eq!(
        max_elevation_in_radius_conservative(&bad, 39.98, -89.98, 1.0),
        Err(QueryError::TilesNotAligned)
    );
    let many = TileSet {
        tiles: vec![g.clone(); MAX_TILES + 1],
    };
    assert_eq!(
        max_elevation_in_radius_conservative(&many, 39.98, -89.98, 1.0),
        Err(QueryError::TooManyTiles)
    );
    // A region over the sample limit on a fine lattice.
    let mut fine = g;
    fine.dlat = 1e-7;
    fine.dlon = 1e-7;
    assert_eq!(
        grid_max_in_radius_conservative(&fine, 39.98, -89.98, 400.0),
        Err(QueryError::TooManySamples)
    );
}

/// A small deterministic generator (xorshift64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Brute-force reference: the highest value of the bilinear surface and of the nearest-sample
/// surface over points on a lattice 1/16 of a step wide, among those `inside` accepts.
fn brute_max(g: &Grid, inside: &dyn Fn(f64, f64) -> bool) -> Option<f32> {
    let sub = 16;
    let mut best: Option<f32> = None;
    for iy in 0..=(g.height - 1) * sub {
        for ix in 0..=(g.width - 1) * sub {
            let (fx, fy) = (
                f64::from(ix) / f64::from(sub),
                f64::from(iy) / f64::from(sub),
            );
            let (lat, lon) = (g.lat0 - fy * g.dlat, g.lon0 + fx * g.dlon);
            if !inside(lat, lon) {
                continue;
            }
            let near = grid_sample(g, fx.round() as u32, fy.round() as u32).unwrap();
            best = Some(best.map_or(near, |b| b.max(near)));
            if let Elevation::Metres(v) = grid_elevation_at(g, lat, lon) {
                best = best.map(|b| b.max(v));
            }
        }
    }
    best
}

fn random_grid(rng: &mut Rng, w: u32, h: u32) -> Grid {
    let mut g = flat(w, h, 0.0);
    for v in g.elev.iter_mut() {
        *v = (rng.next() % 1000) as f32;
    }
    g.lat0 = 41.0;
    g.lon0 = -88.0;
    g
}

fn peak_of(t: &Result<TerrainMax, QueryError>) -> f32 {
    match t {
        Ok(TerrainMax::Complete { peak, .. }) => peak.metres,
        Ok(TerrainMax::Incomplete { peak: Some(p), .. }) => p.metres,
        other => panic!("{other:?}"),
    }
}

// REQ: OA-TIFF-026
#[test]
fn property_against_brute_force_circle() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..300 {
        let (w, h) = (4 + (rng.next() % 5) as u32, 4 + (rng.next() % 5) as u32);
        let g = random_grid(&mut rng, w, h);
        let clat = g.lat0 - rng.unit() * f64::from(h - 1) * D;
        let clon = g.lon0 + rng.unit() * f64::from(w - 1) * D;
        let rnm = 0.02 + rng.unit() * 2.5;
        let r_m = rnm * METRES_PER_NM;
        let got = grid_max_in_radius_conservative(&g, clat, clon, rnm);
        let truth = brute_max(&g, &|la, lo| vincenty(clat, clon, la, lo) <= r_m);
        if let Some(t) = truth {
            assert!(
                peak_of(&got) >= t,
                "case {case}: conservative {} below true {t}",
                peak_of(&got)
            );
        }
        // And it is not absurdly high: no more than the best sample within the radius plus the
        // over-inclusion margin (0.5 percent of the radius and one step diagonal).
        let margin = r_m * 1.005 + 1500.0;
        let mut cap = f32::MIN;
        for row in 0..h {
            for col in 0..w {
                if vincenty(clat, clon, lat_of(&g, row), lon_of(&g, col)) <= margin {
                    cap = cap.max(g.elev[(row * w + col) as usize]);
                }
            }
        }
        assert!(
            peak_of(&got) <= cap,
            "case {case}: {} over cap {cap}",
            peak_of(&got)
        );
    }
}

fn in_poly_ref(poly: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.0 > lat) != (b.0 > lat) && lon < (b.1 - a.1) * (lat - a.0) / (b.0 - a.0) + a.1 {
            inside = !inside;
        }
    }
    inside
}

// REQ: OA-TIFF-027
#[test]
fn property_against_brute_force_polygon() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for case in 0..300 {
        let (w, h) = (4 + (rng.next() % 5) as u32, 4 + (rng.next() % 5) as u32);
        let g = random_grid(&mut rng, w, h);
        let nv = 3 + (rng.next() % 4) as usize;
        let poly: Vec<(f64, f64)> = (0..nv)
            .map(|_| {
                (
                    g.lat0 - rng.unit() * f64::from(h - 1) * D,
                    g.lon0 + rng.unit() * f64::from(w - 1) * D,
                )
            })
            .collect();
        let got = grid_max_in_polygon_conservative(&g, &poly);
        let truth = brute_max(&g, &|la, lo| in_poly_ref(&poly, la, lo));
        if let Some(t) = truth {
            assert!(
                peak_of(&got) >= t,
                "case {case}: conservative {} below true {t}",
                peak_of(&got)
            );
        }
        let all = g.elev.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak_of(&got) <= all);
    }
}

// REQ: OA-TIFF-080
#[test]
fn the_antimeridian_aliases_give_one_elevation() {
    // A tile whose west edge is -180 and one whose east edge is +180, both flat.
    let mut west = flat(11, 11, 120.0);
    west.lon0 = -180.0;
    let mut east = flat(11, 11, 120.0);
    east.lon0 = 179.9;
    for tile in [west, east] {
        let set = TileSet { tiles: vec![tile] };
        for lon in [-180.0, 180.0] {
            let e = elevation_at(&set, 39.95, lon);
            assert_eq!(e, Elevation::Metres(120.0), "lon {lon}");
            assert!(tileset_covers(&set, 39.95, lon), "lon {lon}");
            assert_eq!(
                grid_elevation_at(&set.tiles[0], 39.95, lon),
                Elevation::Metres(120.0),
                "lon {lon}"
            );
        }
    }
}

// REQ: OA-TIFF-084
#[test]
fn infinite_samples_make_a_conservative_maximum_incomplete() {
    let mut g = flat(5, 5, 100.0);
    for r in 0..5 {
        for c in 0..5 {
            set_at(&mut g, c, r, f32::NEG_INFINITY);
        }
    }
    set_at(&mut g, 2, 2, 120.0);
    set_at(&mut g, 3, 3, f32::INFINITY);
    let (la, lo) = (lat_of(&g, 2), lon_of(&g, 2));
    let b = 0.045;
    let poly = [
        (la + b, lo - b),
        (la + b, lo + b),
        (la - b, lo + b),
        (la - b, lo - b),
    ];
    let t = grid_max_in_polygon_conservative(&g, &poly);
    match t {
        Ok(TerrainMax::Incomplete {
            peak, nodata_nodes, ..
        }) => {
            assert_eq!(nodata_nodes, 24, "{peak:?}");
            assert_eq!(peak.map(|p| p.metres), Some(120.0));
        }
        other => panic!("not incomplete: {other:?}"),
    }
}

/// A flat grid on a one arc-second lattice, four samples across, with its corner at (40, -90).
fn one_arc_second() -> Grid {
    let mut g = flat(4, 4, 100.0);
    g.dlat = 1.0 / 3600.0;
    g.dlon = 1.0 / 3600.0;
    g
}

// REQ: OA-TIFF-087
#[test]
fn a_polygon_too_large_for_the_work_budget_is_refused_before_any_cell_is_visited() {
    let g = one_arc_second();
    // 12 by 12 degrees: about 1.87e9 cells, under the sample limit, times the vertices.
    let poly = [(40.0, -90.0), (40.0, -78.0), (28.0, -78.0), (28.0, -90.0)];
    let visits = std::cell::Cell::new(0u64);
    let hit = |_: f64, _: f64| {
        visits.set(visits.get() + 1);
        true
    };
    let bbox = check_polygon(&poly).unwrap();
    assert_eq!(
        collect(std::slice::from_ref(&g), bbox, poly.len(), &hit),
        Err(QueryError::TooMuchWork)
    );
    assert_eq!(visits.get(), 0);
    // The public entry point refuses the same query, and a polygon of many vertices with it.
    assert_eq!(
        grid_max_in_polygon_conservative(&g, &poly),
        Err(QueryError::TooMuchWork)
    );
    let many: Vec<(f64, f64)> = (0..10_000)
        .map(|i| {
            let a = f64::from(i) * std::f64::consts::TAU / 10_000.0;
            (34.0 + 6.0 * a.sin(), -84.0 + 6.0 * a.cos())
        })
        .collect();
    assert_eq!(
        grid_max_in_polygon_conservative(&g, &many),
        Err(QueryError::TooMuchWork)
    );
    // The same cells with a constant-cost test are within the budget and are visited.
    let visits_ok = std::cell::Cell::new(0u64);
    let small = (40.0 - 0.01, 40.0, -90.0, -90.0 + 0.01);
    let hit_ok = |_: f64, _: f64| {
        visits_ok.set(visits_ok.get() + 1);
        false
    };
    assert!(collect(std::slice::from_ref(&g), small, 0, &hit_ok).is_ok());
    assert!(visits_ok.get() > 0);
}

// REQ: OA-TIFF-087
#[test]
fn an_ordinary_sector_sized_query_is_within_the_work_budget() {
    // A 20-vertex sector 3 nm across on a one arc-second lattice, and a 1 nm radius on a 1/3
    // arc-second one: well under the budget, and they answer.
    let g = one_arc_second();
    let sector: Vec<(f64, f64)> = (0..20)
        .map(|i| {
            let a = f64::from(i) * std::f64::consts::TAU / 20.0;
            (39.95 + 0.025 * a.sin(), -89.95 + 0.025 * a.cos())
        })
        .collect();
    assert!(grid_max_in_polygon_conservative(&g, &sector).is_ok());
    let mut fine = flat(2, 2, 100.0);
    fine.dlat = 1.0 / 10_800.0;
    fine.dlon = 1.0 / 10_800.0;
    assert!(grid_max_in_radius_conservative(&fine, 39.99, -89.99, 1.0).is_ok());
}

/// `n` aligned tiles of 4 by 4 samples, all on the lattice of `one_arc_second`, overlapping.
fn overlapping(n: usize) -> Vec<Grid> {
    vec![one_arc_second(); n]
}

// REQ: OA-TIFF-088
#[test]
fn overlapping_tiles_multiply_the_work_estimate_and_an_over_budget_query_is_refused_first() {
    // 0.5 degree box on a one arc-second lattice: 3.2 million cells, each looked up in 1024
    // overlapping tiles when it passes, 3.3e9 lookups against a budget of 2.1e9.
    let tiles = overlapping(MAX_TILES);
    let bbox = (39.5, 40.0, -90.0, -89.5);
    let visits = std::cell::Cell::new(0u64);
    let hit = |_: f64, _: f64| {
        visits.set(visits.get() + 1);
        true
    };
    assert_eq!(collect(&tiles, bbox, 0, &hit), Err(QueryError::TooMuchWork));
    assert_eq!(visits.get(), 0);
}

// REQ: OA-TIFF-088
#[test]
fn the_work_estimate_is_at_least_the_work_counted() {
    let hit_all = |_: f64, _: f64| true;
    let hit_none = |_: f64, _: f64| false;
    let hit_half = |la: f64, _: f64| la > 39.99;
    let sq = (39.99, 40.0, -90.0, -89.99);
    for n in [1usize, 2, 16, 1024] {
        let tiles = overlapping(n);
        for (vertices, hit) in [
            (0usize, &hit_all as &dyn Fn(f64, f64) -> bool),
            (7, &hit_all),
            (0, &hit_none),
            (5, &hit_half),
        ] {
            let (_, w) = collect_counted(&tiles, sq, vertices, hit).unwrap();
            assert!(
                w.estimate >= w.counted as f64,
                "{n} tiles, {vertices} vertices: estimate {} < counted {}",
                w.estimate,
                w.counted
            );
        }
    }
    // With every cell passing, the tile multiplier is most of the counted work.
    let tiles = overlapping(1024);
    let (_, w) = collect_counted(&tiles, sq, 0, &hit_all).unwrap();
    assert!(w.counted > 1024 * 100);
}

// REQ: OA-TIFF-094
#[test]
fn a_box_at_a_lattice_index_too_large_to_hold_is_refused_not_clamped() {
    // A lattice step of 1e-300 degrees is a valid GeoTIFF scale; the box then sits at an index
    // near 1e300, which a clamp to i64 would turn into i64::MAX and overflow against a tile
    // that starts a few steps earlier.
    let tile = |lon0: f64| Grid {
        width: 2,
        height: 2,
        lon0,
        lat0: 40.0,
        dlon: 1e-300,
        dlat: 1e-300,
        elev: vec![1.0; 4],
        nodata_count: 0,
        point_registered: false,
        crs: 4269,
        vertical_cs: None,
    };
    let set = TileSet {
        tiles: vec![tile(-90.0), tile(-90.0 - 5e-300)],
    };
    // A polygon with no extent gives a box of one cell however fine the lattice.
    let poly = [(39.9, -90.05); 3];
    let r = max_elevation_in_polygon_conservative(&set, &poly);
    assert_eq!(r, Err(QueryError::TooManySamples), "{r:?}");
}
