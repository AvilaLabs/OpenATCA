#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! Unit tests on synthetic grids with known analytic fields. The real-data checks skip with a
//! printed note when `OPENATCA_DATA` is absent.

use crate::archive::*;
use crate::check::*;
use crate::error::*;
use crate::hour::*;
use crate::isa::*;
use crate::sample::*;
use crate::*;
use oa_grib2::{
    ij_to_latlon, lambert_setup, to_earth_relative, Field, Grid, LambertInput, Level, Parameter,
    RefTime, LEVEL_ISOBARIC,
};
use std::path::PathBuf;

const NX: u32 = 20;
const NY: u32 = 15;
const LEVELS_HPA: [f64; 7] = [100.0, 200.0, 300.0, 500.0, 700.0, 850.0, 1000.0];

/// A coarse (100 km) RAP-like Lambert grid so the convergence angle changes visibly across it.
fn grid(flags: u8) -> Grid {
    Grid::Lambert(
        lambert_setup(LambertInput {
            nx: NX,
            ny: NY,
            la1: 30.0,
            lo1: 265.0,
            lad: 25.0,
            lov: 265.0,
            dx: 100_000.0,
            dy: 100_000.0,
            latin1: 25.0,
            latin2: 25.0,
            radius: 6_371_229.0,
            flags,
            scan: 0x40,
        })
        .unwrap(),
    )
}

fn ref_time(hour: u8) -> RefTime {
    RefTime {
        year: 2026,
        month: 11,
        day: 14,
        hour,
        minute: 0,
        second: 0,
    }
}

fn valid(hour: u8) -> i64 {
    oa_grib2::ref_unix_seconds(&ref_time(hour))
}

fn field(g: Grid, p: Parameter, pa: f32, hour: u8, values: Vec<f32>) -> Field {
    Field {
        discipline: 0,
        category: 0,
        number: 0,
        parameter: p,
        level: Level {
            kind: LEVEL_ISOBARIC,
            value: pa,
        },
        ref_time: ref_time(hour),
        time_unit: 1,
        forecast_time: 0,
        product_template: 0,
        interval: None,
        packing_template: 0,
        grid: g,
        values,
        missing_count: 0,
    }
}

/// Fields for every level; `f(parameter, i, j, lat, lon, p_hpa)` gives the value at a point.
fn fields<F: Fn(Parameter, f64, f64, f64, f64, f64) -> f64>(
    flags: u8,
    hour: u8,
    f: F,
) -> Vec<Field> {
    let g = grid(flags);
    let mut out = Vec::new();
    for &p in &LEVELS_HPA {
        for par in [
            Parameter::Ugrd,
            Parameter::Vgrd,
            Parameter::Tmp,
            Parameter::Hgt,
        ] {
            let mut vals = Vec::new();
            for j in 0..NY {
                for i in 0..NX {
                    let (lat, lon) = ij_to_latlon(&g, f64::from(i), f64::from(j)).unwrap();
                    vals.push(f(par, f64::from(i), f64::from(j), lat, lon, p) as f32);
                }
            }
            out.push(field(g, par, (p * 100.0) as f32, hour, vals));
        }
    }
    out
}

fn linear(par: Parameter, i: f64, j: f64, _lat: f64, _lon: f64, p: f64) -> f64 {
    let lp = p.ln();
    match par {
        Parameter::Ugrd => 3.0 + 0.5 * i - 0.2 * j + 2.0 * lp,
        Parameter::Vgrd => -4.0 + 0.1 * i + 0.3 * j - 1.0 * lp,
        Parameter::Tmp => 200.0 + 1.5 * i + 0.25 * j + 10.0 * lp,
        _ => 5000.0 - 20.0 * i + 4.0 * j - 1500.0 * lp,
    }
}

fn linear_hour(hour: u8) -> Hour {
    build_hour(fields(0, hour, linear)).unwrap()
}

fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol, "{a} vs {b} (tol {tol})");
}

// REQ: OA-WIND-001
#[test]
fn isa_pressure_altitude() {
    close(pressure_hpa_from_altitude_ft(0.0).unwrap(), 1013.25, 1e-9);
    // Published ISA values: FL180 506.0 hPa, FL300 300.9 hPa, FL360 226.3 hPa, FL400 187.5 hPa.
    close(pressure_hpa_from_altitude_ft(18_000.0).unwrap(), 506.0, 0.1);
    close(pressure_hpa_from_altitude_ft(30_000.0).unwrap(), 300.9, 0.1);
    close(
        pressure_hpa_from_altitude_ft(36_089.24).unwrap(),
        226.32,
        0.01,
    );
    close(
        pressure_hpa_from_altitude_ft(40_000.0).unwrap(),
        187.54,
        0.05,
    );
    // Continuous at the tropopause.
    let below = pressure_hpa_from_altitude_ft(36_089.0).unwrap();
    let above = pressure_hpa_from_altitude_ft(36_090.0).unwrap();
    assert!(below > above && below - above < 0.1);
    for ft in [-1000.0, 0.0, 5000.0, 18_000.0, 36_000.0, 41_000.0, 60_000.0] {
        let p = pressure_hpa_from_altitude_ft(ft).unwrap();
        close(altitude_ft_from_pressure_hpa(p).unwrap(), ft, 1e-6);
    }
}

// REQ: OA-WIND-001
#[test]
fn isa_refuses_what_it_does_not_cover() {
    for bad in [f64::NAN, f64::INFINITY] {
        assert!(matches!(
            pressure_hpa_from_altitude_ft(bad),
            Err(Refusal::BadInput(_))
        ));
    }
    assert!(matches!(
        pressure_hpa_from_altitude_ft(70_000.0),
        Err(Refusal::AltitudeOutsideIsa { .. })
    ));
    assert!(matches!(
        pressure_hpa_from_altitude_ft(-20_000.0),
        Err(Refusal::AltitudeOutsideIsa { .. })
    ));
    assert!(altitude_ft_from_pressure_hpa(0.0).is_err());
    assert!(altitude_ft_from_pressure_hpa(f64::NAN).is_err());
    assert!(altitude_ft_from_pressure_hpa(10.0).is_err());
}

// REQ: OA-WIND-002
#[test]
fn horizontal_bilinear_is_exact_for_a_linear_field() {
    let h = linear_hour(12);
    let g = h.grid;
    for (i, j) in [
        (0.0, 0.0),
        (3.25, 7.5),
        (10.9, 2.1),
        (18.5, 13.75),
        (19.0, 14.0),
    ] {
        let (lat, lon) = ij_to_latlon(&g, i, j).unwrap();
        for p in [100.0, 300.0, 1000.0] {
            let pt = sample_hour_pressure(&h, lat, lon, p).unwrap();
            let want = |par| linear(par, i, j, lat, lon, p);
            close(pt.u, want(Parameter::Ugrd), 1e-3);
            close(pt.v, want(Parameter::Vgrd), 1e-3);
            close(pt.t, want(Parameter::Tmp), 1e-3);
            close(pt.z, want(Parameter::Hgt), 2e-2);
        }
    }
}

// REQ: OA-WIND-003
#[test]
fn vertical_is_linear_in_log_pressure() {
    let h = linear_hour(12);
    let (lat, lon) = ij_to_latlon(&h.grid, 6.0, 5.0).unwrap();
    // The field is exactly linear in ln p, so log-pressure interpolation recovers it between
    // levels; linear-in-p would not.
    for p in [120.0, 250.0, 400.0, 600.0, 777.0, 930.0, 999.9] {
        let pt = sample_hour_pressure(&h, lat, lon, p).unwrap();
        close(pt.t, linear(Parameter::Tmp, 6.0, 5.0, lat, lon, p), 1e-3);
        close(pt.u, linear(Parameter::Ugrd, 6.0, 5.0, lat, lon, p), 1e-3);
    }
    // The pressure altitude route uses the ISA pressure.
    let hours = [&h];
    let q = Query {
        lat,
        lon,
        alt_ft: 18_000.0,
        time_s: valid(12) as f64,
    };
    let s = sample_hours(&hours, &q).unwrap();
    close(
        s.pressure_hpa,
        pressure_hpa_from_altitude_ft(18_000.0).unwrap(),
        1e-12,
    );
    close(
        s.temp_k,
        linear(Parameter::Tmp, 6.0, 5.0, lat, lon, s.pressure_hpa),
        1e-3,
    );
}

// REQ: OA-WIND-004
#[test]
fn grid_relative_components_are_rotated_to_east_north() {
    // A uniform earth-relative wind (20 m/s east, -5 m/s north) stored the way a grid-relative
    // file would store it: turned the other way by the convergence angle at each grid point.
    let (east, north) = (20.0, -5.0);
    let store = |par: Parameter, _i: f64, _j: f64, _lat: f64, lon: f64, _p: f64| {
        let g = grid(0x08);
        // Invert to_earth_relative: (e, n) -> (u, v) with the same angle.
        let (s, c) = oa_grib2::grid_rotation(&g, lon).sin_cos();
        match par {
            Parameter::Ugrd => east * c - north * s,
            Parameter::Vgrd => east * s + north * c,
            Parameter::Tmp => 250.0,
            _ => 1000.0,
        }
    };
    let rotated = build_hour(fields(0x08, 12, store)).unwrap();
    assert!(oa_grib2::components_grid_relative(&rotated.grid));
    for (i, j) in [(1.0, 1.0), (9.5, 7.5), (17.3, 12.2), (2.2, 13.0)] {
        let (lat, lon) = ij_to_latlon(&rotated.grid, i, j).unwrap();
        let pt = sample_hour_pressure(&rotated, lat, lon, 500.0).unwrap();
        close(pt.u, east, 1e-2);
        close(pt.v, north, 1e-2);
        // Left unrotated the field is visibly wrong where the grid is turned from north.
        let raw = sample_hour_unrotated(&rotated, lat, lon, 500.0).unwrap();
        let turn = oa_grib2::grid_rotation(&rotated.grid, lon).abs();
        if turn > 0.1 {
            assert!((raw.u - east).hypot(raw.v - north) > 1.0);
        }
        // Agrees with the oa-grib2 rotation of the interpolated components.
        let (eu, ev) = to_earth_relative(&rotated.grid, lon, raw.u, raw.v);
        close(pt.u, eu, 1e-9);
        close(pt.v, ev, 1e-9);
    }
    // The same values on a grid whose flags say earth-relative are not rotated.
    let flat = build_hour(fields(0x00, 12, store)).unwrap();
    let (lat, lon) = ij_to_latlon(&flat.grid, 17.0, 5.0).unwrap();
    let a = sample_hour_pressure(&flat, lat, lon, 500.0).unwrap();
    let b = sample_hour_unrotated(&flat, lat, lon, 500.0).unwrap();
    assert_eq!((a.u, a.v), (b.u, b.v));
}

fn const_u_hour(hour: u8, u: f64) -> Hour {
    build_hour(fields(0, hour, move |par, _, _, _, _, _| match par {
        Parameter::Ugrd => u,
        Parameter::Vgrd => 0.0,
        Parameter::Tmp => 250.0,
        _ => 1000.0,
    }))
    .unwrap()
}

// REQ: OA-WIND-005
#[test]
fn time_is_linear_between_the_bracketing_hours() {
    let (a, b, c) = (
        const_u_hour(12, 10.0),
        const_u_hour(13, 20.0),
        const_u_hour(14, 50.0),
    );
    let hours = [&a, &b, &c];
    let (lat, lon) = ij_to_latlon(&a.grid, 8.0, 6.0).unwrap();
    let at = |t: i64| {
        sample_hours(
            &hours,
            &Query {
                lat,
                lon,
                alt_ft: 18_000.0,
                time_s: t as f64,
            },
        )
        .unwrap()
    };
    let s = at(valid(12) + 900);
    close(s.u_ms, 12.5, 1e-6);
    assert_eq!((s.hour0_s, s.hour1_s), (valid(12), valid(13)));
    close(s.time_weight, 0.25, 1e-12);
    // On an hour: weight 0 with the next hour, and the last hour uses the one before it.
    let s = at(valid(13));
    close(s.u_ms, 20.0, 1e-9);
    assert_eq!((s.hour0_s, s.hour1_s), (valid(13), valid(14)));
    let s = at(valid(14));
    close(s.u_ms, 50.0, 1e-9);
    assert_eq!((s.hour0_s, s.hour1_s), (valid(13), valid(14)));
    close(at(valid(13) + 1800).u_ms, 35.0, 1e-6);
}

// REQ: OA-WIND-005
#[test]
fn bracket_cases() {
    let t = [100, 200, 400];
    assert_eq!(bracket(&t, 100.0).unwrap(), (0, 1, 0.0));
    assert_eq!(bracket(&t, 150.0).unwrap(), (0, 1, 0.5));
    assert_eq!(bracket(&t, 300.0).unwrap(), (1, 2, 0.5));
    assert_eq!(bracket(&t, 400.0).unwrap(), (1, 2, 1.0));
    assert_eq!(bracket(&[100], 100.0).unwrap(), (0, 0, 0.0));
    assert!(bracket(&[100], 101.0).is_err());
}

// REQ: OA-WIND-006
#[test]
fn refuses_outside_grid_levels_and_time() {
    let a = const_u_hour(12, 10.0);
    let b = const_u_hour(13, 20.0);
    let hours = [&a, &b];
    let (lat, lon) = ij_to_latlon(&a.grid, 8.0, 6.0).unwrap();
    let ok = Query {
        lat,
        lon,
        alt_ft: 18_000.0,
        time_s: valid(12) as f64 + 60.0,
    };
    assert!(sample_hours(&hours, &ok).is_ok());
    let with = |f: &dyn Fn(&mut Query)| {
        let mut q = ok;
        f(&mut q);
        sample_hours(&hours, &q)
    };
    // Outside the grid: far away, and just beyond the first column.
    assert_eq!(with(&|q| q.lat = 0.0), Err(Refusal::OutsideGrid));
    assert_eq!(with(&|q| q.lon = -20.0), Err(Refusal::OutsideGrid));
    let (lat2, lon2) = ij_to_latlon(&a.grid, -0.5, 5.0).unwrap();
    assert_eq!(
        with(&|q| {
            q.lat = lat2;
            q.lon = lon2
        }),
        Err(Refusal::OutsideGrid)
    );
    // Levels are 100..1000 hPa: 55 000 ft is about 91 hPa, -1000 ft about 1050 hPa.
    assert!(matches!(
        with(&|q| q.alt_ft = 55_000.0),
        Err(Refusal::AboveLevels { .. })
    ));
    assert!(matches!(
        with(&|q| q.alt_ft = -1000.0),
        Err(Refusal::BelowLevels { .. })
    ));
    assert!(matches!(
        with(&|q| q.alt_ft = 70_000.0),
        Err(Refusal::AltitudeOutsideIsa { .. })
    ));
    // The level limits themselves are answered.
    assert!(with(&|q| q.alt_ft = altitude_ft_from_pressure_hpa(100.0).unwrap()).is_ok());
    assert!(with(&|q| q.alt_ft = altitude_ft_from_pressure_hpa(1000.0).unwrap()).is_ok());
    // Time outside the span, by one second.
    assert!(matches!(
        with(&|q| q.time_s = valid(12) as f64 - 1.0),
        Err(Refusal::OutsideTime { .. })
    ));
    assert!(matches!(
        with(&|q| q.time_s = valid(13) as f64 + 1.0),
        Err(Refusal::OutsideTime { .. })
    ));
    assert!(matches!(
        with(&|q| q.lat = f64::NAN),
        Err(Refusal::BadInput(_))
    ));
    assert!(matches!(with(&|q| q.lat = 91.0), Err(Refusal::BadInput(_))));
    assert!(matches!(
        with(&|q| q.time_s = f64::NAN),
        Err(Refusal::BadInput(_))
    ));
    assert_eq!(sample_hours(&[], &ok), Err(Refusal::NoHours));
}

// REQ: OA-WIND-007
#[test]
fn a_missing_corner_value_is_refused_not_filled() {
    let mut f = fields(0, 12, linear);
    // Blank one U point on the 500 hPa level.
    for fld in &mut f {
        if fld.parameter == Parameter::Ugrd && fld.level.value == 50_000.0 {
            if let Some(v) = fld.values.get_mut(6 * NX as usize + 8) {
                *v = f32::NAN;
            }
        }
    }
    let h = build_hour(f).unwrap();
    let near = ij_to_latlon(&h.grid, 8.2, 6.2).unwrap();
    assert_eq!(
        sample_hour_pressure(&h, near.0, near.1, 500.0),
        Err(Refusal::MissingValue)
    );
    // A query that brackets 500 hPa also uses that level.
    assert_eq!(
        sample_hour_pressure(&h, near.0, near.1, 600.0),
        Err(Refusal::MissingValue)
    );
    // Far from the blank, and at a level pair that does not touch it, the answer stands.
    assert!(sample_hour_pressure(&h, near.0, near.1, 250.0).is_ok());
    let far = ij_to_latlon(&h.grid, 15.0, 6.0).unwrap();
    assert!(sample_hour_pressure(&h, far.0, far.1, 600.0).is_ok());
}

// REQ: OA-WIND-008
#[test]
fn build_hour_validates_its_fields() {
    let all = fields(0, 12, linear);
    let h = build_hour(all.clone()).unwrap();
    assert_eq!(hour_levels(&h), 7);
    // Ascending pressure, level-major layout: level 0 is 100 hPa.
    assert_eq!(h.levels_pa.first(), Some(&10_000.0));
    assert_eq!(h.levels_pa.last(), Some(&100_000.0));
    assert_eq!(h.valid_s, valid(12));
    let npts = (NX * NY) as usize;
    assert_eq!(h.u.len(), 7 * npts);
    assert_eq!(hour_bytes(&h), 7 * npts as u64 * 16);
    assert_eq!(h.t.get(npts + 3), all[6].values.get(3));

    // Fields that are not isobaric UGRD/VGRD/TMP/HGT are ignored.
    let mut extra = all.clone();
    let mut rh = field(h.grid, Parameter::Rh, 50_000.0, 12, vec![1.0; npts]);
    extra.push(rh.clone());
    rh.level.kind = 103;
    rh.parameter = Parameter::Tmp;
    extra.push(rh);
    assert_eq!(build_hour(extra).unwrap(), h);

    // A level missing one field.
    let mut missing = all.clone();
    missing.retain(|f| !(f.parameter == Parameter::Hgt && f.level.value == 70_000.0));
    assert_eq!(
        build_hour(missing),
        Err(LoadError::MissingField {
            field: "HGT",
            level_pa: 70_000
        })
    );
    // A field twice.
    let mut dup = all.clone();
    dup.push(all[0].clone());
    assert!(matches!(
        build_hour(dup),
        Err(LoadError::DuplicateField { .. })
    ));
    // Fewer than two levels.
    let one: Vec<Field> = all
        .iter()
        .filter(|f| f.level.value == 50_000.0)
        .cloned()
        .collect();
    assert_eq!(build_hour(one), Err(LoadError::TooFewLevels(1)));
    assert_eq!(build_hour(Vec::new()), Err(LoadError::TooFewLevels(0)));
    // A different grid, a short field, a different valid time, a fractional level.
    let mut g2 = all.clone();
    g2[5].grid = grid(0x08);
    assert!(matches!(
        build_hour(g2),
        Err(LoadError::Inconsistent("grid"))
    ));
    let mut short = all.clone();
    short[5].values.pop();
    assert!(matches!(
        build_hour(short),
        Err(LoadError::Inconsistent("point count"))
    ));
    let mut late = all.clone();
    late[5].ref_time.hour = 13;
    assert!(matches!(
        build_hour(late),
        Err(LoadError::Inconsistent("valid time"))
    ));
    let mut frac = all.clone();
    frac[5].level.value = 50_000.5;
    assert_eq!(build_hour(frac), Err(LoadError::BadLevel));
    let mut unit = all;
    unit[0].time_unit = 200;
    assert_eq!(build_hour(unit), Err(LoadError::BadTime));
}

// REQ: OA-WIND-008
#[test]
fn level_count_is_bounded() {
    let g = grid(0);
    let npts = (NX * NY) as usize;
    let mut f = Vec::new();
    for k in 0..=MAX_LEVELS {
        for par in [
            Parameter::Ugrd,
            Parameter::Vgrd,
            Parameter::Tmp,
            Parameter::Hgt,
        ] {
            f.push(field(
                g,
                par,
                10_000.0 + k as f32 * 100.0,
                12,
                vec![0.0; npts],
            ));
        }
    }
    assert!(matches!(build_hour(f), Err(LoadError::TooManyLevels(_))));
}

// REQ: OA-WIND-009
#[test]
fn load_hour_refuses_bytes_that_are_not_an_hour() {
    assert!(matches!(
        load_hour(b"not grib at all, just text"),
        Err(LoadError::Decode(_))
    ));
    assert_eq!(load_hour(&[]), Err(LoadError::TooFewLevels(0)));
}

// REQ: OA-WIND-010
#[test]
fn idx_ranges_select_only_the_needed_messages() {
    let idx = "\
1:0:d=2026111400:REFC:entire atmosphere:anl:
2:100:d=2026111400:UGRD:250 mb:anl:
3:200:d=2026111400:VGRD:250 mb:anl:
4:300:d=2026111400:RH:250 mb:anl:
5:400:d=2026111400:TMP:250 mb:anl:
6:500:d=2026111400:HGT:500-1000 mb:anl:
7:600:d=2026111400:HGT:500 mb:anl:
8:700:d=2026111400:UGRD:10 m above ground:anl:
9:800:d=2026111400:UGRD:300 mb:anl:
";
    let r = needed_ranges_text(idx, 1000).unwrap();
    // 250 mb UGRD+VGRD joined; TMP 250 alone (RH between); HGT 500 mb alone; UGRD 300 mb to end.
    assert_eq!(r, vec![(100, 299), (400, 499), (600, 699), (800, 999)]);
    assert!(needed_ranges_text("garbage", 10).is_err());
    assert_eq!(needed_ranges_text("", 10).unwrap(), vec![]);
}

// REQ: OA-WIND-011
#[test]
fn archive_checks_its_files() {
    let hf = |name: &str, t: i64| HourFile {
        path: PathBuf::from(name),
        valid_s: t,
    };
    assert_eq!(
        archive_files(vec![hf("a", 5), hf("b", 5)]).err(),
        Some(LoadError::DuplicateHour(5))
    );
    let a = archive_files(vec![hf("late", 7200), hf("early", 3600)]).unwrap();
    assert_eq!(a.times(), vec![3600, 7200]);
    assert_eq!(a.held(), 0);
    let q = Query {
        lat: 40.0,
        lon: -90.0,
        alt_ft: 30_000.0,
        time_s: 4000.0,
    };
    // No hours.
    let mut empty = new_archive();
    assert_eq!(
        archive_sample(&mut empty, &q),
        Err(QueryError::Refused(Refusal::NoHours))
    );
    // Refusals come before any file is read; a missing file is a typed load error.
    let mut a = a;
    let early = Query { time_s: 10.0, ..q };
    assert!(matches!(
        archive_sample(&mut a, &early),
        Err(QueryError::Refused(Refusal::OutsideTime { .. }))
    ));
    assert!(matches!(
        archive_sample(&mut a, &q),
        Err(QueryError::Load(LoadError::Io { .. }))
    ));
    assert!(matches!(
        index_files(&[PathBuf::from("/nonexistent/rap.grib2")]),
        Err(LoadError::Io { .. })
    ));
    let many: Vec<PathBuf> = (0..=MAX_ARCHIVE_FILES)
        .map(|k| PathBuf::from(format!("{k}")))
        .collect();
    assert_eq!(
        index_files(&many),
        Err(LoadError::TooManyFiles(MAX_ARCHIVE_FILES + 1))
    );
}

// REQ: OA-WIND-012
#[test]
fn statistics_and_observation_conversion() {
    let mut v = vec![5.0, 1.0, 3.0, 2.0, 4.0, f64::NAN];
    let s = summarize(&mut v).unwrap();
    assert_eq!(s.n, 5);
    close(s.median, 3.0, 1e-12);
    close(s.p95, 4.8, 1e-12);
    close(s.mean, 3.0, 1e-12);
    close(s.max, 5.0, 1e-12);
    assert!(summarize(&mut Vec::new()).is_none());
    // A wind from the west (270) blows east; from the north (360) blows south.
    let (u, v) = obs_components(270.0, 10.0);
    close(u, 10.0 * 1852.0 / 3600.0, 1e-9);
    close(v, 0.0, 1e-9);
    let (u, v) = obs_components(360.0, 10.0);
    close(u, 0.0, 1e-9);
    close(v, -10.0 * 1852.0 / 3600.0, 1e-9);
    // Observations against an archive with no hours are counted as refused, not compared.
    let o = ObsWind {
        lat: 40.0,
        lon: -90.0,
        alt_ft: 30_000.0,
        time_s: 1.0,
        wdir_deg: 270.0,
        wspd_kt: 10.0,
    };
    let c = compare_obs(&mut new_archive(), &[o, o]).unwrap();
    assert_eq!(c.n, 0);
    assert_eq!(c.refused, vec![("no-hours", 2)]);
    assert!(c.diff.is_none());
}

// REQ: OA-WIND-012
#[test]
fn comparison_with_a_reference_field_pair() {
    // The reference is a uniform earth-relative wind; the model is the same wind stored
    // grid-relative, so the rotated difference is near zero and the unrotated one is not.
    let (east, north) = (20.0, -5.0);
    let store = |par: Parameter, _i: f64, _j: f64, _lat: f64, lon: f64, _p: f64| {
        let (s, c) = oa_grib2::grid_rotation(&grid(0x08), lon).sin_cos();
        match par {
            Parameter::Ugrd => east * c - north * s,
            Parameter::Vgrd => east * s + north * c,
            Parameter::Tmp => 250.0,
            _ => 1000.0,
        }
    };
    let h = build_hour(fields(0x08, 12, store)).unwrap();
    let rg = grid(0x00);
    let n = (NX * NY) as usize;
    let u = field(rg, Parameter::Ugrd, 50_000.0, 12, vec![east as f32; n]);
    let v = field(rg, Parameter::Vgrd, 50_000.0, 12, vec![north as f32; n]);
    let c = compare_to_fields(&h, &u, &v, 3);
    assert!(c.n > 20);
    assert!(c.diff.unwrap().p95 < 0.05, "{:?}", c.diff);
    assert!(c.diff_unrotated.unwrap().max > 1.0);
}

fn adsb_report(rows: &[(i64, i32, u8, u16, u16, i16)]) -> oa_adsb::Reports {
    use oa_adsb::{reports_push, Reports, ROW_EMPTY};
    let mut r = Reports::default();
    for &(t_s, alt, flags, ws, wd, oat) in rows {
        reports_push(
            &mut r,
            &oa_adsb::Row {
                time_ns: t_s * 1_000_000_000,
                lat_e7: 400_000_000,
                lon_e7: -900_000_000,
                baro_alt_ft: alt,
                flags,
                ws_kt: ws,
                wd_deg: wd,
                oat_c: oat,
                roll_cdeg: -1_500,
                ..ROW_EMPTY
            },
        );
    }
    r
}

// REQ: OA-WIND-015
#[test]
fn adsb_observations_need_altitude_and_a_wind_or_temperature_and_are_thinned() {
    use crate::adsb::*;
    use oa_adsb::{F_GROUND, NONE_I16, NONE_U16};
    let r = adsb_report(&[
        (0, 35_000, 0, 40, 270, NONE_I16),         // wind only: kept
        (10, 35_000, 0, 40, 270, NONE_I16),        // within the gap: thinned
        (40, 35_000, 0, NONE_U16, NONE_U16, -50),  // temperature only: kept
        (80, 35_000, 0, 40, NONE_U16, NONE_I16),   // speed without direction: nothing
        (120, 35_000, F_GROUND, 40, 270, -50),     // on the ground: skipped
        (160, oa_adsb::NONE_I32, 0, 40, 270, -50), // no altitude: skipped
        (200, 12_000, 0, 5, 359, -9),              // both: kept
    ]);
    let mut out = Vec::new();
    assert_eq!(adsb_obs(&r, 30.0, &mut out), 0);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].wind, Some((270.0, 40.0)));
    assert_eq!(out[0].oat_c, None);
    assert_eq!(out[1].oat_c, Some(-50.0));
    assert_eq!(out[1].wind, None);
    assert_eq!(
        (out[2].alt_ft, out[2].wind, out[2].oat_c),
        (12_000.0, Some((359.0, 5.0)), Some(-9.0))
    );
    close(out[2].lat, 40.0, 1e-9);
    close(out[2].lon, -90.0, 1e-9);
    close(out[2].time_s, 200.0, 1e-9);
    close(out[2].roll_deg.unwrap(), -15.0, 1e-9);
    // With no gap every kept row stays.
    let mut all = Vec::new();
    adsb_obs(&r, 0.0, &mut all);
    assert_eq!(all.len(), 4);
}

fn obs_at(
    alt_ft: f64,
    wind: Option<(f64, f64)>,
    oat_c: Option<f64>,
    roll: Option<f64>,
) -> crate::adsb::AdsbObs {
    crate::adsb::AdsbObs {
        lat: 40.0,
        lon: -90.0,
        alt_ft,
        time_s: valid(12) as f64 + 600.0,
        wind,
        oat_c,
        roll_deg: roll,
        track_rate_dps: None,
    }
}

// REQ: OA-WIND-016
#[test]
fn adsb_comparison_groups_by_altitude_and_turning_and_compares_temperature() {
    use crate::adsb::*;
    // RAP: 10 m/s from the west everywhere, 250 K.
    let (a, b) = (const_u_hour(12, 10.0), const_u_hour(13, 10.0));
    let (lat, lon) = ij_to_latlon(&a.grid, 8.0, 6.0).unwrap();
    let kt = |ms: f64| ms * 3600.0 / 1852.0;
    let mut sampler = |q: &Query| {
        let q = Query { lat, lon, ..*q };
        sample_hours(&[&a, &b], &q).map_err(QueryError::Refused)
    };
    // Band edges and turn states.
    assert_eq!(band_of(9_999.0), 0);
    assert_eq!(band_of(10_000.0), 1);
    assert_eq!(band_of(18_000.0), 2);
    assert_eq!(band_of(45_000.0), 3);
    assert_eq!(
        turn_state(&obs_at(1.0, None, None, Some(-10.5))),
        Turn::Turning
    );
    assert_eq!(
        turn_state(&obs_at(1.0, None, None, Some(9.0))),
        Turn::Straight
    );
    assert_eq!(turn_state(&obs_at(1.0, None, None, None)), Turn::Unknown);
    let mut o = obs_at(1.0, None, None, None);
    o.track_rate_dps = Some(0.6);
    assert_eq!(turn_state(&o), Turn::Turning);
    // Exact agreement; a calm readsb wind (RAP 10 m/s too fast); a wind from the south at the
    // same speed (vector difference 10 sqrt 2); temperature equal and 10 K colder in readsb.
    let mut late = obs_at(35_000.0, Some((270.0, kt(10.0))), Some(-23.15), Some(1.0));
    late.time_s = valid(14) as f64 + 5.0; // after the last hour: refused
    let obs = [
        obs_at(35_000.0, Some((270.0, kt(10.0))), Some(-23.15), Some(1.0)),
        obs_at(35_000.0, Some((270.0, 0.0)), Some(-33.15), Some(15.0)),
        obs_at(5_000.0, Some((180.0, kt(10.0))), None, None),
        obs_at(20_000.0, None, Some(-23.15), None),
        late,
    ];
    let c = compare_adsb(&obs, &mut sampler).unwrap();
    assert_eq!((c.offered, c.rows.len()), (5, 4));
    assert_eq!(c.refused, vec![("outside-time", 1)]);
    let g = all_groups(&c);
    assert_eq!(g.len(), 8);
    let all = &g[0];
    let w = all.wind.unwrap();
    assert_eq!(w.n, 3);
    close(w.median, 10.0, 1e-6); // vectors: 0, 10, 10 sqrt 2
    close(w.max, 10.0 * 2f64.sqrt(), 1e-6);
    close(all.speed_bias, (0.0 + 10.0 + 0.0) / 3.0, 1e-6);
    let t = all.temp_abs.unwrap();
    assert_eq!(t.n, 3);
    close(all.temp_bias, 10.0 / 3.0, 1e-9);
    // By band: below 10000 ft one wind, none with temperature; 30000 and above two winds.
    assert_eq!((g[1].wind.unwrap().n, g[1].temp_abs.is_none()), (1, true));
    assert!(g[2].wind.is_none());
    assert_eq!((g[3].wind.is_none(), g[3].temp_abs.unwrap().n), (true, 1));
    assert_eq!((g[4].wind.unwrap().n, g[4].temp_abs.unwrap().n), (2, 2));
    close(g[4].temp_bias, 5.0, 1e-9);
    // By turn: straight (roll 1), turning (roll 15), unknown (the other two).
    assert_eq!(g[5].name, "straight");
    assert_eq!(g[5].wind.unwrap().n, 1);
    close(g[5].wind.unwrap().median, 0.0, 1e-6);
    assert_eq!(g[6].wind.unwrap().n, 1);
    close(g[6].wind.unwrap().median, 10.0, 1e-6);
    assert_eq!(g[7].wind.unwrap().n, 1);
    // A failed file load stops the comparison with its error.
    let e = compare_adsb(&obs, &mut |_| {
        Err(QueryError::Load(LoadError::Io {
            path: "x".into(),
            kind: std::io::ErrorKind::NotFound,
        }))
    });
    assert!(matches!(e, Err(LoadError::Io { .. })));
    // Nothing offered: empty statistics, no panic.
    let empty = compare_adsb(&[], &mut sampler).unwrap();
    assert!(all_groups(&empty)
        .iter()
        .all(|g| g.wind.is_none() && g.temp_abs.is_none()));
}

fn data_dir() -> Option<PathBuf> {
    match std::env::var_os("OPENATCA_DATA") {
        Some(d) if PathBuf::from(&d).join("raw/rap").is_dir() => Some(PathBuf::from(d)),
        _ => {
            println!("OPENATCA_DATA not set or has no raw/rap: real-data test skipped");
            None
        }
    }
}

fn rap_file(root: &std::path::Path, hour: u8) -> PathBuf {
    root.join(format!(
        "raw/rap/rap.20261114.t{hour:02}z.awp130pgrbf00.grib2"
    ))
}

// REQ: OA-WIND-013
#[test]
fn real_rap_hour_matches_oa_grib2_exactly() {
    let Some(root) = data_dir() else { return };
    let path = rap_file(&root, 12);
    let Ok(data) = std::fs::read(&path) else {
        println!("{} missing: skipped", path.display());
        return;
    };
    let h = read_hour_file(&path).unwrap();
    assert_eq!(h.valid_s, valid(12));
    assert_eq!(hour_levels(&h), 37);
    assert_eq!((h.nx, h.ny), (451, 337));
    // 37 levels x 151 987 points x 4 fields x 4 bytes.
    assert_eq!(hour_bytes(&h), 37 * 151_987 * 16);
    assert_eq!(h, load_hour(&data).unwrap());
    let dec = oa_grib2::decode_stream(&data);
    assert!(dec.stopped.is_none() && dec.skipped.is_empty());
    let r = check_grid_points(&h, &dec.fields, 16);
    assert!(r.n > 10_000);
    assert_eq!(r.raw_max, 0.0);
    assert!(r.wind_max < 1e-3 && r.temp_max < 1e-3 && r.height_max < 1e-2);
}

// REQ: OA-WIND-014
#[test]
fn real_rap_agrees_with_hrrr_and_the_cache_is_bounded() {
    let Some(root) = data_dir() else { return };
    let path = rap_file(&root, 12);
    let hrrr = root.join("raw/hrrr/hrrr.20261114.t12z.wrfprsf00.UV250.grib2");
    let (Ok(_), Ok(hd)) = (std::fs::metadata(&path), std::fs::read(&hrrr)) else {
        println!("RAP t12z or HRRR sample missing: skipped");
        return;
    };
    let h = read_hour_file(&path).unwrap();
    let hf = oa_grib2::decode_stream(&hd);
    let u = hf
        .fields
        .iter()
        .find(|f| f.parameter == Parameter::Ugrd)
        .unwrap();
    let v = hf
        .fields
        .iter()
        .find(|f| f.parameter == Parameter::Vgrd)
        .unwrap();
    let c = compare_to_fields(&h, u, v, 20);
    let d = c.diff.unwrap();
    println!(
        "RAP vs HRRR 250 hPa: N {} median {:.2} p95 {:.2}",
        d.n, d.median, d.p95
    );
    assert!(d.n > 1000);
    // The lane-grib2 measure was a median of 1.18 m/s; the bound leaves room and still fails
    // without the rotation (about 3.4 m/s).
    assert!(d.median < 2.0 && d.p95 < 6.0);
    assert!(c.diff_unrotated.unwrap().median > d.median);

    // Sweep every hour: the cache never holds more than MAX_HOURS_HELD.
    let mut files = Vec::new();
    for hr in 0..=16u8 {
        let p = rap_file(&root, hr);
        if p.exists() {
            files.push(p);
        }
    }
    if files.len() < 4 {
        return;
    }
    let mut a = archive_files(index_files(&files).unwrap()).unwrap();
    let (lat, lon) = (41.9, -87.9);
    let t0 = a.times()[0];
    let tn = *a.times().last().unwrap();
    let mut t = t0 as f64;
    while t <= tn as f64 {
        let s = archive_sample(
            &mut a,
            &Query {
                lat,
                lon,
                alt_ft: 35_000.0,
                time_s: t,
            },
        )
        .unwrap();
        assert!(a.held() <= MAX_HOURS_HELD);
        assert!(s.temp_k > 200.0 && s.temp_k < 250.0);
        assert!(s.u_ms.hypot(s.v_ms) < 120.0);
        t += 1800.0;
    }
    assert_eq!(a.held(), MAX_HOURS_HELD.min(a.held()));
    // Outside the span is refused, not extrapolated.
    let late = Query {
        lat,
        lon,
        alt_ft: 35_000.0,
        time_s: tn as f64 + 1.0,
    };
    assert!(matches!(
        archive_sample(&mut a, &late),
        Err(QueryError::Refused(Refusal::OutsideTime { .. }))
    ));
}

// REQ: OA-WIND-050
#[test]
fn an_infinite_cell_is_refused_at_build_and_never_answered() {
    // build_hour refuses an infinite cell (NaN stays: it is how a file marks a missing value).
    let mut fs = fields(0, 12, linear);
    fs[0].values[7] = f32::INFINITY;
    assert!(build_hour(fs).is_err());
    let mut fs = fields(0, 12, linear);
    fs[1].values[7] = f32::NEG_INFINITY;
    assert!(build_hour(fs).is_err());
    let mut fs = fields(0, 12, linear);
    fs[0].values[7] = f32::NAN;
    assert!(build_hour(fs).is_ok());
    // An hour that reached the sampler with an infinite cell anyway never gives an Ok answer:
    // 0 * inf would be NaN and inf * 0.5 is inf.
    let mut h = linear_hour(12);
    let npts = (NX * NY) as usize;
    let k = 3 * npts + 8 * NX as usize + 8; // level 3 (500 hPa), cell (8, 8)
    h.u[k] = f32::INFINITY;
    h.t[k] = f32::INFINITY;
    let (lat, lon) = ij_to_latlon(&h.grid, 8.4, 8.4).unwrap();
    let r = sample_hour_pressure(&h, lat, lon, 500.0);
    assert!(r.is_err(), "{r:?}");
    let q = Query {
        lat,
        lon,
        alt_ft: 18_289.0,
        time_s: valid(12) as f64,
    };
    assert!(sample_hours(&[&h], &q).is_err());
    // Away from the bad cell the same hour still answers, with finite numbers.
    let (lat, lon) = ij_to_latlon(&h.grid, 2.5, 2.5).unwrap();
    let pt = sample_hour_pressure(&h, lat, lon, 500.0).unwrap();
    assert!([pt.u, pt.v, pt.t, pt.z].iter().all(|x| x.is_finite()));
}

// REQ: OA-WIND-051
#[test]
fn bracket_refuses_duplicate_and_unsorted_times() {
    assert!(bracket(&[1000, 1000], 1000.0).is_err());
    assert!(bracket(&[100, 200, 200, 300], 250.0).is_err());
    assert!(bracket(&[200, 100], 150.0).is_err());
    assert!(bracket(&[100, 300, 200], 150.0).is_err());
    assert_eq!(bracket(&[100, 200, 400], 300.0).unwrap(), (1, 2, 0.5));
}

/// `linear_hour` with the given U points blanked (NaN) on the level at `level_pa`.
fn blanked(hour: u8, level_pa: f32, points: &[(u32, u32)]) -> Hour {
    let mut f = fields(0, hour, linear);
    for fld in &mut f {
        if fld.parameter == Parameter::Ugrd && fld.level.value == level_pa {
            for &(i, j) in points {
                if let Some(v) = fld.values.get_mut((j * NX + i) as usize) {
                    *v = f32::NAN;
                }
            }
        }
    }
    build_hour(f).unwrap()
}

// REQ: OA-WIND-052
#[test]
fn zero_weight_neighbours_do_not_refuse_a_query() {
    let full = linear_hour(12);
    // Exact grid node: the three other corners carry no weight.
    let (la, lo) = ij_to_latlon(&full.grid, 8.0, 6.0).unwrap();
    let want = sample_hour_pressure(&full, la, lo, 500.0).unwrap();
    let h = blanked(12, 50_000.0, &[(9, 6), (8, 7), (9, 7)]);
    assert_eq!(sample_hour_pressure(&h, la, lo, 500.0).unwrap(), want);
    // Exact last column and row: the inner corners carry no weight.
    let (la, lo) = ij_to_latlon(&full.grid, f64::from(NX - 1), f64::from(NY - 1)).unwrap();
    let want = sample_hour_pressure(&full, la, lo, 500.0).unwrap();
    let h = blanked(
        12,
        50_000.0,
        &[(NX - 2, NY - 2), (NX - 1, NY - 2), (NX - 2, NY - 1)],
    );
    assert_eq!(sample_hour_pressure(&h, la, lo, 500.0).unwrap(), want);
    // Exact bottom level: the level above it carries no weight.
    let (la, lo) = ij_to_latlon(&full.grid, 8.2, 6.2).unwrap();
    let want = sample_hour_pressure(&full, la, lo, 1000.0).unwrap();
    let all: Vec<(u32, u32)> = (0..NX).flat_map(|i| (0..NY).map(move |j| (i, j))).collect();
    let h = blanked(12, 85_000.0, &all);
    assert_eq!(sample_hour_pressure(&h, la, lo, 1000.0).unwrap(), want);
    // A weighted input that is missing is still refused.
    let h = blanked(12, 50_000.0, &[(8, 6)]);
    let (la, lo) = ij_to_latlon(&h.grid, 8.0, 6.0).unwrap();
    assert_eq!(
        sample_hour_pressure(&h, la, lo, 500.0),
        Err(Refusal::MissingValue)
    );
    let (la, lo) = ij_to_latlon(&h.grid, 8.3, 6.0).unwrap();
    assert_eq!(
        sample_hour_pressure(&h, la, lo, 500.0),
        Err(Refusal::MissingValue)
    );
}

// REQ: OA-WIND-052
#[test]
fn an_exact_hour_does_not_need_the_other_bracketing_hour() {
    let all: Vec<(u32, u32)> = (0..NX).flat_map(|i| (0..NY).map(move |j| (i, j))).collect();
    let a = blanked(12, 50_000.0, &all);
    let b = linear_hour(13);
    let (la, lo) = ij_to_latlon(&b.grid, 8.2, 6.2).unwrap();
    let want = sample_hour_pressure(&b, la, lo, 500.0).unwrap();
    let q = |t: i64| Query {
        lat: la,
        lon: lo,
        alt_ft: altitude_ft_from_pressure_hpa(500.0).unwrap(),
        time_s: t as f64,
    };
    // Exactly the later hour: the earlier one has missing values and carries no weight.
    let s = sample_hours(&[&a, &b], &q(valid(13))).unwrap();
    assert_eq!((s.u_ms, s.v_ms, s.temp_k), (want.u, want.v, want.t));
    // Between them it is used and refused.
    assert_eq!(
        sample_hours(&[&a, &b], &q(valid(13) - 1800)),
        Err(Refusal::MissingValue)
    );
}

// REQ: OA-WIND-052
#[test]
fn an_exact_hour_does_not_read_the_unused_archive_file() {
    let hf = |name: &str, h: u8| HourFile {
        path: PathBuf::from(name),
        valid_s: valid(h),
    };
    let new_a = || archive_files(vec![hf("a", 12), hf("b", 13), hf("c", 14)]).unwrap();
    let q = |t: i64| Query {
        lat: ij_to_latlon(&linear_hour(12).grid, 8.2, 6.2).unwrap().0,
        lon: ij_to_latlon(&linear_hour(12).grid, 8.2, 6.2).unwrap().1,
        alt_ft: 18_000.0,
        time_s: t as f64,
    };
    let mut reads = Vec::new();
    let mut load = |p: &std::path::Path| -> Result<Hour, LoadError> {
        let name = p.display().to_string();
        reads.push(name.clone());
        match name.as_str() {
            "a" => Ok(linear_hour(12)),
            "c" => Ok(linear_hour(14)),
            _ => Err(LoadError::Io {
                path: name,
                kind: std::io::ErrorKind::NotFound,
            }),
        }
    };
    // Exactly the first hour: the next file (unreadable) carries no weight.
    let mut a = new_a();
    let s = archive_sample_with(&mut a, &q(valid(12)), &mut load).unwrap();
    assert_eq!(
        (s.hour0_s, s.hour1_s, s.time_weight),
        (valid(12), valid(13), 0.0)
    );
    // Exactly the last hour: the earlier file (unreadable) carries no weight.
    let mut a = new_a();
    let s = archive_sample_with(&mut a, &q(valid(14)), &mut load).unwrap();
    assert_eq!(
        (s.hour0_s, s.hour1_s, s.time_weight),
        (valid(13), valid(14), 1.0)
    );
    // Between the first and second hours the unreadable file is needed.
    let mut a = new_a();
    assert!(matches!(
        archive_sample_with(&mut a, &q(valid(12) + 900), &mut load),
        Err(QueryError::Load(LoadError::Io { .. }))
    ));
    assert_eq!(reads, ["a", "c", "a", "b"]);
}

// REQ: OA-WIND-053
#[test]
fn whole_file_reads_stop_at_their_limit() {
    let dir = std::env::temp_dir().join(format!("oa_wind_cap_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("f.idx");
    std::fs::write(&p, [b'x'; 100]).unwrap();
    assert_eq!(read_file_capped(&p, 100).unwrap().len(), 100);
    let big = read_file_capped(&p, 99);
    assert_eq!(big.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    assert!(read_file_capped(&dir.join("missing"), 100).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// REQ: OA-WIND-059
#[test]
fn a_percentile_rank_that_is_not_a_number_is_no_value_not_the_first_element() {
    let v = [1.0, 2.0, 3.0];
    assert_eq!(percentile(&v, f64::NAN), None);
    assert_eq!(percentile(&v, 0.5), Some(2.0));
    assert_eq!(percentile(&v, 2.0), Some(3.0));
}
