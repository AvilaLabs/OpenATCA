//! Real-data validation against USGS 3DEP 1 arc-second GeoTIFF tiles, the FAA NASR airport
//! file and the FAA Digital Obstacle File. Reads `OPENATCA_DATA` (the openatca-data directory)
//! and skips when it is unset or the files are absent. Run with `--nocapture` for the reports.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_tiff::*;
use std::path::PathBuf;

fn data_dir() -> Option<PathBuf> {
    match std::env::var_os("OPENATCA_DATA") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => {
            eprintln!("OPENATCA_DATA is not set; skipping real-data test");
            None
        }
    }
}

/// The 3DEP tile directory, if it holds any tiles.
fn tile_dir() -> Option<PathBuf> {
    let d = data_dir()?.join("raw/terrain/3dep");
    if d.is_dir() {
        Some(d)
    } else {
        eprintln!("{} is absent; skipping real-data test", d.display());
        None
    }
}

/// Tile file name for the 1 degree cell whose south-west corner is (`lat`, `lon`).
fn tile_name(lat: i32, lon: i32) -> String {
    format!("USGS_1_n{:02}w{:03}.tif", lat + 1, -lon)
}

fn load(dir: &std::path::Path, lat: i32, lon: i32) -> Option<Grid> {
    let p = dir.join(tile_name(lat, lon));
    if !p.is_file() {
        return None;
    }
    let bytes = read_file(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    Some(read_geotiff(&bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
}

fn feet(m: f64) -> f64 {
    m / METRES_PER_FOOT
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let ix = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[ix]
}

fn summary(label: &str, diffs: &mut [f64]) -> (f64, f64) {
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (med, p95) = (percentile(diffs, 0.5), percentile(diffs, 0.95));
    eprintln!(
        "{label}: n={} median={:.1} ft p95={:.1} ft max={:.1} ft",
        diffs.len(),
        med,
        p95,
        diffs.last().copied().unwrap_or(f64::NAN)
    );
    (med, p95)
}

/// The tiles present, as (south-west lat, south-west lon) of each 1 degree cell.
fn present_cells(dir: &std::path::Path) -> Vec<(i32, i32)> {
    let mut v = Vec::new();
    for lat in 30..60 {
        for lon in -130..-60 {
            // A tile being downloaded may be incomplete: only whole, readable files count.
            if dir.join(tile_name(lat, lon)).is_file() {
                v.push((lat, lon));
            }
        }
    }
    v
}

// REQ: OA-TIFF-016
#[test]
fn real_tiles_registration_against_neighbours() {
    let Some(dir) = tile_dir() else { return };
    let cells = present_cells(&dir);
    if cells.is_empty() {
        eprintln!("no 3DEP tiles; skipping");
        return;
    }
    let mut pairs_east = 0;
    let mut pairs_north = 0;
    let mut mismatches = 0u64;
    let mut compared = 0u64;
    let mut max_abs = 0.0f32;
    let mut over_1m = 0u64;
    let (mut aligned_sum, mut shifted_sum, mut shifted_n) = (0.0f64, 0.0f64, 0u64);
    let mut prev: Option<((i32, i32), Grid)> = None;
    for &(lat, lon) in &cells {
        let g = load(&dir, lat, lon).unwrap();
        // 3DEP 1 arc-second: 3612 samples, a 6 sample border around the 3600 of the nominal degree.
        assert_eq!((g.width, g.height), (3612, 3612), "{lat} {lon}");
        assert!(!g.point_registered, "3DEP tiles are PixelIsArea");
        assert!(g.crs == 4269 || g.crs == 4326);
        let d = 1.0 / 3600.0;
        assert!(
            (g.dlon - d).abs() < 1e-10 && (g.dlat - d).abs() < 1e-10,
            "{} {}",
            g.dlon,
            g.dlat
        );
        // Absolute registration: under PixelIsArea the outer corner of sample column 6 is the
        // west edge of the nominal degree, and of row 6 the north edge, at integer degrees. A
        // half-pixel error in the convention would put these 0.5 pixel (1.4e-4 degree) off.
        let west_corner = g.lon0 + 6.0 * g.dlon - 0.5 * g.dlon;
        let north_corner = g.lat0 - 6.0 * g.dlat + 0.5 * g.dlat;
        assert!(
            (west_corner - f64::from(lon)).abs() < 1e-9,
            "{lat} {lon}: west {west_corner}"
        );
        assert!(
            (north_corner - f64::from(lat + 1)).abs() < 1e-9,
            "{lat} {lon}: north {north_corner}"
        );
        let mut compare =
            |a: &Grid, ax: usize, ay: usize, b: &Grid, bx: usize, by: usize, w: usize, h: usize| {
                for r in 0..h {
                    for c in 0..w {
                        let va = a.elev[(ay + r) * a.width as usize + ax + c];
                        let vb = b.elev[(by + r) * b.width as usize + bx + c];
                        compared += 1;
                        if va.is_nan() && vb.is_nan() {
                            continue;
                        }
                        if va.is_nan() != vb.is_nan() {
                            mismatches += 1;
                        } else if va != vb {
                            mismatches += 1;
                            max_abs = max_abs.max((va - vb).abs());
                            if (va - vb).abs() > 1.0 {
                                over_1m += 1;
                            }
                        }
                    }
                }
            };
        // East neighbour of the previous cell in this row: shares 12 columns (the border of both).
        if let Some(((plat, plon), p)) = &prev {
            if *plat == lat && *plon + 1 == lon {
                // p's last 12 columns are g's first 12 columns.
                let all_rows = 3612;
                compare(p, 3600, 0, &g, 0, 0, 12, all_rows);
                // Sensitivity: the same overlap with b one column off differs far more.
                for r in 0..all_rows {
                    for c in 0..11usize {
                        let (va, vb) = (p.elev[r * 3612 + 3600 + c], g.elev[r * 3612 + c]);
                        let vs = p.elev[r * 3612 + 3601 + c];
                        if !va.is_nan() && !vb.is_nan() && !vs.is_nan() {
                            aligned_sum += f64::from((va - vb).abs());
                            shifted_sum += f64::from((vs - vb).abs());
                            shifted_n += 1;
                        }
                    }
                }
                // The geographic positions of those samples agree to 1e-9 degree.
                for c in 0..12 {
                    let pl = p.lon0 + f64::from(3600 + c) * p.dlon;
                    let gl = g.lon0 + f64::from(c) * g.dlon;
                    assert!((pl - gl).abs() < 1e-7);
                }
                pairs_east += 1;
            }
        }
        // North neighbour: the cell one degree to the north, if loaded, shares 12 rows.
        if cells.contains(&(lat + 1, lon)) {
            let n = load(&dir, lat + 1, lon).unwrap();
            // n's last 12 rows are g's first 12 rows.
            compare(&n, 0, 3600, &g, 0, 0, 3612, 12);
            let nl = n.lat0 - 3600.0 * n.dlat;
            // The files carry the pixel scale to 12 digits, so rows 3600 apart agree to ~1e-8 degree (1 mm).
            assert!(
                (nl - g.lat0).abs() < 1e-7,
                "{lat} {lon}: north tile lat0 {} g lat0 {} nl {nl}",
                n.lat0,
                g.lat0
            );
            pairs_north += 1;
        }
        prev = Some(((lat, lon), g));
    }
    eprintln!(
        "registration: {} tiles, {pairs_east} east pairs, {pairs_north} north pairs, {compared} overlapping samples compared, {mismatches} differ at all (max {max_abs} m), {over_1m} differ by more than 1 m",
        cells.len()
    );
    if shifted_n > 0 {
        eprintln!(
            "registration sensitivity over {shifted_n} east-pair samples: mean |diff| aligned {:.4} m, one column off {:.4} m",
            aligned_sum / shifted_n as f64,
            shifted_sum / shifted_n as f64
        );
        assert!(
            aligned_sum * 10.0 < shifted_sum,
            "alignment is not clearly better than one pixel off"
        );
    }
    if pairs_east + pairs_north > 0 {
        // Neighbouring tiles share a border cut from one dataset. Some pairs differ by a few
        // decimetres in whole columns (the two tiles were built from different vintages of the
        // same lidar), so require agreement within a metre rather than bit equality.
        let frac = over_1m as f64 / compared as f64;
        assert!(
            frac < 0.005,
            "{over_1m} of {compared} overlapping samples differ by more than 1 m"
        );
    }
}

fn outer_zip_entry(zip_path: &std::path::Path, name: &str) -> Vec<u8> {
    let bytes = oa_zip::read_archive_file(zip_path).unwrap();
    let a = oa_zip::Archive::parse(&bytes).unwrap();
    a.extract(name).unwrap()
}

/// Airports and obstacles against the terrain, tile by tile (a tile is ~52 MB of f32).
// REQ: OA-TIFF-017, OA-TIFF-018
#[test]
fn real_airports_and_obstacles_against_terrain() {
    let (Some(root), Some(dir)) = (data_dir(), tile_dir()) else {
        return;
    };
    let cells = present_cells(&dir);
    let nasr_zip = root.join("raw/nasr-2026-10-29/NASR_CSV_29_Oct_2026.zip");
    let dof_zip = root.join("raw/dof/DOF_260927.zip");
    if cells.is_empty() || !nasr_zip.is_file() || !dof_zip.is_file() {
        eprintln!("terrain, NASR or DOF files absent; skipping");
        return;
    }
    let apt_csv = outer_zip_entry(&nasr_zip, "APT_BASE.csv");
    let airports =
        oa_nasr::apt::parse_airports("APT_BASE.csv", std::io::Cursor::new(apt_csv)).unwrap();
    eprintln!(
        "NASR airports parsed: {} rejected {}",
        airports.records.len(),
        airports.rejected.len()
    );
    let dof_bytes = outer_zip_entry(&dof_zip, "DOF.DAT");
    let dof = oa_dof::parse_dof(&dof_bytes).unwrap();
    eprintln!(
        "DOF: currency {:?}, {} lines, {} obstacles, {} rejected",
        dof.currency,
        dof.lines,
        oa_dof::len(&dof),
        dof.rejected_total
    );
    assert_eq!(dof.rejected_total, 0, "{:?}", dof.rejected.first());

    struct Apt {
        id: String,
        name: String,
        kind: String,
        nasr_ft: f64,
        dem_ft: f64,
        near_max_ft: f64,
    }
    let mut apts: Vec<Apt> = Vec::new();
    let mut apt_nodata = 0;
    let mut shift_abs: [Vec<f64>; 5] = Default::default();
    // (diff ft = (amsl - agl) - terrain, verified, id, agl, amsl, terrain ft)
    let mut obs: Vec<(f64, bool, String, u32, i32, f64)> = Vec::new();
    let mut obs_nodata = 0;
    let mut obs_group_nonunit = 0;
    for &(lat, lon) in &cells {
        let in_cell = |la: f64, lo: f64| la.floor() as i32 == lat && lo.floor() as i32 == lon;
        let mine: Vec<_> = airports
            .records
            .iter()
            .filter(|a| in_cell(a.position.lat, a.position.lon))
            .collect();
        let idx: Vec<usize> = (0..oa_dof::len(&dof))
            .filter(|&i| {
                in_cell(
                    oa_dof::lat_deg(&dof, oa_dof::ObstacleIx(i as u32)).unwrap(),
                    oa_dof::lon_deg(&dof, oa_dof::ObstacleIx(i as u32)).unwrap(),
                )
            })
            .collect();
        if mine.is_empty() && idx.is_empty() {
            continue;
        }
        let g = load(&dir, lat, lon).unwrap();
        for a in mine {
            match grid_elevation_at(&g, a.position.lat, a.position.lon) {
                Elevation::Metres(m) => {
                    let near = grid_max_in_radius(&g, a.position.lat, a.position.lon, 0.25)
                        .ok()
                        .and_then(|q| q.peak)
                        .map_or(f64::NAN, |p| feet(f64::from(p.metres)));
                    apts.push(Apt {
                        id: a.arpt_id.clone(),
                        name: a.arpt_name.clone(),
                        kind: a.site_type_code.clone(),
                        nasr_ft: a.elev,
                        dem_ft: feet(f64::from(m)),
                        near_max_ft: near,
                    });
                    // Registration sensitivity: sample at the same place moved by -1, -0.5, 0,
                    // +0.5 and +1 pixel east and south (a pure convention error looks like this).
                    if a.site_type_code == "A" {
                        for (k, s) in [-1.0, -0.5, 0.0, 0.5, 1.0].iter().enumerate() {
                            let d = s / 3600.0;
                            if let Elevation::Metres(v) =
                                grid_elevation_at(&g, a.position.lat - d, a.position.lon + d)
                            {
                                shift_abs[k].push((feet(f64::from(v)) - a.elev).abs());
                            }
                        }
                    }
                }
                _ => apt_nodata += 1,
            }
        }
        for i in idx {
            let ix = oa_dof::ObstacleIx(i as u32);
            let (la, lo) = (
                oa_dof::lat_deg(&dof, ix).unwrap(),
                oa_dof::lon_deg(&dof, ix).unwrap(),
            );
            match grid_elevation_at(&g, la, lo) {
                Elevation::Metres(m) => {
                    let ground = f64::from(dof.amsl_ft[i]) - f64::from(dof.agl_ft[i]);
                    if dof.quantity[i] != b'1' {
                        obs_group_nonunit += 1;
                    }
                    obs.push((
                        ground - feet(f64::from(m)),
                        dof.verification[i] == b'O',
                        oa_dof::oas_number(&dof, ix).unwrap(),
                        dof.agl_ft[i],
                        dof.amsl_ft[i],
                        feet(f64::from(m)),
                    ));
                }
                _ => obs_nodata += 1,
            }
        }
    }

    // ---- airports ----
    eprintln!(
        "tiles used: {}; airports with terrain: {}, nodata/outside: {apt_nodata}",
        cells.len(),
        apts.len()
    );
    let all = |k: &str| -> Vec<&Apt> {
        apts.iter()
            .filter(|a| k.is_empty() || a.kind == k)
            .collect()
    };
    for kind in ["", "A", "H", "C", "G", "U", "B"] {
        let mut d: Vec<f64> = all(kind)
            .iter()
            .map(|a| (a.dem_ft - a.nasr_ft).abs())
            .collect();
        if !d.is_empty() {
            summary(
                &format!(
                    "airports kind {:?}",
                    if kind.is_empty() { "all" } else { kind }
                ),
                &mut d,
            );
        }
    }
    let mut signed: Vec<f64> = all("A").iter().map(|a| a.dem_ft - a.nasr_ft).collect();
    signed.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "airports type A signed (dem - nasr): median {:.1} ft, p5 {:.1}, p95 {:.1}",
        percentile(&signed, 0.5),
        percentile(&signed, 0.05),
        percentile(&signed, 0.95)
    );
    for (k, s) in [-1.0, -0.5, 0.0, 0.5, 1.0].iter().enumerate() {
        let mut v = shift_abs[k].clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "registration shift {s:+.1} px (type A airports, n={}): median |diff| {:.2} ft, mean {:.2} ft",
            v.len(),
            percentile(&v, 0.5),
            v.iter().sum::<f64>() / v.len().max(1) as f64
        );
    }
    let mut worst: Vec<&Apt> = apts.iter().collect();
    worst.sort_by(|a, b| {
        (b.dem_ft - b.nasr_ft)
            .abs()
            .partial_cmp(&(a.dem_ft - a.nasr_ft).abs())
            .unwrap()
    });
    eprintln!("10 worst airports (all types): id, type, name, nasr ft, 3dep ft, diff, highest 3dep within 0.25 nm");
    for a in worst.iter().take(10) {
        eprintln!(
            "  {:6} {} {:34} nasr {:7.1} dem {:7.1} diff {:+8.1} near-max {:7.1}",
            a.id,
            a.kind,
            a.name,
            a.nasr_ft,
            a.dem_ft,
            a.dem_ft - a.nasr_ft,
            a.near_max_ft
        );
    }
    eprintln!("10 worst type A airports:");
    let mut worst_a: Vec<&Apt> = all("A");
    worst_a.sort_by(|a, b| {
        (b.dem_ft - b.nasr_ft)
            .abs()
            .partial_cmp(&(a.dem_ft - a.nasr_ft).abs())
            .unwrap()
    });
    for a in worst_a.iter().take(10) {
        eprintln!(
            "  {:6} {:34} nasr {:7.1} dem {:7.1} diff {:+8.1} near-max {:7.1}",
            a.id,
            a.name,
            a.nasr_ft,
            a.dem_ft,
            a.dem_ft - a.nasr_ft,
            a.near_max_ft
        );
    }
    let mut type_a: Vec<f64> = all("A")
        .iter()
        .map(|a| (a.dem_ft - a.nasr_ft).abs())
        .collect();
    type_a.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if type_a.len() >= 50 {
        assert!(
            percentile(&type_a, 0.5) < 15.0,
            "median airport difference too large"
        );
        assert!(
            percentile(&type_a, 0.95) < 60.0,
            "p95 airport difference too large"
        );
    }

    // ---- obstacles ----
    eprintln!("obstacles with terrain: {}, nodata/outside: {obs_nodata}, quantity != 1: {obs_group_nonunit}", obs.len());
    for (label, verified) in [
        ("verified", Some(true)),
        ("unverified", Some(false)),
        ("all", None),
    ] {
        let mut d: Vec<f64> = obs
            .iter()
            .filter(|o| verified.is_none_or(|v| o.1 == v))
            .map(|o| o.0.abs())
            .collect();
        if !d.is_empty() {
            summary(&format!("obstacles {label} |(AMSL-AGL) - terrain|"), &mut d);
        }
    }
    let mut sv: Vec<f64> = obs.iter().filter(|o| o.1).map(|o| o.0).collect();
    sv.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "obstacles verified signed ((AMSL-AGL) - terrain): median {:.1} ft p5 {:.1} p95 {:.1}",
        percentile(&sv, 0.5),
        percentile(&sv, 0.05),
        percentile(&sv, 0.95)
    );
    let mut worst: Vec<&(f64, bool, String, u32, i32, f64)> = obs.iter().collect();
    worst.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap());
    eprintln!("10 worst obstacles: oas, verified, agl, amsl, terrain ft, diff");
    for o in worst.iter().take(10) {
        eprintln!(
            "  {} {} agl {:5} amsl {:6} terrain {:7.1} diff {:+8.1}",
            o.2,
            if o.1 { 'O' } else { 'U' },
            o.3,
            o.4,
            o.5,
            o.0
        );
    }
    let mut vd: Vec<f64> = obs.iter().filter(|o| o.1).map(|o| o.0.abs()).collect();
    vd.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if vd.len() >= 100 {
        assert!(
            percentile(&vd, 0.5) < 25.0,
            "median obstacle terrain difference too large"
        );
    }
}

// REQ: OA-TIFF-019
#[test]
fn real_tile_set_across_a_seam() {
    let Some(dir) = tile_dir() else { return };
    let (Some(a), Some(b)) = (load(&dir, 40, -87), load(&dir, 40, -86)) else {
        eprintln!("tiles n41w087/n41w086 absent; skipping");
        return;
    };
    let set = TileSet {
        tiles: vec![a.clone(), b.clone()],
    };
    // Points on both sides of the seam at longitude -86 and straddling it.
    let mut checked = 0;
    for k in 0..200 {
        let lat = 40.2 + f64::from(k) * 0.004;
        for lon in [-86.003, -86.0005, -86.0, -85.9995, -85.997] {
            let want = match grid_elevation_at(if lon < -86.0 { &a } else { &b }, lat, lon) {
                Elevation::Metres(v) => v,
                _ => continue,
            };
            match elevation_at(&set, lat, lon) {
                Elevation::Metres(v) => {
                    // The two tiles can differ by decimetres in the shared border (vintage).
                    assert!((v - want).abs() < 1.0, "{lat},{lon}: {v} vs {want}");
                    checked += 1;
                }
                other => panic!("{lat},{lon}: {other:?}"),
            }
        }
    }
    // A radius over the seam sees the higher of the two sides.
    let q = max_elevation_in_radius(&set, 40.5, -86.0, 6.0).unwrap();
    let qa = grid_max_in_radius(&a, 40.5, -86.0, 6.0).unwrap();
    let qb = grid_max_in_radius(&b, 40.5, -86.0, 6.0).unwrap();
    let want = qa.peak.unwrap().metres.max(qb.peak.unwrap().metres);
    assert_eq!(q.peak.unwrap().metres, want);
    assert!(q.probes_covered);
    eprintln!(
        "seam test: {checked} points agree; 6 nm radius max {:.1} m ({} sample visits, west {} east {})",
        want, q.samples, qa.samples, qb.samples
    );
    // Outside the two tiles.
    assert_eq!(elevation_at(&set, 44.5, -86.0), Elevation::Outside);
}

// REQ: OA-TIFF-028
#[test]
fn real_conservative_against_centre_only() {
    let Some(dir) = tile_dir() else { return };
    // A 3 by 3 block of tiles around Chicago: latitude 40 to 43, longitude -89 to -86.
    let mut tiles = Vec::new();
    for lat in 40..43 {
        for lon in -89..-86 {
            match load(&dir, lat, lon) {
                Some(g) => tiles.push(g),
                None => {
                    eprintln!("tile {} absent; skipping", tile_name(lat, lon));
                    return;
                }
            }
        }
    }
    let set = TileSet { tiles };
    // 500 pseudo-random points in the ZAU area, kept 8 nm inside the block.
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut unit = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let pts: Vec<(f64, f64)> = (0..500)
        .map(|_| (40.2 + unit() * 2.6, -88.8 + unit() * 2.6))
        .collect();
    for rnm in [1.0, 3.0, 5.0] {
        let mut diffs = Vec::new();
        let mut times = Vec::new();
        let (mut below, mut incomplete) = (0, 0);
        for &(lat, lon) in &pts {
            let old = max_elevation_in_radius(&set, lat, lon, rnm).unwrap();
            let t = std::time::Instant::now();
            let new = max_elevation_in_radius_conservative(&set, lat, lon, rnm).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e3);
            let new_m = match new {
                TerrainMax::Complete { peak, .. } => peak.metres,
                TerrainMax::Incomplete { .. } => {
                    incomplete += 1;
                    continue;
                }
            };
            let d = feet(f64::from(new_m) - f64::from(old.peak.unwrap().metres));
            if d < -1e-6 {
                below += 1;
            }
            diffs.push(d);
        }
        diffs.sort_by(f64::total_cmp);
        times.sort_by(f64::total_cmp);
        let exceed = diffs.iter().filter(|&&d| d > 1e-6).count();
        let pos: Vec<f64> = diffs.iter().copied().filter(|&d| d > 1e-6).collect();
        eprintln!(
            "{rnm} nm: {} queries, incomplete {incomplete}; conservative above centre-only in {exceed} ({:.1}%), below in {below}; \
             excess ft over all: median {:.1} p90 {:.1} p99 {:.1} max {:.1}; over the exceeding ones: median {:.1} mean {:.1}; \
             time ms: median {:.2} p99 {:.2} max {:.2}",
            diffs.len(),
            100.0 * exceed as f64 / diffs.len() as f64,
            percentile(&diffs, 0.5),
            percentile(&diffs, 0.9),
            percentile(&diffs, 0.99),
            percentile(&diffs, 1.0),
            percentile(&pos, 0.5),
            pos.iter().sum::<f64>() / pos.len().max(1) as f64,
            percentile(&times, 0.5),
            percentile(&times, 0.99),
            percentile(&times, 1.0),
        );
        assert_eq!(incomplete, 0);
        // The conservative maximum is not lower than the centre-only one, except where the
        // centre-only query's spherical radius reaches a sample the WGS 84 radius does not.
        assert!(
            below * 100 <= diffs.len(),
            "{below} of {} below",
            diffs.len()
        );
    }
}
