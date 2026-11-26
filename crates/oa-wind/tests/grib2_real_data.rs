//! Real NOAA and ECMWF files, read from `OPENATCA_DATA` (the openatca-data directory). Every
//! test skips with a printed note when the variable or the files are absent.
//!
//! RAP files are entirely JPEG 2000 (template 5.40), decoded by `oa-j2k`. Their grid, product
//! and inventory are checked through `inspect_stream`; their packed integers are compared with
//! `derived/grib2-repack/rap/`, the same files with the integers decoded by OpenJPEG outside
//! this repository and re-wrapped as simple packing (`MAKE.py` beside them).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_grib2::*;
use std::path::{Path, PathBuf};

fn data_dir() -> Option<PathBuf> {
    let d = std::env::var_os("OPENATCA_DATA").map(PathBuf::from)?;
    d.is_dir().then_some(d)
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    // The files directly in `dir`; a sealed one is skipped, never opened (OA-SEAL-012).
    let mut v: Vec<PathBuf> = match oa_sealed::walk_unsealed(dir) {
        Ok(w) => w
            .files
            .into_iter()
            .filter(|p| p.parent() == Some(dir) && p.extension().is_some_and(|e| e == "grib2"))
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

macro_rules! skip_unless_data {
    ($d:ident) => {
        let Some($d) = data_dir() else {
            println!("OPENATCA_DATA not set: skipping real-data test");
            return;
        };
    };
}

fn level_hpa(f: &FieldInfo) -> Option<u32> {
    (f.level.kind == LEVEL_ISOBARIC).then(|| (f.level.value / 100.0).round() as u32)
}

/// (variable, level) pairs of an index that this decoder names, in file order.
fn idx_pairs(text: &str) -> Vec<(String, String)> {
    let idx = parse_idx(text).unwrap();
    (0..idx_len(&idx))
        .filter(|&k| ["HGT", "TMP", "UGRD", "VGRD", "RH", "VVEL"].contains(&idx.var[k].as_str()))
        .filter(|&k| idx.level[k].ends_with(" mb"))
        .map(|k| (idx.var[k].clone(), idx.level[k].clone()))
        .collect()
}

// REQ: OA-GRIB-016
#[test]
fn real_rap_and_hrrr_inventory_matches_idx_and_grid() {
    skip_unless_data!(dir);
    let mut checked = 0;
    let mut shown = std::collections::BTreeSet::new();
    let mut packings = std::collections::BTreeMap::<String, u32>::new();
    for sub in ["rap", "hrrr"] {
        for path in files_in(&dir.join("raw").join(sub)) {
            let data = oa_sealed::read(&path).unwrap();
            let ins = inspect_stream(&data);
            assert!(ins.stopped.is_none(), "{path:?}: {:?}", ins.stopped);
            assert!(ins.skipped.is_empty(), "{path:?}: {:?}", ins.skipped);
            assert!(ins.fields.len() as u32 >= ins.messages);
            for f in &ins.fields {
                *packings
                    .entry(format!("{sub} 5.{}", f.packing_template))
                    .or_insert(0) += 1;
                // The first grid point is where the template says.
                let (i, j) = latlon_to_ij(
                    &f.grid,
                    ij_to_latlon(&f.grid, 0.0, 0.0).unwrap().0,
                    ij_to_latlon(&f.grid, 0.0, 0.0).unwrap().1,
                )
                .unwrap();
                assert!(i.abs() < 1e-6 && j.abs() < 1e-6, "{path:?}: {i} {j}");
                let Grid::Lambert(l) = f.grid else {
                    panic!("not Lambert")
                };
                let (la, lo) = ij_to_latlon(&f.grid, 0.0, 0.0).unwrap();
                assert!((la - l.la1).abs() < 1e-7, "{path:?}: la {la} vs {}", l.la1);
                assert!(
                    (wrap(lo) - wrap(l.lo1)).abs() < 1e-7,
                    "{path:?}: lo {lo} vs {}",
                    l.lo1
                );
                if shown.insert(sub) {
                    let (nx, ny) = grid_dims(&f.grid);
                    let (la2, lo2) =
                        ij_to_latlon(&f.grid, f64::from(nx - 1), f64::from(ny - 1)).unwrap();
                    println!("{sub}: {nx} x {ny}, first point ({la:.6}, {lo:.6}) as stated, last point ({la2:.6}, {lo2:.6})");
                }
                assert_eq!(f.discipline, 0);
                assert_ne!(f.parameter, Parameter::Other, "{path:?}");
                assert_eq!(f.level.kind, LEVEL_ISOBARIC);
                assert_eq!(f.ref_time.year, 2026);
            }
            // Message order matches the index: names and levels agree line by line.
            let idx_path = PathBuf::from(format!("{}.idx", path.display()));
            let text = oa_sealed::read_to_string(&idx_path).unwrap();
            let want = idx_pairs(&text);
            let got: Vec<(String, String)> = ins
                .fields
                .iter()
                .map(|f| {
                    (
                        parameter_name(f.parameter).unwrap().to_string(),
                        format!("{} mb", level_hpa(f).unwrap()),
                    )
                })
                .collect();
            // The subsets hold only the fields that were requested: each must appear in the
            // index, in the same order, under the same name and level.
            let mut it = want.iter();
            for g in &got {
                assert!(
                    it.any(|w| w == g),
                    "{path:?}: {g:?} not found in order in the index"
                );
            }
            checked += 1;
        }
    }
    println!("inventory: {checked} files checked; packing templates {packings:?}");
    if checked == 0 {
        println!("no RAP/HRRR files present: nothing to check");
    }
}

fn wrap(d: f64) -> f64 {
    let w = d.rem_euclid(360.0);
    if w >= 180.0 {
        w - 360.0
    } else {
        w
    }
}

/// Every field of the raw RAP and HRRR files (RAP through JPEG 2000).
fn load_fields(dir: &Path) -> Vec<(String, Field)> {
    let mut out = Vec::new();
    for d in [dir.join("raw/rap"), dir.join("raw/hrrr")] {
        for path in files_in(&d) {
            let data = oa_sealed::read(&path).unwrap();
            let t0 = std::time::Instant::now();
            let dec = decode_stream(&data);
            if path.to_string_lossy().contains("/hrrr/") {
                println!(
                    "HRRR decode: {} fields of {} points in {:?}",
                    dec.fields.len(),
                    dec.fields.first().map_or(0, |f| f.values.len()),
                    t0.elapsed()
                );
            }
            assert!(dec.stopped.is_none(), "{path:?}: {:?}", dec.stopped);
            for f in dec.fields {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                out.push((name, f));
            }
        }
    }
    out
}

fn model_of(name: &str) -> &'static str {
    if name.contains("hrrr") {
        "hrrr"
    } else {
        "rap"
    }
}

// REQ: OA-GRIB-017
#[test]
fn real_fields_are_physically_plausible() {
    skip_unless_data!(dir);
    let fields = load_fields(&dir);
    if fields.is_empty() {
        println!("no decodable RAP/HRRR fields present");
        return;
    }
    let (mut ntmp, mut nwind, mut nhgt, mut npts) = (0u64, 0u64, 0u64, 0u64);
    let (mut tmin, mut tmax, mut vmax) = (f32::MAX, f32::MIN, 0f32);
    let mut files = std::collections::BTreeMap::<String, Vec<&Field>>::new();
    for (name, f) in &fields {
        files.entry(name.clone()).or_default().push(f);
        assert_eq!(
            f.missing_count, 0,
            "{name}: a pressure-level field with holes"
        );
        for &v in &f.values {
            assert!(v.is_finite(), "{name}");
        }
        npts += f.values.len() as u64;
        if f.parameter == Parameter::Tmp {
            for &t in &f.values {
                assert!(
                    (180.0..=330.0).contains(&t),
                    "{name}: TMP {t} K at {:?}",
                    f.level
                );
                tmin = tmin.min(t);
                tmax = tmax.max(t);
                ntmp += 1;
            }
        }
    }
    for (name, fs) in &files {
        // Wind speed, per level, from the u and v fields.
        for u in fs.iter().filter(|f| f.parameter == Parameter::Ugrd) {
            let v = fs
                .iter()
                .find(|f| f.parameter == Parameter::Vgrd && f.level == u.level)
                .unwrap();
            for (a, b) in u.values.iter().zip(&v.values) {
                let s = a.hypot(*b);
                assert!(s < 120.0, "{name}: wind {s} m/s at {:?}", u.level);
                vmax = vmax.max(s);
                nwind += 1;
            }
        }
        // Height rises as pressure falls, at every grid point.
        let mut hgt: Vec<&&Field> = fs
            .iter()
            .filter(|f| f.parameter == Parameter::Hgt)
            .collect();
        hgt.sort_by(|a, b| b.level.value.partial_cmp(&a.level.value).unwrap());
        for pair in hgt.windows(2) {
            let (lo, hi) = (pair[0], pair[1]); // lo: higher pressure
            assert!(lo.level.value > hi.level.value);
            for (k, (a, b)) in lo.values.iter().zip(&hi.values).enumerate() {
                assert!(
                    b > a,
                    "{name}: HGT {} Pa = {a} but {} Pa = {b} at point {k}",
                    lo.level.value,
                    hi.level.value
                );
                nhgt += 1;
            }
        }
    }
    println!(
        "physical: {} fields in {} files, {npts} values; TMP {ntmp} pts in [{tmin}, {tmax}] K; wind {nwind} pts, max speed {vmax} m/s; HGT monotonic at {nhgt} point pairs",
        fields.len(),
        files.len()
    );
}

/// Small deterministic generator for sampling.
fn next(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 11) as f64) / ((1u64 << 53) as f64)
}

// REQ: OA-GRIB-018
#[test]
fn real_rap_and_hrrr_250mb_winds_agree() {
    skip_unless_data!(dir);
    let fields = load_fields(&dir);
    let pick = |model: &str, p: Parameter| -> Option<&Field> {
        fields
            .iter()
            .find(|(n, f)| {
                model_of(n) == model
                    && n.contains("t12z")
                    && f.parameter == p
                    && f.level.kind == LEVEL_ISOBARIC
                    && (f.level.value - 25_000.0).abs() < 1.0
            })
            .map(|(_, f)| f)
    };
    let (Some(hu), Some(hv), Some(ru), Some(rv)) = (
        pick("hrrr", Parameter::Ugrd),
        pick("hrrr", Parameter::Vgrd),
        pick("rap", Parameter::Ugrd),
        pick("rap", Parameter::Vgrd),
    ) else {
        println!("need HRRR and RAP 250 mb u and v at 12z: skipping");
        return;
    };
    assert_eq!(hu.ref_time, ru.ref_time, "different analysis times");
    let (nx, ny) = grid_dims(&hu.grid);
    let mut st = 0x9e37_79b9_7f4a_7c15u64;
    let (mut rot, mut unrot) = (Vec::new(), Vec::new());
    let (mut hmean, mut n) = (0f64, 0);
    while rot.len() < 1000 {
        // Inside the HRRR domain, a few cells from the edge.
        let i = 3.0 + next(&mut st) * (f64::from(nx) - 7.0);
        let j = 3.0 + next(&mut st) * (f64::from(ny) - 7.0);
        let (lat, lon) = ij_to_latlon(&hu.grid, i, j).unwrap();
        let Some(h) = wind_at(hu, hv, lat, lon).unwrap() else {
            continue;
        };
        let Some(r) = wind_at(ru, rv, lat, lon).unwrap() else {
            continue;
        };
        rot.push(f64::from(h.east - r.east).hypot(f64::from(h.north - r.north)));
        // Without rotation: the raw grid-relative components of each model, as a careless
        // reader would compare them.
        let (hu_g, hv_g) = (
            sample_bilinear(hu, lat, lon).unwrap(),
            sample_bilinear(hv, lat, lon).unwrap(),
        );
        let (ru_g, rv_g) = (
            sample_bilinear(ru, lat, lon).unwrap(),
            sample_bilinear(rv, lat, lon).unwrap(),
        );
        unrot.push(f64::from(hu_g - ru_g).hypot(f64::from(hv_g - rv_g)));
        hmean += f64::from(h.east.hypot(h.north));
        n += 1;
    }
    let q = |v: &mut Vec<f64>, p: f64| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f64 * p).round() as usize]
    };
    let (rm, r95) = (q(&mut rot, 0.5), q(&mut rot, 0.95));
    let (um, u95) = (q(&mut unrot, 0.5), q(&mut unrot, 0.95));
    println!(
        "RAP vs HRRR 250 mb wind, {n} points, mean HRRR speed {:.1} m/s: earth-relative vector difference median {rm:.2} p95 {r95:.2} m/s; grid-relative components compared without rotation: median {um:.2} p95 {u95:.2} m/s",
        hmean / f64::from(n)
    );
    assert!(rm < 4.0, "median {rm}");
    assert!(r95 < 12.0, "p95 {r95}");
    assert!(um > rm, "rotation must reduce the disagreement");
}

fn read_test_file(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let p = dir.join("raw/grib2-testdata").join(name);
    match oa_sealed::read(&p) {
        Ok(d) => Some(d),
        Err(_) => {
            println!("{p:?} absent: skipping");
            None
        }
    }
}

fn range(f: &Field) -> (f32, f32) {
    f.values
        .iter()
        .copied()
        .filter(|v| !v.is_nan())
        .fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v), a.1.max(v)))
}

// REQ: OA-GRIB-019
#[test]
fn public_ecmwf_test_files_decode_to_physical_fields() {
    skip_unless_data!(dir);
    // 5.0 simple packing, regular latitude/longitude, 2 m temperature.
    if let Some(d) = read_test_file(&dir, "regular_latlon_surface.grib2") {
        let r = decode_stream(&d);
        assert!(r.stopped.is_none() && r.skipped.is_empty());
        let f = &r.fields[0];
        assert_eq!((f.packing_template, f.parameter), (0, Parameter::Tmp));
        assert_eq!(
            f.level,
            Level {
                kind: LEVEL_HEIGHT_ABOVE_GROUND,
                value: 2.0
            }
        );
        let (lo, hi) = range(f);
        println!(
            "regular_latlon_surface: {:?} TMP 2 m in [{lo}, {hi}] K",
            grid_dims(&f.grid)
        );
        assert!(lo > 180.0 && hi < 330.0 && f.missing_count == 0);
    }
    if let Some(d) = read_test_file(&dir, "missing.grib2") {
        let r = decode_stream(&d);
        let f = &r.fields[0];
        let (lo, hi) = range(f);
        assert!(lo > 180.0 && hi < 330.0);
    }
    // 5.3 order 1 with a bitmap (surface pressure on a masked level).
    if let Some(d) = read_test_file(&dir, "gfs.c255.grib2") {
        let r = decode_stream(&d);
        let f = &r.fields[0];
        assert_eq!((f.packing_template, f.parameter), (3, Parameter::Pres));
        let (lo, hi) = range(f);
        println!(
            "gfs.c255: {:?} PRES {} missing, in [{lo}, {hi}] Pa",
            grid_dims(&f.grid),
            f.missing_count
        );
        assert!(lo > 30_000.0 && hi < 110_000.0);
        assert_eq!(f.missing_count, 144 * 73 - 2847);
        assert!(smooth(f, 0.25));
    }
    // 5.3 order 2 with a bitmap.
    if let Some(d) = read_test_file(&dir, "grid_complex_spatial_differencing.grib2") {
        let r = decode_stream(&d);
        let f = &r.fields[0];
        assert_eq!(f.packing_template, 3);
        let (lo, hi) = range(f);
        println!(
            "grid_complex_spatial_differencing: {:?} {} missing, in [{lo}, {hi}]",
            grid_dims(&f.grid),
            f.missing_count
        );
        assert_eq!(f.missing_count, 461 * 181 - 32872);
        assert!(smooth(f, 0.25));
    }
    // 5.3 order 2 with missing-value management 1 and about half the points missing: snow depth.
    if let Some(d) = read_test_file(&dir, "gfs.complex.mvmu.grib2") {
        let r = decode_stream(&d);
        assert!(r.skipped.is_empty() && r.stopped.is_none());
        let f = &r.fields[0];
        assert_eq!((f.packing_template, f.category, f.number), (3, 1, 11));
        let (lo, hi) = range(f);
        println!(
            "gfs.complex.mvmu: {:?} {} missing, snow depth in [{lo}, {hi}] m",
            grid_dims(&f.grid),
            f.missing_count
        );
        assert!(lo >= 0.0 && hi < 20.0);
        assert_eq!(f.missing_count, 1440 * 721 - 481_339);
        assert!(smooth(f, 0.25));
    }
    // Refusals are typed and the walk goes on.
    if let Some(d) = read_test_file(&dir, "constant_field.grib2") {
        let r = decode_stream(&d);
        assert_eq!(r.skipped[0].error, Grib2Error::UnsupportedGridTemplate(1));
    }
}

/// True when at most 1% of adjacent point pairs differ by more than `frac` of the field's range
/// (a mis-differenced field is noise or a runaway ramp).
fn smooth(f: &Field, frac: f32) -> bool {
    let (nx, _) = grid_dims(&f.grid);
    let (lo, hi) = range(f);
    let lim = (hi - lo) * frac;
    let (mut bad, mut n) = (0u32, 0u32);
    for w in f
        .values
        .windows(2)
        .enumerate()
        .filter(|(k, _)| (k + 1) % nx as usize != 0)
    {
        let (a, b) = (w.1[0], w.1[1]);
        if a.is_nan() || b.is_nan() {
            continue;
        }
        n += 1;
        if (a - b).abs() > lim {
            bad += 1;
        }
    }
    println!("smoothness: {bad} of {n} adjacent pairs jump by more than {frac} of range");
    bad * 100 <= n
}

/// (section number, whole section) of every section of a message after section 0.
fn sections(msg: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut pos = 16;
    while pos + 5 <= msg.len() - 4 {
        let len = u32::from_be_bytes(msg[pos..pos + 4].try_into().unwrap()) as usize;
        out.push((msg[pos + 4], &msg[pos..pos + len]));
        pos += len;
    }
    out
}

// REQ: OA-GRIB-021
#[test]
fn rap_jpeg2000_integers_equal_the_openjpeg_repack_exactly() {
    skip_unless_data!(dir);
    let raw = files_in(&dir.join("raw/rap"));
    let (mut files, mut fields, mut points) = (0u32, 0u64, 0u64);
    let (mut total, mut worst) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    let mut depths = std::collections::BTreeMap::<u8, u32>::new();
    for path in &raw {
        let name = path.file_name().unwrap();
        let rp_path = dir.join("derived/grib2-repack/rap").join(name);
        let Ok(repacked) = oa_sealed::read(&rp_path) else {
            println!("{rp_path:?} absent: skipping this file");
            continue;
        };
        let data = oa_sealed::read(path).unwrap();
        let (a, b) = (
            scan_messages(&data).unwrap(),
            scan_messages(&repacked).unwrap(),
        );
        assert_eq!(a.len(), b.len(), "{path:?}: message count");
        for (ma, mb) in a.iter().zip(&b) {
            let sa = sections(&data[ma.offset..ma.offset + ma.length]);
            let sb = sections(&repacked[mb.offset..mb.offset + mb.length]);
            let five_a: Vec<_> = sa.iter().filter(|s| s.0 == 5).collect();
            let seven_a: Vec<_> = sa.iter().filter(|s| s.0 == 7).collect();
            let five_b: Vec<_> = sb.iter().filter(|s| s.0 == 5).collect();
            let seven_b: Vec<_> = sb.iter().filter(|s| s.0 == 7).collect();
            assert_eq!(five_a.len(), five_b.len());
            assert_eq!(five_a.len(), seven_a.len());
            assert_eq!(five_b.len(), seven_b.len());
            for k in 0..five_a.len() {
                let ra = oa_grib2::pack::parse_repr(five_a[k].1).unwrap();
                let rb = oa_grib2::pack::parse_repr(five_b[k].1).unwrap();
                assert_eq!((ra.template, rb.template), (40, 0), "{path:?}");
                assert_eq!(
                    (ra.r.to_bits(), ra.e, ra.d, ra.nbits, ra.ndata),
                    (rb.r.to_bits(), rb.e, rb.d, rb.nbits, rb.ndata),
                    "{path:?}: scaling parameters differ"
                );
                let t0 = std::time::Instant::now();
                let xa = oa_grib2::pack::unpack_ints(&ra, &seven_a[k].1[5..]).unwrap();
                let dt = t0.elapsed();
                let xb = oa_grib2::pack::unpack_ints(&rb, &seven_b[k].1[5..]).unwrap();
                assert_eq!(xa.len(), xb.len());
                if let Some(i) = (0..xa.len()).find(|&i| xa[i] != xb[i]) {
                    panic!(
                        "{path:?} field {k}: sample {i} is {} but the OpenJPEG repack has {}",
                        xa[i], xb[i]
                    );
                }
                total += dt;
                worst = worst.max(dt);
                fields += 1;
                points += xa.len() as u64;
                *depths.entry(ra.nbits).or_insert(0) += 1;
            }
        }
        files += 1;
    }
    if fields == 0 {
        println!("no RAP files with repacked copies: nothing to compare");
        return;
    }
    println!(
        "JPEG 2000 vs OpenJPEG repack: {files} files, {fields} fields, {points} points, 0 mismatches; \
         decode time per field mean {:?}, worst {:?}; fields by bit depth {depths:?}",
        total / fields as u32,
        worst
    );
}

// REQ: OA-GRIB-022
#[test]
fn gefs_jpeg2000_file_decodes_to_physical_fields() {
    skip_unless_data!(dir);
    let Some(d) = read_test_file(&dir, "gep10.t12z.pgrb2af78.grib2") else {
        return;
    };
    let t0 = std::time::Instant::now();
    let r = decode_stream(&d);
    let dt = t0.elapsed();
    assert_eq!(r.messages, 43);
    assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    assert!(
        r.stopped.is_some(),
        "file ends with bytes that are not a message"
    );
    assert_eq!(r.fields.len(), 51);
    let (mut ntmp, mut nrh, mut nwind) = (0, 0, 0);
    let mut hgt: Vec<&Field> = Vec::new();
    for f in &r.fields {
        assert_eq!(f.packing_template, 40);
        assert_eq!(grid_dims(&f.grid), (360, 181));
        assert_eq!(f.missing_count, 0);
        assert!(f.values.iter().all(|v| v.is_finite()));
        let (lo, hi) = range(f);
        match f.parameter {
            Parameter::Tmp => {
                assert!(lo > 180.0 && hi < 330.0, "TMP [{lo}, {hi}]");
                ntmp += 1;
            }
            Parameter::Rh => {
                assert!(lo >= 0.0 && hi <= 100.5, "RH [{lo}, {hi}]");
                nrh += 1;
            }
            Parameter::Ugrd | Parameter::Vgrd => {
                assert!(lo > -120.0 && hi < 120.0, "wind [{lo}, {hi}]");
                nwind += 1;
            }
            Parameter::Hgt if f.level.kind == LEVEL_ISOBARIC => hgt.push(f),
            _ => {}
        }
    }
    // Height rises as pressure falls, at every point.
    hgt.sort_by(|a, b| b.level.value.partial_cmp(&a.level.value).unwrap());
    assert_eq!(hgt.len(), 7);
    for w in hgt.windows(2) {
        assert!(w[0].values.iter().zip(&w[1].values).all(|(a, b)| b > a));
    }
    println!(
        "GEFS 5.40: {} fields decoded in {dt:?}; TMP {ntmp}, RH {nrh}, wind components {nwind}; HGT monotonic over 7 levels",
        r.fields.len()
    );
}
