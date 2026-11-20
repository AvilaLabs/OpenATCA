//! Unit tests over small excerpts of the real FAA files (tests/fixtures) and inline CSV.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nasr::schema::{
    self, parse_schemas, validate, ColumnSpec, DataType, FileSchema, ViolationKind,
};
use oa_nasr::{
    apt, arb, awy, cls, dms_to_decimal, fix, nav, open, position, procedure, NasrError,
    RowErrorKind, MAX_VIOLATIONS_PER_FILE,
};
use std::io::{BufReader, Cursor};
use std::path::PathBuf;

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.csv"))
}

fn fx(name: &str) -> BufReader<std::fs::File> {
    open(&path(name)).unwrap_or_else(|e| panic!("{e}"))
}

fn txt(s: &str) -> Cursor<Vec<u8>> {
    Cursor::new(s.as_bytes().to_vec())
}

/// ARB_SEG header, for inline rows.
const ARB_SEG_HEADER: &str = "\"EFF_DATE\",\"REC_ID\",\"LOCATION_ID\",\"LOCATION_NAME\",\"ALTITUDE\",\"TYPE\",\"POINT_SEQ\",\"LAT_DEG\",\"LAT_MIN\",\"LAT_SEC\",\"LAT_HEMIS\",\"LAT_DECIMAL\",\"LONG_DEG\",\"LONG_MIN\",\"LONG_SEC\",\"LONG_HEMIS\",\"LONG_DECIMAL\",\"BNDRY_PT_DESCRIP\",\"NAS_DESCRIP_FLAG\"\r\n";

fn seg_row(id: &str, seq: u32, lat_min: &str, desc: &str, flag: &str) -> String {
    format!(
        "\"2026/10/29\",\"{id}*{seq}\",\"{id}\",\"X\",\"HIGH\",\"ARTCC\",{seq},41,{lat_min},0,\"N\",41.5,93,28,0,\"W\",-93.46666666,\"{desc}\",\"{flag}\"\r\n"
    )
}

// REQ: OA-NASR-001
#[test]
fn structure_files_parse_into_schemas() {
    let s = parse_schemas("ARB_CSV_DATA_STRUCTURE.csv", fx("ARB_CSV_DATA_STRUCTURE"))
        .unwrap_or_else(|e| panic!("{e}"));
    let base = s.get("ARB_BASE").expect("ARB_BASE schema");
    assert_eq!(base.columns.len(), 20);
    let name = &base.columns[2];
    assert_eq!(name.name, "LOCATION_NAME");
    assert_eq!((name.max_len, name.nullable), (30, true));
    let lat = base
        .columns
        .iter()
        .find(|c| c.name == "LAT_DECIMAL")
        .expect("column");
    assert_eq!(
        lat.data_type,
        DataType::Number {
            precision: 10,
            scale: 8
        }
    );
    assert!(!lat.nullable);
    assert!(s.get("ARB_SEG").is_some());
    assert!(s.get("NOPE").is_none());
    assert!(s.warnings.is_empty());
}

// REQ: OA-NASR-001
#[test]
fn structure_file_whitespace_in_column_name_is_trimmed_and_reported() {
    let s = parse_schemas("FIX_CSV_DATA_STRUCTURE.csv", fx("FIX_CSV_DATA_STRUCTURE"))
        .unwrap_or_else(|e| panic!("{e}"));
    let base = s.get("FIX_BASE").expect("FIX_BASE schema");
    assert!(base.columns.iter().any(|c| c.name == "COUNTRY_CODE"));
    assert_eq!(s.warnings.len(), 1);
    assert!(s.warnings[0].contains("COUNTRY_CODE"));
}

// REQ: OA-NASR-001
#[test]
fn malformed_structure_rows_are_errors_with_line() {
    let head = "CSV File,Column Name,Max Length,Data Type,Nullable\r\n";
    for (row, needle) in [
        ("F,A,x,VARCHAR,No", "Max Length"),
        ("F,A,\"(3,5)\",NUMBER,No", "NUMBER size"),
        ("F,A,5,BLOB,No", "Data Type"),
        ("F,A,5,VARCHAR,Maybe", "Nullable"),
    ] {
        let e = parse_schemas("s.csv", txt(&format!("{head}{row}\r\n"))).expect_err("bad row");
        match e {
            NasrError::Schema {
                line,
                message,
                file,
            } => {
                assert_eq!((file.as_str(), line), ("s.csv", 2));
                assert!(message.contains(needle), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    let dup = format!("{head}F,A,5,VARCHAR,No\r\nF,A,5,VARCHAR,No\r\n");
    assert!(matches!(
        parse_schemas("s.csv", txt(&dup)),
        Err(NasrError::Schema { line: 3, .. })
    ));
    assert!(matches!(
        parse_schemas("s.csv", txt("A,B\r\n1,2\r\n")),
        Err(NasrError::Csv(_))
    ));
}

fn tiny_schema() -> FileSchema {
    let col = |name: &str, max_len, data_type, nullable| ColumnSpec {
        name: name.to_string(),
        max_len,
        data_type,
        nullable,
    };
    FileSchema {
        file: "T".to_string(),
        columns: vec![
            col("ID", 3, DataType::Varchar, false),
            col("NOTE", 5, DataType::Varchar, true),
            col(
                "LAT",
                0,
                DataType::Number {
                    precision: 5,
                    scale: 2,
                },
                true,
            ),
        ],
    }
}

// REQ: OA-NASR-001
#[test]
fn validation_collects_every_kind_with_file_line_column() {
    let data = "ID,NOTE,LAT,EXTRA\r\nabc,ok,1.5,x\r\n,toolong1,12.34,x\r\nabcd,é,1234,x\r\nab,,1.234,x\r\nab,,abc,x\r\nab,,1e2,x\r\nab,,-0.50,x\r\nab,,.5,x\r\nab,,+99.99,x\r\nab,,100,x\r\n";
    let v = validate(&tiny_schema(), "t.csv", txt(data)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.rows, 10);
    let got: Vec<(u64, &str, &ViolationKind)> = v
        .violations
        .iter()
        .map(|x| (x.line, x.column.as_str(), &x.kind))
        .collect();
    assert_eq!(
        got,
        vec![
            (1, "EXTRA", &ViolationKind::UnexpectedColumn),
            (3, "ID", &ViolationKind::NullViolation),
            (3, "NOTE", &ViolationKind::TooLong { max: 5, len: 8 }),
            (4, "ID", &ViolationKind::TooLong { max: 3, len: 4 }),
            (
                4,
                "LAT",
                &ViolationKind::TooManyIntegerDigits { max: 3, digits: 4 }
            ),
            (
                5,
                "LAT",
                &ViolationKind::TooManyFractionDigits { max: 2, digits: 3 }
            ),
            (6, "LAT", &ViolationKind::NotNumeric("abc".to_string())),
            (7, "LAT", &ViolationKind::NotNumeric("1e2".to_string())),
        ]
    );
    assert!(v.violations.iter().all(|x| x.file == "T"));
    assert_eq!(schema::total_violations(&v), 8);
}

// REQ: OA-NASR-001
#[test]
fn validation_reports_missing_column_and_counts_chars_not_bytes() {
    let v = validate(&tiny_schema(), "t.csv", txt("ID,NOTE\r\nabc,ééééé\r\n"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.violations.len(), 1);
    assert_eq!(v.violations[0].kind, ViolationKind::MissingColumn);
    assert_eq!(v.violations[0].column, "LAT");
    assert_eq!(v.violations[0].line, 1);
}

// REQ: OA-NASR-001
#[test]
fn clean_fixtures_validate_against_the_real_schema() {
    let s = parse_schemas("ARB", fx("ARB_CSV_DATA_STRUCTURE")).unwrap_or_else(|e| panic!("{e}"));
    for f in ["ARB_BASE", "ARB_SEG"] {
        let v = validate(s.get(f).expect("schema"), &format!("{f}.csv"), fx(f))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(v.rows > 0);
        assert!(v.violations.is_empty(), "{f}: {:?}", v.violations);
    }
}

// REQ: OA-NASR-001
#[test]
fn real_schema_violation_is_found_in_fixture() {
    // Two real FIX_BASE rows have an empty ARTCC_ID_LOW though the schema says it is required.
    let s = parse_schemas("FIX", fx("FIX_CSV_DATA_STRUCTURE")).unwrap_or_else(|e| panic!("{e}"));
    let v = validate(
        s.get("FIX_BASE").expect("schema"),
        "FIX_BASE.csv",
        fx("FIX_BASE"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.rows, 6);
    let got: Vec<_> = v
        .violations
        .iter()
        .map(|x| (x.line, x.column.as_str()))
        .collect();
    assert_eq!(got, vec![(6, "ARTCC_ID_LOW"), (7, "ARTCC_ID_LOW")]);
    assert!(v
        .violations
        .iter()
        .all(|x| x.kind == ViolationKind::NullViolation));
}

// REQ: OA-NASR-001, OA-GEN-003
#[test]
fn violation_storage_is_capped_but_counted() {
    let mut data = String::from("ID\r\n");
    for _ in 0..MAX_VIOLATIONS_PER_FILE + 7 {
        data.push_str("\r\n");
    }
    let schema = FileSchema {
        file: "T".to_string(),
        columns: vec![ColumnSpec {
            name: "ID".to_string(),
            max_len: 3,
            data_type: DataType::Varchar,
            nullable: false,
        }],
    };
    let v = validate(&schema, "t.csv", txt(&data)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.violations.len(), MAX_VIOLATIONS_PER_FILE);
    assert_eq!(v.dropped, 7);
    assert_eq!(
        schema::total_violations(&v),
        MAX_VIOLATIONS_PER_FILE as u64 + 7
    );
}

// REQ: OA-NASR-001, OA-GEN-003
#[test]
fn csv_limits_surface_as_errors_naming_source_line_and_limit() {
    let big = "x".repeat(oa_csv::MAX_FIELD_BYTES + 1);
    let data = format!("ID,NOTE,LAT\r\nabc,,1\r\nabc,{big},1\r\n");
    let e = validate(&tiny_schema(), "big.csv", txt(&data)).expect_err("limit");
    let NasrError::Csv(c) = &e else {
        panic!("{e:?}")
    };
    assert_eq!((c.source.as_str(), c.line), ("big.csv", 3));
    assert!(e.to_string().contains("MAX_FIELD_BYTES"));
    // A field-count mismatch stops the parse with its line number.
    let e = fix::parse_fixes("f.csv", txt("A,B\r\n1\r\n")).expect_err("count");
    assert!(e.to_string().starts_with("f.csv:2:"));
}

// REQ: OA-NASR-001
#[test]
fn missing_data_column_is_a_fatal_error_naming_it() {
    let e =
        fix::parse_fixes("FIX_BASE.csv", txt("EFF_DATE\r\n2026/10/29\r\n")).expect_err("column");
    assert!(e.to_string().contains("FIX_ID"), "{e}");
}

// REQ: OA-NASR-003
#[test]
fn dms_conversion_signs_and_ranges() {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
    assert!(close(
        dms_to_decimal(42.0, 7.0, 12.68, "N", true).expect("ok"),
        42.0 + 7.0 / 60.0 + 12.68 / 3600.0
    ));
    assert!(dms_to_decimal(20.0, 26.0, 0.0, "S", true).expect("ok") < 0.0);
    assert!(close(
        dms_to_decimal(71.0, 8.0, 30.34, "W", false).expect("ok"),
        -(71.0 + 8.0 / 60.0 + 30.34 / 3600.0)
    ));
    assert_eq!(dms_to_decimal(90.0, 0.0, 0.0, "N", true), Ok(90.0));
    assert_eq!(dms_to_decimal(180.0, 0.0, 0.0, "E", false), Ok(180.0));
    for bad in [
        dms_to_decimal(91.0, 0.0, 0.0, "N", true),
        dms_to_decimal(90.0, 1.0, 0.0, "N", true),
        dms_to_decimal(181.0, 0.0, 0.0, "E", false),
        dms_to_decimal(10.0, 60.0, 0.0, "N", true),
        dms_to_decimal(10.0, 0.0, 60.0, "N", true),
        dms_to_decimal(10.0, -1.0, 0.0, "N", true),
        dms_to_decimal(10.0, 0.0, 0.0, "E", true),
        dms_to_decimal(10.0, 0.0, 0.0, "N", false),
        dms_to_decimal(10.0, 0.0, 0.0, "", true),
        dms_to_decimal(f64::NAN, 0.0, 0.0, "N", true),
        dms_to_decimal(10.0, f64::INFINITY, 0.0, "N", true),
    ] {
        assert!(bad.is_err(), "{bad:?}");
    }
}

// REQ: OA-NASR-003
#[test]
fn fix_positions_match_published_decimals_to_rounding() {
    let p = fix::parse_fixes("FIX_BASE.csv", fx("FIX_BASE")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 6);
    assert!(p.rejected.is_empty());
    let f = &p.records[0];
    assert_eq!(
        (f.fix_id.as_str(), f.icao_region_code.as_str()),
        ("AAALL", "K6")
    );
    assert_eq!(f.fix_use_code, "WP", "blank padding trimmed");
    assert_eq!(f.line, 2);
    assert!((f.position.lat - 42.120_188_888_888).abs() < 1e-9);
    assert!((f.position.lon + 71.141_761_111_111).abs() < 1e-9);
    assert_eq!(f.position.published_lat, 42.12018888);
    assert!(
        position::max_diff(&f.position) < 1e-8,
        "published decimals are truncated to 8 places"
    );
    assert_eq!(f.artcc_id_low.as_deref(), Some("ZBW"));
    // Empty ARTCC fields stay None; the schema validator reports the violation.
    let last = &p.records[5];
    assert_eq!(last.fix_id, "JEXAT");
    assert_eq!(
        (last.artcc_id_high.clone(), last.artcc_id_low.clone()),
        (None, None)
    );
    assert_eq!(last.country_code, "CA");
    assert!(f.charts.as_deref().is_some_and(|c| c.contains("IAP")));
    assert!(!f.pitch_flag);
}

// REQ: OA-NASR-003
#[test]
fn fix_nav_rows_keep_raw_identity_and_optional_numbers() {
    let p = fix::parse_fix_navs("FIX_NAV.csv", fx("FIX_NAV")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 4);
    let a = &p.records[0];
    assert_eq!(
        (a.fix_id.as_str(), a.nav_id.as_str(), a.nav_type.as_str()),
        ("AABEE", "PDK", "LOC")
    );
    assert_eq!(a.bearing, Some(25.51));
    assert_eq!(p.records[2].bearing, Some(348.0));
    assert_eq!(p.records[3].distance, None);
}

// REQ: OA-NASR-003
#[test]
fn bad_position_rows_are_rejected_with_line_and_column() {
    let head = "\"EFF_DATE\",\"FIX_ID\",\"ICAO_REGION_CODE\",\"STATE_CODE\",\"COUNTRY_CODE\",\"LAT_DEG\",\"LAT_MIN\",\"LAT_SEC\",\"LAT_HEMIS\",\"LAT_DECIMAL\",\"LONG_DEG\",\"LONG_MIN\",\"LONG_SEC\",\"LONG_HEMIS\",\"LONG_DECIMAL\",\"FIX_ID_OLD\",\"CHARTING_REMARK\",\"FIX_USE_CODE\",\"ARTCC_ID_HIGH\",\"ARTCC_ID_LOW\",\"PITCH_FLAG\",\"CATCH_FLAG\",\"SUA_ATCAA_FLAG\",\"MIN_RECEP_ALT\",\"COMPULSORY\",\"CHARTS\"\r\n";
    let row = |lat_min: &str, hemis: &str, lat_dec: &str, flag: &str| {
        format!("\"2026/10/29\",\"ZZZZZ\",\"K6\",\"MA\",\"US\",42,{lat_min},12.68,\"{hemis}\",{lat_dec},71,8,30.34,\"W\",-71.14176111,\"\",\"\",\"WP\",\"ZBW\",\"ZBW\",\"{flag}\",\"N\",\"N\",,\"\",\"\"\r\n")
    };
    let data = format!(
        "{head}{}{}{}{}{}{}",
        row("7", "N", "42.12018888", "N"),
        row("60", "N", "42.12018888", "N"),
        row("7", "X", "42.12018888", "N"),
        row("7", "N", "abc", "N"),
        row("7", "N", "142.1", "N"),
        row("7", "N", "42.12018888", "Q"),
    );
    let p = fix::parse_fixes("FIX_BASE.csv", txt(&data)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 1);
    let got: Vec<(u64, &str, &str)> = p
        .rejected
        .iter()
        .map(|e| {
            let k = match &e.kind {
                RowErrorKind::InvalidPosition(_) => "position",
                RowErrorKind::NotANumber(_) => "number",
                RowErrorKind::BadFlag(_) => "flag",
                other => panic!("{other:?}"),
            };
            (e.line, e.column.as_str(), k)
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (3, "LAT_DEG", "position"),
            (4, "LAT_DEG", "position"),
            (5, "LAT_DECIMAL", "number"),
            (6, "LAT_DECIMAL", "position"),
            (7, "PITCH_FLAG", "flag"),
        ]
    );
    assert!(p.rejected[0]
        .to_string()
        .starts_with("FIX_BASE.csv:3: column LAT_DEG"));
}

// REQ: OA-NASR-003
#[test]
fn navaids_record_the_real_dms_decimal_disagreement() {
    let p = nav::parse_navaids("NAV_BASE.csv", fx("NAV_BASE")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 3);
    assert!(p.rejected.is_empty());
    let tdd = p.records.iter().find(|n| n.nav_id == "TDD").expect("TDD");
    // In the FAA data TDD's decimal longitude is the DME antenna's, 5 seconds from its DMS.
    assert!(tdd.position.lon_diff > 1.2e-3 && tdd.position.lon_diff < 1.3e-3);
    assert!(tdd.position.lat_diff > 1.2e-5 && tdd.position.lat_diff < 1.4e-5);
    let dme = tdd.tacan_dme_position.expect("TACAN/DME position");
    assert!(position::max_diff(&dme) < 1e-7);
    assert_eq!(tdd.nav_type, "VOR/DME");
    assert_eq!(tdd.high_alt_artcc_id.as_deref(), Some("ZDV"));
    assert_eq!(tdd.elev, Some(3176.7));
    assert_eq!(tdd.mag_varn_hemis.as_deref(), Some("E"));
    let dpa = p.records.iter().find(|n| n.nav_id == "DPA").expect("DPA");
    assert!(position::max_diff(&dpa.position) < 1e-7);
    assert!(dpa.nas_use_flag);
    let ndb = p.records.iter().find(|n| n.nav_type == "NDB").expect("NDB");
    assert!(ndb.tacan_dme_position.is_none());
}

// REQ: OA-NASR-003
#[test]
fn airport_runways_and_ends_join_by_site_and_runway() {
    let a = apt::parse_airports("APT_BASE.csv", fx("APT_BASE")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(a.records.len(), 1);
    let ord = &a.records[0];
    assert_eq!(
        (
            ord.arpt_id.as_str(),
            ord.site_no.as_str(),
            ord.resp_artcc_id.as_str()
        ),
        ("ORD", "04508.", "ZAU")
    );
    assert_eq!(ord.elev, 680.0);
    assert!(position::max_diff(&ord.position) < 1e-7);

    let r = apt::parse_runways("APT_RWY.csv", fx("APT_RWY")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.records.len(), 10);
    assert!(r.rejected.is_empty());
    assert!(r
        .records
        .iter()
        .all(|x| x.site_no == ord.site_no && x.arpt_id == "ORD"));
    let big = r
        .records
        .iter()
        .find(|x| x.rwy_id == "10L/28R")
        .expect("runway");
    assert_eq!((big.rwy_len, big.rwy_width), (13000.0, 150.0));
    assert_eq!(big.surface_type_code.as_deref(), Some("ASPH-CONC"));

    let e = apt::parse_runway_ends("APT_RWY_END.csv", fx("APT_RWY_END"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(e.rejected.is_empty(), "{:?}", e.rejected);
    let ord_ends: Vec<_> = e
        .records
        .iter()
        .filter(|x| x.site_no == ord.site_no)
        .collect();
    assert!(ord_ends.len() >= 16);
    for end in &ord_ends {
        assert!(
            r.records.iter().any(|x| x.rwy_id == end.rwy_id),
            "{}",
            end.rwy_id
        );
    }
    let e10l = ord_ends
        .iter()
        .find(|x| x.rwy_end_id == "10L")
        .expect("10L");
    let pos = e10l.position.expect("threshold position");
    assert!(position::max_diff(&pos) < 1e-7);
    assert!(e10l.true_alignment.is_some());
    // Ends with no published coordinates have no position rather than an error.
    let none = e
        .records
        .iter()
        .find(|x| x.arpt_id == "AL03")
        .expect("AL03");
    assert!(none.position.is_none());
    // At least one end in the fixture has a displaced threshold.
    let disp = e
        .records
        .iter()
        .find_map(|x| x.displaced_thr_position)
        .expect("displaced threshold");
    assert!(position::max_diff(&disp) < 1e-7);
}

// REQ: OA-NASR-003
#[test]
fn partial_position_is_rejected() {
    let head = "\"EFF_DATE\",\"SITE_NO\",\"ARPT_ID\",\"RWY_ID\",\"RWY_END_ID\",\"RWY_END_LAT_DEG\",\"RWY_END_LAT_MIN\",\"RWY_END_LAT_SEC\",\"RWY_END_LAT_HEMIS\",\"LAT_DECIMAL\",\"RWY_END_LONG_DEG\",\"RWY_END_LONG_MIN\",\"RWY_END_LONG_SEC\",\"RWY_END_LONG_HEMIS\",\"LONG_DECIMAL\",\"TRUE_ALIGNMENT\",\"ILS_TYPE\",\"RWY_END_ELEV\",\"THR_CROSSING_HGT\",\"VISUAL_GLIDE_PATH_ANGLE\",\"DISPLACED_THR_LAT_DEG\",\"DISPLACED_THR_LAT_MIN\",\"DISPLACED_THR_LAT_SEC\",\"DISPLACED_THR_LAT_HEMIS\",\"LAT_DISPLACED_THR_DECIMAL\",\"DISPLACED_THR_LONG_DEG\",\"DISPLACED_THR_LONG_MIN\",\"DISPLACED_THR_LONG_SEC\",\"DISPLACED_THR_LONG_HEMIS\",\"LONG_DISPLACED_THR_DECIMAL\",\"DISPLACED_THR_ELEV\",\"DISPLACED_THR_LEN\",\"TDZ_ELEV\",\"TKOF_RUN_AVBL\",\"TKOF_DIST_AVBL\",\"ACLT_STOP_DIST_AVBL\",\"LNDG_DIST_AVBL\"\r\n";
    let rest = ",,,,,,,,,,,,,,,,,,,,,,";
    let ok = format!("\"d\",\"1\",\"A\",\"09/27\",\"09\",32,55,19.8713,\"N\",32.92218647,85,57,48.0006,\"W\",-85.9633335{rest}\r\n");
    let partial = format!("\"d\",\"1\",\"A\",\"09/27\",\"27\",32,55,,\"N\",32.92218647,85,57,48.0006,\"W\",-85.9633335{rest}\r\n");
    let p = apt::parse_runway_ends("e.csv", txt(&format!("{head}{ok}{partial}")))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 1);
    assert_eq!(p.rejected.len(), 1);
    assert_eq!(
        (p.rejected[0].line, p.rejected[0].kind.clone()),
        (3, RowErrorKind::PartialPosition)
    );
    // A missing component inside a complete-looking position is a missing value.
}

// REQ: OA-NASR-002
#[test]
fn arb_base_has_zau_with_computed_position() {
    let p = arb::parse_base("ARB_BASE.csv", fx("ARB_BASE")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 2);
    let zau = &p.records[0];
    assert_eq!(zau.location_id, "ZAU");
    assert_eq!(zau.location_type, "ARTCC");
    assert_eq!(zau.icao_id.as_deref(), Some("KZAU"));
    assert_eq!(
        zau.cross_ref.as_deref(),
        Some("FACILITY LOCATED AT AURORA, IL")
    );
    assert!((zau.position.lat - (41.0 + 47.0 / 60.0 + 22.11 / 3600.0)).abs() < 1e-12);
    assert!((zau.position.lon + (87.0 + 45.0 / 60.0 + 42.19 / 3600.0)).abs() < 1e-12);
    assert!(position::max_diff(&zau.position) < 1e-8);
}

// REQ: OA-NASR-002
#[test]
fn zau_has_high_and_low_boundaries_in_point_order() {
    let p = arb::parse_boundaries("ARB_SEG.csv", fx("ARB_SEG")).unwrap_or_else(|e| panic!("{e}"));
    assert!(p.rejected.is_empty());
    assert_eq!(p.records.len(), 2);
    let keys: Vec<_> = p
        .records
        .iter()
        .map(|b| {
            (
                b.location_id.as_str(),
                b.altitude.as_str(),
                b.boundary_type.as_str(),
            )
        })
        .collect();
    assert_eq!(
        keys,
        vec![("ZAU", "HIGH", "ARTCC"), ("ZAU", "LOW", "ARTCC")]
    );
    assert_eq!(
        (p.records[0].points.len(), p.records[1].points.len()),
        (31, 34)
    );
    for b in &p.records {
        assert!(b.points.windows(2).all(|w| w[0].point_seq < w[1].point_seq));
        assert!(b
            .points
            .iter()
            .all(|pt| position::max_diff(&pt.position) < 1e-7));
    }
    let high = &p.records[0];
    assert_eq!(high.points[0].point_seq, 10);
    assert_eq!(high.points[0].rec_id, "ZAU*H*54915");
    assert_eq!(high.points[0].description.as_deref(), Some("TO"));
    assert!((high.points[0].position.lat - (41.0 + 40.0 / 60.0)).abs() < 1e-12);
    assert!(high.points[0].position.lon < 0.0);
    let low = &p.records[1];
    assert_eq!(
        low.points.last().and_then(|x| x.description.as_deref()),
        Some("TO POINT OF BEGINNING")
    );
    assert!(low.points.iter().any(|x| x
        .description
        .as_deref()
        .is_some_and(|d| d.contains("/COMMON ZAU-ZKC-ZMP/"))));
}

// REQ: OA-NASR-002
#[test]
fn boundary_points_are_sorted_grouped_and_duplicates_rejected() {
    let mut d = String::from(ARB_SEG_HEADER);
    d.push_str(&seg_row("ZBB", 30, "30", "TO", ""));
    d.push_str(&seg_row("ZAA", 20, "20", "TO", "X"));
    d.push_str(&seg_row("ZBB", 10, "10", "TO", ""));
    d.push_str(&seg_row("ZAA", 10, "10", "TO", ""));
    d.push_str(&seg_row("ZAA", 20, "21", "TO", ""));
    d.push_str(&seg_row("ZBB", 20, "20", "TO POINT OF BEGINNING", ""));
    let p = arb::parse_boundaries("seg.csv", txt(&d)).unwrap_or_else(|e| panic!("{e}"));
    let seqs: Vec<(String, Vec<u32>)> = p
        .records
        .iter()
        .map(|b| {
            (
                b.location_id.clone(),
                b.points.iter().map(|x| x.point_seq).collect(),
            )
        })
        .collect();
    assert_eq!(
        seqs,
        vec![
            ("ZAA".to_string(), vec![10, 20]),
            ("ZBB".to_string(), vec![10, 20, 30])
        ]
    );
    assert_eq!(p.rejected.len(), 1);
    assert_eq!(
        (p.rejected[0].line, p.rejected[0].column.as_str()),
        (6, "POINT_SEQ")
    );
    assert!(matches!(p.rejected[0].kind, RowErrorKind::DuplicateKey(_)));
    // The first of the duplicates is the one kept.
    assert!(p.records[0].points[1].nas_description_only);
    // A bad NAS flag is a rejected row.
    let bad = format!("{ARB_SEG_HEADER}{}", seg_row("ZAA", 10, "10", "TO", "Z"));
    let p = arb::parse_boundaries("seg.csv", txt(&bad)).unwrap_or_else(|e| panic!("{e}"));
    assert!(p.records.is_empty());
    assert!(matches!(p.rejected[0].kind, RowErrorKind::BadFlag(_)));
}

// REQ: OA-NASR-004
#[test]
fn airways_have_ordered_segments_with_mea_and_gap_flags() {
    let a = awy::parse_airways(
        "AWY_BASE.csv",
        fx("AWY_BASE"),
        "AWY_SEG_ALT.csv",
        fx("AWY_SEG_ALT"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(a.rejected.is_empty(), "{:?}", a.rejected);
    assert_eq!(a.records.len(), 2);
    let j80 = a
        .records
        .iter()
        .find(|x| x.base.awy_id == "J80")
        .expect("J80");
    assert_eq!(j80.base.awy_designation, "J");
    assert_eq!(j80.base.awy_location, "C");
    assert!(j80.base.regulatory);
    assert!(j80.base.airway_string.starts_with("OAL ILC MLF"));
    assert_eq!(j80.segments.len(), 28);
    assert!(j80
        .segments
        .windows(2)
        .all(|w| w[0].point_seq < w[1].point_seq));
    let first = &j80.segments[0];
    assert_eq!(
        (first.from_point.as_str(), first.to_point.as_deref()),
        ("OAL", Some("ILC"))
    );
    assert_eq!(first.min_enroute_alt.alt, Some(18000));
    assert_eq!(first.max_auth_alt, Some(45000));
    assert_eq!(first.mag_course, Some(66.67));
    assert_eq!(first.chgovr_pt_dist, Some(44));
    assert!(!first.awy_seg_gap_flag && !first.signal_gap_flag);
    assert_eq!(first.from_pt_type.as_deref(), Some("VORTAC"));
    assert!(first.dogleg == j80.segments[0].dogleg);
    // Each segment ends where the next begins on an airway without gaps.
    for w in j80.segments.windows(2) {
        assert_eq!(w[0].to_point.as_deref(), Some(w[1].from_point.as_str()));
    }
    let j10 = a
        .records
        .iter()
        .find(|x| x.base.awy_id == "J10")
        .expect("J10");
    let gap = j10
        .segments
        .iter()
        .find(|s| s.awy_seg_gap_flag)
        .expect("gap segment");
    assert_eq!(gap.from_point, "LBF");
    assert_eq!(gap.min_enroute_alt.alt, None);
}

// REQ: OA-NASR-004
#[test]
fn airway_segments_are_sorted_and_orphans_and_duplicates_rejected() {
    let base = std::fs::read_to_string(path("AWY_BASE")).expect("fixture");
    let segs = std::fs::read_to_string(path("AWY_SEG_ALT")).expect("fixture");
    let mut lines: Vec<&str> = segs.lines().collect();
    let header = lines.remove(0);
    lines.reverse();
    let mut shuffled = format!("{header}\r\n");
    for l in &lines {
        shuffled.push_str(l);
        shuffled.push_str("\r\n");
    }
    // Orphan: an airway that is not in the base file; duplicate: repeat J80 seq 10.
    let orphan = lines
        .iter()
        .find(|l| l.contains("\"J80\",10,"))
        .expect("J80 10")
        .replace("\"J80\"", "\"J999\"");
    let dup = lines
        .iter()
        .find(|l| l.contains("\"J80\",10,"))
        .expect("J80 10")
        .to_string();
    shuffled.push_str(&format!("{orphan}\r\n{dup}\r\n"));
    let a = awy::parse_airways("b.csv", txt(&base), "s.csv", txt(&shuffled))
        .unwrap_or_else(|e| panic!("{e}"));
    let j80 = a
        .records
        .iter()
        .find(|x| x.base.awy_id == "J80")
        .expect("J80");
    assert_eq!(j80.segments.len(), 28);
    assert!(j80
        .segments
        .windows(2)
        .all(|w| w[0].point_seq < w[1].point_seq));
    let kinds: Vec<_> = a
        .rejected
        .iter()
        .map(|e| (e.file.as_str(), &e.kind))
        .collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(kinds
        .iter()
        .any(|(f, k)| *f == "s.csv" && matches!(k, RowErrorKind::OrphanRecord(_))));
    assert!(kinds
        .iter()
        .any(|(f, k)| *f == "s.csv" && matches!(k, RowErrorKind::DuplicateKey(_))));
    // Airways stay in base file order.
    let ids: Vec<_> = a.records.iter().map(|x| x.base.awy_id.as_str()).collect();
    assert_eq!(ids, vec!["J10", "J80"]);
}

// REQ: OA-NASR-005
#[test]
fn star_has_body_and_transitions_with_runway_associations() {
    use procedure::{ProcedureKind, RoutePortion};
    let p = procedure::parse_procedures(
        ProcedureKind::Star,
        ("STAR_BASE.csv", fx("STAR_BASE")),
        ("STAR_APT.csv", fx("STAR_APT")),
        ("STAR_RTE.csv", fx("STAR_RTE")),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(p.rejected.is_empty(), "{:?}", p.rejected);
    assert_eq!(p.records.len(), 1);
    let s = &p.records[0];
    assert_eq!(s.kind, ProcedureKind::Star);
    assert_eq!(s.base.computer_code, "BENKY.BENKY6");
    assert_eq!(
        (s.base.name.as_str(), s.base.amendment_no.as_str()),
        ("BENKY", "SIX")
    );
    assert!(s.base.rnav_flag);
    assert_eq!(
        procedure::served_airports(&s.base).collect::<Vec<_>>(),
        vec!["ORD"]
    );
    assert_eq!(s.base.graphical_dp_type, None);
    assert_eq!(s.airports.len(), 16);
    assert!(s.airports.iter().all(|a| a.arpt_id == "ORD"));
    assert!(s
        .airports
        .iter()
        .any(|a| a.rwy_end_id.as_deref() == Some("27R") && a.body_name == "BENKY-HIMGO"));
    // Routes: bodies first, then transitions, each in point order.
    let kinds: Vec<_> = s.routes.iter().map(|r| r.portion).collect();
    let first_transition = kinds
        .iter()
        .position(|k| *k == RoutePortion::Transition)
        .expect("transition");
    assert!(
        first_transition > 0
            && kinds[first_transition..]
                .iter()
                .all(|k| *k == RoutePortion::Transition)
    );
    for r in &s.routes {
        assert!(r.points.windows(2).all(|w| w[0].point_seq < w[1].point_seq));
    }
    let loamy = s
        .routes
        .iter()
        .find(|r| r.route_name == "LOAMY TRANSITION")
        .expect("transition");
    assert_eq!(
        loamy.transition_computer_code.as_deref(),
        Some("LOAMY.BENKY6")
    );
    let pts: Vec<_> = loamy.points.iter().map(|p| p.point.as_str()).collect();
    assert_eq!(pts.last(), Some(&"BENKY"));
    assert_eq!(loamy.points.last().and_then(|p| p.next_point.clone()), None);
    let body = &s.routes[0];
    assert_eq!(body.portion, RoutePortion::Body);
    assert_eq!(body.transition_computer_code, None);
    assert_eq!(body.points[0].point, "BENKY");
    assert_eq!(body.points[0].point_type, "RP");
    assert_eq!(
        procedure::associations(&body.points[0]),
        vec![("ORD", Some("27R"))]
    );
}

// REQ: OA-NASR-005
#[test]
fn dp_unassigned_codes_join_by_name_and_artcc() {
    use procedure::ProcedureKind;
    let p = procedure::parse_procedures(
        ProcedureKind::Dp,
        ("DP_BASE.csv", fx("DP_BASE")),
        ("DP_APT.csv", fx("DP_APT")),
        ("DP_RTE.csv", fx("DP_RTE")),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(p.rejected.is_empty(), "{:?}", p.rejected);
    assert_eq!(p.records.len(), 3);
    let accra = &p.records[0];
    assert_eq!(accra.base.computer_code, "ACCRA5.ACCRA");
    assert_eq!(accra.base.graphical_dp_type.as_deref(), Some("SID"));
    assert_eq!(procedure::served_airports(&accra.base).count(), 9);
    assert!(!accra.airports.is_empty());
    assert_eq!(accra.routes[0].points.len(), 6);
    let last = accra.routes[0].points.last().expect("point");
    assert!(procedure::associations(last).contains(&("57C", Some("08"))));
    assert!(procedure::associations(last).contains(&("ENW", None)));
    let ohare = &p.records[1];
    assert_eq!(ohare.base.name, "O'HARE");
    assert_eq!(ohare.base.computer_code, procedure::UNASSIGNED_CODE);
    assert!(ohare.routes.is_empty() && ohare.airports.is_empty());
    let whiteman = &p.records[2];
    assert_eq!(whiteman.base.graphical_dp_type.as_deref(), Some("OBSTACLE"));
    assert!(!whiteman.airports.is_empty());
    assert_eq!(whiteman.routes.len(), 1);
    assert_eq!(whiteman.routes[0].points[0].point, "VNY");
    // Rows for a procedure that is not in the base file are orphans.
    let apt = std::fs::read_to_string(path("DP_APT")).expect("fixture");
    let base_only = "\"EFF_DATE\",\"DP_NAME\",\"AMENDMENT_NO\",\"ARTCC\",\"DP_AMEND_EFF_DATE\",\"RNAV_FLAG\",\"DP_COMPUTER_CODE\",\"GRAPHICAL_DP_TYPE\",\"SERVED_ARPT\"\r\n";
    let q = procedure::parse_procedures(
        ProcedureKind::Dp,
        ("b.csv", txt(base_only)),
        ("a.csv", txt(&apt)),
        ("r.csv", fx("DP_RTE")),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(q.records.is_empty());
    assert_eq!(q.rejected.len(), 26 + 7);
    assert!(q
        .rejected
        .iter()
        .all(|e| matches!(e.kind, RowErrorKind::OrphanRecord(_))));
}

// REQ: OA-NASR-005
#[test]
fn route_portion_must_be_known_and_duplicate_points_are_rejected() {
    use procedure::ProcedureKind;
    let base = "\"EFF_DATE\",\"ARRIVAL_NAME\",\"AMENDMENT_NO\",\"ARTCC\",\"STAR_AMEND_EFF_DATE\",\"RNAV_FLAG\",\"STAR_COMPUTER_CODE\",\"SERVED_ARPT\"\r\n\"d\",\"N\",\"ONE\",\"ZAU\",\"d\",\"Y\",\"N.N1\",\"ORD\"\r\n\"d\",\"N\",\"ONE\",\"ZAU\",\"d\",\"Y\",\"N.N1\",\"ORD\"\r\n";
    let apt = "\"EFF_DATE\",\"STAR_COMPUTER_CODE\",\"ARTCC\",\"BODY_NAME\",\"BODY_SEQ\",\"ARPT_ID\",\"RWY_END_ID\"\r\n";
    let rte_head = "\"EFF_DATE\",\"STAR_COMPUTER_CODE\",\"ARTCC\",\"ROUTE_PORTION_TYPE\",\"ROUTE_NAME\",\"BODY_SEQ\",\"TRANSITION_COMPUTER_CODE\",\"POINT_SEQ\",\"POINT\",\"ICAO_REGION_CODE\",\"POINT_TYPE\",\"NEXT_POINT\",\"ARPT_RWY_ASSOC\"\r\n";
    let row = |portion: &str, seq: u32, pt: &str| {
        format!("\"d\",\"N.N1\",\"ZAU\",\"{portion}\",\"R\",1,\"\",{seq},\"{pt}\",\"K5\",\"WP\",\"\",\"\"\r\n")
    };
    let rte = format!(
        "{rte_head}{}{}{}{}",
        row("BODY", 20, "BBB"),
        row("BODY", 10, "AAA"),
        row("BODY", 10, "DUP"),
        row("MIDDLE", 10, "ZZZ")
    );
    let p = procedure::parse_procedures(
        ProcedureKind::Star,
        ("b", txt(base)),
        ("a", txt(apt)),
        ("r", txt(&rte)),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.records.len(), 1);
    let pts: Vec<_> = p.records[0].routes[0]
        .points
        .iter()
        .map(|x| x.point.as_str())
        .collect();
    assert_eq!(pts, vec!["AAA", "BBB"]);
    let kinds: Vec<_> = p
        .rejected
        .iter()
        .map(|e| (e.file.as_str(), e.line, &e.kind))
        .collect();
    assert!(kinds
        .iter()
        .any(|(f, l, k)| *f == "b" && *l == 3 && matches!(k, RowErrorKind::DuplicateKey(_))));
    assert!(kinds
        .iter()
        .any(|(f, l, k)| *f == "r" && *l == 5 && matches!(k, RowErrorKind::UnknownValue(_))));
    assert!(kinds
        .iter()
        .any(|(f, l, k)| *f == "r" && *l == 4 && matches!(k, RowErrorKind::DuplicateKey(_))));
    assert_eq!(kinds.len(), 3);
}

// REQ: OA-NASR-006
#[test]
fn class_airspace_flags_per_airport() {
    let p =
        cls::parse_class_airspace("CLS_ARSP.csv", fx("CLS_ARSP")).unwrap_or_else(|e| panic!("{e}"));
    assert!(p.rejected.is_empty());
    assert_eq!(p.records.len(), 3);
    let get = |id: &str| p.records.iter().find(|c| c.arpt_id == id).expect("airport");
    let ord = get("ORD");
    assert_eq!(ord.site_no, "04508.");
    assert_eq!(
        (ord.class_b, ord.class_c, ord.class_d, ord.class_e),
        (true, false, false, false)
    );
    assert_eq!((get("MDW").class_c, get("MDW").class_b), (true, false));
    assert!(get("DPA").class_d);
    assert_eq!(ord.airspace_hrs, None);
    // A flag other than Y or empty is a rejected row.
    let bad = "\"EFF_DATE\",\"SITE_NO\",\"SITE_TYPE_CODE\",\"STATE_CODE\",\"ARPT_ID\",\"CITY\",\"COUNTRY_CODE\",\"CLASS_B_AIRSPACE\",\"CLASS_C_AIRSPACE\",\"CLASS_D_AIRSPACE\",\"CLASS_E_AIRSPACE\",\"AIRSPACE_HRS\",\"REMARK\"\r\n\"d\",\"1\",\"A\",\"IL\",\"XYZ\",\"C\",\"US\",\"N\",\"\",\"\",\"\",\"\",\"\"\r\n";
    let p = cls::parse_class_airspace("c.csv", txt(bad)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(p.rejected.len(), 1);
    assert_eq!(
        (p.rejected[0].line, p.rejected[0].column.as_str()),
        (2, "CLASS_B_AIRSPACE")
    );
}

// REQ: OA-GEN-001
#[test]
fn every_source_file_forbids_unsafe_code() {
    let lib = include_str!("../src/lib.rs");
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(dir).expect("src dir") {
        let p = entry.expect("entry").path();
        let src = std::fs::read_to_string(&p).expect("source");
        assert!(
            !src.contains("unsafe {") && !src.contains("unsafe fn"),
            "{p:?}"
        );
    }
}

// REQ: OA-GEN-002
#[test]
fn only_dependencies_are_the_in_workspace_csv_and_sealed_guard_crates() {
    let toml = include_str!("../Cargo.toml");
    let after = toml
        .split("[dependencies]")
        .nth(1)
        .expect("dependencies table");
    let deps: Vec<&str> = after
        .lines()
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .collect();
    // oa-sealed opens the input (OA-SEAL-013); it has no dependencies of its own.
    assert_eq!(deps.len(), 2, "{deps:?}");
    assert!(deps[0].starts_with("oa-sealed") && deps[0].contains("path ="));
    assert!(deps[1].starts_with("oa-csv") && deps[1].contains("path ="));
    assert!(!toml.contains("[dev-dependencies]") && !toml.contains("[build-dependencies]"));
}

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn run_all_parsers(bytes: &[u8]) {
    let _ = arb::parse_base("t", Cursor::new(bytes));
    let _ = arb::parse_boundaries("t", Cursor::new(bytes));
    let _ = fix::parse_fixes("t", Cursor::new(bytes));
    let _ = fix::parse_fix_navs("t", Cursor::new(bytes));
    let _ = nav::parse_navaids("t", Cursor::new(bytes));
    let _ = apt::parse_airports("t", Cursor::new(bytes));
    let _ = apt::parse_runways("t", Cursor::new(bytes));
    let _ = apt::parse_runway_ends("t", Cursor::new(bytes));
    let _ = awy::parse_airway_bases("t", Cursor::new(bytes));
    let _ = awy::parse_airway_segments("t", Cursor::new(bytes));
    let _ = cls::parse_class_airspace("t", Cursor::new(bytes));
    for kind in [procedure::ProcedureKind::Star, procedure::ProcedureKind::Dp] {
        let _ = procedure::parse_bases(kind, "t", Cursor::new(bytes));
        let _ = procedure::parse_airports(kind, "t", Cursor::new(bytes));
        let _ = procedure::parse_procedures(
            kind,
            ("b", Cursor::new(bytes)),
            ("a", Cursor::new(bytes)),
            ("r", Cursor::new(bytes)),
        );
    }
    let _ = parse_schemas("t", Cursor::new(bytes));
    let _ = validate(&tiny_schema(), "t", Cursor::new(bytes));
}

// REQ: OA-GEN-004
#[test]
fn fuzz_mutated_real_files_never_panic() {
    let names = [
        "ARB_BASE",
        "ARB_SEG",
        "FIX_BASE",
        "FIX_NAV",
        "NAV_BASE",
        "APT_BASE",
        "APT_RWY_END",
        "AWY_BASE",
        "AWY_SEG_ALT",
        "STAR_BASE",
        "STAR_RTE",
        "DP_BASE",
        "DP_APT",
        "CLS_ARSP",
        "ARB_CSV_DATA_STRUCTURE",
    ];
    let mut rng = XorShift(0x1234_5678_9ABC_DEF1);
    for name in names {
        let mut base = std::fs::read(path(name)).expect("fixture");
        base.truncate(3000);
        for _ in 0..150 {
            let mut b = base.clone();
            for _ in 0..(1 + rng.next() % 6) {
                if b.is_empty() {
                    break;
                }
                let i = (rng.next() as usize) % b.len();
                match rng.next() % 4 {
                    0 => b[i] = (rng.next() >> 16) as u8,
                    1 => {
                        b.remove(i);
                    }
                    2 => b.insert(i, b"\",\r\n.-0"[(rng.next() % 7) as usize]),
                    _ => b.truncate(i),
                }
            }
            run_all_parsers(&b);
        }
    }
}

// REQ: OA-GEN-004
#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = XorShift(0xFEED_FACE_0BAD_F00D);
    let alphabet = b"\",\r\n0123456789.-+eNSEWY ABCx";
    for _ in 0..1500 {
        let len = (rng.next() % 300) as usize;
        let b: Vec<u8> = (0..len)
            .map(|_| {
                let r = rng.next();
                if r & 7 == 0 {
                    (r >> 8) as u8
                } else {
                    alphabet[(r >> 8) as usize % alphabet.len()]
                }
            })
            .collect();
        run_all_parsers(&b);
    }
}

// REQ: OA-GEN-003
#[test]
fn rejected_rows_past_the_cap_are_counted_not_stored() {
    let header = "\"EFF_DATE\",\"SITE_NO\",\"SITE_TYPE_CODE\",\"STATE_CODE\",\"ARPT_ID\",\"CITY\",\"COUNTRY_CODE\",\"CLASS_B_AIRSPACE\",\"CLASS_C_AIRSPACE\",\"CLASS_D_AIRSPACE\",\"CLASS_E_AIRSPACE\",\"AIRSPACE_HRS\",\"REMARK\"\r\n";
    // A flag other than Y or empty rejects the row.
    let bad = "\"d\",\"1\",\"A\",\"IL\",\"XYZ\",\"C\",\"US\",\"N\",\"\",\"\",\"\",\"\",\"\"\r\n";
    let total = oa_nasr::MAX_REJECTED_PER_FILE + 50;
    let mut text = String::from(header);
    for _ in 0..total {
        text.push_str(bad);
    }
    let p = cls::parse_class_airspace("c.csv", txt(&text)).unwrap_or_else(|e| panic!("{e}"));
    assert!(p.records.is_empty());
    assert_eq!(p.rejected.len(), oa_nasr::MAX_REJECTED_PER_FILE);
    assert_eq!(p.rejected_dropped, 50);
    // The kept rows are the first ones, in file order.
    assert_eq!(p.rejected[0].line, 2);
    // A file under the cap drops nothing.
    let small = cls::parse_class_airspace("c.csv", txt(&format!("{header}{bad}{bad}")))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((small.rejected.len(), small.rejected_dropped), (2, 0));
}

// REQ: OA-SEAL-013
#[test]
fn open_refuses_a_sealed_path() {
    let e = oa_nasr::open(std::path::Path::new("/x/scds/stdds-20261119-holdout.tar"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("OA-SEAL-001"), "{e}");
}
