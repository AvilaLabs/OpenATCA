#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::bits::{sign_mag, BitReader};
use crate::grid::{lambert_setup, LambertInput};
use crate::pack::BitmapSpec;
use crate::testenc::*;
use crate::*;

fn expected(x: i64, r: f32, e: i64, d: i64) -> f32 {
    ((f64::from(r) + x as f64 * 2f64.powi(e as i32)) / 10f64.powi(d as i32)) as f32
}

/// A small Lambert grid with the HRRR projection and a 4 x 3 size.
fn small_grid() -> Vec<u8> {
    sect3_lambert(&hrrr_spec(4, 3))
}

fn temp_product(level_hpa: u32) -> Vec<u8> {
    sect4(0, 0, 0, 100, 0, level_hpa * 100)
}

fn lambert_field(values: Vec<f32>, flags: u8) -> Field {
    let mut spec = hrrr_spec(4, 3);
    spec.flags = flags;
    let g = parse_grid(&sect3_lambert(&spec)).unwrap();
    Field {
        discipline: 0,
        category: 2,
        number: 2,
        parameter: Parameter::Ugrd,
        level: Level {
            kind: 100,
            value: 25_000.0,
        },
        ref_time: RefTime {
            year: 2026,
            month: 11,
            day: 14,
            hour: 18,
            minute: 0,
            second: 0,
        },
        time_unit: 1,
        forecast_time: 0,
        product_template: 0,
        interval: None,
        packing_template: 0,
        grid: g,
        missing_count: values.iter().filter(|v| v.is_nan()).count() as u32,
        values,
    }
}

// REQ: OA-GRIB-001
#[test]
fn bit_reader_and_sign_magnitude() {
    let mut w = BitWriter::new();
    let vals: Vec<(u64, u32)> = (0..200u64)
        .map(|k| {
            (
                (k * 2_654_435_761) & ((1u64 << (k as u32 % 33)) - 1),
                k as u32 % 33,
            )
        })
        .collect();
    for &(v, n) in &vals {
        w.put(v, n);
    }
    let mut r = BitReader::new(&w.bytes);
    for &(v, n) in &vals {
        assert_eq!(u64::from(r.read(n).unwrap()), v, "width {n}");
    }
    assert_eq!(sign_mag(&[0x80, 0x05], 0, 2), Some(-5));
    assert_eq!(sign_mag(&[0x00, 0x05], 0, 2), Some(5));
    assert_eq!(sign_mag(&[0x80, 0x00], 0, 2), Some(0));
    assert!(BitReader::new(&[0xff]).read(9).is_none());
}

// REQ: OA-GRIB-001, OA-GRIB-007
#[test]
fn simple_packing_round_trips_every_width() {
    for nbits in 0..=32u32 {
        let top = if nbits == 0 { 0 } else { (1u64 << nbits) - 1 };
        let x: Vec<u64> = vec![
            0,
            top,
            top / 2,
            1.min(top),
            top / 3,
            top,
            0,
            1.min(top),
            top,
            top / 5,
            2.min(top),
            top,
        ];
        let p = encode_simple(&x, 100.0, -3, 2);
        let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
        let f = decode_message(&m).unwrap();
        assert_eq!(f.len(), 1);
        for (k, &v) in x.iter().enumerate() {
            assert_eq!(
                f[0].values[k],
                expected(v as i64, 100.0, -3, 2),
                "nbits {nbits} k {k}"
            );
        }
        assert_eq!(f[0].packing_template, 0);
    }
}

// REQ: OA-GRIB-007
#[test]
fn simple_packing_scaling_and_constant_field() {
    let x: Vec<u64> = (0..12).collect();
    let p = encode_simple(&x, -5.0, 1, 1);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let f = decode_message(&m).unwrap();
    for (k, v) in f[0].values.iter().enumerate() {
        assert_eq!(*v, expected(k as i64, -5.0, 1, 1));
    }
    // nbits = 0: every value is R / 10^D.
    let p = encode_simple(&[0; 12], 273.15, 0, 0);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let f = decode_message(&m).unwrap();
    assert!(f[0].values.iter().all(|&v| v == 273.15));
}

fn ramp(n: usize, f: impl Fn(usize) -> i64) -> Vec<Option<i64>> {
    (0..n).map(|k| Some(f(k))).collect()
}

fn complex_msg(
    x: &[Option<i64>],
    o: &ComplexOpts,
    r: f32,
    e: i64,
    d: i64,
) -> Result<Vec<Field>, Grib2Error> {
    let p = encode_complex(x, r, e, d, o);
    let bm = sect6(None, false);
    let m = simple_message(small_grid(), temp_product(500), &p, bm);
    decode_message(&m)
}

// REQ: OA-GRIB-008
#[test]
fn complex_packing_groups_widths_and_lengths() {
    let x = ramp(12, |k| {
        [5, 5, 5, 9, 1000, 1003, 1001, 1002, 0, 70000, 3, 80000][k]
    });
    let o = ComplexOpts {
        group_lens: vec![3, 1, 4, 2, 2],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 0,
    };
    let f = complex_msg(&x, &o, 2.5, -1, 1).unwrap();
    assert_eq!(f[0].packing_template, 2);
    for (k, v) in x.iter().enumerate() {
        assert_eq!(f[0].values[k], expected(v.unwrap(), 2.5, -1, 1));
    }
    // One group, all groups equal length, single-value groups.
    for lens in [vec![12], vec![4, 4, 4], vec![1; 12]] {
        let o = ComplexOpts {
            group_lens: lens,
            order: 0,
            missing_mgmt: 0,
            extra_octets: 0,
        };
        let f = complex_msg(&x, &o, 0.0, 0, 0).unwrap();
        for (k, v) in x.iter().enumerate() {
            assert_eq!(f[0].values[k], v.unwrap() as f32);
        }
    }
}

// REQ: OA-GRIB-008
#[test]
fn complex_packing_missing_value_management() {
    let x: Vec<Option<i64>> = vec![
        Some(3),
        None,
        Some(4),
        Some(5),
        None,
        None,
        None,
        Some(7),
        Some(7),
        Some(7),
        Some(100),
        None,
    ];
    for mgmt in [1u8, 2] {
        let o = ComplexOpts {
            group_lens: vec![3, 4, 3, 2],
            order: 0,
            missing_mgmt: mgmt,
            extra_octets: 0,
        };
        let f = complex_msg(&x, &o, 0.0, 0, 0).unwrap();
        assert_eq!(f[0].missing_count, 5, "mgmt {mgmt}");
        for (k, v) in x.iter().enumerate() {
            match v {
                Some(v) => assert_eq!(f[0].values[k], *v as f32),
                None => assert!(f[0].values[k].is_nan()),
            }
        }
    }
}

// REQ: OA-GRIB-009
#[test]
fn spatial_differencing_orders_one_and_two() {
    let smooth = ramp(12, |k| 1000 + (k as i64) * 7 + (k as i64 * k as i64) / 2);
    let rough = ramp(12, |k| [90, 4, 800, 2, 55, 12000, 3, 3, 3, 40, 1, 999][k]);
    for order in [1u8, 2] {
        for (name, x) in [("smooth", &smooth), ("rough", &rough)] {
            for octets in [2usize, 4] {
                let o = ComplexOpts {
                    group_lens: vec![4, 4, 4],
                    order,
                    missing_mgmt: 0,
                    extra_octets: octets,
                };
                let f = complex_msg(x, &o, 10.0, -2, 1).unwrap();
                assert_eq!(f[0].packing_template, 3);
                for (k, v) in x.iter().enumerate() {
                    assert_eq!(
                        f[0].values[k],
                        expected(v.unwrap(), 10.0, -2, 1),
                        "{name} order {order} k {k}"
                    );
                }
            }
        }
    }
}

// REQ: OA-GRIB-009
#[test]
fn spatial_differencing_with_missing_points_skips_them() {
    let x: Vec<Option<i64>> = vec![
        Some(500),
        Some(510),
        None,
        Some(530),
        Some(525),
        None,
        Some(560),
        Some(561),
        Some(570),
        None,
        Some(571),
        Some(573),
    ];
    for order in [1u8, 2] {
        let o = ComplexOpts {
            group_lens: vec![6, 6],
            order,
            missing_mgmt: 1,
            extra_octets: 2,
        };
        let f = complex_msg(&x, &o, 0.0, 0, 0).unwrap();
        for (k, v) in x.iter().enumerate() {
            match v {
                Some(v) => assert_eq!(f[0].values[k], *v as f32, "order {order} k {k}"),
                None => assert!(f[0].values[k].is_nan()),
            }
        }
    }
}

// REQ: OA-GRIB-010
#[test]
fn bitmap_present_absent_and_previous() {
    let x: Vec<u64> = vec![10, 20, 30, 40, 50];
    let bits = [
        true, false, true, true, false, false, true, false, true, false, false, false,
    ];
    let p = encode_simple(&x, 0.0, 0, 0);
    let m = simple_message(
        small_grid(),
        temp_product(500),
        &p,
        sect6(Some(&bits), false),
    );
    let f = decode_message(&m).unwrap();
    assert_eq!(f[0].missing_count, 7);
    let mut it = x.iter();
    for (k, &b) in bits.iter().enumerate() {
        if b {
            assert_eq!(f[0].values[k], *it.next().unwrap() as f32);
        } else {
            assert!(f[0].values[k].is_nan());
        }
    }
    // Two fields; the second uses indicator 254 and omits section 3.
    let p2 = encode_simple(&[1, 2, 3, 4, 5], 0.0, 0, 0);
    let m = message(
        0,
        &[
            sect1(2026, 11, 14, 18),
            small_grid(),
            temp_product(500),
            p.sect5.clone(),
            sect6(Some(&bits), false),
            p.sect7.clone(),
            temp_product(250),
            p2.sect5.clone(),
            sect6(None, true),
            p2.sect7.clone(),
        ],
    );
    let f = decode_message(&m).unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[1].level.value, 25_000.0);
    assert_eq!(f[1].values[0], 1.0);
    assert!(f[1].values[1].is_nan());
    assert_eq!(f[1].missing_count, 7);
    // 254 with nothing before it is refused; a bitmap whose set bits differ from the data too.
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, true));
    assert!(matches!(decode_message(&m), Err(Grib2Error::BadBitmap(_))));
    let few = [
        true, false, false, false, false, false, false, false, false, false, false, false,
    ];
    let m = simple_message(
        small_grid(),
        temp_product(500),
        &p,
        sect6(Some(&few), false),
    );
    assert!(matches!(decode_message(&m), Err(Grib2Error::BadBitmap(_))));
    // Predefined bitmap indicator.
    let spec = crate::pack::parse_bitmap(&sect6(Some(&bits), false), 12).unwrap();
    assert!(matches!(spec, BitmapSpec::Present(_)));
}

// REQ: OA-GRIB-001
#[test]
fn files_with_several_messages_and_fields() {
    let a = simple_message(
        small_grid(),
        temp_product(500),
        &encode_simple(&[1; 12], 0.0, 0, 0),
        sect6(None, false),
    );
    let p = encode_simple(&(0..12).collect::<Vec<u64>>(), 0.0, 0, 0);
    let b = simple_message(
        small_grid(),
        sect4(0, 2, 2, 100, 0, 25_000),
        &p,
        sect6(None, false),
    );
    // Multi-field message: second field repeats section 3.
    let c = message(
        0,
        &[
            sect1(2026, 11, 14, 18),
            small_grid(),
            sect4(0, 2, 3, 100, 0, 25_000),
            p.sect5.clone(),
            sect6(None, false),
            p.sect7.clone(),
            small_grid(),
            sect4(0, 3, 5, 103, 0, 10),
            p.sect5.clone(),
            sect6(None, false),
            p.sect7.clone(),
        ],
    );
    let mut file = a.clone();
    file.extend(&b);
    file.extend(&c);
    let spans = scan_messages(&file).unwrap();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[1].offset, a.len());
    let d = decode_stream(&file);
    assert_eq!(d.messages, 3);
    assert!(d.skipped.is_empty());
    let params: Vec<Parameter> = d.fields.iter().map(|f| f.parameter).collect();
    assert_eq!(
        params,
        vec![
            Parameter::Tmp,
            Parameter::Ugrd,
            Parameter::Vgrd,
            Parameter::Hgt
        ]
    );
    assert_eq!(
        d.fields[3].level,
        Level {
            kind: 103,
            value: 10.0
        }
    );
    assert_eq!(d.fields[0].ref_time.hour, 18);
}

// REQ: OA-GRIB-002
#[test]
fn framing_errors_are_typed() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let good = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    assert!(decode_message(&good).is_ok());
    // Truncated anywhere: never a panic, always an error.
    for cut in 0..good.len() {
        let d = decode_stream(&good[..cut]);
        assert!(d.stopped.is_some() || cut == 0, "cut {cut}");
        assert!(d.fields.is_empty());
    }
    let mut bad = good.clone();
    let n = bad.len();
    bad[n - 1] = b'8';
    assert_eq!(decode_message(&bad), Err(Grib2Error::BadEndMarker));
    let mut bad = good.clone();
    bad[7] = 1;
    assert_eq!(
        message_length(&bad, 0),
        Err(Grib2Error::UnsupportedEdition(1))
    );
    let mut bad = good.clone();
    bad[0] = b'X';
    assert_eq!(
        message_length(&bad, 0),
        Err(Grib2Error::NotGrib { offset: 0 })
    );
    // Absurd length.
    let mut bad = good.clone();
    bad[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(matches!(
        message_length(&bad, 0),
        Err(Grib2Error::BadMessageLength(_))
    ));
    let mut bad = good.clone();
    bad[8..16].copy_from_slice(&((MAX_MESSAGE_BYTES + 1).to_be_bytes()));
    assert!(matches!(
        message_length(&bad, 0),
        Err(Grib2Error::BadMessageLength(_))
    ));
    // Garbage after the last message.
    let mut bad = good.clone();
    bad.extend(b"junk");
    let d = decode_stream(&bad);
    assert_eq!(d.fields.len(), 1);
    assert_eq!(
        d.stopped.map(|s| s.error),
        Some(Grib2Error::Truncated("indicator section"))
    );
    // Section order: data before grid.
    let m = message(0, &[sect1(2026, 11, 14, 18), p.sect7.clone()]);
    assert_eq!(
        decode_message(&m),
        Err(Grib2Error::SectionOrder { section: 7 })
    );
    let m = message(0, &[sect1(2026, 11, 14, 18), temp_product(500)]);
    assert_eq!(
        decode_message(&m),
        Err(Grib2Error::MissingSection("grid definition"))
    );
    // Section length running past the message.
    let mut bad = good.clone();
    bad[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(matches!(
        decode_message(&bad),
        Err(Grib2Error::BadSectionLength { .. })
    ));
}

// REQ: OA-GRIB-002
#[test]
fn point_and_field_limits_refuse() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    // Grid larger than the limit.
    let mut spec = hrrr_spec(5000, 5000);
    spec.nx = 5000;
    let m = simple_message(
        sect3_lambert(&spec),
        temp_product(500),
        &p,
        sect6(None, false),
    );
    assert!(matches!(decode_message(&m), Err(Grib2Error::GridPoints(_))));
    // Count disagreeing with the dimensions.
    let mut g = small_grid();
    g[6..10].copy_from_slice(&13u32.to_be_bytes());
    let m = simple_message(g, temp_product(500), &p, sect6(None, false));
    assert!(matches!(decode_message(&m), Err(Grib2Error::BadGrid(_))));
    // Too many fields.
    let mut secs = vec![sect1(2026, 11, 14, 18), small_grid()];
    for _ in 0..=MAX_FIELDS_PER_MESSAGE {
        secs.extend([
            temp_product(500),
            p.sect5.clone(),
            sect6(None, false),
            p.sect7.clone(),
        ]);
    }
    assert_eq!(
        decode_message(&message(0, &secs)),
        Err(Grib2Error::TooManyFields)
    );
    // Value count differing from the grid.
    let short = encode_simple(&[1; 5], 0.0, 0, 0);
    let m = simple_message(small_grid(), temp_product(500), &short, sect6(None, false));
    assert!(matches!(decode_message(&m), Err(Grib2Error::BadPacking(_))));
    // Group lengths that do not add up.
    let x = ramp(12, |k| k as i64);
    let o = ComplexOpts {
        group_lens: vec![6, 6],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 0,
    };
    let mut c = encode_complex(&x, 0.0, 0, 0, &o);
    let n = c.sect5.len();
    c.sect5[n - 5..n - 1].copy_from_slice(&7u32.to_be_bytes()); // last group length 7 of 6
    let m = simple_message(small_grid(), temp_product(500), &c, sect6(None, false));
    assert!(matches!(decode_message(&m), Err(Grib2Error::BadPacking(_))));
}

// REQ: OA-GRIB-011
#[test]
fn png_and_other_templates_are_refused_by_number_and_the_file_continues() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let ok = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    for tmpl in [41u16, 1, 50] {
        let mut q = p.clone();
        q.sect5[9..11].copy_from_slice(&tmpl.to_be_bytes());
        let bad = simple_message(small_grid(), temp_product(500), &q, sect6(None, false));
        assert_eq!(
            decode_message(&bad),
            Err(Grib2Error::UnsupportedPacking(tmpl))
        );
        let mut file = bad.clone();
        file.extend(&ok);
        let d = decode_stream(&file);
        assert_eq!(d.messages, 2);
        assert_eq!(d.fields.len(), 1);
        assert_eq!(d.skipped.len(), 1);
        assert_eq!(d.skipped[0].offset, 0);
        assert_eq!(d.skipped[0].error, Grib2Error::UnsupportedPacking(tmpl));
    }
}

/// Frozen codestreams of oa-j2k's tests: 4 x 3 samples 10, 20 .. 120 (8 bits, 1 level), and
/// 3 x 2 samples 200, 3, 150, 0, 255, 77.
const J2K_A: [u8; 109] = [
    0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x03,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x03,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x01, 0xFF, 0x52, 0x00,
    0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0xFF, 0x5C, 0x00, 0x07, 0x40,
    0x40, 0x48, 0x48, 0x50, 0xFF, 0x64, 0x00, 0x08, 0x00, 0x01, 0x74, 0x65, 0x73, 0x74, 0xFF, 0x90,
    0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1D, 0x00, 0x01, 0xFF, 0x93, 0xCF, 0xB4, 0x14, 0x06,
    0x48, 0x82, 0x44, 0x99, 0xC0, 0xF9, 0x01, 0x80, 0x0C, 0xFF, 0x5F, 0xFF, 0xD9,
];
const J2K_B: [u8; 115] = [
    0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x01, 0xFF, 0x52, 0x00,
    0x0C, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x04, 0x04, 0x00, 0x01, 0xFF, 0x5C, 0x00, 0x07, 0x40,
    0x40, 0x48, 0x48, 0x50, 0xFF, 0x64, 0x00, 0x08, 0x00, 0x01, 0x74, 0x65, 0x73, 0x74, 0xFF, 0x90,
    0x00, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x01, 0xFF, 0x93, 0xC3, 0xE7, 0x04, 0x07,
    0x1A, 0xC1, 0xF3, 0x82, 0x8F, 0xB4, 0x0E, 0x7E, 0x60, 0x40, 0x03, 0x1F, 0x0B, 0x11, 0x95, 0x01,
    0xF7, 0xFF, 0xD9,
];

// REQ: OA-GRIB-023
#[test]
fn template_5_40_decodes_through_oa_j2k_with_scaling_and_bitmap() {
    // Packed X = 10, 20 .. 120; value = (100 + X * 2^1) / 10^1.
    let p = encode_jpeg2000(&J2K_A, 12, 100.0, 1, 1, 8, 0);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let f = &decode_message(&m).unwrap()[0];
    assert_eq!(f.packing_template, 40);
    for (k, v) in f.values.iter().enumerate() {
        assert_eq!(*v, expected(10 * (k as i64 + 1), 100.0, 1, 1), "point {k}");
    }
    // The integers before scaling.
    let rp = pack::parse_repr(&p.sect5).unwrap();
    assert_eq!(rp.template, 40);
    assert_eq!(
        pack::unpack_ints(&rp, &p.sect7[5..]).unwrap(),
        [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120]
    );
    // Six values spread over twelve points by a bitmap.
    let bm: Vec<bool> = (0..12).map(|k| k % 2 == 1).collect();
    let p = encode_jpeg2000(&J2K_B, 6, 0.0, 0, 0, 8, 0);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(Some(&bm), false));
    let f = &decode_message(&m).unwrap()[0];
    assert_eq!(f.missing_count, 6);
    let got: Vec<f32> = f.values.iter().copied().filter(|v| !v.is_nan()).collect();
    assert_eq!(got, [200.0, 3.0, 150.0, 0.0, 255.0, 77.0]);
    assert!(f.values[0].is_nan() && f.values[1] == 200.0);
    // A field of zero bits per value is constant and carries no codestream.
    let p = encode_jpeg2000(&[], 12, 273.5, 0, 0, 0, 0);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let f = &decode_message(&m).unwrap()[0];
    assert!(f.values.iter().all(|&v| v == 273.5));
}

// REQ: OA-GRIB-024
#[test]
fn template_5_40_refuses_inconsistent_sections_and_damaged_codestreams() {
    let run = |p: &Packed| {
        decode_message(&simple_message(
            small_grid(),
            temp_product(500),
            p,
            sect6(None, false),
        ))
    };
    // Lossy compression type.
    let r = run(&encode_jpeg2000(&J2K_A, 12, 0.0, 0, 0, 8, 1));
    assert!(matches!(r, Err(Grib2Error::BadPacking(_))), "{r:?}");
    // Bit depth, then point count, that disagree with the codestream.
    let r = run(&encode_jpeg2000(&J2K_A, 12, 0.0, 0, 0, 7, 0));
    assert!(matches!(r, Err(Grib2Error::BadPacking(_))), "{r:?}");
    let r = run(&encode_jpeg2000(&J2K_A, 6, 0.0, 0, 0, 8, 0));
    assert!(
        matches!(
            r,
            Err(Grib2Error::BadPacking(_) | Grib2Error::GridPoints(_))
        ),
        "{r:?}"
    );
    // A damaged codestream is a typed JPEG 2000 error.
    let mut bad = J2K_A;
    bad[0] = 0;
    let r = run(&encode_jpeg2000(&bad, 12, 0.0, 0, 0, 8, 0));
    assert_eq!(
        r,
        Err(Grib2Error::Jpeg2000(oa_j2k::J2kError::NotCodestream))
    );
    let r = run(&encode_jpeg2000(&J2K_A[..60], 12, 0.0, 0, 0, 8, 0));
    assert!(matches!(r, Err(Grib2Error::Jpeg2000(_))), "{r:?}");
    // The section must be 23 octets.
    let mut p = encode_jpeg2000(&J2K_A, 12, 0.0, 0, 0, 8, 0);
    p.sect5.truncate(p.sect5.len() - 2);
    p.sect5[3] -= 2;
    assert!(matches!(run(&p), Err(Grib2Error::BadSectionLength { .. })));
    // One field refused does not stop the next message of a file.
    let ok = simple_message(
        small_grid(),
        temp_product(500),
        &encode_simple(&[1; 12], 0.0, 0, 0),
        sect6(None, false),
    );
    let mut file = simple_message(
        small_grid(),
        temp_product(500),
        &encode_jpeg2000(&bad, 12, 0.0, 0, 0, 8, 0),
        sect6(None, false),
    );
    file.extend(&ok);
    let d = decode_stream(&file);
    assert_eq!((d.fields.len(), d.skipped.len()), (1, 1));
}

// REQ: OA-GRIB-020
#[test]
fn inspect_reads_the_inventory_without_unpacking() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let mut q = p.clone();
    q.sect5[9..11].copy_from_slice(&41u16.to_be_bytes());
    let jp2 = simple_message(
        small_grid(),
        sect4(0, 2, 2, 100, 0, 25_000),
        &q,
        sect6(None, false),
    );
    let ok = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let mut file = jp2.clone();
    file.extend(&ok);
    let i = inspect_stream(&file);
    assert_eq!((i.messages, i.fields.len(), i.skipped.len()), (2, 2, 0));
    assert!(i.stopped.is_none());
    assert_eq!(
        (i.fields[0].parameter, i.fields[0].packing_template),
        (Parameter::Ugrd, 41)
    );
    assert_eq!(
        (i.fields[1].parameter, i.fields[1].packing_template),
        (Parameter::Tmp, 0)
    );
    assert_eq!(
        i.fields[0].level,
        Level {
            kind: LEVEL_ISOBARIC,
            value: 25_000.0
        }
    );
    // Framing is still checked.
    file.truncate(file.len() - 3);
    assert!(inspect_stream(&file).stopped.is_some());
}

// REQ: OA-GRIB-006
#[test]
fn product_templates_parameters_and_levels() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let cases = [
        (0u8, 0u8, Parameter::Tmp),
        (1, 1, Parameter::Rh),
        (2, 2, Parameter::Ugrd),
        (2, 3, Parameter::Vgrd),
        (2, 8, Parameter::Vvel),
        (3, 5, Parameter::Hgt),
        (3, 0, Parameter::Pres),
        (7, 7, Parameter::Other),
    ];
    for (cat, num, want) in cases {
        for template in [0u16, 8] {
            let m = simple_message(
                small_grid(),
                sect4(template, cat, num, 100, 0, 85_000),
                &p,
                sect6(None, false),
            );
            let f = decode_message(&m).unwrap();
            assert_eq!(f[0].parameter, want);
            assert_eq!((f[0].category, f[0].number), (cat, num));
            assert_eq!(f[0].product_template, template);
            assert_eq!(
                f[0].level,
                Level {
                    kind: LEVEL_ISOBARIC,
                    value: 85_000.0
                }
            );
            assert_eq!(f[0].forecast_time, 6);
        }
    }
    // Scale factor: 2 m given as 20 with scale 1; sign-magnitude negative scale.
    let m = simple_message(
        small_grid(),
        sect4(0, 0, 0, 103, 1, 20),
        &p,
        sect6(None, false),
    );
    assert_eq!(
        decode_message(&m).unwrap()[0].level,
        Level {
            kind: LEVEL_HEIGHT_ABOVE_GROUND,
            value: 2.0
        }
    );
    let m = simple_message(
        small_grid(),
        sect4(0, 0, 0, 100, -2, 5),
        &p,
        sect6(None, false),
    );
    assert_eq!(decode_message(&m).unwrap()[0].level.value, 500.0);
    // Missing surface.
    let m = simple_message(
        small_grid(),
        sect4(0, 0, 0, 255, 0, 0),
        &p,
        sect6(None, false),
    );
    assert_eq!(decode_message(&m).unwrap()[0].level.kind, 255);
    // Other product templates refused (4.2 derived forecast).
    let m = simple_message(
        small_grid(),
        sect4(2, 0, 0, 100, 0, 1),
        &p,
        sect6(None, false),
    );
    assert_eq!(
        decode_message(&m),
        Err(Grib2Error::UnsupportedProductTemplate(2))
    );
    // Ensemble forms 4.1 and 4.11 share the first 34 octets.
    for template in [1u16, 11] {
        let mut sec = sect4(0, 3, 5, 100, 0, 5000);
        sec[7..9].copy_from_slice(&template.to_be_bytes());
        let extra = if template == 1 { 3 } else { 27 };
        sec.extend(std::iter::repeat_n(0u8, extra));
        if template == 11 {
            // The interval's end time (octets 38-44) must be a calendar time.
            sec[37..42].copy_from_slice(&[0x07, 0xEA, 11, 14, 19]);
        }
        let n = sec.len() as u32;
        sec[0..4].copy_from_slice(&n.to_be_bytes());
        let m = simple_message(small_grid(), sec, &p, sect6(None, false));
        let f = decode_message(&m).unwrap();
        assert_eq!(
            (f[0].parameter, f[0].product_template),
            (Parameter::Hgt, template)
        );
    }
    assert_eq!(parameter_name(Parameter::Ugrd), Some("UGRD"));
    assert_eq!(parameter_name(Parameter::Other), None);
}

// REQ: OA-GRIB-006
#[test]
fn times() {
    let t = RefTime {
        year: 2026,
        month: 11,
        day: 14,
        hour: 18,
        minute: 0,
        second: 0,
    };
    assert_eq!(ref_unix_seconds(&t), 1_794_679_200);
    let epoch = RefTime {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };
    assert_eq!(ref_unix_seconds(&epoch), 0);
    let leap = RefTime {
        year: 2024,
        month: 3,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };
    assert_eq!(ref_unix_seconds(&leap), 1_709_251_200);
    assert_eq!(forecast_seconds(1, 6), Some(21_600));
    assert_eq!(forecast_seconds(10, 1), Some(10_800));
    assert_eq!(forecast_seconds(9, 1), None);
}

fn hrrr_grid() -> Grid {
    parse_grid(&sect3_lambert(&hrrr_spec(1799, 1059))).unwrap()
}

// REQ: OA-GRIB-003
#[test]
fn lambert_template_is_read() {
    let Grid::Lambert(l) = hrrr_grid() else {
        panic!()
    };
    assert_eq!((l.nx, l.ny), (1799, 1059));
    assert!((l.la1 - 21.138123).abs() < 1e-9);
    assert!((l.lo1 - 237.280472).abs() < 1e-9);
    assert!((l.lov - 262.5).abs() < 1e-9);
    assert_eq!(l.dx, 3000.0);
    assert_eq!(l.radius, 6_371_229.0);
    // Tangent cone at 38.5: n = sin(38.5 deg).
    assert!((l.n - 38.5f64.to_radians().sin()).abs() < 1e-12);
    assert_eq!(l.flags, 0x08);
    assert!(components_grid_relative(&Grid::Lambert(l)));
}

// REQ: OA-GRIB-003
#[test]
fn lambert_projection_round_trips_and_hits_the_first_point() {
    let g = hrrr_grid();
    // First point maps to (0, 0) and back to the stated latitude and longitude.
    let (i, j) = latlon_to_ij(&g, 21.138123, 237.280472 - 360.0).unwrap();
    assert!(i.abs() < 1e-6 && j.abs() < 1e-6, "{i} {j}");
    let (la, lo) = ij_to_latlon(&g, 0.0, 0.0).unwrap();
    assert!((la - 21.138123).abs() < 1e-9 && (lo - (237.280472 - 360.0)).abs() < 1e-9);
    // Round trip over the whole grid, including the corners.
    for &(i, j) in &[
        (0.0, 0.0),
        (1798.0, 0.0),
        (0.0, 1058.0),
        (1798.0, 1058.0),
        (899.5, 529.25),
        (12.3, 987.6),
    ] {
        let (la, lo) = ij_to_latlon(&g, i, j).unwrap();
        let (i2, j2) = latlon_to_ij(&g, la, lo).unwrap();
        assert!(
            (i - i2).abs() < 1e-6 && (j - j2).abs() < 1e-6,
            "{i},{j} -> {la},{lo} -> {i2},{j2}"
        );
    }
    // Longitudes given as 0..360 or -180..180 agree.
    assert_eq!(latlon_to_ij(&g, 40.0, 270.0), latlon_to_ij(&g, 40.0, -90.0));
    // Grid length: neighbouring points are 3000 m apart on the sphere (at the standard parallel).
    let (la_a, lo_a) = ij_to_latlon(&g, 899.0, 500.0).unwrap();
    let (la_b, lo_b) = ij_to_latlon(&g, 900.0, 500.0).unwrap();
    let d = great_circle(la_a, lo_a, la_b, lo_b, 6_371_229.0);
    // Row 500 is at about 38.5 degrees north only roughly; the map factor is within a few percent.
    assert!((d - 3000.0).abs() < 90.0, "{d}");
    // The standard parallel is where the scale is exact: find j with latitude 38.5 on LoV.
    let (i0, j0) = latlon_to_ij(&g, 38.5, 262.5).unwrap();
    let (la_a, lo_a) = ij_to_latlon(&g, i0, j0).unwrap();
    let (la_b, lo_b) = ij_to_latlon(&g, i0 + 1.0, j0).unwrap();
    let d = great_circle(la_a, lo_a, la_b, lo_b, 6_371_229.0);
    assert!((d - 3000.0).abs() < 0.5, "{d}");
}

fn great_circle(la1: f64, lo1: f64, la2: f64, lo2: f64, r: f64) -> f64 {
    let (p1, p2) = (la1.to_radians(), la2.to_radians());
    let dl = (lo2 - lo1).to_radians();
    let a = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

// REQ: OA-GRIB-003
#[test]
fn lambert_secant_and_southern_hemisphere() {
    // RAP 13 km style: secant cone, 25 N both parallels (tangent) and a two-parallel case.
    let mk = |latin1, latin2, la1, lad| {
        lambert_setup(LambertInput {
            nx: 100,
            ny: 80,
            la1,
            lo1: -100.0,
            lad,
            lov: -95.0,
            dx: 10_000.0,
            dy: 10_000.0,
            latin1,
            latin2,
            radius: 6_371_229.0,
            flags: 0,
            scan: 0x40,
        })
        .unwrap()
    };
    for g in [
        mk(25.0, 25.0, 20.0, 25.0),
        mk(30.0, 60.0, 35.0, 40.0),
        mk(-30.0, -60.0, -50.0, -45.0),
    ] {
        let g = Grid::Lambert(g);
        for &(i, j) in &[(0.0, 0.0), (99.0, 79.0), (40.5, 20.25)] {
            let (la, lo) = ij_to_latlon(&g, i, j).unwrap();
            let (i2, j2) = latlon_to_ij(&g, la, lo).unwrap();
            assert!((i - i2).abs() < 1e-6 && (j - j2).abs() < 1e-6, "{i},{j}");
        }
    }
    // Southern cone constant is negative; parallels on opposite sides are refused.
    assert!(mk(-30.0, -60.0, -50.0, -45.0).n < 0.0);
    let bad = lambert_setup(LambertInput {
        nx: 2,
        ny: 2,
        la1: 0.0,
        lo1: 0.0,
        lad: 10.0,
        lov: 0.0,
        dx: 1.0,
        dy: 1.0,
        latin1: 10.0,
        latin2: -10.0,
        radius: 6e6,
        flags: 0,
        scan: 0,
    });
    assert!(matches!(bad, Err(Grib2Error::BadGrid(_))));
}

// REQ: OA-GRIB-004
#[test]
fn latlon_template_gfs() {
    let m = parse_grid(&sect3_latlon(&gfs_spec())).unwrap();
    let Grid::LatLon(l) = m else { panic!() };
    assert!(l.global && l.j_dir == -1);
    assert_eq!(latlon_to_ij(&m, 90.0, 0.0), Some((0.0, 0.0)));
    let (i, j) = latlon_to_ij(&m, 40.0, -105.0).unwrap();
    assert!((i - 1020.0).abs() < 1e-9 && (j - 200.0).abs() < 1e-9);
    assert_eq!(ij_to_latlon(&m, 1020.0, 200.0), Some((40.0, -105.0)));
    // South-to-north variant.
    let mut s = gfs_spec();
    s.scan = 0x40;
    s.la1_micro = -90_000_000;
    s.la2_micro = 90_000_000;
    let m = parse_grid(&sect3_latlon(&s)).unwrap();
    let (_, j) = latlon_to_ij(&m, -89.75, 0.0).unwrap();
    assert!((j - 1.0).abs() < 1e-9);
    // Disagreeing far corner refused.
    let mut s = gfs_spec();
    s.la2_micro = -80_000_000;
    assert!(matches!(
        parse_grid(&sect3_latlon(&s)),
        Err(Grib2Error::BadGrid(_))
    ));
}

// REQ: OA-GRIB-005
#[test]
fn unsupported_grid_features_are_refused() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    for scan in [0x80u8, 0xC0, 0x20, 0x10, 0x41] {
        let mut spec = hrrr_spec(4, 3);
        spec.scan = scan;
        let m = simple_message(
            sect3_lambert(&spec),
            temp_product(500),
            &p,
            sect6(None, false),
        );
        assert_eq!(
            decode_message(&m),
            Err(Grib2Error::UnsupportedScanMode(scan))
        );
        let mut s = gfs_spec();
        s.scan = scan;
        assert_eq!(
            parse_grid(&sect3_latlon(&s)),
            Err(Grib2Error::UnsupportedScanMode(scan))
        );
    }
    for shape in [2u8, 3, 4, 5, 7, 9] {
        let mut spec = hrrr_spec(4, 3);
        spec.shape = shape;
        assert_eq!(
            parse_grid(&sect3_lambert(&spec)),
            Err(Grib2Error::UnsupportedEarthShape(shape))
        );
    }
    for shape in [0u8, 6, 8] {
        let mut spec = hrrr_spec(4, 3);
        spec.shape = shape;
        assert!(parse_grid(&sect3_lambert(&spec)).is_ok());
    }
    // Other grid templates (polar stereographic 3.20, rotated 3.1).
    let mut g = small_grid();
    g[12..14].copy_from_slice(&20u16.to_be_bytes());
    assert_eq!(parse_grid(&g), Err(Grib2Error::UnsupportedGridTemplate(20)));
}

// REQ: OA-GRIB-012
#[test]
fn bilinear_interpolation() {
    // v = 2 i + 3 j + 1 on the grid, so any bilinear sample of grid indexes is exact.
    let vals: Vec<f32> = (0..3)
        .flat_map(|j| (0..4).map(move |i| 2.0 * i as f32 + 3.0 * j as f32 + 1.0))
        .collect();
    let f = lambert_field(vals, 0x08);
    for &(i, j) in &[
        (0.0, 0.0),
        (3.0, 2.0),
        (1.5, 0.5),
        (2.25, 1.75),
        (3.0, 0.0),
        (0.0, 2.0),
    ] {
        let (la, lo) = ij_to_latlon(&f.grid, i, j).unwrap();
        let v = sample_bilinear(&f, la, lo).unwrap();
        assert!(
            (f64::from(v) - (2.0 * i + 3.0 * j + 1.0)).abs() < 1e-4,
            "{i},{j}: {v}"
        );
    }
    // Outside.
    let (la, lo) = ij_to_latlon(&f.grid, 3.2, 1.0).unwrap();
    assert_eq!(sample_bilinear(&f, la, lo), None);
    let (la, lo) = ij_to_latlon(&f.grid, 1.0, -0.2).unwrap();
    assert_eq!(sample_bilinear(&f, la, lo), None);
    assert_eq!(sample_bilinear(&f, f64::NAN, 0.0), None);
    // Missing neighbour spoils the cell but not cells that avoid it.
    let mut vals = f.values.clone();
    vals[5] = f32::NAN; // (i=1, j=1)
    let g = lambert_field(vals, 0x08);
    let (la, lo) = ij_to_latlon(&g.grid, 1.5, 0.5).unwrap();
    assert_eq!(sample_bilinear(&g, la, lo), None);
    let (la, lo) = ij_to_latlon(&g.grid, 2.5, 1.5).unwrap();
    assert!(sample_bilinear(&g, la, lo).is_some());
}

// REQ: OA-GRIB-012
#[test]
fn bilinear_wraps_on_a_global_grid() {
    // A 4 x 3 global grid with 90 degree spacing.
    let s = LatLonSpec {
        ni: 4,
        nj: 3,
        la1_micro: 90_000_000,
        lo1_micro: 0,
        la2_micro: -90_000_000,
        lo2_micro: 270_000_000,
        di_micro: 90_000_000,
        dj_micro: 90_000_000,
        scan: 0,
    };
    let grid = parse_grid(&sect3_latlon(&s)).unwrap();
    let mut f = lambert_field(
        vec![
            0.0, 10.0, 20.0, 30.0, 0.0, 10.0, 20.0, 30.0, 0.0, 10.0, 20.0, 30.0,
        ],
        0,
    );
    f.grid = grid;
    // Between column 3 (270 E = 30) and column 0 (360 E = 0): halfway is 15 mixing 30 and 0.
    assert!((sample_bilinear(&f, 0.0, 315.0).unwrap() - 15.0).abs() < 1e-5);
    assert!((sample_bilinear(&f, 0.0, -45.0).unwrap() - 15.0).abs() < 1e-5);
    assert!((sample_bilinear(&f, 0.0, 45.0).unwrap() - 5.0).abs() < 1e-5);
}

/// Direction vectors of true east and true north at a point, in grid (x, y) axes, found by
/// projecting small steps. Independent of the closed-form rotation angle.
fn local_axes(g: &Grid, lat: f64, lon: f64) -> ((f64, f64), (f64, f64)) {
    let h = 1e-4;
    let (i0, j0) = latlon_to_ij(g, lat, lon).unwrap();
    let (ie, je) = latlon_to_ij(g, lat, lon + h).unwrap();
    let (inn, jn) = latlon_to_ij(g, lat + h, lon).unwrap();
    // +j is north in a 0x40 grid, so index steps are metric up to one scale shared by both axes.
    let e = (ie - i0, je - j0);
    let n = (inn - i0, jn - j0);
    let ne = e.0.hypot(e.1);
    let nn = n.0.hypot(n.1);
    ((e.0 / ne, e.1 / ne), (n.0 / nn, n.1 / nn))
}

// REQ: OA-GRIB-013
#[test]
fn wind_rotation_matches_finite_difference_axes() {
    let g = hrrr_grid();
    let pts = [
        (38.5, -97.5),
        (40.0, -75.0),
        (45.0, -110.0),
        (30.0, -120.0),
        (48.0, -62.0),
        (25.0, -85.0),
    ];
    for &(lat, lon) in &pts {
        let (east, north) = local_axes(&g, lat, lon);
        // A grid is turned: east is not along +x unless on the central meridian.
        for &(ue, ve) in &[(10.0, 0.0), (0.0, 10.0), (-7.0, 12.0), (25.0, -30.0)] {
            // Construct the grid-relative components from the earth-relative wind.
            let ug = ue * east.0 + ve * north.0;
            let vg = ue * east.1 + ve * north.1;
            let (e2, n2) = to_earth_relative(&g, lon, ug, vg);
            assert!(
                (e2 - ue).abs() < 2e-3 && (n2 - ve).abs() < 2e-3,
                "{lat},{lon}: {e2},{n2} vs {ue},{ve}"
            );
        }
        // The convergence angle is the angle of true north from the grid's +y.
        let th = grid_rotation(&g, lon);
        assert!((north.0 + th.sin()).abs() < 1e-3 && (north.1 - th.cos()).abs() < 1e-3);
    }
}

// REQ: OA-GRIB-013
#[test]
fn wind_rotation_signs_and_end_to_end() {
    let g = hrrr_grid();
    // On the central meridian grid and earth axes agree.
    let (e, n) = to_earth_relative(&g, -97.5, 10.0, 5.0);
    assert!((e - 10.0).abs() < 1e-9 && (n - 5.0).abs() < 1e-9);
    // East of the central meridian (northern hemisphere) a grid-relative +y vector (grid north)
    // points east of true north: it has a positive east component. West of it, negative.
    let (e, _) = to_earth_relative(&g, -75.0, 0.0, 10.0);
    assert!(e > 0.0);
    let (e, _) = to_earth_relative(&g, -120.0, 0.0, 10.0);
    assert!(e < 0.0);
    // Magnitude is preserved.
    let (e, n) = to_earth_relative(&g, -70.0, 3.0, 4.0);
    assert!((e.hypot(n) - 5.0).abs() < 1e-9);
    // A grid already earth-relative is left alone.
    let mut spec = hrrr_spec(4, 3);
    spec.flags = 0;
    let g2 = parse_grid(&sect3_lambert(&spec)).unwrap();
    assert_eq!(to_earth_relative(&g2, -70.0, 3.0, 4.0), (3.0, 4.0));
    // End to end on a small grid: a uniform grid-relative wind (0, 10) is a different earth wind
    // at different longitudes; with the flag clear the same field is returned unrotated.
    let u = lambert_field(vec![0.0; 12], 0x08);
    let v = lambert_field(vec![10.0; 12], 0x08);
    let (la, lo) = ij_to_latlon(&u.grid, 3.0, 1.0).unwrap();
    let w = wind_at(&u, &v, la, lo).unwrap().unwrap();
    let th = grid_rotation(&u.grid, lo);
    assert!((f64::from(w.east) - 10.0 * th.sin()).abs() < 1e-4);
    assert!((f64::from(w.north) - 10.0 * th.cos()).abs() < 1e-4);
    assert!(
        th.abs() > 0.05,
        "test grid is far enough from LoV to rotate"
    );
    let (u0, v0) = (
        lambert_field(vec![0.0; 12], 0),
        lambert_field(vec![10.0; 12], 0),
    );
    let w0 = wind_at(&u0, &v0, la, lo).unwrap().unwrap();
    assert_eq!((w0.east, w0.north), (0.0, 10.0));
    // Mismatched grids and outside positions.
    assert_eq!(wind_at(&u, &v0, la, lo), Err(Grib2Error::GridMismatch));
    assert_eq!(wind_at(&u, &v, 0.0, 0.0), Ok(None));
}

// REQ: OA-GRIB-014
#[test]
fn idx_parses_and_finds_byte_ranges() {
    let text = "1:0:d=2026111418:HGT:500 mb:anl:\n2:1000:d=2026111418:TMP:500 mb:anl:\n3:2500:d=2026111418:UGRD:250 mb:anl:\r\n\n4:4000:d=2026111418:VGRD:250 mb:anl:\n5:5000:d=2026111418:UGRD:250 mb:6 hour fcst:ens:x:\n";
    let i = parse_idx(text).unwrap();
    assert_eq!(idx_len(&i), 5);
    assert_eq!(i.date[0], 2026111418);
    let rows = find_rows(&i, "UGRD", &isobaric_label(250));
    assert_eq!(rows, vec![2, 4]);
    assert_eq!(byte_range(&i, 2, None), Some((2500, Some(3999))));
    assert_eq!(byte_range(&i, 4, None), Some((5000, None)));
    assert_eq!(byte_range(&i, 4, Some(9000)), Some((5000, Some(8999))));
    assert_eq!(byte_range(&i, 9, None), None);
    // Two fields in one message share an offset and one range.
    let multi = "1:0:d=2026111418:UGRD:250 mb:anl:\n2.1:700:d=2026111418:UGRD:300 mb:anl:\n2.2:700:d=2026111418:VGRD:300 mb:anl:\n3:900:d=2026111418:TMP:300 mb:anl:\n";
    let m = parse_idx(multi).unwrap();
    assert_eq!(
        (m.msg.clone(), m.sub.clone()),
        (vec![1, 2, 2, 3], vec![0, 1, 2, 0])
    );
    assert_eq!(byte_range(&m, 1, None), Some((700, Some(899))));
    assert_eq!(byte_range(&m, 2, None), Some((700, Some(899))));
    assert!(parse_idx("2.1:5:d=2026111418:A:b:c:\n2.2:6:d=2026111418:A:b:c:").is_err());
    assert!(parse_idx("2.2:5:d=2026111418:A:b:c:\n2.1:5:d=2026111418:A:b:c:").is_err());
    assert!(find_rows(&i, "UGRD", "300 mb").is_empty());
    for (bad, why) in [
        ("x:0:d=2026111418:A:b:c:", "message number"),
        ("1:y:d=2026111418:A:b:c:", "byte offset"),
        ("1:0:2026111418:A:b:c:", "missing d="),
        ("1:0:d=20261114:A:b:c:", "date is not YYYYMMDDHH"),
        ("1:0:d=2026111418::b:c:", "missing variable"),
        ("1:0:d=2026111418:A:b", "missing forecast"),
        (
            "2:5:d=2026111418:A:b:c:\n3:5:d=2026111418:A:b:c:",
            "message number or offset does not increase",
        ),
    ] {
        match parse_idx(bad) {
            Err(Grib2Error::BadIdx { why: w, .. }) => assert_eq!(w, why, "{bad}"),
            other => panic!("{bad}: {other:?}"),
        }
    }
    let long = format!("1:0:d=2026111418:A:{}:c:", "x".repeat(2000));
    assert!(matches!(
        parse_idx(&long),
        Err(Grib2Error::BadIdx { line: 1, .. })
    ));
}

// REQ: OA-GRIB-015
#[test]
fn mutated_messages_never_panic() {
    let x = ramp(12, |k| 100 + (k as i64 * 37) % 11);
    let o = ComplexOpts {
        group_lens: vec![4, 4, 4],
        order: 2,
        missing_mgmt: 1,
        extra_octets: 2,
    };
    let p = encode_complex(&x, 1.0, 0, 0, &o);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut decoded = 0;
    for _ in 0..20_000 {
        let mut b = m.clone();
        for _ in 0..1 + (state % 3) {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pos = (state >> 33) as usize % b.len();
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            b[pos] = (state >> 40) as u8;
        }
        if !decode_stream(&b).fields.is_empty() {
            decoded += 1;
        }
    }
    assert!(decoded > 0);
}

/// A message whose section 5 claims `ndata` values, patched into an otherwise valid field.
fn with_ndata(mut p: Packed, ndata: u32, bitmap: Vec<u8>) -> Vec<u8> {
    p.sect5[5..9].copy_from_slice(&ndata.to_be_bytes());
    simple_message(small_grid(), temp_product(500), &p, bitmap)
}

// REQ: OA-GRIB-025
#[test]
fn value_count_is_checked_against_the_grid_before_anything_is_allocated() {
    // A section 5 that claims 4,294,967,295 values at zero bits each would have the unpacker
    // reserve 16 GiB (4 bytes per value, more for the complex templates) while the data section
    // is empty. Every template must be refused first, with nothing sized from the claim.
    let huge = u32::MAX;
    let refused = |m: &[u8]| {
        matches!(
            decode_message(m),
            Err(Grib2Error::BadPacking(_) | Grib2Error::BadBitmap(_))
        )
    };
    // 5.0 simple packing, zero bits per value, no bitmap.
    let p = encode_simple(&[0; 12], 1.0, 0, 0);
    assert!(refused(&with_ndata(p, huge, sect6(None, false))));
    // 5.40 JPEG 2000, zero bits per value (the "constant field" path sizes a vector by ndata).
    let p = encode_jpeg2000(&[], 12, 273.5, 0, 0, 0, 0);
    assert!(refused(&with_ndata(p, huge, sect6(None, false))));
    // 5.2 and 5.3 with one group whose stated length is the same enormous count.
    for order in [0u8, 2] {
        let o = ComplexOpts {
            group_lens: vec![12],
            order,
            missing_mgmt: 0,
            extra_octets: 2,
        };
        let mut p = encode_complex(&[Some(7); 12], 1.0, 0, 0, &o);
        p.sect5[42..46].copy_from_slice(&huge.to_be_bytes());
        assert!(refused(&with_ndata(p, huge, sect6(None, false))));
    }
    // With a bitmap the count must equal the set bits, not merely fit the grid.
    let mask: Vec<bool> = (0..12).map(|k| k % 2 == 0).collect();
    let p = encode_simple(&[1, 2, 3, 4, 5, 6], 0.0, 0, 0);
    assert!(refused(&with_ndata(
        p.clone(),
        huge,
        sect6(Some(&mask), false)
    )));
    assert!(refused(&with_ndata(
        p.clone(),
        7,
        sect6(Some(&mask), false)
    )));
    // A claim within the grid but past what the data section holds is refused too.
    let p = encode_simple(&[1, 2, 3], 0.0, 0, 0);
    assert!(refused(&with_ndata(p, 12, sect6(None, false))));
    // The unpackers refuse a count beyond the grid limit when called directly.
    let rp = crate::pack::parse_repr(&encode_simple(&[0; 12], 1.0, 0, 0).sect5).unwrap();
    let big = crate::pack::Repr { ndata: huge, ..rp };
    assert!(matches!(
        crate::pack::unpack(&big, &[]),
        Err(Grib2Error::GridPoints(_))
    ));
}

// REQ: OA-GRIB-050
#[test]
fn scaling_that_leaves_the_f32_range_is_refused() {
    // R = 0, E = 400, D = -40, X = 1 scales to about 2.6e160, and 1e40 (E = 0) is past f32 too.
    for (e, d) in [(400i64, -40i64), (0, -40)] {
        let p = encode_simple(&[1, 0], 0.0, e, d);
        let rp = crate::pack::parse_repr(&p.sect5).unwrap();
        let payload = &p.sect7[5..];
        let r = crate::pack::unpack(&rp, payload);
        assert_eq!(r, Err(Grib2Error::NonFinite { index: 0 }), "E {e} D {d}");
        assert!(r.unwrap_err().to_string().contains("finite"));
    }
    // Complex packing goes through the same scaling.
    let o = ComplexOpts {
        group_lens: vec![2],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 2,
    };
    let p = encode_complex(&[Some(1), Some(1)], 0.0, 400, -40, &o);
    let rp = crate::pack::parse_repr(&p.sect5).unwrap();
    assert!(crate::pack::unpack(&rp, &p.sect7[5..]).is_err());
    // A whole message with such a field is refused by the decoder, not returned.
    let p = encode_simple(&[1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0], 0.0, 400, -40);
    let m = simple_message(small_grid(), temp_product(500), &p, sect6(None, false));
    assert!(decode_message(&m).is_err());
    // Large but representable values still decode.
    let p = encode_simple(&[1, 0], 0.0, 0, -30);
    let rp = crate::pack::parse_repr(&p.sect5).unwrap();
    let v = crate::pack::unpack(&rp, &p.sect7[5..]).unwrap();
    assert_eq!(v[0], 1e30f32);
}

// REQ: OA-GRIB-051
#[test]
fn zero_bit_group_tables_cannot_amplify_a_tiny_section() {
    // 100,000 groups of one point each, every table zero bits wide, no payload at all: the
    // descriptor vectors would be sized from a few hundred bytes of metadata.
    let n = 100_000usize;
    let o = ComplexOpts {
        group_lens: vec![1; n],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 2,
    };
    let x: Vec<Option<i64>> = vec![Some(0); n];
    let p = encode_complex(&x, 1.0, 0, 0, &o);
    assert!(p.sect7.len() <= 5, "no data octets");
    let rp = crate::pack::parse_repr(&p.sect5).unwrap();
    let limits = crate::pack::Limits {
        scratch_bytes: 1 << 20,
    };
    let r = crate::pack::unpack_with(&rp, &p.sect7[5..], &limits);
    assert!(
        matches!(r, Err(Grib2Error::ScratchBudget { .. })),
        "{:?}",
        r.map(|v| v.len())
    );
    // The cap check runs on the numbers alone (nothing allocated): the largest grid with one
    // group per point needs 28 bytes a point, past the default 128 MiB.
    use crate::pack::{check_scratch, scratch_bytes, Limits};
    let max = crate::MAX_GRID_POINTS;
    let d = Limits::default();
    let Err(Grib2Error::ScratchBudget { needed, budget }) = check_scratch(max, max, &d) else {
        panic!("not refused");
    };
    assert_eq!(needed, scratch_bytes(max, max));
    assert_eq!(budget, 128 << 20);
    // An HRRR-size grid with one group per point is within it.
    assert!(check_scratch(1_905_141, 1_905_141, &d).is_ok());
}

/// A complex-packed 5.2 field of `n` points as the zero-cost construction: `ng` length-one groups
/// with 1-bit tables of zeros (reference 0, width 0, length 1) and no value bits.
#[cfg(test)]
fn zero_cost_groups(n: u32, ng: u32) -> (crate::pack::Repr, Vec<u8>) {
    let rp = crate::pack::Repr {
        template: 2,
        ndata: n,
        r: 0.0,
        e: 0,
        d: 0,
        nbits: 1,
        complex: Some(crate::pack::Complex {
            missing_mgmt: 0,
            ngroups: ng,
            width_ref: 0,
            width_bits: 1,
            length_ref: 1,
            length_inc: 0,
            last_length: 1,
            length_bits: 1,
            diff_order: 0,
            extra_octets: 0,
        }),
    };
    (rp, vec![0u8; 3 * (ng as usize).div_ceil(8)])
}

// REQ: OA-GRIB-052
#[test]
fn scratch_cap_refuses_one_group_per_point_before_allocating() {
    use crate::pack::{unpack_with, Limits};
    // The cheap construction at a few hundred thousand points: the tables are 3 bits a group,
    // so the payload is ~110 kB, yet the working set is ~8 MB against a 1 MiB cap.
    let n = 300_000u32;
    let (rp, payload) = zero_cost_groups(n, n);
    assert!(payload.len() < 120_000);
    let limits = Limits {
        scratch_bytes: 1 << 20,
    };
    let r = unpack_with(&rp, &payload, &limits);
    let Err(Grib2Error::ScratchBudget { needed, budget }) = r else {
        panic!("not refused: {:?}", r.map(|v| v.len()));
    };
    assert_eq!(budget, 1 << 20);
    assert_eq!(
        needed,
        crate::pack::scratch_bytes(u64::from(n), u64::from(n))
    );
    assert!(needed > 8_000_000);
    // The default cap is 128 MiB.
    assert_eq!(Limits::default().scratch_bytes, 128 << 20);
}

// REQ: OA-GRIB-053
#[test]
fn ordinary_constant_field_decodes_under_the_cap() {
    use crate::pack::{unpack_with, Limits};
    let o = ComplexOpts {
        group_lens: vec![1000],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 2,
    };
    let p = encode_complex(&vec![Some(7); 1000], 0.0, 0, 0, &o);
    let rp = crate::pack::parse_repr(&p.sect5).unwrap();
    let limits = Limits {
        scratch_bytes: 1 << 20,
    };
    let v = unpack_with(&rp, &p.sect7[5..], &limits).unwrap();
    assert_eq!(v.len(), 1000);
    assert!(v.iter().all(|&x| x == 7.0));
    assert_eq!(crate::pack::unpack(&rp, &p.sect7[5..]).unwrap(), v);
}

// REQ: OA-GRIB-054
#[test]
fn more_groups_than_points_is_refused() {
    let (rp, payload) = zero_cost_groups(10, 11);
    assert_eq!(
        crate::pack::unpack(&rp, &payload),
        Err(Grib2Error::BadPacking("more groups than points"))
    );
    // As many groups as points is allowed.
    let (rp, payload) = zero_cost_groups(10, 10);
    assert_eq!(crate::pack::unpack(&rp, &payload).unwrap().len(), 10);
}

/// A complex field whose width table adds the width reference to a 32-bit delta.
fn width_overflow_case() -> (crate::pack::Repr, Vec<u8>) {
    let rp = crate::pack::Repr {
        template: 2,
        ndata: 1,
        r: 0.0,
        e: 0,
        d: 0,
        nbits: 1,
        complex: Some(crate::pack::Complex {
            missing_mgmt: 0,
            ngroups: 1,
            width_ref: 32,
            width_bits: 32,
            length_ref: 1,
            length_inc: 0,
            last_length: 1,
            length_bits: 1,
            diff_order: 0,
            extra_octets: 0,
        }),
    };
    // Reference (1 bit, padded to an octet), width delta 0xFFFFFFFF (4 octets), length (1 bit).
    let mut payload = vec![0u8];
    payload.extend([0xFF; 4]);
    payload.push(0);
    (rp, payload)
}

// REQ: OA-GRIB-055
#[test]
fn group_width_sum_cannot_overflow() {
    let (rp, payload) = width_overflow_case();
    assert_eq!(
        crate::pack::unpack(&rp, &payload),
        Err(Grib2Error::BadPacking("group width over 32 bits"))
    );
}

/// The same descriptor as a whole message: one group of 12 points, width reference 32 and width
/// delta 0xFFFFFFFF. Committed as a fuzz seed (`fuzz/seeds/grib2/width-overflow.grib2`).
fn width_overflow_message() -> Vec<u8> {
    let o = ComplexOpts {
        group_lens: vec![12],
        order: 0,
        missing_mgmt: 0,
        extra_octets: 0,
    };
    let mut p = encode_complex(&ramp(12, |k| k as i64), 0.0, 0, 0, &o);
    p.sect5[35] = 32;
    p.sect5[36] = 32;
    let mut payload = p.sect7[5..].to_vec();
    payload.iter_mut().for_each(|b| *b = 0xFF);
    let mut s7 = be(5 + payload.len() as u64, 4);
    s7.push(7);
    s7.extend(payload);
    p.sect7 = s7;
    let bm = sect6(None, false);
    simple_message(small_grid(), temp_product(500), &p, bm)
}

// REQ: OA-GRIB-055
#[test]
fn width_overflow_message_is_refused_and_seeded() {
    let m = width_overflow_message();
    assert_eq!(
        decode_message(&m),
        Err(Grib2Error::BadPacking("group width over 32 bits"))
    );
    let seed = include_bytes!("../../../fuzz/seeds/grib2/width-overflow.grib2");
    assert_eq!(seed.as_slice(), m.as_slice());
}

// REQ: OA-GRIB-056
#[test]
fn reference_time_is_a_real_calendar_time() {
    use crate::product::parse_ref_time;
    let bad = |y: u16, m: u8, d: u8| {
        assert_eq!(
            parse_ref_time(&sect1(y, m, d, 0)),
            Err(Grib2Error::BadProduct(
                "reference time is not a calendar time"
            )),
            "{y}-{m}-{d}"
        );
    };
    // Not a leap year: 2023, and 1900 (divisible by 100 but not 400).
    bad(2023, 2, 29);
    bad(1900, 2, 29);
    bad(2024, 2, 30);
    bad(2024, 2, 31);
    bad(2023, 2, 31);
    bad(2024, 4, 31);
    bad(2024, 6, 31);
    bad(2024, 9, 31);
    bad(2024, 11, 31);
    bad(2024, 1, 0);
    bad(2024, 13, 1);
    for (y, m, d) in [(2000, 2, 29), (2024, 2, 29), (2023, 2, 28), (2024, 12, 31)] {
        let t = parse_ref_time(&sect1(y, m, d, 23)).unwrap();
        assert_eq!((t.year, t.month, t.day), (y, m, d));
    }
    // Hour, minute and second ranges.
    let mut s = sect1(2024, 1, 1, 0);
    s[16] = 24;
    assert!(parse_ref_time(&s).is_err());
    s[16] = 23;
    s[17] = 60;
    assert!(parse_ref_time(&s).is_err());
    s[17] = 59;
    s[18] = 61;
    assert!(parse_ref_time(&s).is_err());
    s[18] = 59;
    assert!(parse_ref_time(&s).is_ok());
}

// REQ: OA-GRIB-057
#[test]
fn statistical_interval_is_exposed() {
    use crate::product::parse_product;
    // Template 4.0 and 4.1 are instantaneous.
    let t0 = parse_product(&sect4(0, 2, 2, 100, 0, 25_000)).unwrap();
    assert_eq!(t0.interval, None);
    // Template 4.8: end 2026-11-14 19:00:00, one range, statistical process 1 (accumulation).
    let s8 = sect4(8, 2, 2, 100, 0, 25_000);
    let t8 = parse_product(&s8).unwrap();
    let iv = t8.interval.unwrap();
    assert_eq!(
        (iv.end.year, iv.end.month, iv.end.day, iv.end.hour),
        (2026, 11, 14, 19)
    );
    assert_eq!((iv.ranges, iv.statistic), (1, 1));
    // Template 4.11 is 4.8 with three ensemble octets after octet 34.
    let mut s11 = s8.clone();
    s11.splice(34..34, [1, 0, 1]);
    s11[7..9].copy_from_slice(&11u16.to_be_bytes());
    let n = s11.len() as u32;
    s11[0..4].copy_from_slice(&n.to_be_bytes());
    assert_eq!(parse_product(&s11).unwrap().interval, Some(iv));
    // The field carries it through a whole message.
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let m = simple_message(small_grid(), s8, &p, sect6(None, false));
    assert_eq!(decode_message(&m).unwrap()[0].interval, Some(iv));
    // An end time that is not on the calendar is refused.
    let mut bad = sect4(8, 2, 2, 100, 0, 25_000);
    bad[36] = 2;
    bad[37] = 30;
    assert!(parse_product(&bad).is_err());
}

// REQ: OA-GRIB-058
#[test]
fn peek_reads_the_first_field_from_the_head_of_a_message() {
    let p = encode_simple(&[1; 12], 0.0, 0, 0);
    let m = simple_message(
        small_grid(),
        sect4(0, 2, 3, 100, 0, 25_000),
        &p,
        sect6(None, false),
    );
    let whole = inspect_stream(&m).fields.remove(0);
    // Section 5 ends by octet 173; the rest is never looked at.
    assert_eq!(peek_info(&m[..175]).unwrap(), whole);
    assert!(peek_info(&m[..60]).is_err());
    assert!(peek_info(b"not a message at all").is_err());
}
