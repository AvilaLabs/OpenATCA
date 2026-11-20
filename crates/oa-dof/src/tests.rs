#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // Test fixtures and loop counters, not parsed input.
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]

use crate::*;

const HEADER: &str = "  CURRENCY DATE = 09/27/26\r\n\
                                   LATITUDE     LONGITUDE     OBSTACLE             AGL   AMSL LT ACC MAR FAA          ACTION\r\n\
OAS#      V CO ST CITY             DEG MIN SEC  DEG MIN SEC   TYPE                 HT    HT     H V IND STUDY           JDATE\r\n\
-------------------------------------------------------------------------------------------------------------------------------\r\n";

const REAL: &str = "01-001307 O US AL DAUPHIN ISLAND   30 10 45.00N 088 04 39.00W RIG                1 00236 00236 R 5 D M 1990ASO01578OE C 2014138 ";
const UNVERIFIED: &str = "02-000152 U US AK KANAGA BAY       51 43 02.00N 177 11 52.00W POLE               1 00058 00158 N     N                A 1989023 ";
const PR: &str = "PR-046790 U PR    CATA\u{ff}O           18 26 35.93N 066 06 53.94W BLDG               3 00116 -0120 N 4 D N 2023ASO29156OE A 2025303 ";
const SOUTH_EAST: &str = "AG-020012 O AG    ST JOHNS         17 07 00.50S 061 50 30.00E T-L TWR            2 00300 00310 H 9 I P                C 2024366 ";

fn file(lines: &[&str]) -> Vec<u8> {
    let mut s = HEADER.to_string();
    for l in lines {
        s.push_str(l);
        s.push_str("\r\n");
    }
    // The PR sample stands for a Windows-1252 byte, not the UTF-8 for U+00FF.
    let mut b: Vec<u8> = Vec::new();
    for ch in s.chars() {
        b.push(ch as u32 as u8);
    }
    b
}

// REQ: OA-DOF-001
#[test]
fn parses_every_field() {
    let d = parse_dof(&file(&[REAL, UNVERIFIED, PR, SOUTH_EAST])).unwrap();
    assert_eq!(
        d.currency,
        Currency {
            year: 2026,
            month: 9,
            day: 27
        }
    );
    assert_eq!((d.lines, d.rejected_total, len(&d)), (8, 0, 4));
    let i = ObstacleIx(0);
    assert_eq!(oas_number(&d, i).unwrap(), "01-001307");
    assert_eq!(d.verification[0], b'O');
    assert_eq!(&d.country[0], b"US");
    assert_eq!(&d.state[0], b"AL");
    assert_eq!(text(&d.city[0]), "DAUPHIN ISLAND");
    assert_eq!(d.lat_e7[0], 301_791_667);
    assert_eq!(d.lon_e7[0], -880_775_000);
    assert_eq!(text(&d.kind[0]), "RIG");
    assert_eq!(d.quantity[0], b'1');
    assert_eq!((d.agl_ft[0], d.amsl_ft[0]), (236, 236));
    assert_eq!(d.lighting[0], b'R');
    assert_eq!((d.h_acc[0], d.v_acc[0]), (b'5', b'D'));
    assert_eq!(d.marking[0], b'M');
    assert_eq!(text(&d.study[0]), "1990ASO01578OE");
    assert_eq!(d.action[0], b'C');
    assert_eq!(d.julian[0], 2_014_138);
    // Unverified, no accuracy codes, no study number.
    assert_eq!(d.verification[1], b'U');
    assert_eq!((d.h_acc[1], d.v_acc[1]), (b' ', b' '));
    assert_eq!(text(&d.study[1]), "");
    assert_eq!(d.action[1], b'A');
    assert_eq!(d.julian[1], 1_989_023);
    // Blank state, Windows-1252 city, negative AMSL, quantity 3.
    assert_eq!(text(&d.state[2]), "");
    assert_eq!(text(&d.city[2]), "CATA\u{ff}O");
    assert_eq!(d.amsl_ft[2], -120);
    assert_eq!(d.quantity[2], b'3');
    assert_eq!(oas_number(&d, ObstacleIx(2)).unwrap(), "PR-046790");
    // Letters in the OAS code, southern and eastern hemispheres, unknown vertical accuracy.
    assert_eq!(oas_number(&d, ObstacleIx(3)).unwrap(), "AG-020012");
    assert!(d.lat_e7[3] < 0 && d.lon_e7[3] > 0);
    assert_eq!(d.v_acc[3], b'I');
    assert_eq!(text(&d.kind[3]), "T-L TWR");
    assert_eq!(lat_deg(&d, ObstacleIx(0)).unwrap(), 30.1791667);
    assert_eq!(lat_deg(&d, ObstacleIx(9)), None);
    assert_eq!(
        lighting_name(d.lighting[3]),
        Some("High intensity white strobe & red")
    );
    assert_eq!(
        marking_name(d.marking[3]),
        Some("Orange or orange and white paint")
    );
    assert_eq!(text(b"O\x92FALLON  "), "O\u{2019}FALLON");
}

// REQ: OA-DOF-002
#[test]
fn coordinates_are_exact_and_bounded() {
    let with = |lat: &str, lon: &str| {
        let mut l = REAL.to_string();
        l.replace_range(35..47, lat);
        l.replace_range(48..61, lon);
        parse_dof(&file(&[&l])).unwrap()
    };
    // 1e-7 degree units: 0.01 arc second is 2.78e-6 degrees, so every value is representable.
    let d = with("00 00 00.01N", "000 00 00.01E");
    assert_eq!((d.lat_e7[0], d.lon_e7[0]), (28, 28));
    let d = with("45 30 30.00S", "120 15 45.50W");
    assert_eq!(d.lat_e7[0], -455_083_333);
    assert_eq!(d.lon_e7[0], -1_202_626_389);
    // Poles and the antimeridian are inside; one hundredth of a second past is not.
    assert_eq!(with("90 00 00.00N", "180 00 00.00E").lat_e7[0], 900_000_000);
    assert_eq!(
        with("90 00 00.00S", "180 00 00.00W").lon_e7[0],
        -1_800_000_000
    );
    for (lat, lon) in [
        ("90 00 00.01N", "000 00 00.00E"),
        ("00 00 00.00N", "180 00 00.01E"),
        ("00 60 00.00N", "000 00 00.00E"),
        ("00 00 60.00N", "000 00 00.00E"),
        ("00 00 00.00X", "000 00 00.00E"),
        ("00 00 00.00N", "000 00 00.00X"),
        ("0A 00 00.00N", "000 00 00.00E"),
        ("00 00 0.000N", "000 00 00.00E"),
    ] {
        let mut l = REAL.to_string();
        l.replace_range(35..47, lat);
        l.replace_range(48..61, lon);
        let d = parse_dof(&file(&[&l])).unwrap();
        assert_eq!(len(&d), 0, "{lat} {lon}");
        assert_eq!(d.rejected_total, 1);
    }
}

// REQ: OA-DOF-003
#[test]
fn accuracy_codes_decode_per_readme() {
    let h = [20.0, 50.0, 100.0, 250.0, 500.0, 1000.0];
    for (i, ft) in h.iter().enumerate() {
        let code = b'1' + i as u8;
        assert!((horizontal_accuracy_ft(code).unwrap() - ft).abs() < 1e-9);
        assert!((horizontal_accuracy_m(code).unwrap() - ft * 0.3048).abs() < 1e-9);
    }
    assert_eq!(horizontal_accuracy_m(b'7'), Some(926.0));
    assert_eq!(horizontal_accuracy_m(b'8'), Some(1852.0));
    assert!((horizontal_accuracy_ft(b'8').unwrap() - 6076.115).abs() < 0.001);
    assert_eq!(horizontal_accuracy_m(b'9'), None);
    assert_eq!(horizontal_accuracy_m(b' '), None);
    assert_eq!(horizontal_accuracy_m(b'0'), None);
    let v = [3.0, 10.0, 20.0, 50.0, 125.0, 250.0, 500.0, 1000.0];
    for (i, ft) in v.iter().enumerate() {
        let code = b'A' + i as u8;
        assert_eq!(vertical_accuracy_ft(code), Some(*ft));
        assert!((vertical_accuracy_m(code).unwrap() - ft * 0.3048).abs() < 1e-9);
    }
    assert_eq!(vertical_accuracy_ft(b'I'), None);
    assert_eq!(vertical_accuracy_ft(b' '), None);
    assert_eq!(vertical_accuracy_ft(b'J'), None);
    // Every lighting and marking letter of the README has a name; others have none.
    for c in b"RDHMSFCWLNU" {
        assert!(lighting_name(*c).is_some());
    }
    for c in b"PWMFSNU" {
        assert!(marking_name(*c).is_some());
    }
    assert!(lighting_name(b'X').is_none() && marking_name(b'X').is_none());
}

// REQ: OA-DOF-004
#[test]
fn each_bad_field_is_a_typed_line_error() {
    // (1-based column of the byte to damage, replacement, expected field)
    let cases: &[(usize, u8, Field)] = &[
        (10, b'X', Field::Separator(10)),
        (62, b'X', Field::Separator(62)),
        (120, b'X', Field::Separator(120)),
        (3, b'_', Field::OasDash),
        (1, b'a', Field::OasCode),
        (6, b'Z', Field::OasNumber),
        (11, b'X', Field::Verification),
        (14, b'1', Field::Country),
        (16, b'1', Field::State),
        (20, 0x07, Field::City),
        (36, b'X', Field::Latitude),
        (49, b'X', Field::Longitude),
        (63, 0x01, Field::Type),
        (82, b'0', Field::Quantity),
        (82, b'x', Field::Quantity),
        (84, b'-', Field::Agl),
        (92, b'Q', Field::Amsl),
        (96, b'Z', Field::Lighting),
        (98, b'0', Field::HorizontalAccuracy),
        (100, b'Z', Field::VerticalAccuracy),
        (102, b'Z', Field::Marking),
        (104, 0x01, Field::Study),
        (119, b'D', Field::Action),
        (121, b'x', Field::Julian),
    ];
    for &(col, byte, want) in cases {
        let mut l = REAL.as_bytes().to_vec();
        l[col - 1] = byte;
        let mut f = HEADER.as_bytes().to_vec();
        f.extend(&l);
        f.extend(b"\r\n");
        let d = parse_dof(&f).unwrap();
        assert_eq!(len(&d), 0, "column {col}");
        assert_eq!(
            d.rejected[0],
            LineError {
                line: 5,
                kind: LineErrorKind::Field(want)
            },
            "column {col}"
        );
    }
    // Julian dates: day 000, day 366 in a common year, day 367, a year out of range.
    for (j, ok) in [
        ("2014000", false),
        ("2014365", true),
        ("2014366", false),
        ("2012366", true),
        ("2012367", false),
        ("1800001", false),
    ] {
        let mut l = REAL.to_string();
        l.replace_range(120..127, j);
        assert_eq!(len(&parse_dof(&file(&[&l])).unwrap()) == 1, ok, "{j}");
    }
    // Lengths: 126 bytes is short, 127 and 128 are fine, 1025 is long.
    let mut l = REAL.to_string();
    l.truncate(126);
    let d = parse_dof(&file(&[&l])).unwrap();
    assert_eq!(d.rejected[0].kind, LineErrorKind::TooShort(126));
    for n in [127usize, 128] {
        let mut l = REAL.to_string();
        l.truncate(n);
        assert_eq!(len(&parse_dof(&file(&[&l])).unwrap()), 1, "{n}");
    }
    let long = format!("{REAL}{}", "x".repeat(1000));
    let d = parse_dof(&file(&[&long])).unwrap();
    assert!(matches!(d.rejected[0].kind, LineErrorKind::TooLong(n) if n > MAX_LINE_BYTES));
    let msg = d.rejected[0].to_string();
    assert!(msg.contains("line 5"), "{msg}");
}

// REQ: OA-DOF-005
#[test]
fn counts_line_endings_and_header_checks() {
    // Bad lines in the middle are counted and skipped; good ones keep their order.
    let d = parse_dof(&file(&[REAL, "garbage", UNVERIFIED, "", PR])).unwrap();
    assert_eq!(len(&d), 3);
    assert_eq!(d.lines, 9);
    assert_eq!(d.rejected_total, 2);
    assert_eq!(
        d.rejected.iter().map(|e| e.line).collect::<Vec<_>>(),
        vec![6, 8]
    );
    assert_eq!(d.oas_number, vec![1307, 152, 46790]);
    // LF only, and no final newline.
    let mut lf = file(&[REAL, UNVERIFIED]);
    lf.retain(|&b| b != b'\r');
    assert_eq!(len(&parse_dof(&lf).unwrap()), 2);
    lf.pop();
    assert_eq!(len(&parse_dof(&lf).unwrap()), 2);
    // The rejected list is capped, the total is not.
    let bad: Vec<&str> = vec!["x"; MAX_REJECTED + 25];
    let d = parse_dof(&file(&bad)).unwrap();
    assert_eq!(d.rejected.len(), MAX_REJECTED);
    assert_eq!(d.rejected_total, (MAX_REJECTED + 25) as u64);
    // Header problems are fatal.
    assert_eq!(parse_dof(b""), Err(DofError::BadCurrency));
    assert_eq!(
        parse_dof(b"  CURRENCY DATE = 13/27/26\r\n"),
        Err(DofError::BadCurrency)
    );
    assert_eq!(
        parse_dof(b"  CURRENCY DATE = 02/30/26\r\n"),
        Err(DofError::BadCurrency)
    );
    assert_eq!(
        parse_dof(b"nonsense\r\na\r\nb\r\nc\r\n"),
        Err(DofError::BadCurrency)
    );
    assert_eq!(
        parse_dof(b"  CURRENCY DATE = 09/27/26\r\n"),
        Err(DofError::BadHeader)
    );
    assert_eq!(
        parse_dof(b"  CURRENCY DATE = 09/27/26\r\na\r\nb\r\n---\r\n"),
        Err(DofError::BadHeader)
    );
    // A header with no obstacles is a valid empty file.
    let d = parse_dof(&file(&[])).unwrap();
    assert_eq!((len(&d), d.lines, d.rejected_total), (0, 4, 0));
    // Two-digit year pivot.
    let c = parse_currency(b"  CURRENCY DATE = 01/02/85").unwrap();
    assert_eq!(
        c,
        Currency {
            year: 1985,
            month: 1,
            day: 2
        }
    );
}

// REQ: OA-DOF-006
#[test]
fn bounding_box_query() {
    let d = parse_dof(&file(&[REAL, UNVERIFIED, PR, SOUTH_EAST])).unwrap();
    let hits = |a, b, c, e| {
        in_bbox(&d, a, b, c, e)
            .unwrap()
            .iter()
            .map(|i| i.0)
            .collect::<Vec<_>>()
    };
    // Around Dauphin Island (30.18, -88.08).
    assert_eq!(hits(30.0, 30.5, -88.5, -87.5), vec![0]);
    // Edges are inclusive.
    assert_eq!(hits(30.1791667, 30.1791667, -88.0775, -88.0775), vec![0]);
    assert_eq!(hits(30.1791668, 31.0, -89.0, -87.0), Vec::<u32>::new());
    // The whole world.
    assert_eq!(hits(-90.0, 90.0, -180.0, 180.0), vec![0, 1, 2, 3]);
    // Southern and eastern hemispheres.
    assert_eq!(hits(-18.0, -17.0, 61.0, 62.0), vec![3]);
    // Antimeridian crossing: Kanaga Bay is at 177 deg 11 min W, the Antigua sample at 61 E.
    assert_eq!(hits(50.0, 52.0, 170.0, -170.0), vec![1]);
    assert_eq!(hits(50.0, 52.0, -170.0, 170.0), Vec::<u32>::new());
    assert_eq!(ObstacleIx::NONE.0, u32::MAX);
}

// REQ: OA-DOF-007
#[test]
fn never_panics_on_damage() {
    let full = file(&[REAL, UNVERIFIED, PR, SOUTH_EAST]);
    for cut in 0..full.len() {
        let _ = parse_dof(&full[..cut]);
    }
    let mut damaged = full.clone();
    for i in 0..damaged.len() {
        for flip in [0xffu8, 0x20, 0x00] {
            damaged[i] = flip;
            let d = parse_dof(&damaged);
            if let Ok(d) = d {
                // Whatever survives is internally consistent.
                assert_eq!(d.lat_e7.len(), d.lon_e7.len());
                assert_eq!(d.lat_e7.len(), d.julian.len());
                assert_eq!(
                    d.lines,
                    d.lat_e7.len() as u64 + d.rejected_total + HEADER_LINES
                );
            }
        }
        damaged[i] = full[i];
    }
    let mut x: u32 = 7;
    for _ in 0..300 {
        let mut junk = HEADER.as_bytes().to_vec();
        for _ in 0..400 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            junk.push((x >> 24) as u8);
        }
        let _ = parse_dof(&junk);
    }
}

// REQ: OA-DOF-009
#[test]
fn a_non_finite_box_bound_is_refused_not_read_as_zero_degrees() {
    // An obstacle at 0 N 0 E: a NaN bound read as 0 degrees would match it.
    let mut d = parse_dof(&file(&[REAL])).unwrap();
    d.lat_e7[0] = 0;
    d.lon_e7[0] = 0;
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for slot in 0..4 {
            let mut b = [-1.0, 1.0, -1.0, 1.0];
            b[slot] = bad;
            let r = in_bbox(&d, b[0], b[1], b[2], b[3]);
            assert!(r.is_err(), "bound {slot} = {bad} matched {r:?}");
        }
    }
    assert_eq!(
        in_bbox(&d, -1.0, 1.0, -1.0, 1.0).unwrap(),
        vec![ObstacleIx(0)]
    );
}

// REQ: OA-DOF-010
#[test]
fn seconds_field_edges_are_exact() {
    assert_eq!(seconds_hundredths(b"59.99"), Some(5999));
    assert_eq!(seconds_hundredths(b"60.00"), None);
    assert_eq!(seconds_hundredths(b"00.00"), Some(0));
    assert_eq!(seconds_hundredths(b"9.999"), None);
    assert_eq!(seconds_hundredths(b"99.99"), None);
    assert_eq!(seconds_hundredths(b"5x.00"), None);
    assert_eq!(seconds_hundredths(b"+5.00"), None);
}

// REQ: OA-SEAL-013
#[test]
fn read_file_refuses_a_sealed_path() {
    let p = std::path::Path::new("/x/raw/adsblol/2026.11.21/a.dat");
    let e = read_file(p).unwrap_err().to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
}
