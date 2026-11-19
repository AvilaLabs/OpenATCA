//! The encoder glue, the files and the notes (OA-CMP-007 to OA-CMP-016).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::PathBuf;

use oa_asterix::cat062::Cat062;
use oa_asterix::{decode_block, decode_stream, encode_block, Block, SourceId};
use oa_bench::legacy::{flight_ix, push_th, Legacy, NONE, NO_ALT};
use oa_compass::core::convert::*;
use oa_compass::shell::encode::{cat062_record, BlockSink, RECORDS_PER_BLOCK};
use oa_compass::shell::error::ExportError;
use oa_compass::shell::export::{read_back, Layout, SystemExport};
use oa_compass::shell::notes::{
    clock_text, data_sources_json, readme, ERAM_SOURCE, OPENATCA_SOURCE,
};
use oa_compass::shell::run::{export_eram, export_output};

/// 2026-11-15T00:00:00Z, Unix seconds, and the day count.
const DAY_S: i64 = 20_772 * 86_400;
const DAY: i64 = 20_772;

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cmp-{name}"));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// A small deterministic generator (splitmix64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn track_row(g: &mut Rng, t_ns: i64) -> TrackRow {
    let ve = ((g.unit() - 0.5) * 600.0) as f32;
    let ve = if g.next().is_multiple_of(7) { f32::NAN } else { ve };
    TrackRow {
        t_ns,
        lat_e7: ((g.unit() * 2.0 - 1.0) * 85.0e7) as i32,
        lon_e7: ((g.unit() * 2.0 - 1.0) * 179.0e7) as i32,
        vel_east_mps: ve,
        vel_north_mps: ((g.unit() - 0.5) * 600.0) as f32,
        pos_cov_m2: [
            (g.unit() * 5000.0) as f32,
            0.0,
            (g.unit() * 5000.0) as f32,
        ],
        alt_ft: ((g.unit() - 0.05) * 45_000.0) as f32,
        has_alt: !g.next().is_multiple_of(5),
        vrate_fpm: ((g.unit() - 0.5) * 8000.0) as f32,
        coast: g.next().is_multiple_of(4),
        radar_disagrees: g.next().is_multiple_of(9),
        uncertain: g.next().is_multiple_of(6),
        reacquired: g.next().is_multiple_of(6),
        address: (!g.next().is_multiple_of(8)).then(|| (g.next() % 0x100_0000) as u32),
        monosensor: true,
    }
}

fn decode_all(bytes: &[u8]) -> Vec<Cat062> {
    decode_stream(bytes)
        .unwrap()
        .into_iter()
        .flat_map(|b| match b {
            Block::Cat062(r) => r,
            _ => panic!("not category 62"),
        })
        .collect()
}

// REQ: OA-CMP-007
#[test]
fn generated_rows_of_both_systems_round_trip_through_the_decoder_with_the_values_intact() {
    let mut g = Rng(7);
    let mut sink = BlockSink::new(Vec::new(), OPENATCA_SOURCE, "OpenATCA");
    let mut want = Vec::new();
    for i in 0..3000i64 {
        let r = track_row(&mut g, DAY_S * 1_000_000_000 + i * 250_000_000);
        let mut c = track_record(&r).unwrap();
        c.values.track_number = (i % 500 + 1) as u16;
        sink.push(&c.values, (r.lat_e7, r.lon_e7)).unwrap();
        want.push((r, c.values));
    }
    let (bytes, stats) = sink.finish().unwrap();
    assert_eq!(stats.records, 3000);
    assert_eq!(stats.blocks, 3000u64.div_ceil(RECORDS_PER_BLOCK as u64));
    assert_eq!(stats.bytes, bytes.len() as u64);
    assert!(stats.max_lat_error_e7 <= 27 && stats.max_lon_error_e7 <= 27);
    let recs = decode_all(&bytes);
    assert_eq!(recs.len(), 3000);
    for (rec, (row, v)) in recs.iter().zip(&want) {
        // The decoded record is the record built from the values, and its fields are the values.
        assert_eq!(rec, &cat062_record(v, OPENATCA_SOURCE));
        assert_eq!(rec.source, OPENATCA_SOURCE);
        assert_eq!(rec.time, Some(v.time));
        let p = rec.position.unwrap();
        assert_eq!((p.lat, p.lon), (v.lat, v.lon));
        assert_eq!(rec.velocity.map(|x| (x.vx, x.vy)), v.velocity);
        assert_eq!(rec.baro_altitude.map(|a| a.raw), v.baro_alt);
        assert!(rec.baro_altitude.is_none_or(|a| !a.qnh));
        assert_eq!(rec.measured_fl, None);
        assert_eq!(rec.climb_rate, v.climb);
        assert_eq!(rec.derived.as_ref().and_then(|d| d.address), v.address);
        assert_eq!(rec.track_number, Some(v.track_number));
        // I015 and I500 as raw items; the item present exactly when the value is.
        let i015 = rec.raw.iter().find(|r| r.frn == 3).unwrap();
        assert_eq!(i015.bytes, vec![1]);
        let i500 = rec.raw.iter().find(|r| r.frn == 27);
        match (i500, v.accuracy) {
            (Some(i), Some((la, lo))) => {
                assert_eq!(i.bytes[0], 0x20);
                assert_eq!(u16::from_be_bytes([i.bytes[1], i.bytes[2]]), la);
                assert_eq!(u16::from_be_bytes([i.bytes[3], i.bytes[4]]), lo);
            }
            (None, None) => {}
            other => panic!("{other:?}"),
        }
        // Status: CNF clear, CST in the third extent when coasting, AFF in the first.
        let st = rec.track_status.as_ref().unwrap();
        assert!(!st.tentative && st.monosensor && !st.spi && !st.geometric_more_reliable);
        assert_eq!(st.altitude_source, 0);
        match (row.coast, row.radar_disagrees) {
            (true, aff) => assert_eq!(st.ext, vec![(u8::from(aff) * 8) | 1, 1, 0x80]),
            (false, true) => assert_eq!(st.ext, vec![0x08]),
            (false, false) => assert!(st.ext.is_empty()),
        }
    }
}

// REQ: OA-CMP-007
#[test]
fn eram_rows_round_trip_with_address_and_identification() {
    let mut g = Rng(11);
    let mut sink = BlockSink::new(Vec::new(), ERAM_SOURCE, "ERAM");
    let mut want = Vec::new();
    for i in 0..700i64 {
        let e = EramRow {
            pos_ms: DAY_S * 1000 + i * 12_000,
            lat_e7: ((g.unit() * 2.0 - 1.0) * 80.0e7) as i32,
            lon_e7: ((g.unit() * 2.0 - 1.0) * 170.0e7) as i32,
            alt_ft: (!g.next().is_multiple_of(4)).then(|| (g.next() % 45_000) as i32),
            vel_dkt: (!g.next().is_multiple_of(4))
                .then(|| ((g.next() % 6000) as i16 - 3000, (g.next() % 6000) as i16 - 3000)),
            coast: g.next().is_multiple_of(3),
            address: (!g.next().is_multiple_of(3)).then(|| (g.next() % 0x100_0000) as u32),
            callsign: (!g.next().is_multiple_of(3)).then_some(*b"DAL42\0\0\0"),
        };
        let mut c = eram_record(&e).unwrap();
        c.values.track_number = (i % 60 + 1) as u16;
        sink.push(&c.values, (e.lat_e7, e.lon_e7)).unwrap();
        want.push(c.values);
    }
    let (bytes, _) = sink.finish().unwrap();
    for (rec, v) in decode_all(&bytes).iter().zip(&want) {
        assert_eq!(rec, &cat062_record(v, ERAM_SOURCE));
        assert_eq!(rec.measured_fl, v.measured_fl);
        assert_eq!(rec.baro_altitude.map(|a| (a.raw, a.qnh)), v.baro_alt.map(|r| (r, v.baro_qnh)));
        assert_eq!(rec.derived.as_ref().and_then(|d| d.address), v.address);
        assert_eq!(rec.derived.as_ref().and_then(|d| d.ident), v.ident);
        // No I015, no I500 for ERAM.
        assert!(rec.raw.is_empty());
    }
}

// REQ: OA-CMP-008
#[test]
fn a_record_with_every_item_fits_the_stated_bounds() {
    let v = RecordValues {
        service: Some(255),
        time: 11_059_199,
        lat: -(1 << 24),
        lon: (1 << 25) - 1,
        velocity: Some((i16::MIN, i16::MAX)),
        track_number: u16::MAX,
        coasting: true,
        monosensor: true,
        aff: true,
        address: Some(0xFF_FFFF),
        ident: Some(*b"ABCDEFGH"),
        baro_alt: Some(6000),
        baro_qnh: true,
        measured_fl: Some(-60),
        climb: Some(i16::MIN),
        accuracy: Some((u16::MAX, u16::MAX)),
    };
    let block = Block::Cat062(vec![cat062_record(&v, OPENATCA_SOURCE)]);
    let bytes = encode_block(&block).unwrap();
    assert_eq!(bytes.len() - 3, 49);
    const _: () = assert!(3 + RECORDS_PER_BLOCK * 49 < 65_535 / 4);
    // A block of the most records is far under the LEN; the encoder refuses one over it.
    let many = Block::Cat062(vec![cat062_record(&v, OPENATCA_SOURCE); RECORDS_PER_BLOCK]);
    let bytes = encode_block(&many).unwrap();
    assert_eq!(bytes.len(), 3 + RECORDS_PER_BLOCK * 49);
    let (back, n) = decode_block(&bytes).unwrap();
    assert_eq!((back, n), (many, bytes.len()));
}

// REQ: OA-CMP-008
#[test]
fn the_sink_cuts_blocks_at_the_record_limit() {
    let mut g = Rng(3);
    let mut sink = BlockSink::new(Vec::new(), OPENATCA_SOURCE, "OpenATCA");
    for i in 0..(RECORDS_PER_BLOCK as i64 * 2 + 1) {
        let r = track_row(&mut g, DAY_S * 1_000_000_000 + i * 1_000_000_000);
        let c = track_record(&r).unwrap();
        sink.push(&c.values, (r.lat_e7, r.lon_e7)).unwrap();
    }
    let (bytes, stats) = sink.finish().unwrap();
    assert_eq!(stats.blocks, 3);
    let blocks = decode_stream(&bytes).unwrap();
    let sizes: Vec<usize> = blocks
        .iter()
        .map(|b| match b {
            Block::Cat062(r) => r.len(),
            _ => 0,
        })
        .collect();
    assert_eq!(sizes, vec![RECORDS_PER_BLOCK, RECORDS_PER_BLOCK, 1]);
}

fn converted(t_ns: i64, lat_e7: i32) -> Converted {
    let mut g = Rng(1);
    let mut r = track_row(&mut g, t_ns);
    r.lat_e7 = lat_e7;
    r.lon_e7 = 100_000_000;
    track_record(&r).unwrap()
}

fn export_to(name: &str, split: bool, day: i64) -> (SystemExport, PathBuf) {
    let out = tmp(name);
    let layout = Layout {
        out: out.clone(),
        day,
        split,
    };
    (SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, layout), out)
}

// REQ: OA-CMP-009
#[test]
fn a_row_on_another_day_refuses_the_export_and_leaves_no_file() {
    let (mut ex, out) = export_to("midnight", false, DAY);
    let t = DAY_S * 1_000_000_000;
    ex.push(&converted(t, 100_000_000), 1, (100_000_000, 100_000_000)).unwrap();
    // One second into the next day.
    let e = ex
        .push(&converted(t + 86_401 * 1_000_000_000, 100_000_000), 1, (0, 0))
        .unwrap_err();
    assert!(matches!(e, ExportError::CrossesMidnight { .. }), "{e}");
    let text = e.to_string();
    assert!(text.contains("2026.11.16") && text.contains("2026.11.15"), "{text}");
    ex.abort();
    let left: Vec<_> = fs::read_dir(&out).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
}

// REQ: OA-CMP-009
#[test]
fn split_days_writes_one_directory_per_day_and_no_file_holds_two_days() {
    let (mut ex, out) = export_to("split", true, DAY);
    let t = DAY_S * 1_000_000_000;
    for i in 0..10 {
        ex.push(&converted(t - 5 * 1_000_000_000 + i * 1_000_000_000, 100_000_000), 1, (0, 0))
            .ok();
    }
    // Rows are on the 14th (before t) and the 15th; the file of the 14th is separate.
    let fin = ex.finish().unwrap();
    let report = fin.commit().unwrap();
    assert_eq!(report.days.len(), 2);
    for d in &report.days {
        let dir = out.join(oa_compass::shell::export::date_text(d.day));
        let bytes = fs::read(dir.join("openatca.ast")).unwrap();
        let recs = decode_all(&bytes);
        assert_eq!(recs.len() as u64, d.sink.records);
        assert!(!dir.join("openatca.ast.partial").exists());
    }
    let days: Vec<i64> = report.days.iter().map(|d| d.day).collect();
    assert_eq!(days, vec![DAY - 1, DAY]);
    assert_eq!(report.days[0].sink.records, 5);
    assert_eq!(report.days[1].sink.records, 5);
}

// REQ: OA-CMP-010
#[test]
fn track_numbers_are_stable_per_key_from_one_and_never_wrap() {
    let (mut ex, out) = export_to("tracks", false, DAY);
    let t = DAY_S * 1_000_000_000;
    // Keys A, B, A, C: numbers 1, 2, 1, 3.
    for (i, k) in [500u32, 7, 500, 99_999].iter().enumerate() {
        ex.push(&converted(t + i as i64 * 1_000_000_000, 100_000_000), *k, (0, 0)).unwrap();
    }
    let rep = ex.finish().unwrap().commit().unwrap();
    assert_eq!(rep.days[0].tracks, 3);
    let recs = decode_all(&fs::read(out.join("openatca.ast")).unwrap());
    let nums: Vec<u16> = recs.iter().map(|r| r.track_number.unwrap()).collect();
    assert_eq!(nums, vec![1, 2, 1, 3]);
    // 65 535 keys fit, the next is refused.
    let (mut ex, _) = export_to("tracks-full", false, DAY);
    for k in 0..65_535u32 {
        ex.push(&converted(t, 100_000_000), k, (0, 0)).unwrap();
    }
    ex.push(&converted(t, 100_000_000), 0, (0, 0)).unwrap();
    let e = ex.push(&converted(t, 100_000_000), 65_535, (0, 0)).unwrap_err();
    assert!(matches!(e, ExportError::TrackNumbers { .. }), "{e}");
    ex.abort();
}

// REQ: OA-CMP-011
#[test]
fn a_record_earlier_than_the_one_before_is_refused() {
    let (mut ex, _) = export_to("order", false, DAY);
    let t = DAY_S * 1_000_000_000;
    ex.push(&converted(t + 5_000_000_000, 100_000_000), 1, (0, 0)).unwrap();
    ex.push(&converted(t + 5_000_000_000, 100_000_000), 2, (0, 0)).unwrap();
    let e = ex.push(&converted(t + 4_000_000_000, 100_000_000), 1, (0, 0)).unwrap_err();
    assert!(matches!(e, ExportError::OutOfOrder { .. }), "{e}");
    ex.abort();
}

fn eram_legacy() -> Legacy {
    let mut leg = Legacy::default();
    let a = flight_ix(&mut leg, 1).unwrap();
    let b = flight_ix(&mut leg, 2).unwrap();
    leg.flights.callsign[a as usize] = *b"UAL1\0\0\0\0";
    leg.flights.addr[a as usize] = 0xABCDEF;
    leg.flights.callsign[b as usize] = *b"bad-cs\0\0";
    let t0 = DAY_S * 1000;
    // Pushed out of time order, with a duplicate, a conflict and a row for an unknown flight.
    push_th(&mut leg, a, t0 + 12_000, 0, 400_000_000, -880_000_000, 35_000, Some((2000, 0)), false);
    push_th(&mut leg, b, t0, 0, 410_000_000, -870_000_000, NO_ALT, None, true);
    push_th(&mut leg, a, t0, 0, 400_000_000, -880_000_000, 35_000, Some((2000, 0)), false);
    push_th(&mut leg, a, t0, 0, 400_000_000, -880_000_000, 35_000, Some((2000, 0)), false);
    push_th(&mut leg, a, t0, 0, 400_100_000, -880_000_000, 35_000, Some((2000, 0)), false);
    push_th(&mut leg, 77, t0, 0, 400_000_000, -880_000_000, 35_000, None, false);
    leg
}

// REQ: OA-CMP-012
#[test]
fn eram_samples_are_sorted_deduplicated_and_counted() {
    let out = tmp("eram");
    let mut ex = SystemExport::new(
        "ERAM",
        "eram.ast",
        ERAM_SOURCE,
        Layout {
            out: out.clone(),
            day: DAY,
            split: false,
        },
    );
    let info = export_eram(&eram_legacy(), &mut ex).unwrap();
    assert_eq!(info.rows_in, 6);
    assert_eq!(info.exact_duplicates, 1);
    assert_eq!(info.conflicting_same_time, 1);
    assert_eq!(info.unknown_flight, 1);
    let rep = ex.finish().unwrap().commit().unwrap();
    assert_eq!(rep.days[0].sink.records, 4);
    let recs = decode_all(&fs::read(out.join("eram.ast")).unwrap());
    let times: Vec<u32> = recs.iter().map(|r| r.time.unwrap()).collect();
    assert_eq!(times, vec![0, 0, 0, 12 * 128]);
    // At the same time the lower flight index comes first: flight a's two different rows (in
    // input order), then flight b, then flight a's later sample.
    let ids: Vec<Option<[u8; 8]>> = recs
        .iter()
        .map(|r| r.derived.as_ref().and_then(|d| d.ident))
        .collect();
    assert_eq!(ids[0], Some(*b"UAL1    "));
    assert_eq!(ids[1], Some(*b"UAL1    "));
    assert_eq!(ids[2], None); // "bad-cs" is not encodable: omitted, not altered
    assert_eq!(recs[2].derived, None);
    assert_eq!(recs[0].derived.as_ref().unwrap().address, Some(0xABCDEF));
    let lat = |r: &Cat062| r.position.unwrap().lat;
    assert_ne!(lat(&recs[0]), lat(&recs[1]));
    // Flight b coasts and has no altitude and no velocity.
    assert_eq!(recs[2].measured_fl, None);
    assert_eq!(recs[2].velocity, None);
    assert_eq!(recs[2].track_status.as_ref().unwrap().ext, vec![1, 1, 0x80]);
    assert_eq!(recs[0].measured_fl, Some(1400));
    // Tracks: a is 1 (first appearance), b is 2.
    assert_eq!(recs[0].track_number, Some(1));
    assert_eq!(recs[2].track_number, Some(2));
    assert_eq!(recs[3].track_number, Some(1));
    assert_eq!(NONE, u32::MAX);
}

// REQ: OA-CMP-013
#[test]
fn a_file_that_does_not_read_back_as_written_fails() {
    let mut g = Rng(5);
    let mut sink = BlockSink::new(Vec::new(), OPENATCA_SOURCE, "OpenATCA");
    for i in 0..600i64 {
        let r = track_row(&mut g, DAY_S * 1_000_000_000 + i * 1_000_000_000);
        let c = track_record(&r).unwrap();
        sink.push(&c.values, (r.lat_e7, r.lon_e7)).unwrap();
    }
    let (bytes, stats) = sink.finish().unwrap();
    let dir = tmp("readback");
    let p = dir.join("f.ast");
    fs::write(&p, &bytes).unwrap();
    assert_eq!(
        read_back(&p, OPENATCA_SOURCE, stats.records, stats.bytes).unwrap(),
        (600, 3)
    );
    // Wrong expected count, wrong source.
    assert!(read_back(&p, OPENATCA_SOURCE, 599, stats.bytes).is_err());
    assert!(read_back(&p, ERAM_SOURCE, stats.records, stats.bytes).is_err());
    // Truncated in the last block.
    fs::write(&p, &bytes[..bytes.len() - 5]).unwrap();
    assert!(read_back(&p, OPENATCA_SOURCE, stats.records, stats.bytes).is_err());
    // A byte flipped in the first record's time of day, so the second block's times go back.
    let mut bad = bytes.clone();
    let mut lens = Vec::new();
    let mut pos = 0;
    while pos < bad.len() {
        lens.push(pos);
        pos += usize::from(u16::from_be_bytes([bad[pos + 1], bad[pos + 2]]));
    }
    // Zero the time of the first record of the third block (FSPEC 4 octets, I010 2, I015 1).
    let third = lens[2];
    let fspec_len = bad[third + 3..].iter().position(|b| b & 1 == 0).unwrap() + 1;
    let time_at = third + 3 + fspec_len + 2 + 1;
    bad[time_at..time_at + 3].copy_from_slice(&[0, 0, 0]);
    fs::write(&p, &bad).unwrap();
    assert!(read_back(&p, OPENATCA_SOURCE, stats.records, stats.bytes).is_err());
    // A missing file.
    assert!(read_back(&dir.join("none.ast"), OPENATCA_SOURCE, 0, 0).is_err());
}

// REQ: OA-CMP-014
#[test]
fn the_data_source_file_parses_and_names_both_systems() {
    let text = data_sources_json();
    let doc = oa_json::parse(text.as_bytes(), &oa_json::DEFAULT_LIMITS).unwrap();
    let root = oa_json::NodeIx(0);
    let get = |o, k: &str| doc.get(o, k).unwrap();
    assert_eq!(doc.str(get(root, "content_type")), Some("data_sources"));
    assert_eq!(doc.str(get(root, "content_version")), Some("0.2"));
    let list = get(root, "data_sources");
    let items: Vec<_> = doc.elements(list).collect();
    assert_eq!(items.len(), 3);
    let want = [
        ("OpenATCA", 255, 1, "Tracker"),
        ("ERAM", 255, 2, "Tracker"),
        ("ADS-B", 255, 3, "ADS-B"),
    ];
    for (it, (name, sac, sic, ty)) in items.iter().zip(want) {
        assert_eq!(doc.str(get(*it, "ds_type")), Some(ty));
        assert_eq!(doc.str(get(*it, "name")), Some(name));
        assert_eq!(doc.as_i64(get(*it, "sac")), Some(sac));
        assert_eq!(doc.as_i64(get(*it, "sic")), Some(sic));
    }
    assert_eq!(OPENATCA_SOURCE, SourceId { sac: 255, sic: 1 });
    assert_eq!(ERAM_SOURCE, SourceId { sac: 255, sic: 2 });
}

// REQ: OA-CMP-015
#[test]
fn the_readme_states_the_date_the_sources_the_items_and_every_omission_with_its_count() {
    let out = tmp("readme");
    let layout = Layout {
        out: out.clone(),
        day: DAY,
        split: false,
    };
    let mut ex = SystemExport::new("ERAM", "eram.ast", ERAM_SOURCE, layout);
    let info = export_eram(&eram_legacy(), &mut ex).unwrap();
    ex.counts.push(("samples in the input".into(), info.rows_in));
    let mut fin = ex.finish().unwrap();
    fin.report.inputs.push("test input".into());
    let rep = fin.commit().unwrap();
    let text = readme(DAY, &[&rep], "commit test", false, None);
    for needle in [
        "UTC date to give at import (UTC Day): 2026.11.15",
        "ERAM  (SAC 255 SIC 2)",
        "records written: 4 in 1 data blocks",
        "I136 measured flight level (ERAM at or above 18 000 ft): 3",
        "I500 accuracies, APW only: 0",
        "4: I500 accuracy: no covariance in the source",
        "4: I060 Mode 3/A code: not in the source row",
        "ERAM publishes no accuracy",
        "samples in the input: 6",
        "ingest: SFDPS documents offered: 0",
        "ingest: flight table overflow (must be 0): 0",
        "a row differing only in receive time is a duplicate",
        "it is NOT established as pressure altitude",
        "MON = 0 for ERAM",
        "not an outside conformance test",
        "input: test input",
        "Edition 1.21",
        "commit test",
        "SAC 255 is a test pair",
        "00:00:00.000 to 00:00:12.000 UTC",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in\n{text}");
    }
    assert_eq!(clock_text(43_200 * 128 + 64), "12:00:00.500");
}

// REQ: OA-CMP-016
#[test]
fn tracker_output_exports_ticks_only_and_counts_the_update_rows() {
    use oa_track::{output_new, TrackOutput, O_COAST, O_HAS_ALT, O_TICK, O_UNCERTAIN};
    // Build a table by hand: two tick rows and one update row.
    let mut o: TrackOutput = output_new(8);
    let mut push = |flags: u8, t_s: i64, addr: u32| {
        o.t_ns.push(DAY_S * 1_000_000_000 + t_s * 1_000_000_000);
        o.addr.push(addr);
        o.lat_e7.push(410_000_000);
        o.lon_e7.push(-880_000_000);
        o.vel_east_mps.push(100.0);
        o.vel_north_mps.push(0.0);
        o.pos_cov_m2.push([100.0, 0.0, 100.0]);
        o.vel_cov_m2s2.push([1.0, 0.0, 1.0]);
        o.turn_rate_rps.push(0.0);
        o.alt_ft.push(if flags & O_HAS_ALT != 0 { 30_000.0 } else { f32::NAN });
        o.alt_var_ft2.push(1.0);
        o.vrate_fpm.push(0.0);
        o.mu_h.push([1.0, 0.0]);
        o.mu_v.push([1.0, 0.0]);
        o.flags.push(flags);
    };
    push(O_TICK | O_HAS_ALT, 0, 0xA00001);
    push(O_HAS_ALT, 0, 0xA00001);
    push(O_TICK | O_COAST | O_UNCERTAIN, 1, 0x4000_0005);
    let (mut ex, out) = export_to("ticks", false, DAY);
    let mut info = oa_compass::shell::run::TrackRunInfo::default();
    export_output(&o, &mut ex, &mut info, true).unwrap();
    assert_eq!(info.tick_rows, 2);
    assert_eq!(info.update_rows_not_exported, 1);
    assert_eq!((info.coasting, info.uncertain), (1, 1));
    ex.finish().unwrap().commit().unwrap();
    let recs = decode_all(&fs::read(out.join("openatca.ast")).unwrap());
    assert_eq!(recs.len(), 2);
    // A radar-handle key is no aircraft address; a coasting row has no altitude here.
    assert_eq!(recs[0].derived.as_ref().and_then(|d| d.address), Some(0xA00001));
    assert_eq!(recs[1].derived, None);
    assert_eq!(recs[1].baro_altitude, None);
    assert_eq!(recs[0].baro_altitude.map(|a| a.raw), Some(1200));
    assert_eq!(recs[1].track_status.as_ref().unwrap().ext, vec![1, 1, 0x80]);
}

// REQ: OA-CMP-007
#[test]
fn a_slice_of_a_real_recording_round_trips_through_the_tracker_and_the_encoder() {
    use oa_adsb::{reports_len, reports_push, reports_row, Reports};
    let clip = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../openatca-data/derived/adsb/zau-2026.11.13-1s.oaf");
    if !clip.exists() {
        eprintln!("skipped: {} is absent", clip.display());
        return;
    }
    let loaded = oa_bench::input::load_adsb(&clip).unwrap();
    let t_min = loaded.reports.time_ns.iter().copied().min().unwrap();
    let mut slice = Reports::default();
    for i in 0..reports_len(&loaded.reports) {
        let row = reports_row(&loaded.reports, i).unwrap();
        if row.time_ns < t_min + 180 * 1_000_000_000 {
            reports_push(&mut slice, &row);
        }
    }
    drop(loaded);
    assert!(reports_len(&slice) > 1000);
    let (meas, _) =
        oa_track::adsb::adsb_measurements(&slice, &oa_track::adsb::AdsbConfig::default());
    let cfg = oa_compass::shell::run::scorer_config(&meas);
    let out = tmp("real");
    let layout = Layout {
        out: out.clone(),
        day: 0,
        split: true,
    };
    let mut ex = SystemExport::new("OpenATCA", "openatca.ast", OPENATCA_SOURCE, layout);
    let info = oa_compass::shell::run::run_tracker(&meas, &cfg, &mut ex, true).unwrap();
    assert!(info.tick_rows > 1000, "{info:?}");
    let rep = ex.finish().unwrap().commit().unwrap();
    let written: u64 = rep.days.iter().map(|d| d.sink.records).sum();
    assert_eq!(written + rep.refused.iter().sum::<u64>(), info.tick_rows);
    for d in &rep.days {
        let dir = out.join(oa_compass::shell::export::date_text(d.day));
        let recs = decode_all(&fs::read(dir.join("openatca.ast")).unwrap());
        assert_eq!(recs.len() as u64, d.sink.records);
        assert!(d.sink.max_lat_error_e7 <= 27 && d.sink.max_lon_error_e7 <= 27);
        // Ticks are at whole seconds.
        assert!(recs.iter().all(|r| r.time.unwrap() % 128 == 0));
    }
}

// REQ: OA-CMP-017
#[test]
fn the_sfdps_ingest_counts_are_reported_and_a_full_table_refuses_the_export() {
    use oa_bench::legacy::Counts;
    use oa_compass::shell::run::eram_input_counts;
    let mut c = Counts::default();
    c.docs[0] = 9;
    c.note("SFDPS rows dropped, reason no GUFI column entry".into(), 4);
    c.note("SFDPS position rows without a position time (no TH sample)".into(), 2);
    c.th_no_alt = 7;
    let lines = eram_input_counts(&c).unwrap();
    let get = |k: &str| lines.iter().find(|(l, _)| l.contains(k)).map(|x| x.1);
    assert_eq!(get("documents offered"), Some(9));
    assert_eq!(get("no GUFI"), Some(4));
    assert_eq!(get("without a position time"), Some(2));
    assert_eq!(get("without a reported altitude"), Some(7));
    c.overflow_th = 1;
    assert!(eram_input_counts(&c).is_err());
    c.overflow_th = 0;
    c.overflow_flights = 3;
    assert!(eram_input_counts(&c).is_err());
    // export_eram refuses before writing anything.
    let mut leg = eram_legacy();
    leg.counts.overflow_th = 5;
    let out = tmp("eram-overflow");
    let mut ex = SystemExport::new(
        "ERAM",
        "eram.ast",
        ERAM_SOURCE,
        Layout { out: out.clone(), day: DAY, split: false },
    );
    assert!(export_eram(&leg, &mut ex).is_err());
    ex.abort();
    assert!(fs::read_dir(&out).unwrap().next().is_none());
}

// REQ: OA-CMP-018
#[test]
fn the_readme_states_the_inputs_mode_and_the_leak_warning() {
    use oa_compass::shell::run::InputsMode;
    let out = tmp("mode");
    let mut ex = SystemExport::new(
        "ERAM",
        "eram.ast",
        ERAM_SOURCE,
        Layout { out, day: DAY, split: false },
    );
    export_eram(&eram_legacy(), &mut ex).unwrap();
    let rep = ex.finish().unwrap().commit().unwrap();
    let all = readme(DAY, &[&rep], "b", false, Some(InputsMode::All));
    assert!(all.contains("ALL (--inputs all)") && all.contains("like-for-like"));
    let split = readme(DAY, &[&rep], "b", false, Some(InputsMode::Split));
    assert!(split.contains("SPLIT") && split.contains("NOT like-for-like") && split.contains("leaked truth"));
}

// REQ: OA-CMP-007
#[test]
fn the_verification_compares_a_wrapped_longitude_around_the_circle() {
    let mut g = Rng(9);
    let mut r = track_row(&mut g, DAY_S * 1_000_000_000);
    r.lon_e7 = 1_800_000_000;
    let c = track_record(&r).unwrap();
    let mut sink = BlockSink::new(Vec::new(), OPENATCA_SOURCE, "OpenATCA");
    sink.push(&c.values, (r.lat_e7, r.lon_e7)).unwrap();
    let (bytes, stats) = sink.finish().unwrap();
    assert!(stats.max_lon_error_e7 <= 27, "{}", stats.max_lon_error_e7);
    assert_eq!(decode_all(&bytes)[0].position.unwrap().lon, -(1 << 25));
}
