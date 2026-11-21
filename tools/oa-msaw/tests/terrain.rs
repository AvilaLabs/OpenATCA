//! The lazy terrain path: tiles read from a directory, one query over only the tiles of the cells
//! it touches. The tiles are tiny GeoTIFFs written here (4 by 4 samples a degree), named as 3DEP
//! names them, so that a missing neighbour, a nodata sample and a tile on another lattice can be
//! made on purpose. Every value is made up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};

use oa_msaw::{SamplePoint, FLOOR_NONE};
use oa_msaw_run::floors::{tile_name, Floors};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("oa-msaw-terrain-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// A little-endian classic GeoTIFF of 32-bit floats in one strip, pixel-is-area, geographic
/// NAD83, the north-west corner at (`lon`, `lat`) and `step` degrees a sample. `-9999` is nodata.
fn geotiff(w: u32, h: u32, lon: f64, lat: f64, step: f64, data: &[f32]) -> Vec<u8> {
    let mut file: Vec<u8> = vec![b'I', b'I', 42, 0, 0, 0, 0, 0];
    let strip_at = file.len() as u32;
    for v in data {
        file.extend(v.to_le_bytes());
    }
    let strip_len = (data.len() * 4) as u32;
    // (tag, type, count, bytes)
    let dbl = |v: &[f64]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
    let keys: Vec<u16> = vec![1, 1, 0, 3, 1024, 0, 1, 2, 1025, 0, 1, 1, 2048, 0, 1, 4269];
    let mut ents: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (256, 4, 1, le32(w).to_vec()),
        (257, 4, 1, le32(h).to_vec()),
        (258, 3, 1, le16(32).to_vec()),
        (259, 3, 1, le16(1).to_vec()),
        (262, 3, 1, le16(1).to_vec()),
        (273, 4, 1, le32(strip_at).to_vec()),
        (277, 3, 1, le16(1).to_vec()),
        (278, 4, 1, le32(h).to_vec()),
        (279, 4, 1, le32(strip_len).to_vec()),
        (284, 3, 1, le16(1).to_vec()),
        (339, 3, 1, le16(3).to_vec()),
        (33550, 12, 3, dbl(&[step, step, 0.0])),
        (33922, 12, 6, dbl(&[0.0, 0.0, 0.0, lon, lat, 0.0])),
        (
            34735,
            3,
            keys.len() as u32,
            keys.iter().flat_map(|k| le16(*k)).collect(),
        ),
        (42113, 2, 6, b"-9999\0".to_vec()),
    ];
    ents.sort_by_key(|e| e.0);
    let mut at = Vec::new();
    for e in &ents {
        if e.3.len() > 4 {
            if file.len() % 2 == 1 {
                file.push(0);
            }
            at.push(Some(file.len() as u32));
            file.extend(&e.3);
        } else {
            at.push(None);
        }
    }
    if file.len() % 2 == 1 {
        file.push(0);
    }
    let ifd = file.len() as u32;
    file[4..8].copy_from_slice(&le32(ifd));
    file.extend(le16(ents.len() as u16));
    for (e, a) in ents.iter().zip(at) {
        file.extend(le16(e.0));
        file.extend(le16(e.1));
        file.extend(le32(e.2));
        match a {
            Some(o) => file.extend(le32(o)),
            None => {
                let mut v = e.3.clone();
                v.resize(4, 0);
                file.extend(v);
            }
        }
    }
    file.extend(le32(0));
    file
}

/// Writes the tile of the one-degree cell whose south-west corner is (`lat`, `lon`): samples at
/// `step` degrees from two steps beyond each edge of the cell (as 3DEP tiles are), all `metres`
/// but the `holes` (row, column), which are nodata.
fn write_tile(dir: &Path, lat: i32, lon: i32, step: f64, metres: f32, holes: &[(usize, usize)]) {
    let n = (1.0 / step).round() as usize + 4;
    let mut data = vec![metres; n * n];
    for &(r, c) in holes {
        data[r * n + c] = -9999.0;
    }
    let bytes = geotiff(
        n as u32,
        n as u32,
        f64::from(lon) - 2.0 * step,
        f64::from(lat + 1) + 2.0 * step,
        step,
        &data,
    );
    std::fs::write(dir.join(tile_name(lat, lon).unwrap()), bytes).unwrap();
}

fn point(lat: f64, lon: f64) -> SamplePoint {
    SamplePoint {
        lat_e7: (lat * 1e7).round() as i32,
        lon_e7: (lon * 1e7).round() as i32,
        radius_m: 400.0,
        track_deg: -1,
    }
}

// REQ: OA-MSAW-047
#[test]
fn a_missing_neighbour_and_a_nodata_sample_spoil_only_their_own_queries_and_no_lattice_does() {
    let dir = scratch("lazy");
    // Cells (40,-89) and (40,-88) are on one lattice; (40,-87) has a slightly different spacing,
    // which is no matter: each tile is reduced alone. (41,-89) has a nodata sample at its row 52,
    // column 52 (the middle).
    write_tile(&dir, 40, -89, 0.01, 100.0, &[]);
    write_tile(&dir, 40, -87, 0.0101, 100.0, &[]);
    write_tile(&dir, 41, -89, 0.01, 100.0, &[(52, 52)]);
    let mut f = Floors::new(Some(dir.clone()), None, None);
    // Inside the first tile: answered (100 m is 329 ft rounded up).
    assert_eq!(f.floor_at(&point(40.5, -88.5)).unwrap().terrain_ft, 329);
    // At the east edge of the first tile the neighbour (40,-88) is not there: no answer, counted
    // as an area outside the tiles.
    assert_eq!(
        f.floor_at(&point(40.5, -88.001)).unwrap().terrain_ft,
        FLOOR_NONE
    );
    assert_eq!(f.stats.terrain_missing, 1);
    // With the neighbour in place the same place is answered (a new Floors: the absent tile was
    // noted once).
    write_tile(&dir, 40, -88, 0.01, 250.0, &[]);
    let mut g = Floors::new(Some(dir.clone()), None, None);
    let s = g.floor_at(&point(40.5, -88.001)).unwrap();
    assert_eq!(
        s.terrain_ft, 821,
        "the higher of the two tiles under the circle"
    );
    // A nodata sample under the circle: no answer, counted as nodata (not as a missing tile). The
    // sample is at about (41.48, -88.48).
    let hole = g.floor_at(&point(41.48, -88.48)).unwrap();
    assert_eq!(hole.terrain_ft, FLOOR_NONE);
    assert_eq!(g.stats.terrain_nodata, 1);
    // The tile on another lattice answers like any other, and nothing is counted misaligned.
    let other = g.floor_at(&point(40.5, -86.5)).unwrap();
    assert_eq!(other.terrain_ft, 329);
    assert_eq!(g.stats.terrain_misaligned, 0);
    // Away from the hole, the good tiles still answer.
    assert_eq!(g.floor_at(&point(40.5, -88.5)).unwrap().terrain_ft, 329);
    assert_eq!(
        g.stats.terrain_absent,
        g.stats.terrain_missing
            + g.stats.terrain_nodata
            + g.stats.terrain_misaligned
            + g.stats.terrain_refused
    );
    // Three tiles were read once each.
    assert_eq!(g.stats.tiles_loaded, 4);
    assert_eq!(g.stats.tiles_reloaded, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
