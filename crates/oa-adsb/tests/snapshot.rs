#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::{decode_snapshot, Filter, Reports, SOURCE_NAMES, ZAU_BOX, F_GROUND, F_RATE_GEOM};

const NOW_MS: i64 = 1_795_564_800_000;

fn body(items: &str, total: usize) -> Vec<u8> {
    format!(r#"{{"ac":[{items}],"ctime":{},"msg":"No error","now":{},"ptime":1,"total":{total}}}"#, NOW_MS - 40, NOW_MS,).into_bytes()
}

fn ac(hex: &str, seen_pos: &str, extra: &str) -> String {
    format!(r#"{{"hex":"{hex}","type":"adsb_icao","seen":0.1,"seen_pos":{seen_pos},"lat":41.9,"lon":-87.9,"alt_baro":12000,"alt_geom":12200,"gs":250.5,"track":359.9,"baro_rate":-640,"geom_rate":-700,"nic":8,"nac_p":9,"nac_v":2,"sil":2,"sil_type":"perhour","version":2,"gva":2,"sda":1,"nic_baro":1,"flight":"TEST123 ","squawk":"1200","category":"A3","mlat":[],"tisb":[]{extra}}}"#)
}

#[test]
fn maps_snapshot_fields_and_estimated_source_time_without_inventing_age() {
    let json = body(&ac("a1b2c3", "2.5", ""), 1);
    let mut out = Reports::default();
    let stats = decode_snapshot(&json, NOW_MS + 100, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.aircraft_reported, 1);
    assert_eq!(stats.accepted, 1);
    assert_eq!(stats.source_types[0], 1);
    assert_eq!(out.time_ns[0], (NOW_MS - 2_500) * 1_000_000);
    assert_eq!(out.icao[0], 0xa1_b2_c3);
    assert_eq!(out.baro_alt_ft[0], 12_000);
    assert_eq!(out.geom_alt_ft[0], 12_200);
    assert_eq!(out.gs_dkt[0], 2_505);
    assert_eq!(out.track_ddeg[0], 3_599);
    assert_eq!(out.vrate_fpm[0], -700); // geometric rate is present; this simple mapping retains it.
    assert_ne!(out.flags[0] & F_RATE_GEOM, 0);
    assert_eq!(out.nic[0], 8);
    assert_eq!(&out.callsign[0], b"TEST123\0");
    assert_eq!(SOURCE_NAMES[out.source[0] as usize], "adsb_icao");
}

#[test]
fn seen_pos_boundary_is_inclusive_and_old_rows_remain_in_denominator() {
    let items = format!("{},{}", ac("a1b2c3", "15", ""), ac("a1b2c4", "15.001", ""));
    let mut out = Reports::default();
    let stats = decode_snapshot(&body(&items, 2), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.aircraft_reported, 2);
    assert_eq!(stats.aircraft_seen, 2);
    assert_eq!(stats.accepted, 1);
    assert_eq!(stats.too_old, 1);
    assert_eq!(out.time_ns.len(), 1);
}

#[test]
fn no_position_does_not_fallback_to_lastposition() {
    let item = r#"{"hex":"a1b2c3","type":"adsb_icao","seen":1.0,"seen_pos":90.0,"lat":null,"lon":null,"lastPosition":{"lat":41.9,"lon":-87.9,"nic":8,"rc":75,"seen_pos":90.0},"mlat":[],"tisb":[]}"#;
    let mut out = Reports::default();
    let stats = decode_snapshot(&body(item, 1), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.last_position_only, 1);
    assert_eq!(stats.accepted, 0);
    assert!(out.time_ns.is_empty());
}

#[test]
fn total_mismatch_duplicate_keys_bad_age_and_sealed_time_are_not_silently_repaired() {
    let mut out = Reports::default();
    assert!(decode_snapshot(&body(&ac("a1b2c3", "1", ""), 2), NOW_MS, 15_000, &ZAU_BOX, &mut out).is_err());
    assert!(decode_snapshot(br#"{"ac":[],"ac":[],"ctime":1795564800000,"msg":"No error","now":1795564800000,"ptime":1,"total":0}"#, NOW_MS, 15_000, &ZAU_BOX, &mut out).is_err());
    let missing_age = body(r#"{"hex":"a1b2c3","type":"adsb_icao","lat":41.9,"lon":-87.9,"mlat":[],"tisb":[]}"#, 1);
    let stats = decode_snapshot(&missing_age, NOW_MS, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.invalid_age, 1);
    let sealed_now = 1_795_132_800_000i64;
    let sealed = format!(r#"{{"ac":[],"ctime":{},"msg":"No error","now":{},"ptime":1,"total":0}}"#, sealed_now, sealed_now).into_bytes();
    assert!(matches!(decode_snapshot(&sealed, sealed_now, 15_000, &ZAU_BOX, &mut out), Err(oa_adsb::SnapshotError::SealedTime)));
    let end_ns = oa_sealed::sealed_windows().iter().map(|window| window.to_ns).max().unwrap();
    let just_after_window = end_ns / 1_000_000 + 1_000;
    let row_sealed = format!(r#"{{"ac":[{}],"ctime":{},"msg":"No error","now":{},"ptime":1,"total":1}}"#, ac("a1b2c3", "2", ""), just_after_window, just_after_window).into_bytes();
    let stats = decode_snapshot(&row_sealed, just_after_window + 1, 15_000, &Filter { from_ns: 0, to_ns: i64::MAX, ..ZAU_BOX }, &mut out).unwrap();
    assert_eq!(stats.sealed_window_dropped, 1);
}

#[test]
fn unknown_position_source_and_ground_state_are_explicitly_counted_and_encoded() {
    let item = r#"{"hex":"~a1b2c3","type":"mode_s","seen":0.1,"seen_pos":0.5,"lat":41.9,"lon":-87.9,"alt_baro":"ground","gs":null,"track":null,"baro_rate":null,"geom_rate":null,"nic":null,"nac_p":null,"nac_v":null,"sil":null,"sil_type":null,"version":null,"gva":null,"sda":null,"nic_baro":null,"mlat":["lat"],"tisb":["lat"]}"#;
    let mut out = Reports::default();
    let stats = decode_snapshot(&body(item, 1), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.non_adsb_position, 1);
    assert_eq!(stats.with_mlat_fields, 1);
    assert_eq!(stats.with_tisb_fields, 1);
    assert_eq!(out.icao[0], 0x01_a1_b2_c3);
    assert_ne!(out.flags[0] & F_GROUND, 0);
    assert_eq!(out.baro_alt_ft[0], i32::MIN);
    assert_eq!(out.source[0], 7); // mode_s is preserved, not relabeled ADS-B.
}

#[test]
fn age_limit_is_applied_to_raw_value_not_rounded_milliseconds() {
    let item = ac("a1b2c3", "15.0004", "");
    let mut out = Reports::default();
    let stats = decode_snapshot(&body(&item, 1), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.too_old, 1);
    assert_eq!(stats.accepted, 0);
}

#[test]
fn source_flags_must_be_present_and_cannot_conflict_with_direct_adsb_type() {
    let conflict = ac("a1b2c3", "1", "").replace("\"mlat\":[]", "\"mlat\":[\"lat\"]");
    let missing = r#"{"hex":"a1b2c4","type":"adsb_icao","seen_pos":1,"lat":41.9,"lon":-87.9}"#;
    let items = format!("{conflict},{missing}");
    let mut out = Reports::default();
    let stats = decode_snapshot(&body(&items, 2), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out).unwrap();
    assert_eq!(stats.aircraft_seen, 2);
    assert_eq!(stats.source_conflict, 1);
    assert_eq!(stats.invalid_source_identity, 1);
    assert_eq!(stats.accepted, 0);
}

#[test]
fn object_member_limit_is_enforced_before_row_decoding() {
    let extra = (0..140).map(|i| format!(r#""extra{i}":0"#)).collect::<Vec<_>>().join(",");
    let item = ac("a1b2c3", "1", &format!(",{extra}"));
    let mut out = Reports::default();
    assert!(matches!(decode_snapshot(&body(&item, 1), NOW_MS + 1, 15_000, &ZAU_BOX, &mut out), Err(oa_adsb::SnapshotError::StructureLimit)));
    assert!(out.time_ns.is_empty());
}

#[test]
fn oversized_aircraft_array_is_refused_before_row_processing() {
    let items = std::iter::repeat_n("{}", oa_adsb::MAX_SNAPSHOT_ROWS + 1)
        .collect::<Vec<_>>().join(",");
    let json = body(&items, oa_adsb::MAX_SNAPSHOT_ROWS + 1);
    let mut out = Reports::default();
    assert!(matches!(
        decode_snapshot(&json, NOW_MS + 1, 15_000, &ZAU_BOX, &mut out),
        Err(oa_adsb::SnapshotError::TooManyRows(_))
    ));
    assert!(out.time_ns.is_empty());
}
