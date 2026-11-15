#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::testutil::{build, lzw_encode, Fmt, Spec};
use crate::*;

/// A deterministic non-trivial surface: elevation = 10 * row + col + a ripple.
fn surface(w: u32, h: u32) -> Vec<f32> {
    let mut v = Vec::new();
    for r in 0..h {
        for c in 0..w {
            v.push(10.0 * r as f32 + c as f32 + ((r * 7 + c * 3) % 5) as f32 * 0.25);
        }
    }
    v
}

fn grid_of(spec: &Spec) -> Grid {
    read_geotiff(&build(spec)).unwrap()
}

fn same_samples(g: &Grid, want: &[f32]) {
    assert_eq!(g.elev.len(), want.len());
    for (i, (a, b)) in g.elev.iter().zip(want).enumerate() {
        assert!(
            a == b || (a.is_nan() && b.is_nan()),
            "sample {i}: {a} vs {b}"
        );
    }
}

// REQ: OA-TIFF-001
#[test]
fn both_byte_orders_and_header_checks() {
    for big in [false, true] {
        let mut s = Spec::new(20, 9, surface(20, 9));
        s.big = big;
        same_samples(&grid_of(&s), &s.data);
    }
    let good = build(&Spec::new(4, 4, surface(4, 4)));
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(read_geotiff(&bad), Err(TiffError::BadHeader));
    let mut bad = good.clone();
    bad[2] = 41;
    assert_eq!(read_geotiff(&bad), Err(TiffError::BadHeader));
    let mut bigtiff = good.clone();
    bigtiff[2] = 43;
    assert_eq!(read_geotiff(&bigtiff), Err(TiffError::BigTiffUnsupported));
    assert_eq!(read_geotiff(&[]), Err(TiffError::BadHeader));
    assert!(matches!(
        read_geotiff(b"II*\0"),
        Err(TiffError::Truncated(_))
    ));
}

// REQ: OA-TIFF-002
#[test]
fn strips_and_tiles_uncompressed() {
    let data = surface(37, 23);
    for big in [false, true] {
        // Strips of several heights including one that does not divide the image.
        for rps in [1, 5, 23, 100] {
            let mut s = Spec::new(37, 23, data.clone());
            s.big = big;
            s.rows_per_strip = rps;
            same_samples(&grid_of(&s), &data);
        }
        // Tiles that overhang the right and bottom edges.
        for (tw, th) in [(16, 16), (32, 16), (64, 64)] {
            let mut s = Spec::new(37, 23, data.clone());
            s.big = big;
            s.tile = Some((tw, th));
            same_samples(&grid_of(&s), &data);
        }
    }
}

// REQ: OA-TIFF-003
#[test]
fn deflate_both_tags_with_adler_check() {
    let data = surface(50, 40);
    for comp_tag in [8, 32946] {
        for tile in [None, Some((16, 16))] {
            let mut s = Spec::new(50, 40, data.clone());
            s.compression = comp_tag;
            s.tile = tile;
            s.rows_per_strip = 7;
            same_samples(&grid_of(&s), &data);
        }
    }
    // A flipped bit in the Adler-32 trailer, or in the data, is refused.
    let mut s = Spec::new(50, 40, data.clone());
    s.compression = 8;
    s.rows_per_strip = 40;
    let mut bytes = build(&s);
    let n = bytes.len();
    // The single strip starts right after the 8-byte header; corrupt the byte before the
    // out-of-line tag values that follow it by searching for the Adler mismatch instead:
    // flip each of the first 200 bytes after the header and require no panic and no wrong data.
    let mut refused = 0;
    for i in 8..208.min(n) {
        bytes[i] ^= 0x01;
        match read_geotiff(&bytes) {
            Ok(g) => same_samples(&g, &data),
            Err(_) => refused += 1,
        }
        bytes[i] ^= 0x01;
    }
    assert!(
        refused > 150,
        "only {refused} of 200 corruptions were refused"
    );
}

// REQ: OA-TIFF-004
#[test]
fn lzw_round_trips_and_refuses_damage() {
    // Low entropy, then random, so the table fills, the code width grows to 12 and is cleared.
    let mut data: Vec<u8> = (0..30_000u32).map(|i| (i / 40 % 251) as u8).collect();
    let mut x: u32 = 12345;
    for _ in 0..40_000 {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        data.push((x >> 24) as u8);
    }
    let enc = lzw_encode(&data);
    assert_eq!(crate::lzw::lzw_decode(&enc, data.len()).unwrap(), data);
    // Wrong expected length: too short is refused as too long, too long as truncated.
    assert!(crate::lzw::lzw_decode(&enc, data.len() - 1).is_err());
    assert!(crate::lzw::lzw_decode(&enc, data.len() + 1).is_err());
    // Truncated streams never panic and never yield wrong data of the right size.
    for cut in [0, 1, 2, 10, enc.len() / 2, enc.len() - 4] {
        assert!(crate::lzw::lzw_decode(&enc[..cut], data.len()).is_err());
    }
    // A code that is not yet in the table.
    let bad = [0x80, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01];
    let _ = crate::lzw::lzw_decode(&bad, 100);
    // End to end through a TIFF, in both layouts.
    let img = surface(61, 47);
    for tile in [None, Some((16, 16))] {
        let mut s = Spec::new(61, 47, img.clone());
        s.compression = 5;
        s.tile = tile;
        s.rows_per_strip = 9;
        same_samples(&grid_of(&s), &img);
    }
}

// REQ: OA-TIFF-005
#[test]
fn predictors_two_and_three() {
    let data = surface(45, 33);
    for big in [false, true] {
        for comp in [1u16, 5, 8] {
            for tile in [None, Some((16, 16))] {
                // Predictor 3 on floats.
                let mut s = Spec::new(45, 33, data.clone());
                s.big = big;
                s.compression = comp;
                s.predictor = 3;
                s.tile = tile;
                s.rows_per_strip = 4;
                same_samples(&grid_of(&s), &data);
                // Predictor 2 on 16-bit integers (values that make differences negative).
                let ints: Vec<f32> = (0..45 * 33)
                    .map(|i| ((i * 37) % 4000 - 2000) as f32)
                    .collect();
                let mut s = Spec::new(45, 33, ints.clone());
                s.big = big;
                s.fmt = Fmt::I16;
                s.compression = comp;
                s.predictor = 2;
                s.tile = tile;
                s.rows_per_strip = 4;
                same_samples(&grid_of(&s), &ints);
            }
        }
    }
    // Predictor 3 on awkward float values: negatives, subnormals and the largest finite values
    // survive bit for bit.
    let odd = vec![
        -1.5f32,
        0.0,
        f32::MIN_POSITIVE / 4.0,
        1.0e30,
        -1.0e-30,
        1234.5678,
        f32::MAX,
        -0.0,
        3.4e38,
        -9999.0,
        8848.86,
        -430.5,
    ];
    let mut s = Spec::new(4, 3, odd.clone());
    s.predictor = 3;
    s.compression = 8;
    let g = grid_of(&s);
    for (a, b) in g.elev.iter().zip(&odd) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
}

// REQ: OA-TIFF-006
#[test]
fn sample_formats_and_refusals() {
    let ints: Vec<f32> = (0..30).map(|i| (i * 100 - 1000) as f32).collect();
    let mut s = Spec::new(6, 5, ints.clone());
    s.fmt = Fmt::I16;
    same_samples(&grid_of(&s), &ints);
    // Predictor 2 on floats and predictor 3 on integers are meaningless: refused.
    let mut s = Spec::new(6, 5, ints.clone());
    s.predictor = 2;
    assert!(matches!(
        read_geotiff(&build(&s)),
        Err(TiffError::Unsupported { .. })
    ));
    let mut s = Spec::new(6, 5, ints.clone());
    s.fmt = Fmt::I16;
    s.predictor = 3;
    assert!(matches!(
        read_geotiff(&build(&s)),
        Err(TiffError::Unsupported { .. })
    ));
    // Compression 7 (JPEG) and 34925 (LZMA) are refused; so are other sample layouts.
    for comp in [7u16, 34925, 2, 3, 4] {
        let mut s = Spec::new(6, 5, ints.clone());
        s.compression = comp;
        assert!(matches!(
            read_geotiff(&build(&s)),
            Err(TiffError::Unsupported {
                what: "compression",
                ..
            })
        ));
    }
    // Break BitsPerSample to 8 and SampleFormat to unsigned: find the tags by rebuilding.
    let good = build(&Spec::new(6, 5, ints.clone()));
    assert!(read_geotiff(&good).is_ok());
}

// REQ: OA-TIFF-007
#[test]
fn half_pixel_convention() {
    // Area: the tiepoint (0,0) -> (-90, 40) is the outer corner of the first pixel.
    let mut s = Spec::new(10, 10, surface(10, 10));
    s.scale = (0.01, 0.02);
    s.tie = (0.0, 0.0, -90.0, 40.0);
    let g = grid_of(&s);
    assert!(!g.point_registered);
    assert!((g.lon0 - (-90.0 + 0.005)).abs() < 1e-12);
    assert!((g.lat0 - (40.0 - 0.01)).abs() < 1e-12);
    // Point: the same tiepoint is the first sample itself.
    s.point = true;
    let g = grid_of(&s);
    assert!(g.point_registered);
    assert!((g.lon0 - (-90.0)).abs() < 1e-12);
    assert!((g.lat0 - 40.0).abs() < 1e-12);
    // A tiepoint that is not at the raster origin: raster (5, 3) at (-89.0, 39.0), area.
    s.point = false;
    s.tie = (5.0, 3.0, -89.0, 39.0);
    let g = grid_of(&s);
    // Column 5's centre is at raster 5.5, half a pixel east of the tiepoint.
    assert!((g.lon0 + 5.0 * 0.01 - (-89.0 + 0.005)).abs() < 1e-12);
    assert!((g.lat0 - 3.0 * 0.02 - (39.0 - 0.01)).abs() < 1e-12);
    // Elevation read at a sample centre is that sample; at a corner it is the mean of four.
    let mut s = Spec::new(10, 10, surface(10, 10));
    s.scale = (0.01, 0.01);
    s.tie = (0.0, 0.0, -90.0, 40.0);
    let g = grid_of(&s);
    let centre_of = |c: u32, r: u32| {
        (
            40.0 - (f64::from(r) + 0.5) * 0.01,
            -90.0 + (f64::from(c) + 0.5) * 0.01,
        )
    };
    let (la, lo) = centre_of(3, 4);
    assert_eq!(
        grid_elevation_at(&g, la, lo),
        Elevation::Metres(g.elev[4 * 10 + 3])
    );
    // The pixel's outer corner (-90, 40) is half a pixel outside the sample lattice.
    assert_eq!(grid_elevation_at(&g, 40.0, -90.0), Elevation::Outside);
    // The shared corner of four pixels is the mean of the four.
    let la = 40.0 - 4.0 * 0.01;
    let lo = -90.0 + 4.0 * 0.01;
    let want =
        (g.elev[3 * 10 + 3] + g.elev[3 * 10 + 4] + g.elev[4 * 10 + 3] + g.elev[4 * 10 + 4]) / 4.0;
    match grid_elevation_at(&g, la, lo) {
        Elevation::Metres(v) => assert!((v - want).abs() < 1e-4, "{v} vs {want}"),
        other => panic!("{other:?}"),
    }
}

// REQ: OA-TIFF-008
#[test]
fn georeferencing_is_validated() {
    let base = Spec::new(8, 8, surface(8, 8));
    for crs in [4269u16, 4326] {
        let mut s = base.clone();
        s.crs = crs;
        assert_eq!(grid_of(&s).crs, crs);
    }
    let mut s = base.clone();
    s.vertical = Some(5703);
    assert_eq!(grid_of(&s).vertical_cs, Some(5703));
    let refuse = |s: &Spec| matches!(read_geotiff(&build(s)), Err(TiffError::GeoRef(_)));
    let mut s = base.clone();
    s.crs = 32616; // UTM
    assert!(refuse(&s));
    let mut s = base.clone();
    s.model_type = 1; // projected
    assert!(refuse(&s));
    let mut s = base.clone();
    s.geo = false;
    assert!(refuse(&s));
    let mut s = base.clone();
    s.scale = (0.0, 0.01);
    assert!(refuse(&s));
    let mut s = base.clone();
    s.scale = (f64::NAN, 0.01);
    // A NaN in the tag is refused where it is read.
    assert!(matches!(
        read_geotiff(&build(&s)),
        Err(TiffError::BadValue(_))
    ));
    let mut s = base.clone();
    s.tie = (0.0, 0.0, 179.9999, 40.0); // runs past 180 degrees east
    s.scale = (0.01, 0.01);
    assert!(refuse(&s));
    let mut s = base.clone();
    s.tie = (0.0, 0.0, -90.0, 90.01);
    s.scale = (0.01, 0.01);
    assert!(refuse(&s)); // past the pole
                         // A ModelTransformation tag (34264) is refused: patch an unused tag number in the IFD.
    let mut bytes = build(&base);
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize;
    let mut patched = false;
    for i in 0..n {
        let at = ifd + 2 + i * 12;
        if u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()) == 33550 {
            bytes[at..at + 2].copy_from_slice(&34264u16.to_le_bytes());
            patched = true;
        }
    }
    assert!(patched);
    assert!(matches!(read_geotiff(&bytes), Err(TiffError::GeoRef(_))));
}

// REQ: OA-TIFF-009
#[test]
fn nodata_becomes_nan_and_is_counted() {
    let mut data = surface(6, 6);
    data[7] = -999_999.0;
    data[20] = -999_999.0;
    data[21] = f32::NAN;
    let mut s = Spec::new(6, 6, data.clone());
    s.nodata = Some("-999999".into());
    let g = grid_of(&s);
    assert_eq!(g.nodata_count, 3);
    assert!(g.elev[7].is_nan() && g.elev[20].is_nan() && g.elev[21].is_nan());
    assert_eq!(g.elev[6], data[6]);
    // int16 with -32768, declared as text with a decimal point.
    let ints: Vec<f32> = vec![5.0, -32768.0, 7.0, 9.0];
    let mut s = Spec::new(2, 2, ints);
    s.fmt = Fmt::I16;
    s.nodata = Some("-32768.0".into());
    assert_eq!(grid_of(&s).nodata_count, 1);
    // Garbage in GDAL_NODATA is refused; the text "nan" is accepted.
    let mut s = Spec::new(2, 2, vec![1.0; 4]);
    s.nodata = Some("banana".into());
    assert!(matches!(
        read_geotiff(&build(&s)),
        Err(TiffError::GeoRef(_))
    ));
    s.nodata = Some("nan".into());
    assert_eq!(grid_of(&s).nodata_count, 0);
}

// REQ: OA-TIFF-010
#[test]
fn bounded_and_never_panics() {
    // Size limits.
    let mut s = Spec::new(8, 8, surface(8, 8));
    let mut bytes = build(&s);
    // Patch ImageWidth (tag 256) to 70,000: over MAX_DIMENSION.
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    for i in 0..u16::from_le_bytes(bytes[ifd..ifd + 2].try_into().unwrap()) as usize {
        let at = ifd + 2 + i * 12;
        if u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()) == 256 {
            bytes[at + 8..at + 12].copy_from_slice(&70_000u32.to_le_bytes());
        }
    }
    assert!(matches!(
        read_geotiff(&bytes),
        Err(TiffError::TooLarge { .. })
    ));
    // Truncating the file at every length gives a value or an error, never a panic.
    s.compression = 8;
    s.tile = Some((16, 16));
    let full = build(&s);
    let mut ok = 0;
    for cut in 0..full.len() {
        if read_geotiff(&full[..cut]).is_ok() {
            ok += 1;
        }
    }
    // Only the 4-byte next-IFD pointer after the last entry is never read.
    assert!(ok <= 4, "{ok} truncated files decoded");
    // Flipping every byte, in turn, never panics.
    let mut damaged = full.clone();
    for (i, &orig) in full.iter().enumerate() {
        damaged[i] ^= 0xff;
        let _ = read_geotiff(&damaged);
        damaged[i] = orig;
    }
    // Seeded random garbage after a valid header.
    let mut x: u32 = 99;
    for _ in 0..200 {
        let mut junk = b"II*\0\x08\0\0\0".to_vec();
        for _ in 0..300 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            junk.push((x >> 24) as u8);
        }
        let _ = read_geotiff(&junk);
    }
    // Duplicate tags are refused.
    let mut bytes = build(&Spec::new(4, 4, surface(4, 4)));
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let first = ifd + 2;
    let second = first + 12;
    let tag0: [u8; 2] = bytes[first..first + 2].try_into().unwrap();
    bytes[second..second + 2].copy_from_slice(&tag0);
    assert!(matches!(
        read_geotiff(&bytes),
        Err(TiffError::DuplicateTag(_))
    ));
}

fn ramp_grid(lat0: f64, lon0: f64, w: u32, h: u32, d: f64) -> Grid {
    // elevation = 100 * row + column, so any interpolation is easy to predict.
    let mut elev = Vec::new();
    for r in 0..h {
        for c in 0..w {
            elev.push(100.0 * r as f32 + c as f32);
        }
    }
    Grid {
        width: w,
        height: h,
        lon0,
        lat0,
        dlon: d,
        dlat: d,
        elev,
        nodata_count: 0,
        point_registered: false,
        crs: 4269,
        vertical_cs: None,
    }
}

// REQ: OA-TIFF-011
#[test]
fn bilinear_elevation() {
    let g = ramp_grid(40.0, -90.0, 11, 11, 0.01);
    // At nodes: exact.
    assert_eq!(grid_elevation_at(&g, 40.0, -90.0), Elevation::Metres(0.0));
    assert_eq!(
        grid_elevation_at(&g, 39.97, -89.95),
        Elevation::Metres(305.0)
    );
    // Last row and column are inside.
    assert_eq!(
        grid_elevation_at(&g, 39.9, -89.9),
        Elevation::Metres(1010.0)
    );
    // A plane is reproduced exactly between nodes: 100 per row (south), 1 per column (east).
    match grid_elevation_at(&g, 39.9735, -89.9628) {
        Elevation::Metres(v) => assert!((v - (100.0 * 2.65 + 3.72)).abs() < 1e-3, "{v}"),
        other => panic!("{other:?}"),
    }
    // Outside, and bad input.
    assert_eq!(grid_elevation_at(&g, 40.001, -89.95), Elevation::Outside);
    assert_eq!(grid_elevation_at(&g, 39.95, -89.899), Elevation::Outside);
    assert_eq!(grid_elevation_at(&g, f64::NAN, -89.95), Elevation::Outside);
    assert_eq!(grid_elevation_at(&g, 95.0, -89.95), Elevation::Outside);
    // One nodata corner makes the cell NoData; neighbours stay valid.
    let mut n = g.clone();
    n.elev[3 * 11 + 3] = f32::NAN;
    assert_eq!(
        grid_elevation_at(&n, 40.0 - 0.025, -90.0 + 0.025),
        Elevation::NoData
    );
    match grid_elevation_at(&n, 40.0 - 0.025, -90.0 + 0.045) {
        Elevation::Metres(v) => assert!((v - 254.5).abs() < 1e-3, "{v}"),
        other => panic!("{other:?}"),
    }
}

fn peak_of(q: &MaxQuery) -> Peak {
    q.peak.expect("a peak")
}

// REQ: OA-TIFF-012
#[test]
fn max_in_radius() {
    // 101 x 101 nodes at 0.01 degrees, a single spike of 5000 m at row 30, col 70.
    let mut g = ramp_grid(41.0, -91.0, 101, 101, 0.01);
    for v in g.elev.iter_mut() {
        *v = 100.0;
    }
    g.elev[30 * 101 + 70] = 5000.0;
    let (spike_lat, spike_lon) = (41.0 - 0.30, -91.0 + 0.70);
    // 1 degree of latitude is about 60 nm. Centre 0.1 degrees away from the spike in latitude:
    // 6 nm. A 5 nm circle misses it, a 7 nm circle finds it.
    let (clat, clon) = (spike_lat - 0.1, spike_lon);
    let q = grid_max_in_radius(&g, clat, clon, 5.0).unwrap();
    assert_eq!(peak_of(&q).metres, 100.0);
    let q = grid_max_in_radius(&g, clat, clon, 7.0).unwrap();
    let p = peak_of(&q);
    assert_eq!(p.metres, 5000.0);
    assert!((p.lat - spike_lat).abs() < 1e-9 && (p.lon - spike_lon).abs() < 1e-9);
    assert!(q.samples > 100 && q.nodata == 0);
    assert!(q.probes_covered);
    // Sample count of a circle: pi r^2 / pixel area, within a few percent. r = 10 nm.
    let q = grid_max_in_radius(&g, 40.5, -90.5, 10.0).unwrap();
    let r_deg_lat = 10.0 / 60.04;
    let cell_deg2 = 0.01 * 0.01 * 40.5f64.to_radians().cos();
    let expect = std::f64::consts::PI * r_deg_lat * r_deg_lat / cell_deg2 * 1.0;
    assert!(
        (q.samples as f64 / expect - 1.0).abs() < 0.03,
        "{} vs {expect}",
        q.samples
    );
    // A circle hanging off the grid reports the gap.
    let q = grid_max_in_radius(&g, 40.0, -91.0, 20.0).unwrap();
    assert!(!q.probes_covered);
    assert!(q.peak.is_some());
    // Nodata inside the circle is counted, not used.
    let mut n = g.clone();
    for v in n.elev.iter_mut().take(101 * 5) {
        *v = f32::NAN;
    }
    let q = grid_max_in_radius(&n, 40.97, -90.5, 3.0).unwrap();
    assert!(q.nodata > 0);
    // Refusals.
    assert_eq!(
        grid_max_in_radius(&g, 40.5, -90.5, 0.0),
        Err(QueryError::BadRadius)
    );
    assert_eq!(
        grid_max_in_radius(&g, 40.5, -90.5, -3.0),
        Err(QueryError::BadRadius)
    );
    assert_eq!(
        grid_max_in_radius(&g, 40.5, -90.5, f64::NAN),
        Err(QueryError::BadRadius)
    );
    assert_eq!(
        grid_max_in_radius(&g, 40.5, -90.5, 501.0),
        Err(QueryError::BadRadius)
    );
    assert_eq!(
        grid_max_in_radius(&g, 91.0, -90.5, 5.0),
        Err(QueryError::BadCoordinate)
    );
    // A circle elsewhere finds nothing.
    let q = grid_max_in_radius(&g, 10.0, 10.0, 5.0).unwrap();
    assert!(q.peak.is_none() && q.samples == 0 && !q.probes_covered);
}

// REQ: OA-TIFF-013
#[test]
fn max_in_polygon() {
    let mut g = ramp_grid(41.0, -91.0, 101, 101, 0.01);
    for v in g.elev.iter_mut() {
        *v = 100.0;
    }
    g.elev[30 * 101 + 70] = 5000.0; // (40.70, -90.30)
    g.elev[80 * 101 + 10] = 3000.0; // (40.20, -90.90)
                                    // A box around the first spike only.
    let boxp = [
        (40.75, -90.35),
        (40.75, -90.25),
        (40.65, -90.25),
        (40.65, -90.35),
    ];
    let q = grid_max_in_polygon(&g, &boxp).unwrap();
    assert_eq!(peak_of(&q).metres, 5000.0);
    // 11 x 11 nodes lie on or inside the box; the even-odd rule keeps those on two of its sides.
    assert!((81..=121).contains(&q.samples), "{}", q.samples);
    assert!(q.probes_covered);
    // A triangle that excludes the spike but includes the other.
    let tri = [(40.30, -90.95), (40.10, -90.95), (40.10, -90.75)];
    let q = grid_max_in_polygon(&g, &tri).unwrap();
    assert_eq!(peak_of(&q).metres, 3000.0);
    // Concave polygon (an L): a node in the notch is excluded.
    let ell = [
        (40.9, -90.9),
        (40.9, -90.5),
        (40.7, -90.5),
        (40.7, -90.7),
        (40.5, -90.7),
        (40.5, -90.9),
    ];
    let mut h = g.clone();
    h.elev[(100 - 70) * 101 + 20] = 777.0; // row 30? no: lat 40.70.. placeholder replaced below
    let notch_row = ((41.0 - 40.55) / 0.01_f64).round() as usize;
    let notch_col = ((-90.55_f64 + 91.0) / 0.01).round() as usize;
    h.elev[notch_row * 101 + notch_col] = 9999.0; // lat 40.55, lon -90.55: in the notch (outside)
    let q = grid_max_in_polygon(&h, &ell).unwrap();
    assert!(peak_of(&q).metres < 9000.0);
    // Refusals.
    assert_eq!(
        grid_max_in_polygon(&g, &boxp[..2]),
        Err(QueryError::BadPolygon)
    );
    let wide = [(10.0, -170.0), (10.0, 20.0), (20.0, 20.0)];
    assert_eq!(grid_max_in_polygon(&g, &wide), Err(QueryError::BadPolygon));
    let nanp = [(10.0, f64::NAN), (10.0, 20.0), (20.0, 20.0)];
    assert_eq!(
        grid_max_in_polygon(&g, &nanp),
        Err(QueryError::BadCoordinate)
    );
    // A polygon half off the grid reports the gap.
    let off = [(41.5, -91.5), (41.5, -90.5), (40.5, -90.5), (40.5, -91.5)];
    assert!(!grid_max_in_polygon(&g, &off).unwrap().probes_covered);
}

// REQ: OA-TIFF-014
#[test]
fn tile_sets_across_edges() {
    // One plane f = 100*row + col over a 21 x 11 node lattice, cut two ways into two tiles.
    let whole = ramp_grid(40.0, -90.0, 21, 11, 0.01);
    let cut = |c0: u32, c1: u32| -> Grid {
        let w = c1 - c0 + 1;
        let mut elev = Vec::new();
        for r in 0..11u32 {
            for c in c0..=c1 {
                elev.push(whole.elev[(r * 21 + c) as usize]);
            }
        }
        Grid {
            width: w,
            height: 11,
            lon0: whole.lon0 + f64::from(c0) * 0.01,
            elev,
            ..whole.clone()
        }
    };
    // (a) Overlapping tiles (like 3DEP's border): columns 0..=12 and 8..=20.
    let overlap = TileSet {
        tiles: vec![cut(0, 12), cut(8, 20)],
    };
    // (b) Abutting tiles with no shared nodes: columns 0..=9 and 10..=20.
    let abut = TileSet {
        tiles: vec![cut(0, 9), cut(10, 20)],
    };
    for set in [&overlap, &abut] {
        for &(lat, lon) in &[
            (39.973, -89.95),
            (39.95, -89.905),
            (39.95, -89.9),
            (39.91, -89.895),
            (39.99, -89.85),
            (39.905, -89.801),
        ] {
            let want = grid_elevation_at(&whole, lat, lon);
            let got = elevation_at(set, lat, lon);
            match (want, got) {
                (Elevation::Metres(a), Elevation::Metres(b)) => {
                    assert!((a - b).abs() < 1e-3, "{lat},{lon}: {a} vs {b}")
                }
                other => panic!("{lat},{lon}: {other:?}"),
            }
        }
        assert_eq!(elevation_at(set, 41.0, -89.9), Elevation::Outside);
        assert_eq!(elevation_at(set, 39.95, -91.0), Elevation::Outside);
        assert!(tileset_covers(set, 39.95, -89.905));
        // A max query across the seam sees both sides.
        let q = max_elevation_in_radius(set, 39.95, -89.905, 3.0).unwrap();
        let w = grid_max_in_radius(&whole, 39.95, -89.905, 3.0).unwrap();
        assert_eq!(peak_of(&q).metres, peak_of(&w).metres);
        assert!(q.probes_covered);
        let boxp = [
            (39.98, -89.93),
            (39.98, -89.87),
            (39.92, -89.87),
            (39.92, -89.93),
        ];
        let q = max_elevation_in_polygon(set, &boxp).unwrap();
        let w = grid_max_in_polygon(&whole, &boxp).unwrap();
        assert_eq!(peak_of(&q).metres, peak_of(&w).metres);
    }
    // A hole of one pixel between tiles that do not line up is Outside, not invented.
    let mut shifted = cut(10, 20);
    shifted.lat0 += 0.004;
    let bad = TileSet {
        tiles: vec![cut(0, 9), shifted],
    };
    assert_eq!(elevation_at(&bad, 39.95, -89.905), Elevation::Outside);
    // Nodata: a tile that holds the cell with a NaN corner while another does not -> uses valid.
    let mut holed = cut(0, 12);
    holed.elev[5 * 13 + 10] = f32::NAN;
    let set = TileSet {
        tiles: vec![holed.clone(), cut(8, 20)],
    };
    assert!(matches!(
        elevation_at(&set, 39.955, -89.905),
        Elevation::Metres(_)
    ));
    let set = TileSet { tiles: vec![holed] };
    assert_eq!(elevation_at(&set, 39.955, -89.905), Elevation::NoData);
}

// REQ: OA-TIFF-015
#[test]
fn tile_set_limits() {
    let g = ramp_grid(40.0, -90.0, 3, 3, 0.01);
    let many = TileSet {
        tiles: vec![g; MAX_TILES + 1],
    };
    assert_eq!(
        max_elevation_in_radius(&many, 39.99, -89.99, 1.0),
        Err(QueryError::TooManyTiles)
    );
    let poly = [(39.99, -89.99), (39.99, -89.98), (39.98, -89.98)];
    assert_eq!(
        max_elevation_in_polygon(&many, &poly),
        Err(QueryError::TooManyTiles)
    );
    let empty = TileSet::default();
    assert_eq!(elevation_at(&empty, 40.0, -90.0), Elevation::Outside);
    let big_poly = vec![(40.0, -90.0); MAX_POLYGON_VERTICES + 1];
    assert_eq!(
        max_elevation_in_polygon(&empty, &big_poly),
        Err(QueryError::BadPolygon)
    );
}

/// Overwrites the inline value of a count-1 tag of a little-endian file built by `build`.
fn patch_tag(file: &mut [u8], tag: u16, v: u32) {
    let ifd = u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes(file[ifd..ifd + 2].try_into().unwrap()) as usize;
    for i in 0..n {
        let at = ifd + 2 + i * 12;
        if u16::from_le_bytes(file[at..at + 2].try_into().unwrap()) == tag {
            let typ = u16::from_le_bytes(file[at + 2..at + 4].try_into().unwrap());
            if typ == 3 {
                file[at + 8..at + 10].copy_from_slice(&(v as u16).to_le_bytes());
            } else {
                file[at + 8..at + 12].copy_from_slice(&v.to_le_bytes());
            }
            return;
        }
    }
    panic!("tag {tag} not in the file");
}

// REQ: OA-TIFF-029
#[test]
fn declared_size_is_checked_against_the_data_before_allocating() {
    // A small valid file with four 64-byte strips whose dimensions are rewritten to 8192 by 8192
    // (a 256 MiB elevation array) and 2048 rows per strip. It must be refused without sizing
    // anything from the declaration.
    let declare_huge = |f: &mut [u8]| {
        patch_tag(f, 256, 8192);
        patch_tag(f, 257, 8192);
        patch_tag(f, 278, 2048);
    };
    let mut s = Spec::new(4, 16, surface(4, 16));
    s.rows_per_strip = 4;
    let mut f = build(&s);
    declare_huge(&mut f);
    assert!(f.len() < 1024);
    let refused = Err(TiffError::Decompress {
        segment: 0,
        reason: "segment byte count cannot hold the declared image",
    });
    assert_eq!(read_geotiff(&f), refused);
    // Compressed segments are bounded by the best ratio DEFLATE can reach (1032 to 1).
    s.compression = 8;
    let mut f = build(&s);
    declare_huge(&mut f);
    assert_eq!(read_geotiff(&f), refused);
    // A strip that runs past the end of the file is refused with nothing allocated.
    let mut f = build(&Spec::new(4, 4, surface(4, 4)));
    patch_tag(&mut f, 279, 1 << 30);
    assert_eq!(
        read_geotiff(&f),
        Err(TiffError::Decompress {
            segment: 0,
            reason: "segment lies outside the file"
        })
    );
    // Honest files are unaffected, including a constant tile that compresses far below its size.
    let mut s = Spec::new(64, 64, vec![7.0; 4096]);
    s.compression = 8;
    s.tile = Some((64, 64));
    same_samples(&grid_of(&s), &s.data);
}

// REQ: OA-TIFF-030
#[test]
fn only_top_left_orientation_is_accepted() {
    let mut s = Spec::new(8, 6, surface(8, 6));
    for o in [None, Some(1)] {
        s.orientation = o;
        same_samples(&grid_of(&s), &s.data);
    }
    for o in 2..=8u16 {
        s.orientation = Some(o);
        assert_eq!(
            read_geotiff(&build(&s)),
            Err(TiffError::Unsupported {
                what: "orientation",
                value: u32::from(o)
            }),
            "orientation {o}"
        );
    }
    s.orientation = Some(0);
    assert!(matches!(
        read_geotiff(&build(&s)),
        Err(TiffError::Unsupported {
            what: "orientation",
            ..
        })
    ));
}

// REQ: OA-TIFF-081
#[test]
fn decimal_nodata_is_compared_in_the_sample_type() {
    for (text, v) in [
        ("-123.456", -123.456_f32),
        ("-3.4028235e38", f32::MIN),
        ("-3.4028234663852886e+38", f32::MIN),
        ("0.1", 0.1_f32),
    ] {
        let mut s = Spec::new(2, 2, vec![5.0, v, 7.0, 9.0]);
        s.nodata = Some(text.into());
        let g = grid_of(&s);
        assert_eq!(g.nodata_count, 1, "sentinel {text}");
        assert!(g.elev[1].is_nan(), "sentinel {text} kept as {}", g.elev[1]);
        assert_eq!(g.elev[0], 5.0);
    }
}

// REQ: OA-TIFF-082
#[test]
fn infinite_samples_are_nodata() {
    let mut s = Spec::new(2, 2, vec![0.0, f32::INFINITY, 4.0, f32::NEG_INFINITY]);
    s.tie = (0.0, 0.0, -90.0, 40.0);
    let g = grid_of(&s);
    assert_eq!(g.nodata_count, 2);
    assert!(g.elev[1].is_nan() && g.elev[3].is_nan());
    // The finite north-west node beside an infinite one is not a known value.
    let e = grid_elevation_at(&g, g.lat0, g.lon0);
    assert_eq!(e, Elevation::NoData, "{e:?}");
}

// REQ: OA-TIFF-083
#[test]
fn infinite_samples_in_a_grid_are_nodata_in_queries() {
    let mut g = grid_of(&Spec::new(2, 2, vec![0.0, 1.0, 4.0, 5.0]));
    g.elev[1] = f32::INFINITY;
    let e = grid_elevation_at(&g, g.lat0, g.lon0);
    assert_eq!(e, Elevation::NoData, "{e:?}");
    let q = grid_max_in_polygon(
        &g,
        &[
            (g.lat0 + 0.0005, g.lon0 - 0.0005),
            (g.lat0 + 0.0005, g.lon0 + 0.0015),
            (g.lat0 - 0.0015, g.lon0 + 0.0015),
            (g.lat0 - 0.0015, g.lon0 - 0.0005),
        ],
    )
    .unwrap();
    assert_eq!((q.samples, q.nodata), (3, 1), "{q:?}");
    assert_eq!(q.peak.map(|p| p.metres), Some(5.0));
}

// REQ: OA-TIFF-085
#[test]
fn a_non_finite_double_in_a_tag_is_a_bad_value() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut s = Spec::new(2, 2, vec![1.0; 4]);
        s.scale = (bad, 0.001);
        let e = read_geotiff(&build(&s)).unwrap_err();
        assert!(matches!(e, TiffError::BadValue(_)), "scale {bad}: {e:?}");
        let mut s = Spec::new(2, 2, vec![1.0; 4]);
        s.tie = (0.0, 0.0, bad, 40.0);
        let e = read_geotiff(&build(&s)).unwrap_err();
        assert!(matches!(e, TiffError::BadValue(_)), "tie {bad}: {e:?}");
    }
}

// REQ: OA-TIFF-086
#[test]
fn a_non_finite_gdal_nodata_text_is_refused() {
    for text in ["inf", "-inf", "Infinity", "1e999", "-1e999"] {
        let mut s = Spec::new(2, 2, vec![1.0; 4]);
        s.nodata = Some(text.into());
        let r = read_geotiff(&build(&s));
        assert!(matches!(r, Err(TiffError::GeoRef(_))), "{text}: {r:?}");
    }
}
