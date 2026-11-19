//! The CAT021 Ed 2.4 export: the conversion of a report to item values (OA-CMP-019 to 022), the
//! file (OA-CMP-023, 024), the shared duplicate handling (OA-CMP-025) and the README and data
//! source file (OA-CMP-026).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;

use oa_adsb::{
    reports_len, reports_push, Reports, F_GROUND, F_RATE_GEOM, F_STALE, NON_ICAO, ROW_EMPTY,
    SIL_PER_SAMPLE,
};
use oa_asterix::{decode_stream, raw_to_e7, Block, Cat021Edition, SourceId};
use oa_compass::core::adsb::*;
use oa_compass::shell::adsb_export::{read_back, AdsbExport, EDITION};
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::Layout;
use oa_compass::shell::notes::{
    data_sources_json, readme_with, Extras, ADSB_SOURCE, OPENATCA_SOURCE,
};
use oa_compass::shell::run::{export_adsb_reports, ClipInfo, InputsMode};

/// 2026-11-15T00:00:00Z, Unix nanoseconds, and the day count.
const DAY_NS: i64 = 20_772 * 86_400 * 1_000_000_000;
const DAY: i64 = 20_772;

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cmp-adsb-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// A report with everything present, at 12:00:00 on the day.
fn full() -> AdsbRow {
    AdsbRow {
        time_ns: DAY_NS + 43_200 * 1_000_000_000,
        address: 0x3C_66B0,
        non_icao: false,
        lat_e7: 500_000_000,
        lon_e7: -100_000_000,
        baro_alt_ft: Some(30_000),
        geom_alt_ft: Some(25_875),
        ground: false,
        vrate_fpm: Some(-2176),
        rate_geometric: false,
        gs_dkt: Some(4465),
        track_ddeg: Some(3090),
        callsign: *b"DLH7YA  ",
        category: 0xA3,
        nic: Some(8),
        nac_p: Some(8),
        nac_v: Some(2),
        sil: Some(3),
        sil_per_sample: Some(true),
        version: Some(2),
        gva: Some(2),
        sda: Some(2),
    }
}

/// The note counts of one conversion.
fn noted(c: &AdsbConverted) -> Vec<usize> {
    (0..ADSB_NOTES)
        .filter(|&i| c.notes & (1 << i) != 0)
        .collect()
}

/// Notes every record carries: what the source never has.
const ALWAYS: [usize; 6] = [
    ANOTE_TRACK_NUMBER,
    ANOTE_TOA_POSITION,
    ANOTE_TOMR_VELOCITY,
    ANOTE_DESCRIPTOR_DEFAULTS,
    ANOTE_NICBARO,
    ANOTE_PIC,
];

// REQ: OA-CMP-019
#[test]
fn a_full_report_converts_to_the_counts_the_item_definitions_give() {
    let c = adsb_record(&full()).unwrap();
    let v = c.values;
    assert_eq!(c.day, DAY);
    // 43 200 s * 128; 50 deg and -10 deg in counts of 180/2^30 degrees (worked out by hand).
    assert_eq!(v.time, 5_529_600);
    assert_eq!((v.lat, v.lon), (298_261_618, -59_652_324));
    assert_eq!(v.address, 0x3C_66B0);
    assert_eq!((v.atp, v.ground), (0, false));
    // 25 875 ft / 6.25 = 4140; FL 300 = 1200 quarter levels; -2176 / 6.25 = -348.16.
    assert_eq!(v.geometric_height, Some(4140));
    assert_eq!(v.flight_level, Some(1200));
    assert_eq!((v.baro_rate, v.geometric_rate), (Some(-348), None));
    // 446.5 kt = 2032.1 counts of 2^-14 NM/s; 309.0 deg = 56 251.7 counts of 360/2^16 degrees.
    assert_eq!(v.ground_vector, Some((2032, 56_252)));
    assert_eq!(v.ident, Some(*b"DLH7YA  "));
    assert_eq!(v.category, Some(3));
    assert_eq!(v.mops_version, Some(2));
    assert_eq!(
        v.quality,
        Some(QualityValues {
            nacv: 2,
            nic: 8,
            nacp: 8,
            sil: 3,
            sils: true,
            sda: 2,
            gva: 2
        })
    );
    // Only what the source never has is noted.
    let mut expect = ALWAYS.to_vec();
    expect.push(ANOTE_LTT);
    expect.push(ANOTE_GBS);
    expect.push(ANOTE_NIC_V2);
    expect.sort_unstable();
    assert_eq!(noted(&c), expect);
}

// REQ: OA-CMP-020
#[test]
fn what_the_report_lacks_is_left_out_and_noted_by_reason() {
    let mut r = full();
    r.baro_alt_ft = None;
    r.geom_alt_ft = None;
    r.vrate_fpm = None;
    r.gs_dkt = None;
    r.callsign = [0; 8];
    r.category = 0;
    r.nic = None;
    r.nac_p = None;
    r.nac_v = None;
    r.sil = None;
    r.gva = None;
    r.sda = None;
    r.sil_per_sample = None;
    r.version = None;
    let c = adsb_record(&r).unwrap();
    let v = c.values;
    assert!(v.flight_level.is_none() && v.geometric_height.is_none());
    assert!(v.baro_rate.is_none() && v.geometric_rate.is_none());
    assert!(v.ground_vector.is_none() && v.ident.is_none() && v.category.is_none());
    // I090 is in every record: the primary at the unknown codes.
    assert_eq!(
        v.quality,
        Some(QualityValues {
            nacv: 0,
            nic: 0,
            nacp: 0,
            sil: 0,
            sils: false,
            sda: 0,
            gva: 0
        })
    );
    assert!(v.mops_version.is_none());
    let n = noted(&c);
    for i in [
        ANOTE_FL_ABSENT,
        ANOTE_HEIGHT_ABSENT,
        ANOTE_RATE_ABSENT,
        ANOTE_VECTOR_ABSENT,
        ANOTE_IDENT_ABSENT,
        ANOTE_CATEGORY_ABSENT,
        ANOTE_QUALITY_ABSENT,
        ANOTE_MOPS_ABSENT,
    ] {
        assert!(n.contains(&i), "{}", ADSB_NOTE_NAMES[i]);
    }
    // No I210, so no LTT to note; NICBARO and PIC are still not in the source.
    assert!(n.contains(&ANOTE_NICBARO) && n.contains(&ANOTE_PIC) && !n.contains(&ANOTE_LTT));
    // A callsign padded with NULs is the callsign with spaces; a NUL inside it is refused.
    let mut p = full();
    p.callsign = *b"N911ZP\0\0";
    assert_eq!(adsb_record(&p).unwrap().values.ident, Some(*b"N911ZP  "));
    p.callsign = *b"N9\x0011ZP\0";
    let c = adsb_record(&p).unwrap();
    assert!(c.values.ident.is_none() && noted(&c).contains(&ANOTE_IDENT_CHARS));
    // A ground report has no flight level, an all-space callsign is none.
    let mut g = full();
    g.ground = true;
    g.callsign = *b"        ";
    let c = adsb_record(&g).unwrap();
    assert!(c.values.ground && c.values.flight_level.is_none());
    assert!(noted(&c).contains(&ANOTE_FL_ABSENT) && noted(&c).contains(&ANOTE_IDENT_ABSENT));
    // I160 is the airborne vector: omitted and counted on the ground, though the report has it.
    assert!(c.values.ground_vector.is_none());
    assert!(noted(&c).contains(&ANOTE_VECTOR_GROUND) && !noted(&c).contains(&ANOTE_GBS));
}

// REQ: OA-CMP-020
#[test]
fn a_field_that_cannot_be_encoded_is_left_out_never_clipped() {
    let mut r = full();
    r.baro_alt_ft = Some(160_000);
    r.geom_alt_ft = Some(160_000);
    r.vrate_fpm = Some(110_000);
    r.track_ddeg = Some(3600);
    r.callsign = *b"AB@ 1234";
    r.category = 0xB5;
    r.version = Some(3);
    let c = adsb_record(&r).unwrap();
    let v = c.values;
    assert!(v.flight_level.is_none() && v.geometric_height.is_none());
    assert!(v.baro_rate.is_none() && v.ground_vector.is_none());
    assert!(v.ident.is_none() && v.category.is_none() && v.mops_version.is_none());
    let n = noted(&c);
    for i in [
        ANOTE_FL_RANGE,
        ANOTE_HEIGHT_RANGE,
        ANOTE_RATE_RANGE,
        ANOTE_VECTOR_RANGE,
        ANOTE_IDENT_CHARS,
        ANOTE_CATEGORY_NO_CODE,
        ANOTE_MOPS_VERSION,
    ] {
        assert!(n.contains(&i), "{}", ADSB_NOTE_NAMES[i]);
    }
    // The limits themselves are accepted: FL 1500 and -15 (150 000 ft, -1500 ft), height the same.
    let mut r = full();
    r.baro_alt_ft = Some(150_000);
    r.geom_alt_ft = Some(150_000);
    let v = adsb_record(&r).unwrap().values;
    assert_eq!(
        (v.flight_level, v.geometric_height),
        (Some(6000), Some(24_000))
    );
    r.baro_alt_ft = Some(-1500);
    r.geom_alt_ft = Some(-1500);
    let v = adsb_record(&r).unwrap().values;
    assert_eq!(
        (v.flight_level, v.geometric_height),
        (Some(-60), Some(-240))
    );
}

// REQ: OA-CMP-021
#[test]
fn geometric_rate_non_icao_address_ground_and_partial_quality_follow_the_report() {
    let mut r = full();
    r.rate_geometric = true;
    r.non_icao = true;
    r.ground = true;
    r.nac_v = None;
    r.sil_per_sample = None;
    let c = adsb_record(&r).unwrap();
    let v = c.values;
    assert_eq!((v.baro_rate, v.geometric_rate), (None, Some(-348)));
    assert_eq!((v.atp, v.ground), (3, true));
    let q = v.quality.unwrap();
    assert_eq!((q.nacv, q.sils), (0, false));
    let n = noted(&c);
    assert!(n.contains(&ANOTE_ATP_ANONYMOUS) && n.contains(&ANOTE_QUALITY_PARTIAL));
    // A value that does not fit its bits counts as absent: NACv is three bits.
    let mut r = full();
    r.nac_v = Some(9);
    let c = adsb_record(&r).unwrap();
    assert_eq!(c.values.quality.unwrap().nacv, 0);
    assert!(noted(&c).contains(&ANOTE_QUALITY_PARTIAL));
}

// REQ: OA-CMP-021
#[test]
fn the_emitter_categories_map_to_the_codes_of_the_item() {
    // I021/020 (Ed 2.4 5.2.5): 1 light, 2 small, 3 medium, 4 high vortex, 5 heavy, 6 manoeuvrable,
    // 10 rotocraft, 11 glider, 12 lighter than air, 13 UAV, 14 space, 15 ultralight, 16
    // parachutist, 20 to 24 the obstacles and surface vehicles.
    let table: [(u8, u8); 18] = [
        (0xA1, 1),
        (0xA2, 2),
        (0xA3, 3),
        (0xA4, 4),
        (0xA5, 5),
        (0xA6, 6),
        (0xA7, 10),
        (0xB1, 11),
        (0xB2, 12),
        (0xB3, 16),
        (0xB4, 15),
        (0xB6, 13),
        (0xB7, 14),
        (0xC1, 20),
        (0xC2, 21),
        (0xC3, 22),
        (0xC4, 23),
        (0xC5, 24),
    ];
    for (cat, code) in table {
        assert_eq!(emitter_code(cat), Some(code), "{cat:#x}");
    }
    for cat in [0xA0, 0xB0, 0xB5, 0xC0, 0xC6, 0xC7, 0xD1, 0x00, 0x41] {
        assert_eq!(emitter_code(cat), None, "{cat:#x}");
    }
}

// REQ: OA-CMP-022
#[test]
fn positions_at_the_limits_and_beyond_and_times_round_to_the_day() {
    let mut r = full();
    r.lat_e7 = 900_000_001;
    assert_eq!(adsb_record(&r), Err(AdsbRefusal::Position));
    r.lat_e7 = 0;
    r.lon_e7 = -1_800_000_001;
    assert_eq!(adsb_record(&r), Err(AdsbRefusal::Position));
    // +180 degrees is 2^30 counts: written as -180 and noted; -180 stays.
    r.lon_e7 = 1_800_000_000;
    let c = adsb_record(&r).unwrap();
    assert_eq!(c.values.lon, -(1 << 30));
    assert!(noted(&c).contains(&ANOTE_LON_WRAPPED));
    r.lon_e7 = -1_800_000_000;
    let c = adsb_record(&r).unwrap();
    assert_eq!(c.values.lon, -(1 << 30));
    assert!(!noted(&c).contains(&ANOTE_LON_WRAPPED));
    r.lat_e7 = 900_000_000;
    assert_eq!(adsb_record(&r).unwrap().values.lat, 1 << 29);
    // A time within 1/256 s before midnight belongs to the next day, at 0.
    let mut r = full();
    r.time_ns = DAY_NS - 1_000_000;
    let c = adsb_record(&r).unwrap();
    assert_eq!((c.day, c.values.time), (DAY, 0));
    r.time_ns = DAY_NS - 5_000_000;
    let c = adsb_record(&r).unwrap();
    assert_eq!((c.day, c.values.time), (DAY - 1, 86_400 * 128 - 1));
    // The position error is within half the source LSB plus half the item LSB.
    for e7 in [
        -1_799_999_999,
        -123_456_789,
        -1,
        0,
        1,
        77,
        899_999_999,
        1_799_999_999,
    ] {
        let mut r = full();
        r.lat_e7 = e7 / 2;
        r.lon_e7 = e7;
        let v = adsb_record(&r).unwrap().values;
        let (la, lo) = (raw_to_e7(v.lat, 30).unwrap(), raw_to_e7(v.lon, 30).unwrap());
        assert!((i64::from(la) - i64::from(e7 / 2)).abs() <= 1, "{e7}");
        assert!((i64::from(lo) - i64::from(e7)).abs() <= 1, "{e7}");
    }
}

fn layout(name: &str, split: bool) -> (Layout, PathBuf) {
    let out = tmp(name);
    (
        Layout {
            out: out.clone(),
            day: DAY,
            split,
        },
        out,
    )
}

fn at(base: &AdsbRow, units: i64, addr: u32) -> AdsbConverted {
    let mut r = *base;
    r.time_ns = DAY_NS + units * 1_000_000_000 / 128;
    r.address = addr;
    adsb_record(&r).unwrap()
}

// REQ: OA-CMP-023
#[test]
fn the_file_decodes_as_ed24_with_every_value_and_position_error_within_bounds() {
    let (lay, out) = layout("file", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let base = full();
    let mut want = Vec::new();
    for k in 0..700i64 {
        let mut r = base;
        r.lat_e7 = -450_000_000 + (k as i32) * 123_457;
        r.lon_e7 = 170_000_000 + (k as i32) * 11_111;
        r.baro_alt_ft = (k % 5 != 0).then_some(1000 + (k as i32) * 7);
        r.non_icao = k % 9 == 0;
        r.version = (k % 4 != 3).then_some((k % 3) as u8);
        let c = adsb_record(&{
            r.time_ns = DAY_NS + (k * 100) * 1_000_000_000 / 128;
            r.address = 0x100000 + k as u32;
            r
        })
        .unwrap();
        ex.push(&c, (r.lat_e7, r.lon_e7)).unwrap();
        want.push((c, r));
    }
    let f = ex.finish().unwrap();
    let rep = f.commit().unwrap();
    let d = &rep.days[0];
    assert_eq!((d.stats.records, d.stats.blocks), (700, 3));
    assert!(d.stats.max_lat_error_e7 <= 1 && d.stats.max_lon_error_e7 <= 1);
    assert_eq!((d.read_back_records, d.read_back_blocks), (700, 3));
    let bytes = fs::read(out.join("adsb.ast")).unwrap();
    assert_eq!(bytes.len() as u64, d.stats.bytes);
    let mut recs = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let len = usize::from(u16::from_be_bytes([bytes[at + 1], bytes[at + 2]]));
        let (b, used) = oa_asterix::decode_block_with(&bytes[at..], EDITION).unwrap();
        assert_eq!((used, bytes[at]), (len, 21));
        let Block::Cat021(r) = b else { panic!() };
        recs.extend(r);
        at += len;
    }
    assert_eq!(recs.len(), 700);
    for (rec, (c, r)) in recs.iter().zip(want.iter()) {
        assert_eq!(rec.source, SourceId { sac: 255, sic: 13 });
        assert_eq!(rec.address, Some(c.values.address));
        assert_eq!(rec.tomr_position, Some(c.values.time));
        assert_eq!(rec.flight_level, c.values.flight_level);
        assert_eq!(
            rec.descriptor.as_ref().unwrap().atp,
            u8::from(r.non_icao) * 3
        );
        assert_eq!(rec.mops.map(|m| m.version), c.values.mops_version);
        assert_eq!(rec.target_id, Some(*b"DLH7YA  "));
        assert_eq!(rec.emitter_category, Some(3));
        assert!(rec.position.is_none() && rec.toa_position.is_none() && rec.track_number.is_none());
    }
    // The same octets do not decode as Ed 2.7 any differently here: the items used are the same.
    assert!(oa_asterix::decode_block_with(&bytes, Cat021Edition::Ed27).is_ok());
    // The file reads back, and a changed record count does not.
    let p = out.join("adsb.ast");
    assert_eq!(
        read_back(&p, ADSB_SOURCE_PAIR, 700, d.stats.bytes)
            .unwrap()
            .0,
        700
    );
    assert!(read_back(&p, ADSB_SOURCE_PAIR, 699, d.stats.bytes).is_err());
    assert!(read_back(&p, OPENATCA_SOURCE, 700, d.stats.bytes).is_err());
}

const ADSB_SOURCE_PAIR: SourceId = ADSB_SOURCE;

// REQ: OA-CMP-024
#[test]
fn the_file_refuses_disorder_another_day_and_splits_days_on_request() {
    let (lay, _) = layout("order", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let base = full();
    ex.push(&at(&base, 1000, 1), (0, 0)).unwrap();
    let e = ex.push(&at(&base, 999, 2), (0, 0)).unwrap_err();
    assert!(matches!(e, ExportError::OutOfOrder { .. }));
    let mut next = base;
    next.time_ns = DAY_NS + 86_400 * 1_000_000_000 + 5_000_000_000;
    let e = ex.push(&adsb_record(&next).unwrap(), (0, 0)).unwrap_err();
    assert!(matches!(e, ExportError::CrossesMidnight { .. }));
    ex.abort();
    assert!(!tmp("order").join("adsb.ast.partial").exists());
    // Split by day: one directory per day, no file holds two days.
    let (lay, out) = layout("days", true);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    ex.push(&at(&base, 1000, 1), (0, 0)).unwrap();
    ex.push(&adsb_record(&next).unwrap(), (0, 0)).unwrap();
    let rep = ex.finish().unwrap().commit().unwrap();
    assert_eq!(rep.days.len(), 2);
    assert!(out.join("2026.11.15/adsb.ast").exists() && out.join("2026.11.16/adsb.ast").exists());
    for d in &rep.days {
        assert_eq!(d.stats.records, 1);
    }
}

fn push_row(r: &mut Reports, t_s: f64, addr: u32, f: impl FnOnce(&mut oa_adsb::Row)) {
    let mut row = oa_adsb::Row {
        icao: addr,
        time_ns: DAY_NS + (t_s * 1.0e9) as i64,
        lat_e7: 410_000_000,
        lon_e7: -870_000_000,
        baro_alt_ft: 20_000,
        nic: 8,
        nac_p: 9,
        nac_v: 2,
        sil: 3,
        version: 2,
        source: 0,
        ..ROW_EMPTY
    };
    f(&mut row);
    reports_push(r, &row);
}

// REQ: OA-CMP-023
#[test]
fn reports_are_written_in_time_order_with_stale_and_non_adsb_ones_left_out_and_counted() {
    let mut r = Reports::default();
    // Out of time order in the input, two aircraft interleaved.
    push_row(&mut r, 30.0, 0x100002, |_| {});
    push_row(&mut r, 10.0, 0x100001, |_| {});
    push_row(&mut r, 20.0, 0x100001, |x| x.flags |= F_STALE);
    push_row(&mut r, 21.0, 0x100001, |x| x.source = 5); // mlat
    push_row(&mut r, 22.0, 0x100001, |x| x.source = 7); // mode_s
    push_row(&mut r, 5.0, 0x100002, |x| {
        x.flags |= F_GROUND | F_RATE_GEOM;
        x.baro_alt_ft = oa_adsb::NONE_I32;
        x.vrate_fpm = 640;
        x.sil_type = SIL_PER_SAMPLE;
        x.icao = NON_ICAO | 0x100002;
    });
    push_row(&mut r, 40.0, 0x100003, |x| x.source = oa_adsb::NONE_U8);
    let (lay, out) = layout("reports", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let info = export_adsb_reports(&r, &mut ex).unwrap();
    assert_eq!(info.rows_in, 7);
    assert_eq!((info.stale, info.converted), (1, 4));
    assert_eq!(
        info.other_sources,
        vec![("mlat".to_string(), 1), ("mode_s".to_string(), 1)]
    );
    let rep = ex.finish().unwrap().commit().unwrap();
    assert_eq!(rep.days[0].stats.records, 4);
    let blocks = decode_stream_ed24(&fs::read(out.join("adsb.ast")).unwrap());
    let times: Vec<u32> = blocks.iter().map(|c| c.tomr_position.unwrap()).collect();
    assert_eq!(times, vec![5 * 128, 10 * 128, 30 * 128, 40 * 128]);
    let first = &blocks[0];
    assert_eq!(first.descriptor.as_ref().unwrap().atp, 3);
    assert_eq!(first.descriptor.as_ref().unwrap().ext, vec![0x44]);
    assert!(first.flight_level.is_none() && first.baro_rate.is_none());
    assert_eq!(first.geometric_rate.unwrap().raw, 102);
    assert_eq!(
        oa_asterix::cat021::quality_sils(first.quality.as_ref().unwrap()),
        Some(1)
    );
}

fn decode_stream_ed24(bytes: &[u8]) -> Vec<oa_asterix::cat021::Cat021> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (b, used) = oa_asterix::decode_block_with(&bytes[at..], EDITION).unwrap();
        let Block::Cat021(r) = b else { panic!() };
        out.extend(r);
        at += used;
    }
    out
}

// REQ: OA-CMP-025
#[test]
fn all_mode_uses_the_scorers_split_code_with_halving_off_and_counts_what_it_removes() {
    let mut r = Reports::default();
    for (t, lat) in [
        (1.0, 1),
        (2.0, 2),
        (2.0, 3),
        (2.0, 3),
        (3.0, 4),
        (3.0, 4),
        (4.0, 5),
    ] {
        push_row(&mut r, t, 0x100001, |x| x.lat_e7 = lat * 1000);
    }
    let (all, none, st) = oa_track::score::split_reports_with(&r, false);
    assert_eq!((reports_len(&all), reports_len(&none)), (5, 0));
    assert_eq!(
        (
            st.rows_in,
            st.duplicates_removed,
            st.conflict_groups,
            st.conflict_rows
        ),
        (7, 2, 1, 2)
    );
    // The halving run removes the same copies and finds the same conflicts.
    let (a, b, hs) = oa_track::score::split_reports(&r);
    assert_eq!(reports_len(&a) + reports_len(&b), 5);
    assert_eq!(
        (hs.duplicates_removed, hs.conflict_groups),
        (st.duplicates_removed, st.conflict_groups)
    );
    // Both conflicting reports reach the ADS-B file; the copies do not.
    let (lay, out) = layout("dedup", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let info = export_adsb_reports(&all, &mut ex).unwrap();
    assert_eq!(info.converted, 5);
    ex.finish().unwrap().commit().unwrap();
    let recs = decode_stream_ed24(&fs::read(out.join("adsb.ast")).unwrap());
    let lats: Vec<i32> = recs
        .iter()
        .map(|c| raw_to_e7(c.position_hr.unwrap().lat, 30).unwrap())
        .collect();
    // Counts of 180/2^30 degrees: each within one unit of 1e-7 degree of the report.
    for (got, want) in lats.iter().zip([1000, 2000, 3000, 4000, 5000]) {
        assert!((got - want).abs() <= 1, "{lats:?}");
    }
    assert_eq!(lats.len(), 5);
}

// REQ: OA-CMP-026
#[test]
fn the_data_source_file_lists_the_adsb_source_and_the_readme_states_dedup_basis_and_independence() {
    let text = data_sources_json();
    let doc = oa_json::parse(text.as_bytes(), &oa_json::DEFAULT_LIMITS).unwrap();
    let list = doc.get(oa_json::NodeIx(0), "data_sources").unwrap();
    let third = doc.elements(list).nth(2).unwrap();
    assert_eq!(doc.str(doc.get(third, "ds_type").unwrap()), Some("ADSB"));
    assert_eq!(doc.as_i64(doc.get(third, "sic").unwrap()), Some(13));
    assert!(
        text.contains(r#""ds_type":"ADSB","name":"ADS-B","sac":255,"sic":13"#),
        "{text}"
    );
    assert!(text.contains(r#""name":"OpenATCA","sac":255,"sic":11"#));
    // README.
    let (lay, _) = layout("readme", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let base = full();
    for k in 0..10 {
        ex.push(&at(&base, 1000 + k, 1), (500_000_000, -100_000_000))
            .unwrap();
    }
    ex.counts.push(("reports written".into(), 10));
    let rep = ex.finish().unwrap().commit().unwrap();
    let clip = ClipInfo {
        fed: 10,
        left: 0,
        split: oa_track::score::SplitStats {
            rows_in: 14,
            aircraft: 1,
            set_a: 10,
            duplicates_removed: 3,
            conflict_groups: 1,
            conflict_rows: 2,
            rows_invalid: 1,
            ..Default::default()
        },
        adsb: None,
        gate: oa_track::adsb::AdsbStats {
            rows_in: 10,
            passed: 6,
            removed: {
                let mut r = [0u64; 13];
                r[4] = 3;
                r[2] = 1;
                r
            },
            ..Default::default()
        },
    };
    let text = readme_with(
        DAY,
        &[],
        "commit test",
        false,
        Some(InputsMode::All),
        &Extras {
            adsb: Some(&rep),
            clip: Some(&clip),
        },
    );
    for needle in [
        "CAT021 Ed 2.4",
        "SAC 255 SIC 13",
        "split_reports_with",
        "KEEP ALL",
        "3 duplicate reports removed",
        "1 conflicting (address, time) groups holding 2 reports",
        "INDEPENDENCE",
        "NOT the set the tracker used",
        "superset of what the tracker was fed",
        "Of the 10 reports of the whole clip offered to the tracker, 6 became measurements",
        "nacp 3;",
        "source_not_adsb 1;",
        "TIS-B, ADS-R and MLAT",
        "UNVERIFIED",
        "not according to Ed 2.4 itself",
        "asserts \"not on ground\"",
        "UK-008",
        "NOT independent of OpenATCA's inputs",
        "ERAM also uses ADS-B",
        "I073 time of message reception of position: 10",
        "I131 position in WGS-84, high resolution",
        "I071 time of applicability for position: the source has no avionics time",
        "10: I161 track number",
        "I210 LTT",
        "limit 1)",
    ] {
        assert!(text.contains(needle), "missing {needle:?}\n{text}");
    }
}

// REQ: OA-CMP-025
#[test]
fn a_real_clip_goes_through_the_shared_dedup_into_both_files() {
    let clip = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../openatca-data/derived/adsb/zau-2026.11.13-1s.oaf");
    if !clip.exists() {
        eprintln!("skipped: {} is absent", clip.display());
        return;
    }
    let out = tmp("realclip");
    let lay = Layout {
        out: out.clone(),
        day: 0,
        split: true,
    };
    let mut ex = oa_compass::shell::export::SystemExport::new(
        "OpenATCA",
        "openatca.ast",
        OPENATCA_SOURCE,
        lay.clone(),
    );
    let mut ax = AdsbExport::new(ADSB_SOURCE, lay);
    let (run, info) =
        oa_compass::shell::run::export_adsb_clip(&clip, &mut ex, Some(&mut ax), InputsMode::All)
            .unwrap();
    assert!(run.tick_rows > 1000);
    assert_eq!(info.left, 0);
    let a = info.adsb.as_ref().unwrap();
    assert_eq!(info.fed, info.split.set_a);
    assert_eq!(a.rows_in, info.fed);
    assert_eq!(
        a.converted + a.stale + a.other_sources.iter().map(|x| x.1).sum::<u64>(),
        a.rows_in
    );
    // The gate counts are the tracker adapter's own, on the same reports.
    let clip_in = oa_bench::input::load_adsb(&clip).unwrap();
    let (fed_set, _, _) = oa_track::score::split_reports_with(&clip_in.reports, false);
    let (_, want) =
        oa_track::adsb::adsb_measurements(&fed_set, &oa_track::adsb::AdsbConfig::default());
    assert_eq!(info.gate, want, "gate counts differ from adsb_measurements");
    ex.finish().unwrap().commit().unwrap();
    let rep = ax.finish().unwrap().commit().unwrap();
    let written: u64 = rep.days.iter().map(|d| d.stats.records).sum();
    assert_eq!(written + rep.refused.iter().sum::<u64>(), a.converted);
    for d in &rep.days {
        assert!(d.stats.max_lat_error_e7 <= 1 && d.stats.max_lon_error_e7 <= 1);
        assert_eq!(d.read_back_records, d.stats.records);
    }
    // The README states those counts.
    let text = oa_compass::shell::notes::readme_with(
        rep.days[0].day,
        &[],
        "commit test",
        true,
        Some(InputsMode::All),
        &oa_compass::shell::notes::Extras {
            adsb: Some(&rep),
            clip: Some(&info),
        },
    );
    let dropped = want.rows_in - want.passed;
    let line = format!(
        "Of the {} reports of the whole clip offered to the tracker, {} became measurements (the \"measurements offered to the tracker\" total) and {dropped} were dropped by its adapter:",
        want.rows_in, want.passed
    );
    assert!(
        text.contains(&line),
        "README lacks {line:?}\n{}",
        text.lines()
            .filter(|l| l.contains("RELATION"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    for (name, &n) in oa_truth::REMOVE_NAMES.iter().zip(want.removed.iter()) {
        if n > 0 {
            assert!(
                text.contains(&format!(" {name} {n};")),
                "README lacks {name} {n}"
            );
        }
    }
    let _ = decode_stream; // the whole-file decode is the read-back above
}

// REQ: OA-CMP-028
#[test]
fn quality_is_in_every_record_with_extensions_only_where_a_bit_is_set() {
    use oa_compass::shell::adsb_export::cat021_record;
    let ext_of = |f: &dyn Fn(&mut AdsbRow)| {
        let mut r = full();
        f(&mut r);
        let v = adsb_record(&r).unwrap().values;
        cat021_record(&v, ADSB_SOURCE).quality.unwrap()
    };
    let zero = |r: &mut AdsbRow| {
        r.nic = Some(0);
        r.nac_p = Some(0);
        r.nac_v = Some(0);
        r.sil = Some(0);
        r.sil_per_sample = Some(false);
        r.sda = Some(0);
        r.gva = Some(0);
    };
    // Nothing set: the primary subfield alone, no extension.
    let q = ext_of(&zero);
    assert_eq!((q.nacv, q.nic, q.ext.len()), (0, 0, 0));
    // Only NACp: the first extension alone (FX 0), NACp in bits 5/2.
    let q = ext_of(&|r| {
        zero(r);
        r.nac_p = Some(9);
    });
    assert_eq!(q.ext, vec![9 << 1]);
    // Only GVA: the second extension, with the first (all zero, FX 1) in front of it.
    let q = ext_of(&|r| {
        zero(r);
        r.gva = Some(2);
    });
    assert_eq!(q.ext, vec![0x01, 2 << 1]);
    // A report without any quality data still carries I090.
    let q = ext_of(&|r| {
        r.nic = None;
        r.nac_p = None;
        r.nac_v = None;
        r.sil = None;
        r.sda = None;
        r.gva = None;
        r.sil_per_sample = None;
    });
    assert_eq!((q.nacv, q.nic, q.ext.len()), (0, 0, 0));
    // The records decode as Ed 2.4 and every one has I090 (checked in the file test below).
}

// REQ: OA-CMP-028
#[test]
fn values_outside_the_mops_are_not_encoded_and_the_notes_name_sils_and_nic_v2() {
    let mut r = full();
    r.nic = Some(12);
    r.nac_p = Some(12);
    r.nac_v = Some(5);
    let c = adsb_record(&r).unwrap();
    let q = c.values.quality.unwrap();
    assert_eq!((q.nic, q.nacp, q.nacv), (0, 0, 0));
    assert!(noted(&c).contains(&ANOTE_QUALITY_RANGE) && noted(&c).contains(&ANOTE_QUALITY_PARTIAL));
    // The limits themselves are encoded.
    let mut r = full();
    r.nic = Some(11);
    r.nac_p = Some(11);
    r.nac_v = Some(4);
    let q = adsb_record(&r).unwrap().values.quality.unwrap();
    assert_eq!((q.nic, q.nacp, q.nacv), (11, 11, 4));
    // No SIL type with a SIL: SILS written 0 and noted; version 2 with a NIC: mapping unverified.
    let mut r = full();
    r.sil_per_sample = None;
    let c = adsb_record(&r).unwrap();
    assert!(noted(&c).contains(&ANOTE_SILS) && noted(&c).contains(&ANOTE_NIC_V2));
    r.version = Some(1);
    assert!(!noted(&adsb_record(&r).unwrap()).contains(&ANOTE_NIC_V2));
}

// REQ: OA-CMP-028
#[test]
fn the_extension_chain_carries_sils_only_sda_only_and_both_extensions() {
    use oa_compass::shell::adsb_export::cat021_record;
    let ext_of = |f: &dyn Fn(&mut AdsbRow)| {
        let mut r = full();
        r.nic = Some(0);
        r.nac_p = Some(0);
        r.nac_v = Some(0);
        r.sil = Some(0);
        r.sil_per_sample = Some(false);
        r.sda = Some(0);
        r.gva = Some(0);
        f(&mut r);
        let v = adsb_record(&r).unwrap().values;
        cat021_record(&v, ADSB_SOURCE).quality.unwrap().ext
    };
    // SILS alone and SDA alone: the second extension, the empty first one chaining to it.
    assert_eq!(
        ext_of(&|r| r.sil_per_sample = Some(true)),
        vec![0x01, 1 << 5]
    );
    assert_eq!(ext_of(&|r| r.sda = Some(2)), vec![0x01, 2 << 3]);
    // Both extensions with values: the first carries SIL and NACp, with FX.
    assert_eq!(
        ext_of(&|r| {
            r.sil = Some(3);
            r.nac_p = Some(8);
            r.sda = Some(1);
            r.gva = Some(2);
            r.sil_per_sample = Some(true);
        }),
        vec![(3 << 5) | (8 << 1) | 1, (1 << 5) | (1 << 3) | (2 << 1)]
    );
}

// REQ: OA-CMP-028
#[test]
fn every_record_of_the_file_has_i090_and_ground_records_have_gbs_and_no_i160() {
    let (lay, out) = layout("i090file", false);
    let mut ex = AdsbExport::new(ADSB_SOURCE, lay);
    let base = full();
    for k in 0..300i64 {
        let mut r = base;
        r.address = 0x200000 + k as u32;
        r.time_ns = DAY_NS + (k * 128) * 1_000_000_000 / 128;
        r.ground = k % 3 == 0;
        if k % 4 == 0 {
            r.nic = None;
            r.nac_p = None;
            r.nac_v = None;
            r.sil = None;
            r.sda = None;
            r.gva = None;
            r.sil_per_sample = None;
        }
        let c = adsb_record(&r).unwrap();
        ex.push(&c, (r.lat_e7, r.lon_e7)).unwrap();
    }
    ex.finish().unwrap().commit().unwrap();
    let recs = decode_stream_ed24(&fs::read(out.join("adsb.ast")).unwrap());
    assert_eq!(recs.len(), 300);
    let mut ground = 0;
    for rec in &recs {
        assert!(rec.quality.is_some(), "a record without I090");
        let gbs = rec
            .descriptor
            .as_ref()
            .unwrap()
            .ext
            .first()
            .is_some_and(|e| e & 0x40 != 0);
        if gbs {
            ground += 1;
            assert!(rec.ground_vector.is_none(), "GBS=1 with I160");
        }
    }
    assert_eq!(ground, 100);
}
