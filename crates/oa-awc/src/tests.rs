#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! Unit tests with hand-made fixtures. The real-data checks are in `tests/real_data.rs`.

use crate::error::Reason;
use crate::hazard::*;
use crate::metar::*;
use crate::pirep::*;
use crate::taf::*;
use crate::units::*;
use crate::*;

fn metar(id: &str, obs: i64, receipt: &str, altim: &str, raw: &str) -> String {
    format!(
        r#"{{"icaoId":"{id}","receiptTime":"{receipt}","obsTime":{obs},"temp":1.1,"dewp":-1.1,"wdir":320,"wspd":4,"visib":"10+","altim":{altim},"metarType":"METAR","rawOb":"{raw}","lat":41.98,"lon":-87.9,"elev":201,"fltCat":"MVFR"}}"#
    )
}

const T0: i64 = 1_794_657_420; // 2026-11-14T11:57:00Z
const RCPT: &str = "2026-11-14T12:00:17.091Z";

fn arr(items: &[String]) -> Vec<u8> {
    format!("[{}]", items.join(",")).into_bytes()
}

fn load_metar(files: &[Vec<u8>]) -> (MetarTable, MetarStats, DedupeReport) {
    let mut t = new_metar_table();
    let mut s = MetarStats::default();
    for f in files {
        parse_metar(f, &mut t, &mut s).unwrap();
    }
    let d = finalize_metar(&mut t);
    (t, s, d)
}

fn good_metar(id: &str, obs: i64, altim: &str, raw_alt: &str) -> String {
    metar(
        id,
        obs,
        RCPT,
        altim,
        &format!("METAR {id} 141157Z 32004KT 10SM OVC023 01/M01 {raw_alt} RMK AO2"),
    )
}

// REQ: OA-AWC-001
#[test]
fn iso_time_is_strict() {
    assert_eq!(
        parse_iso_ms("2026-11-14T18:00:00Z"),
        Some(1_794_679_200_000)
    );
    assert_eq!(
        parse_iso_ms("2026-11-14T11:57:00.091Z"),
        Some(1_794_657_420_091)
    );
    assert_eq!(
        parse_iso_ms("2026-11-14T11:57:00.1Z"),
        Some(1_794_657_420_100)
    );
    for bad in [
        "2026-11-14T18:00:00",
        "2026-11-14 18:00:00Z",
        "2026-02-30T00:00:00Z",
        "2026-11-14T24:00:00Z",
        "2026-11-14T18:00:60Z",
        "2026-11-14T18:00:00.Z",
        "2026-11-14T18:00:00.1234Z",
        "1999-12-31T23:59:59Z",
        "2026-11-14T18:00:00+00:00",
        "",
    ] {
        assert_eq!(parse_iso_ms(bad), None, "{bad}");
    }
    assert_eq!(
        pack_ident("KORD", 3).map(unpack_ident).as_deref(),
        Some("KORD")
    );
    assert!(pack_ident("kord", 3).is_none());
    assert!(pack_ident("KO", 3).is_none());
    assert!(pack_ident("KORDX", 3).is_none());
}

// REQ: OA-AWC-002
#[test]
fn metar_fields_and_units() {
    let rec = metar(
        "KORD",
        T0,
        RCPT,
        "1023.1",
        "METAR KORD 141157Z 32004KT 10SM A3021 RMK AO2",
    );
    let (t, s, _) = load_metar(&[arr(&[rec])]);
    assert_eq!((s.records, s.accepted), (1, 1));
    assert_eq!(metar_len(&t), 1);
    assert_eq!(t.altim_dhpa[0], 10231);
    assert_eq!(metar_altim_hpa(&t, 0), Some(1023.1));
    assert_eq!(t.temp_dc[0], 11);
    assert_eq!(t.dewp_dc[0], -11);
    assert_eq!(t.wdir_deg[0], 320);
    assert_eq!(t.wspd_kt[0], 4);
    assert_eq!(t.wgst_kt[0], NONE_U16);
    assert_eq!(t.visib_sm100[0], 1000);
    assert_eq!(t.flags[0] & FLAG_VISIB_PLUS, FLAG_VISIB_PLUS);
    assert_eq!(t.lat_e5[0], 4_198_000);
    assert_eq!(t.elev_m[0], 201);
    assert_eq!(FLT_CAT_NAMES[t.flt_cat[0] as usize], "MVFR");
    assert!(metar_raw(&t, 0).starts_with("METAR KORD"));
    assert_eq!(unpack_ident(t.station[0]), "KORD");
    // Variable wind, a numeric visibility, and absent optional fields.
    let rec = r#"{"icaoId":"KMSN","receiptTime":"2026-11-14T12:00:17.091Z","obsTime":1794657420,"wdir":"VRB","wspd":3,"visib":2.5,"altim":null,"metarType":"SPECI","rawOb":"SPECI KMSN 141157Z VRB03KT 2 1/2SM","lat":43.1,"lon":-89.3,"elev":262}"#;
    let (t, s, _) = load_metar(&[format!("[{rec}]").into_bytes()]);
    assert_eq!(s.accepted, 1);
    assert_eq!(t.wdir_deg[0], WDIR_VRB);
    assert_eq!(t.visib_sm100[0], 250);
    assert_eq!(t.flags[0] & FLAG_VISIB_PLUS, 0);
    assert_eq!(t.altim_dhpa[0], NONE_U16);
    assert_eq!(t.kind[0], KIND_SPECI);
    assert_eq!(t.temp_dc[0], NONE_I16);
    assert_eq!(s.altim_missing, 1);
}

// REQ: OA-AWC-003
#[test]
fn rawob_altimeter_and_cross_check() {
    assert_eq!(
        raw_altimeter_dhpa("METAR KORD 141157Z A2992 RMK AO2"),
        Some(10132)
    );
    assert_eq!(
        raw_altimeter_dhpa("METAR CYYZ 141157Z 32004KT Q1021 NOSIG"),
        Some(10210)
    );
    assert_eq!(raw_altimeter_dhpa("METAR KORD 141157Z 32004KT 10SM"), None);
    // A group after RMK is not the altimeter.
    assert_eq!(raw_altimeter_dhpa("METAR KORD 141157Z RMK A2992"), None);
    assert_eq!(raw_altimeter_dhpa("METAR KORD A1000"), None); // 33.9 hPa: out of bounds
    let items = [
        good_metar("KAAA", T0, "1023.1", "A3021"), // agrees
        good_metar("KBBB", T0, "1023.1", "A3041"), // 0.7 hPa apart: disagrees
        good_metar("KCCC", T0, "1021", "Q1021"),   // agrees
        metar("KDDD", T0, RCPT, "1020.0", "METAR KDDD 141157Z 10SM"), // no group in rawOb
    ];
    let (t, s, _) = load_metar(&[arr(&items)]);
    assert_eq!(
        (s.altim_checked, s.altim_disagree, s.altim_no_raw),
        (3, 1, 1)
    );
    // The disagreeing row keeps its other fields but has no trusted altimeter.
    let b = t
        .station
        .iter()
        .position(|&x| unpack_ident(x) == "KBBB")
        .unwrap();
    assert_eq!(t.altim_dhpa[b], NONE_U16);
    assert_eq!(t.raw_altim_dhpa[b], 10298);
    assert_eq!(t.flags[b] & FLAG_ALTIM_WITHHELD, FLAG_ALTIM_WITHHELD);
    assert_eq!(t.wspd_kt[b], 4);
    assert_eq!(
        altimeter_at(&t, "KBBB", T0 + 60, 5400),
        Err(AltimeterError::NoAltimeter)
    );
    assert!(altimeter_at(&t, "KAAA", T0 + 60, 5400).is_ok());
}

// REQ: OA-AWC-004
#[test]
fn metar_duplicates_are_removed_deterministically() {
    let a = good_metar("KORD", T0, "1023.1", "A3021");
    let a_late = metar(
        "KORD",
        T0,
        "2026-11-14T12:05:00.000Z",
        "1023.1",
        "METAR KORD 141157Z 32004KT 10SM OVC023 01/M01 A3021 RMK AO2 CORRECTED",
    );
    let b = good_metar("KORD", T0 + 3600, "1022.0", "A3018");
    let speci =
        good_metar("KORD", T0, "1023.1", "A3021").replace("\"METAR\",\"rawOb", "\"SPECI\",\"rawOb");
    let f1 = arr(&[a.clone(), b.clone()]);
    let f2 = arr(&[a.clone(), a_late.clone(), speci]);
    let (t12, s12, d12) = load_metar(&[f1.clone(), f2.clone()]);
    let (t21, _, d21) = load_metar(&[f2, f1]);
    assert_eq!(t12, t21, "file order must not matter");
    assert_eq!(d12, d21);
    assert_eq!(s12.accepted, 5);
    assert_eq!(d12.rows_in, 5);
    // Kept: KORD METAR at T0, KORD SPECI at T0, KORD METAR at T0+3600.
    assert_eq!(
        (d12.rows_kept, d12.duplicates, d12.conflicting_keys),
        (3, 2, 1)
    );
    // The copy with the later receipt time won.
    assert!(metar_raw(&t12, 0).ends_with("CORRECTED"));
    // Finalizing again changes nothing.
    let mut again = t12.clone();
    let d = finalize_metar(&mut again);
    assert_eq!((d.duplicates, again), (0, t12));
}

// REQ: OA-AWC-005
#[test]
fn altimeter_lookup_returns_latest_with_age_and_refuses_stale() {
    let items = [
        good_metar("KORD", T0, "1023.1", "A3021"),
        good_metar("KORD", T0 + 3600, "1022.0", "A3018"),
        good_metar("KORD", T0 + 7200, "1021.0", "A3015"),
        good_metar("KMDW", T0 + 3600, "1020.0", "A3012"),
    ];
    let (t, _, _) = load_metar(&[arr(&items)]);
    let max = DEFAULT_MAX_ALTIMETER_AGE_S;
    // Exactly at an observation: age 0.
    let f = altimeter_at(&t, "KORD", T0 + 3600, max).unwrap();
    assert_eq!((f.hpa, f.obs_s, f.age_s), (1022.0, T0 + 3600, 0));
    // Between observations: the earlier one, with its age.
    let f = altimeter_at(&t, "KORD", T0 + 3600 + 1800, max).unwrap();
    assert_eq!((f.hpa, f.age_s), (1022.0, 1800));
    // Never a later observation.
    let f = altimeter_at(&t, "KORD", T0 + 3599, max).unwrap();
    assert_eq!(f.hpa, 1023.1);
    assert_eq!(
        altimeter_at(&t, "KORD", T0 - 1, max),
        Err(AltimeterError::NoObservationYet)
    );
    // Stale: older than the stated maximum.
    assert_eq!(
        altimeter_at(&t, "KMDW", T0 + 3600 + 5401, max),
        Err(AltimeterError::Stale { age_s: 5401 })
    );
    assert!(altimeter_at(&t, "KMDW", T0 + 3600 + 5400, max).is_ok());
    // Caller's own limit.
    assert_eq!(
        altimeter_at(&t, "KORD", T0 + 3600 + 1800, 600),
        Err(AltimeterError::Stale { age_s: 1800 })
    );
    assert_eq!(
        altimeter_at(&t, "KXXX", T0, max),
        Err(AltimeterError::UnknownStation)
    );
    assert_eq!(
        altimeter_at(&t, "bad!", T0, max),
        Err(AltimeterError::UnknownStation)
    );
    assert_eq!(metar_station_count(&t), 2);
}

// REQ: OA-AWC-006
#[test]
fn indicated_altitude_follows_the_isa_relation() {
    let same = indicated_altitude_ft(5000.0, ISA_SEA_LEVEL_HPA).unwrap();
    assert!((same - 5000.0).abs() < 0.5, "{same}");
    // 1023.1 hPa is 9.85 hPa above standard: about 270 ft more at the surface.
    let up = indicated_altitude_ft(0.0, 1023.1).unwrap();
    assert!((260.0..275.0).contains(&up), "{up}");
    // Below standard the indication is lower: 0.5 inHg is about 463 ft near the surface.
    let down = indicated_altitude_ft(0.0, 29.42 * HPA_PER_INHG).unwrap();
    assert!((-475.0..-455.0).contains(&down), "{down}");
    assert!(indicated_altitude_ft(0.0, 500.0).is_none());
    assert!(indicated_altitude_ft(0.0, f64::NAN).is_none());
    assert!(indicated_altitude_ft(40_000.0, 1013.0).is_none());
}

// REQ: OA-AWC-007
#[test]
fn bad_metar_records_are_refused_and_counted_by_reason() {
    let ok = good_metar("KORD", T0, "1023.1", "A3021");
    let items = [
        ok.clone(),
        "5".to_string(),                                            // not-object
        ok.replace("\"icaoId\":\"KORD\",", ""),                     // missing-field
        ok.replace("KORD\",\"receiptTime", "kord\",\"receiptTime"), // bad-ident
        ok.replace("\"altim\":1023.1", "\"altim\":1500.0"),         // out-of-range
        ok.replace("\"temp\":1.1", "\"temp\":\"warm\""),            // bad-type
        ok.replace("2026-11-14T12:00:17.091Z", "yesterday"),        // bad-time
        ok.replace("\"METAR\",\"rawOb", "\"TAF\",\"rawOb"),         // unknown-code
        ok.replace("\"lat\":41.98", "\"lat\":91.0"),                // out-of-range
        ok.replace("\"obsTime\":1794657420", "\"obsTime\":5"),      // out-of-range
    ];
    let (t, s, d) = load_metar(&[arr(&items)]);
    assert_eq!((s.records, s.accepted), (10, 1));
    assert_eq!(metar_len(&t), 1);
    assert_eq!(d.rows_kept, 1);
    let c = s.refused;
    assert_eq!(c[Reason::NotObject as usize], 1);
    assert_eq!(c[Reason::MissingField as usize], 1);
    assert_eq!(c[Reason::BadIdent as usize], 1);
    assert_eq!(c[Reason::OutOfRange as usize], 3);
    assert_eq!(c[Reason::BadType as usize], 1);
    assert_eq!(c[Reason::BadTime as usize], 1);
    assert_eq!(c[Reason::UnknownCode as usize], 1);
    assert_eq!(c.iter().sum::<u32>(), 9);
}

// REQ: OA-AWC-008
#[test]
fn a_bad_file_is_refused_whole_and_changes_nothing() {
    let mut t = new_metar_table();
    let mut s = MetarStats::default();
    let ok = arr(&[good_metar("KORD", T0, "1023.1", "A3021")]);
    parse_metar(&ok, &mut t, &mut s).unwrap();
    let before = (t.clone(), s);
    assert!(matches!(
        parse_metar(b"[{\"icaoId\":", &mut t, &mut s),
        Err(ParseError::Json(_))
    ));
    assert_eq!(
        parse_metar(b"{}", &mut t, &mut s),
        Err(ParseError::NotArray)
    );
    assert!(parse_metar(b"", &mut t, &mut s).is_err());
    let big = format!("[{}]", vec!["1"; MAX_RECORDS_PER_FILE + 1].join(","));
    assert_eq!(
        parse_metar(big.as_bytes(), &mut t, &mut s),
        Err(ParseError::TooManyRecords)
    );
    assert_eq!((t, s), before);
    // The table row limit is checked before anything is appended.
    let doc = oa_json::parse(b"[1,2,3]", &oa_json::DEFAULT_LIMITS).unwrap();
    assert_eq!(
        crate::pool::top_array(&doc, MAX_TABLE_ROWS - 2, 10, MAX_TABLE_ROWS),
        Err(ParseError::TableFull)
    );
    assert!(crate::pool::top_array(&doc, MAX_TABLE_ROWS - 3, 10, MAX_TABLE_ROWS).is_ok());
}

const PIREP: &str = r#"{"receiptTime":"2026-11-14T12:30:09.450Z","obsTime":1794657420,"qcField":0,"icaoId":"","acType":"A333","lat":39.8147,"lon":-86.3675,"fltLvl":350,"fltLvlType":"OTHER","clouds":null,"visib":null,"wxString":"","temp":-52,"wdir":null,"wspd":null,"icgBas1":null,"icgTop1":null,"icgInt1":"","icgType1":"","icgBas2":null,"icgTop2":null,"icgInt2":"","icgType2":"","tbBas1":330,"tbTop1":350,"tbInt1":"LGT","tbType1":"CHOP","tbFreq1":"OCNL","tbBas2":null,"tbTop2":null,"tbInt2":"","tbType2":"","tbFreq2":"","vertGust":null,"brkAction":"","pirepType":"PIREP","rawOb":"IND UA /OV VHP/TM 1157/FL350/TP A333/TB LGT DURC 330-350"}"#;

fn load_pirep(items: &[String]) -> (PirepTable, PirepStats, PirepDedupe) {
    let mut t = new_pirep_table();
    let mut s = PirepStats::default();
    parse_pirep(&arr(items), &mut t, &mut s).unwrap();
    let d = finalize_pirep(&mut t);
    (t, s, d)
}

// REQ: OA-AWC-009
#[test]
fn pirep_fields_codes_and_refusals() {
    let descent = PIREP
        .replace(
            "\"fltLvl\":350,\"fltLvlType\":\"OTHER\"",
            "\"fltLvl\":0,\"fltLvlType\":\"DURD\"",
        )
        .replace("\"lat\":39.8147", "\"lat\":39.9");
    let airep = PIREP
        .replace("\"acType\":\"A333\"", "\"acType\":\"UAL378\"")
        .replace("\"pirepType\":\"PIREP\"", "\"pirepType\":\"AIREP\"")
        .replace("\"icgInt2\":\"\"", "\"icgInt2\":\"NEGclr\"")
        .replace("\"lat\":39.8147", "\"lat\":39.8");
    let items = [
        PIREP.to_string(),
        PIREP.to_string(), // duplicate
        descent,
        airep,
        PIREP.replace(
            "\"tbBas1\":330,\"tbTop1\":350",
            "\"tbBas1\":350,\"tbTop1\":330",
        ), // reversed
        PIREP.replace("\"tbInt1\":\"LGT\"", "\"tbInt1\":\"EXTREMELY\""), // unknown
        PIREP.replace("\"fltLvl\":350", "\"fltLvl\":900"),               // range
    ];
    let (t, s, d) = load_pirep(&items);
    assert_eq!((s.records, s.accepted), (7, 4));
    assert_eq!(s.refused[Reason::ReversedBand as usize], 1);
    assert_eq!(s.refused[Reason::UnknownCode as usize], 1);
    assert_eq!(s.refused[Reason::OutOfRange as usize], 1);
    assert_eq!((d.rows_kept, d.duplicates), (3, 1));
    let i = t.lat_e5.iter().position(|&v| v == 3_981_470).unwrap();
    assert_eq!(pirep_alt_ft(&t, i), Some(35_000));
    assert_eq!((t.lat_e5[i], t.lon_e5[i]), (3_981_470, -8_636_750));
    assert_eq!(unpack_ident(t.ac_type[i]), "A333");
    assert_eq!(t.temp_c[i], -52);
    assert_eq!((t.turb_base[i], t.turb_top[i]), (330, 350));
    assert_eq!(INTENSITY_NAMES[t.turb_int[i] as usize], "LGT");
    assert_eq!(TURB_TYPE_NAMES[t.turb_type[i] as usize], "CHOP");
    assert_eq!(FREQ_NAMES[t.turb_freq[i] as usize], "OCNL");
    assert_eq!(t.ice_int[i], 0);
    assert_eq!(t.wdir_deg[i], NONE_U16);
    assert!(pirep_raw(&t, i).contains("TP A333"));
    let d = t.lat_e5.iter().position(|&v| v == 3_990_000).unwrap();
    assert_eq!(
        (pirep_alt_ft(&t, d), FL_TYPE_NAMES[t.fl_type[d] as usize]),
        (None, "DURD")
    );
    let a = t.lat_e5.iter().position(|&v| v == 3_980_000).unwrap();
    assert_eq!(t.ac_type[a], NO_IDENT);
    assert_eq!(PIREP_TYPE_NAMES[t.ptype[a] as usize], "AIREP");
    assert_eq!(INTENSITY_NAMES[t.ice_int2[a] as usize], "NEG");
    assert_eq!(pirep_len(&t), 3);
}

fn sigmet(
    id: &str,
    series: &str,
    from: i64,
    coords: &str,
    geom: &str,
    base: &str,
    top: &str,
) -> String {
    format!(
        r#"{{"icaoId":"{id}","firId":"WAAF","receiptTime":"2026-11-14T11:42:37.863Z","issueTime":"2026-11-14T11:40:00.000Z","validTimeFrom":{from},"validTimeTo":{to},"seriesId":"{series}","hazard":"VA","base":{base},"top":{top},"geom":"{geom}","coords":{coords}}}"#,
        to = from + 21_600
    )
}

const SQUARE: &str =
    r#"[{"lon":10,"lat":10},{"lon":20,"lat":10},{"lon":20,"lat":20},{"lon":10,"lat":20}]"#;
const V0: i64 = 1_794_656_400;

fn load_hazard(src: u8, files: &[Vec<u8>]) -> (HazardTable, HazardStats, HazardDedupe) {
    let mut t = new_hazard_table();
    let mut s = HazardStats::default();
    for f in files {
        match src {
            SRC_SIGMET => parse_sigmet(f, &mut t, &mut s),
            SRC_AIRSIGMET => parse_airsigmet(f, &mut t, &mut s),
            _ => parse_gairmet(f, &mut t, &mut s),
        }
        .unwrap();
    }
    let d = finalize_hazard(&mut t);
    (t, s, d)
}

// REQ: OA-AWC-010
#[test]
fn sigmet_polygons_altitudes_and_validity() {
    let two = r#"[[{"lon":0,"lat":0},{"lon":1,"lat":0},{"lon":1,"lat":1}],[{"lon":5,"lat":5},{"lon":6,"lat":5},{"lon":6,"lat":6},{"lon":5,"lat":6}]]"#;
    let items = [
        sigmet("WAAA", "12", V0, SQUARE, "AREA", "0", "6000"),
        sigmet("WAAA", "13", V0, two, "AREAS", "null", "49000"),
        sigmet(
            "WAAA",
            "14",
            V0,
            r#"[{"lon":1,"lat":1},{"lon":2,"lat":2}]"#,
            "AREA",
            "null",
            "null",
        ), // 2 points
        sigmet("WAAA", "15", V0, SQUARE, "AREA", "9000", "6000"), // base above top
        sigmet("WAAA", "16", V0, SQUARE, "BLOB", "null", "null"), // unknown geom
    ];
    let (t, s, _) = load_hazard(SRC_SIGMET, &[arr(&items)]);
    assert_eq!((s.records, s.accepted), (5, 2));
    assert_eq!(s.refused[Reason::BadGeometry as usize], 1);
    assert_eq!(s.refused[Reason::ReversedBand as usize], 1);
    assert_eq!(s.refused[Reason::UnknownCode as usize], 1);
    assert_eq!(hazard_len(&t), 2);
    assert_eq!(hazard_name(&t, 0), "VA");
    assert_eq!(hazard_label(&t, 0), "WAAA 12");
    assert_eq!((t.base_ft[0], t.top_ft[0]), (0, 6000));
    assert_eq!((t.valid_from_s[0], t.valid_to_s[0]), (V0, V0 + 21_600));
    assert_eq!(t.issue_s[0], 1_794_656_400);
    assert_eq!(hazard_rings(&t, 0).len(), 1);
    assert_eq!(ring_points(&t, 0).len(), 4);
    assert_eq!((t.base_ft[1], t.top_ft[1]), (NO_ALT, 49_000));
    assert_eq!(t.geom[1], GEOM_AREAS);
    let rings = hazard_rings(&t, 1);
    assert_eq!(rings.len(), 2);
    assert_eq!(ring_points(&t, rings.start).len(), 3);
    assert_eq!(ring_points(&t, rings.start + 1).len(), 4);
}

// REQ: OA-AWC-011
#[test]
fn us_sigmet_altitude_band_is_the_extreme_of_the_given_values() {
    let rec = |lo1: &str, lo2: &str, hi1: &str, hi2: &str| {
        format!(
            r#"{{"icaoId":"KKCI","alphaChar":"C","seriesId":"OSCAR 10","receiptTime":"2026-11-14T16:51:11.085Z","creationTime":"2026-11-14T16:50:00.000Z","validTimeFrom":1794675000,"validTimeTo":1794689400,"airSigmetType":"SIGMET","hazard":"TURB","altitudeHi1":{hi1},"altitudeHi2":{hi2},"altitudeLow1":{lo1},"altitudeLow2":{lo2},"severity":5,"coords":{SQUARE}}}"#
        )
    };
    let items = [
        rec("18000", "null", "null", "33000"),
        rec("null", "null", "28000", "28000"),
        rec("null", "null", "null", "null"),
        rec("24000", "12000", "30000", "null"),
    ];
    let (t, s, _) = load_hazard(SRC_AIRSIGMET, &[arr(&items)]);
    assert_eq!(s.accepted, 4);
    let bands: Vec<(i32, i32)> = (0..4).map(|i| (t.base_ft[i], t.top_ft[i])).collect();
    assert!(bands.contains(&(18_000, 33_000)));
    assert!(bands.contains(&(NO_ALT, 28_000)));
    assert!(bands.contains(&(NO_ALT, NO_ALT)));
    assert!(bands.contains(&(12_000, 30_000)));
    assert_eq!(t.severity[0], 5);
    assert_eq!(hazard_label(&t, 0).split(' ').next(), Some("KKCI"));
}

fn gairmet(hazard: &str, geom: &str, base: &str, top: &str, level: &str, coords: &str) -> String {
    format!(
        r#"{{"tag":"1C","forecastHour":3,"validTime":"2026-11-14T18:00:00.000Z","hazard":"{hazard}","geometryType":"AREA","severity":null,"top":{top},"base":{base},"fzltop":"110","fzlbase":"SFC","level":{level},"receiptTime":1794672719,"issueTime":1794672660,"expireTime":1794690000,"product":"ZULU","geom":"{geom}","coords":{coords}}}"#
    )
}

const GSQUARE: &str = r#"[{"lat":"10.0","lon":"10.0"},{"lat":"10.0","lon":"20.0"},{"lat":"20.0","lon":"20.0"},{"lat":"20.0","lon":"10.0"}]"#;

// REQ: OA-AWC-012
#[test]
fn gairmet_strings_hundreds_of_feet_and_freezing_level() {
    let items = [
        gairmet("ICE", "AREA", "\"FZL\"", "\"130\"", "null", GSQUARE),
        gairmet("IFR", "AREA", "null", "null", "null", GSQUARE),
        gairmet("TURB-LO", "AREA", "\"SFC\"", "\"060\"", "null", GSQUARE),
        gairmet(
            "FZLVL",
            "LINE",
            "null",
            "null",
            "\"040\"",
            r#"[{"lat":"1.0","lon":"2.0"},{"lat":"3.0","lon":"4.0"}]"#,
        ),
        gairmet("ICE", "AREA", "\"060\"", "\"040\"", "null", GSQUARE), // base above top
        gairmet("ICE", "AREA", "\"6X\"", "null", "null", GSQUARE),     // bad altitude text
    ];
    let (t, s, _) = load_hazard(SRC_GAIRMET, &[arr(&items)]);
    assert_eq!((s.records, s.accepted), (6, 4));
    assert_eq!(s.refused[Reason::ReversedBand as usize], 1);
    assert_eq!(s.refused[Reason::BadType as usize], 1);
    let ice = (0..hazard_len(&t))
        .find(|&i| hazard_name(&t, i) == "ICE")
        .unwrap();
    assert_eq!((t.base_ft[ice], t.top_ft[ice]), (NO_ALT, 13_000));
    assert_eq!(t.flags[ice] & FLAG_BASE_FZL, FLAG_BASE_FZL);
    assert_eq!((t.fzl_base_ft[ice], t.fzl_top_ft[ice]), (0, 11_000));
    let turb = (0..hazard_len(&t))
        .find(|&i| hazard_name(&t, i) == "TURB-LO")
        .unwrap();
    assert_eq!((t.base_ft[turb], t.top_ft[turb]), (0, 6000));
    let fz = (0..hazard_len(&t))
        .find(|&i| hazard_name(&t, i) == "FZLVL")
        .unwrap();
    assert_eq!((t.level_ft[fz], t.geom[fz]), (4000, GEOM_LINE));
    // validTime to expireTime.
    assert_eq!(
        (t.valid_from_s[ice], t.valid_to_s[ice]),
        (1_794_679_200, 1_794_690_000)
    );
    // Coordinates given as text are read.
    assert_eq!(
        t.lat_e5[ring_points(&t, hazard_rings(&t, ice).start).start],
        1_000_000
    );
}

// REQ: OA-AWC-013
#[test]
fn hazards_dedupe_activity_and_coverage() {
    let a = sigmet("WAAA", "12", V0, SQUARE, "AREA", "0", "6000");
    let b = sigmet("WBBB", "1", V0 + 10_000, SQUARE, "AREA", "null", "null");
    // The same report seen in two snapshots, with a different receipt time.
    let a2 = a.replace("11:42:37.863Z", "12:42:37.863Z");
    let (t, _, d) = load_hazard(
        SRC_SIGMET,
        &[arr(&[a.clone(), b.clone()]), arr(&[a2.clone(), a.clone()])],
    );
    let (t_rev, _, _) = load_hazard(SRC_SIGMET, &[arr(&[a, b]), arr(&[a2])]);
    assert_eq!(hazard_len(&t_rev), 2);
    assert_eq!((d.rows_in, d.rows_kept, d.duplicates), (4, 2, 2));
    assert_eq!(t, t_rev, "file order must not matter");
    let mut act = Vec::new();
    active_hazards_at(&t, V0 - 1, &mut act);
    assert!(act.is_empty());
    active_hazards_at(&t, V0 + 100, &mut act);
    assert_eq!(act.len(), 1);
    active_hazards_at(&t, V0 + 12_000, &mut act);
    assert_eq!(act.len(), 2);
    active_hazards_at(&t, V0 + 21_600 + 1, &mut act);
    assert_eq!(act.len(), 1);
    let w = (0..2).find(|&i| hazard_label(&t, i) == "WAAA 12").unwrap();
    assert!(hazard_covers(&t, w, 15.0, 15.0, None));
    assert!(hazard_covers(&t, w, 15.0, 15.0, Some(3000)));
    assert!(
        !hazard_covers(&t, w, 15.0, 15.0, Some(7000)),
        "above the top"
    );
    assert!(!hazard_covers(&t, w, 25.0, 15.0, None));
    assert!(!hazard_covers(&t, w, 15.0, 25.0, None));
    assert!(!hazard_covers(&t, 99, 15.0, 15.0, None));
    // The other has open bounds.
    let o = 1 - w;
    assert!(hazard_covers(&t, o, 15.0, 15.0, Some(45_000)));
    // Antimeridian polygon: lon 179..181, a point at -179.5 is inside.
    let am = r#"[{"lon":179,"lat":0},{"lon":181,"lat":0},{"lon":181,"lat":2},{"lon":179,"lat":2}]"#;
    let (t, _, _) = load_hazard(
        SRC_SIGMET,
        &[arr(&[sigmet("NFFF", "1", V0, am, "AREA", "null", "null")])],
    );
    assert!(hazard_covers(&t, 0, 1.0, -179.5, None));
    assert!(hazard_covers(&t, 0, 1.0, 180.5, None));
    assert!(!hazard_covers(&t, 0, 1.0, -178.0, None));
    // A line covers nothing.
    let (t, _, _) = load_hazard(
        SRC_GAIRMET,
        &[arr(&[gairmet(
            "FZLVL", "LINE", "null", "null", "\"040\"", GSQUARE,
        )])],
    );
    assert!(!hazard_covers(&t, 0, 15.0, 15.0, None));
}

const NZ_RAW: &str = "WSNZ21 NZKL 141800 NZZC SIGMET 22 VALID 141800/142200 NZKL- NZZC NEW ZEALAND FIR SEV TURB OBS AT 2007Z S4452 E16900 SFC/4000FT=";

fn with_raw(base: String, raw: &str) -> String {
    base.replacen('{', &format!(r#"{{"rawSigmet":"{raw}","#), 1)
}

// REQ: OA-AWC-018
#[test]
fn single_coordinate_area_is_a_point_hazard() {
    let pt = r#"[{"lon":169.0,"lat":-44.8667}]"#;
    let items = [with_raw(
        sigmet("NZKL", "22", V0, pt, "AREA", "0", "4000"),
        NZ_RAW,
    )];
    let (t, s, _) = load_hazard(SRC_SIGMET, &[arr(&items)]);
    assert_eq!((s.records, s.accepted, s.points), (1, 1, 1));
    assert_eq!(t.geom[0], GEOM_POINT);
    assert_eq!(hazard_rings(&t, 0).len(), 1);
    assert_eq!(ring_points(&t, 0).len(), 1);
    assert_eq!((t.lat_e5[0], t.lon_e5[0]), (-4_486_670, 16_900_000));
    // A point covers nothing, at its own position too.
    assert!(!hazard_covers(&t, 0, -44.8667, 169.0, None));
}

// REQ: OA-AWC-018
#[test]
fn two_coordinate_area_is_refused() {
    let two = r#"[{"lon":1,"lat":1},{"lon":2,"lat":2}]"#;
    let items = [sigmet("NZKL", "23", V0, two, "AREA", "null", "null")];
    let (t, s, _) = load_hazard(SRC_SIGMET, &[arr(&items)]);
    assert_eq!((s.records, s.accepted, s.points), (1, 0, 0));
    assert_eq!(s.refused[Reason::BadGeometry as usize], 1);
    assert_eq!(hazard_len(&t), 0);
    // Only an international SIGMET may be a point; a one-coordinate G-AIRMET area is refused.
    let one = r#"[{"lon":"1","lat":"1"}]"#;
    let g = gairmet("TURB-HI", "AREA", "\"040\"", "\"080\"", "null", one);
    let (_, s, _) = load_hazard(SRC_GAIRMET, &[arr(&[g])]);
    assert_eq!(s.refused[Reason::BadGeometry as usize], 1);
    let why = describe_hazard_refusals(arr(&items).as_slice(), SRC_SIGMET).unwrap();
    assert_eq!(why, ["record 0 NZKL 23: bad-geometry (coords)"]);
}

// REQ: OA-AWC-019
#[test]
fn point_position_is_cross_checked_against_raw_sigmet() {
    let at = |lat: f64, lon: f64, raw: &str, series: &str| {
        let c = format!(r#"[{{"lon":{lon},"lat":{lat}}}]"#);
        with_raw(sigmet("NZKL", series, V0, &c, "AREA", "null", "null"), raw)
    };
    let items = [
        at(-44.8667, 169.0, NZ_RAW, "1"),
        // Minutes form, west and north, no time token.
        at(4.5, -17.25, "X FCST AT N0430 W01715 Y", "2"),
        // Whole-degree form.
        at(45.0, 120.0, "X OBS AT 1200Z N45 E120 Y", "3"),
        // Disagreements: latitude sign, and minutes beyond one.
        at(44.8667, 169.0, NZ_RAW, "4"),
        at(-44.5, 169.0, NZ_RAW, "5"),
        // No stated position, or not in the ICAO form: not checked.
        at(1.0, 1.0, "X SEV TURB OBS Y", "6"),
        at(1.0, 1.0, "X OBS AT 1200Z NEAR SOMEWHERE Y", "7"),
        sigmet("NZKL", "8", V0, r#"[{"lon":1,"lat":1}]"#, "AREA", "0", "10"),
    ];
    let (t, s, _) = load_hazard(SRC_SIGMET, &[arr(&items)]);
    assert_eq!(
        (s.accepted, s.points),
        (8, 8),
        "a disagreement is not a refusal"
    );
    assert_eq!((s.point_raw_checked, s.point_raw_disagree), (5, 2));
    assert_eq!(hazard_len(&t), 8);
}

// REQ: OA-AWC-020
#[test]
fn point_hazards_are_found_by_distance_not_coverage() {
    let p = |lat: f64, lon: f64, series: &str, base: &str, top: &str| {
        let c = format!(r#"[{{"lon":{lon},"lat":{lat}}}]"#);
        sigmet("NZKL", series, V0, &c, "AREA", base, top)
    };
    let items = [
        p(-44.8667, 169.0, "1", "0", "4000"),
        p(-42.8333, 172.55, "2", "12000", "15000"),
        p(10.0, -179.9, "3", "null", "null"), // across the antimeridian from lon 179.9
        sigmet("WAAA", "4", V0, SQUARE, "AREA", "null", "null"),
    ];
    let (t, s, _) = load_hazard(SRC_SIGMET, &[arr(&items)]);
    assert_eq!((s.accepted, s.points), (4, 3));
    let label_at = |out: &[u32]| -> Vec<String> {
        out.iter()
            .map(|&i| hazard_label(&t, i as usize).to_string())
            .collect()
    };
    let mut out = Vec::new();
    // Within 1 km of the first point only; the distance is the caller's.
    point_hazards_near(&t, -44.87, 169.0, 1_000.0, None, &mut out);
    assert_eq!(label_at(&out), ["NZKL 1"]);
    point_hazards_near(&t, -44.87, 169.0, 10.0, None, &mut out);
    assert!(out.is_empty());
    // Both New Zealand points, within 400 km; an altitude outside a band excludes it.
    point_hazards_near(&t, -43.8, 170.8, 400_000.0, None, &mut out);
    assert_eq!(label_at(&out), ["NZKL 1", "NZKL 2"]);
    point_hazards_near(&t, -43.8, 170.8, 400_000.0, Some(13_000), &mut out);
    assert_eq!(label_at(&out), ["NZKL 2"]);
    // Across the antimeridian: 0.2 degrees of longitude at 10 N is about 22 km.
    point_hazards_near(&t, 10.0, 179.9, 25_000.0, None, &mut out);
    assert_eq!(label_at(&out), ["NZKL 3"]);
    // The polygon is not a point, and no point covers.
    point_hazards_near(&t, 15.0, 15.0, 1e9, None, &mut out);
    assert_eq!(out.len(), 3);
    for i in 0..hazard_len(&t) {
        if t.geom[i] == GEOM_POINT {
            let (la, lo) = (f64::from(t.lat_e5[i]) / 1e5, f64::from(t.lon_e5[i]) / 1e5);
            assert!(!hazard_covers(&t, i, la, lo, None));
        }
    }
    // Bad distances return nothing; `out` is replaced.
    point_hazards_near(&t, 0.0, 0.0, -1.0, None, &mut out);
    assert!(out.is_empty());
    point_hazards_near(&t, 0.0, 0.0, f64::NAN, None, &mut out);
    assert!(out.is_empty());
}

const TAF: &str = r#"{"icaoId":"KRNH","dbPopTime":"2026-11-14T17:38:21.102Z","bulletinTime":"2026-11-14T17:38:00.000Z","issueTime":"2026-11-14T17:38:00.000Z","validTimeFrom":1794679200,"validTimeTo":1794765600,"rawTAF":"TAF KRNH 141738Z 1418/1518 00000KT P6SM FEW250 FM151500 18005G15KT 3SM BKN008 OVC015","mostRecent":1,"remarks":"","lat":45.14764,"lon":-92.53352,"elev":302,"prior":4,"name":"New Richmond Muni","fcsts":[{"timeFrom":1794679200,"timeTo":1794754800,"timeBec":null,"fcstChange":null,"probability":null,"wdir":0,"wspd":0,"wgst":null,"visib":"6+","altim":null,"clouds":[{"cover":"FEW","base":25000,"type":null}]},{"timeFrom":1794754800,"timeTo":1794765600,"timeBec":null,"fcstChange":"FM","probability":null,"wdir":"180","wspd":5,"wgst":15,"visib":3,"altim":null,"clouds":[{"cover":"OVC","base":1500,"type":null},{"cover":"BKN","base":800,"type":null}]},{"timeFrom":1794760000,"timeTo":1794765600,"fcstChange":"PROB","probability":30,"wdir":"VRB","wspd":3,"visib":"","clouds":[]}]}"#;

// REQ: OA-AWC-014
#[test]
fn taf_headers_periods_and_duplicates() {
    let mut t = new_taf_table();
    let mut s = TafStats::default();
    let later = TAF.replace("141738Z", "141838Z").replace(
        "\"issueTime\":\"2026-11-14T17:38:00.000Z\"",
        "\"issueTime\":\"2026-11-14T18:38:00.000Z\"",
    );
    let bad = TAF.replace("\"fcsts\":[", "\"fcsts\":[{\"timeFrom\":5,\"timeTo\":6},");
    parse_taf(&arr(&[TAF.to_string(), later, bad]), &mut t, &mut s).unwrap();
    parse_taf(&arr(&[TAF.to_string()]), &mut t, &mut s).unwrap();
    let d = finalize_taf(&mut t);
    assert_eq!((s.records, s.accepted), (4, 3));
    assert_eq!(s.refused[Reason::OutOfRange as usize], 1);
    assert_eq!((d.rows_in, d.rows_kept, d.duplicates), (3, 2, 1));
    assert_eq!(taf_len(&t), 2);
    assert_eq!(unpack_ident(t.station[0]), "KRNH");
    assert!(taf_raw(&t, 0).contains("141738Z"));
    assert_eq!(t.issue_s[0], 1_794_677_880);
    let p = taf_periods(&t, 0);
    assert_eq!(p.len(), 3);
    let k = p.start;
    assert_eq!(
        (t.p_wdir_deg[k], t.p_wspd_kt[k], t.p_visib_sm100[k]),
        (0, 0, 600)
    );
    assert_eq!(t.p_flags[k] & FLAG_VISIB_PLUS, FLAG_VISIB_PLUS);
    assert_eq!(t.p_ceiling_ft[k], NO_ALT);
    assert_eq!(CHANGE_NAMES[t.p_change[k + 1] as usize], "FM");
    assert_eq!(
        (
            t.p_wdir_deg[k + 1],
            t.p_wgst_kt[k + 1],
            t.p_visib_sm100[k + 1]
        ),
        (180, 15, 300)
    );
    assert_eq!(t.p_ceiling_ft[k + 1], 800);
    assert_eq!(
        (t.p_wdir_deg[k + 2], t.p_prob[k + 2], t.p_visib_sm100[k + 2]),
        (WDIR_VRB, 30, NONE_U16)
    );
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn run_all(bytes: &[u8]) {
    let (mut m, mut ms) = (new_metar_table(), MetarStats::default());
    let _ = parse_metar(bytes, &mut m, &mut ms);
    let _ = finalize_metar(&mut m);
    let (mut p, mut ps) = (new_pirep_table(), PirepStats::default());
    let _ = parse_pirep(bytes, &mut p, &mut ps);
    let _ = finalize_pirep(&mut p);
    let (mut t, mut ts) = (new_taf_table(), TafStats::default());
    let _ = parse_taf(bytes, &mut t, &mut ts);
    let _ = finalize_taf(&mut t);
    for src in [SRC_SIGMET, SRC_AIRSIGMET, SRC_GAIRMET] {
        let (mut h, mut hs) = (new_hazard_table(), HazardStats::default());
        let _ = match src {
            SRC_SIGMET => parse_sigmet(bytes, &mut h, &mut hs),
            SRC_AIRSIGMET => parse_airsigmet(bytes, &mut h, &mut hs),
            _ => parse_gairmet(bytes, &mut h, &mut hs),
        };
        let _ = finalize_hazard(&mut h);
        let mut act = Vec::new();
        active_hazards_at(&h, V0, &mut act);
        for i in 0..hazard_len(&h) {
            let _ = hazard_covers(&h, i, 1.0, 1.0, Some(1000));
        }
    }
}

// REQ: OA-AWC-015
#[test]
fn no_input_panics() {
    let samples: Vec<Vec<u8>> = vec![
        arr(&[good_metar("KORD", T0, "1023.1", "A3021")]),
        arr(&[PIREP.to_string()]),
        arr(&[TAF.to_string()]),
        arr(&[sigmet("WAAA", "12", V0, SQUARE, "AREA", "0", "6000")]),
        arr(&[gairmet(
            "ICE", "AREA", "\"FZL\"", "\"130\"", "null", GSQUARE,
        )]),
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for s in &samples {
        for n in 0..s.len() {
            run_all(&s[..n]);
        }
        for _ in 0..400 {
            let mut m = s.clone();
            for _ in 0..1 + rng.next() % 4 {
                let at = (rng.next() as usize) % m.len();
                m[at] = (rng.next() & 0xFF) as u8;
            }
            run_all(&m);
        }
    }
    for junk in [
        &b"null"[..],
        b"[null,[],{},\"x\",1e999,-0]",
        b"[{\"coords\":[[[[]]]]}]",
        b"\xff\xfe",
    ] {
        run_all(junk);
    }
}

// REQ: OA-AWC-017
#[test]
fn summary_scans_a_directory_and_reports() {
    let dir = std::env::temp_dir().join(format!("oa-awc-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    // Three hours of one station in the ZAU box, a duplicate across files, and a bad file.
    let items: Vec<String> = (0..3)
        .map(|h| good_metar("KORD", T0 + h * 3600, "1023.1", "A3021"))
        .collect();
    std::fs::write(dir.join("metar_a.json"), arr(&items[..2])).unwrap();
    std::fs::write(dir.join("sub").join("metar_b.json"), arr(&items[1..])).unwrap();
    std::fs::write(dir.join("pirep_a.json"), arr(&[PIREP.to_string()])).unwrap();
    std::fs::write(dir.join("taf_a.json"), arr(&[TAF.to_string()])).unwrap();
    std::fs::write(
        dir.join("sigmet_a.json"),
        arr(&[sigmet("WAAA", "12", V0, SQUARE, "AREA", "0", "6000")]),
    )
    .unwrap();
    std::fs::write(
        dir.join("gairmet_a.json"),
        arr(&[gairmet("IFR", "AREA", "null", "null", "null", GSQUARE)]),
    )
    .unwrap();
    std::fs::write(dir.join("airsigmet_a.json"), b"[]").unwrap();
    std::fs::write(dir.join("metar_bad.json"), b"[{").unwrap();
    std::fs::write(dir.join("notes.json"), b"[]").unwrap();
    let s = summary::scan_dir(&dir).unwrap();
    let text = summary::format_summary(&s);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(s.metar_stats.files, 2);
    assert_eq!(s.metar_dedupe.duplicates, 1);
    assert_eq!(metar_len(&s.metar), 3);
    assert_eq!(s.file_errors.len(), 1);
    assert_eq!(s.file_errors[0].0, "metar_bad.json");
    assert_eq!(s.unrecognized, vec!["notes.json".to_string()]);
    assert_eq!(s.availability.stations, 1);
    assert_eq!(s.availability.hours, 3);
    assert_eq!(s.availability.stations_ok, 1);
    assert_eq!(s.availability.lookups, 3);
    assert!(text.contains("METAR  files=2 records=4 accepted=4"));
    assert!(text.contains("duplicates=1"));
    assert!(text.contains("FILES REFUSED WHOLE: 1"));
    assert!(text.contains("PIREP"));
    assert!(text.contains("GAIRMET"));
}
