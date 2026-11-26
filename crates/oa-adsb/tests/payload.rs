//! The OAF1 MESSAGE payload of ADSB reports (docs/ADSB_PAYLOAD.md) and the summary checks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::*;

fn sample_rows(n: usize) -> Reports {
    let mut r = Reports::default();
    for i in 0..n {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00000 + (i as u32 % 7);
        row.time_ns = 1_700_000_000_000_000_000 + i as i64 * 5_000_000_000;
        row.lat_e7 = 400_000_000 + i as i32 * 1000;
        row.lon_e7 = -900_000_000 - i as i32 * 1000;
        row.baro_alt_ft = 30000 - i as i32;
        row.gs_dkt = 4500;
        row.track_ddeg = ((i % 360) * 10) as u16;
        row.flags = (i % 32) as u8;
        row.source = (i % 14) as u8;
        row.callsign = *b"UAL123\0\0";
        row.squawk = 1200;
        row.nac_p = (i % 12) as u8;
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-ADSB-010
#[test]
fn payload_round_trips_and_has_the_documented_layout() {
    let r = sample_rows(100);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 10, 60).unwrap();
    assert_eq!(p.len(), PAYLOAD_HEADER_LEN + 50 * RECORD_LEN);
    assert_eq!(&p[..4], b"ADS3");
    assert_eq!(RECORD_LEN, 80);
    assert_eq!(u16::from_le_bytes([p[4], p[5]]), 80);
    assert_eq!(u32::from_le_bytes([p[8], p[9], p[10], p[11]]), 50);
    // First record: icao at 0, lat at 4, time at 24, callsign at 40, nac_p at 50.
    let rec = &p[16..96];
    let row10 = reports_row(&r, 10).unwrap();
    assert_eq!(
        u32::from_le_bytes(rec[0..4].try_into().unwrap()),
        row10.icao
    );
    assert_eq!(
        i32::from_le_bytes(rec[4..8].try_into().unwrap()),
        row10.lat_e7
    );
    assert_eq!(
        i64::from_le_bytes(rec[24..32].try_into().unwrap()),
        row10.time_ns
    );
    assert_eq!(&rec[40..48], b"UAL123\0\0");
    assert_eq!(rec[50], row10.nac_p);
    // Measurements at 57..79 and the NIC_baro byte (79) are absent in this sample.
    assert!(rec[57..71].iter().all(|&b| b == 0xff));
    assert!(rec[71..79].chunks(2).all(|c| c == [0x00, 0x80]));
    assert_eq!(rec[79], 0);
    let mut back = Reports::default();
    assert_eq!(decode_payload(&p, &mut back).unwrap(), 50);
    for i in 0..50 {
        assert_eq!(reports_row(&back, i), reports_row(&r, 10 + i));
    }
}

// REQ: OA-ADSB-011
#[test]
fn bad_payloads_are_refused_and_append_nothing() {
    let r = sample_rows(5);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 5).unwrap();
    let mut out = Reports::default();
    let mut bad = |b: &[u8], want: PayloadError| {
        assert_eq!(decode_payload(b, &mut out).unwrap_err(), want);
        assert_eq!(reports_len(&out), 0);
    };
    bad(&p[..10], PayloadError::Length);
    bad(&p[..p.len() - 1], PayloadError::Length);
    let mut m = p.clone();
    m[0] = b'X';
    bad(&m, PayloadError::Magic);
    let mut m = p.clone();
    m[4] = 65;
    bad(&m, PayloadError::Layout);
    let mut m = p.clone();
    m[6] = 1;
    bad(&m, PayloadError::Layout);
    let mut m = p.clone();
    m[12] = 1;
    bad(&m, PayloadError::Layout);
    let mut m = p.clone();
    m[8..12].copy_from_slice(&(MAX_ROWS_PER_FRAME as u32 + 1).to_le_bytes());
    bad(&m, PayloadError::TooManyRows);
    // Record 3: latitude out of range, then unknown flag bits, then unknown source, then a
    // non-zero reserved byte.
    for (at, v) in [(4usize, 0x7fu8), (38, 0x80), (39, 14), (79, 9)] {
        let mut m = p.clone();
        let rec = 16 + 3 * RECORD_LEN;
        if at == 4 {
            m[rec + 7] = v;
        } else {
            m[rec + at] = v;
        }
        bad(&m, PayloadError::BadRecord(3));
    }
    // Encoder range checks.
    let mut q = Vec::new();
    assert_eq!(encode_payload(&mut q, &r, 3, 3), Err(PayloadError::Range));
    assert_eq!(encode_payload(&mut q, &r, 0, 6), Err(PayloadError::Range));
    let big = sample_rows(MAX_ROWS_PER_FRAME + 1);
    assert_eq!(
        encode_payload(&mut q, &big, 0, MAX_ROWS_PER_FRAME + 1),
        Err(PayloadError::TooManyRows)
    );
    assert!(encode_payload(&mut q, &big, 0, MAX_ROWS_PER_FRAME).is_ok());
    assert!(q.len() <= oa_feed::MAX_PAYLOAD);
}

// REQ: OA-ADSB-011
#[test]
fn lenient_decode_reports_each_dropped_input_ordinal_and_keeps_siblings() {
    let source = sample_rows(5);
    let mut payload = Vec::new();
    encode_payload(&mut payload, &source, 0, 5).unwrap();
    // Corrupt the latitude of input rows 1 and 3. The other three records remain independently
    // decodable and their original input ordinals are not renumbered by the drops.
    for row in [1usize, 3] {
        let at = PAYLOAD_HEADER_LEN + row * RECORD_LEN;
        payload[at + 7] = 0x7f;
    }
    let mut decoded = Reports::default();
    let mut dropped = Vec::new();
    let counts = decode_payload_lenient_with_dropped(&payload, &mut decoded, |ordinal| {
        dropped.push(ordinal);
    })
    .unwrap();
    assert_eq!(counts.records, 5);
    assert_eq!(counts.kept, 3);
    assert_eq!(counts.dropped, 2);
    assert_eq!(counts.first_dropped, Some(1));
    assert_eq!(dropped, [1, 3]);
    assert_eq!(reports_len(&decoded), 3);
    for (decoded_ordinal, source_ordinal) in [0usize, 2, 4].into_iter().enumerate() {
        assert_eq!(
            reports_row(&decoded, decoded_ordinal).unwrap(),
            reports_row(&source, source_ordinal).unwrap()
        );
    }
}

fn track(points: &[(u32, i64, i32, i32, u8, u8)]) -> Reports {
    let mut r = Reports::default();
    for &(icao, t_s, lat, lon, flags, nac_p) in points {
        let mut row = ROW_EMPTY;
        row.icao = icao;
        row.time_ns = t_s * 1_000_000_000;
        row.lat_e7 = lat;
        row.lon_e7 = lon;
        row.flags = flags;
        row.nac_p = nac_p;
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-ADSB-012
#[test]
fn summary_counts_aircraft_nac_p_and_position_jumps() {
    // One degree of latitude is 60 NM. 60 NM in 600 s is 360 kt: fine. 60 NM in 60 s is 3600 kt.
    let r = track(&[
        (1, 0, 400_000_000, -900_000_000, 0, 8),
        (1, 600, 410_000_000, -900_000_000, 0, 7),
        (1, 660, 420_000_000, -900_000_000, 0, NONE_U8), // 3600 kt: a jump
        (1, 720, 430_000_000, -900_000_000, F_NEW_LEG, 11), // would be a jump, but a new leg
        (2, 0, 400_000_000, -900_000_000, 0, 9),
        (2, 0, 400_000_000, -900_000_000, 0, 9), // same instant, same place: fine
        (2, 0, 400_100_000, -900_000_000, 0, 9), // same instant, 0.6 NM away: a jump
        (2, 0, 400_102_000, -900_000_000, 0, 9), // same instant, 0.012 NM from the last: noise
    ]);
    let s = summarize(&r);
    assert_eq!((s.reports, s.aircraft), (8, 2));
    assert_eq!((s.nac_p_known, s.nac_p_ge_8), (7, 6));
    assert_eq!((s.pairs, s.jumps, s.jitter), (5, 2, 1));
    assert!(s.max_implied_kt > 3500.0);
    // Input order does not matter; rows are ordered by aircraft and time inside.
    let mut rev = Reports::default();
    for i in (0..reports_len(&r)).rev() {
        reports_push(&mut rev, &reports_row(&r, i).unwrap());
    }
    let s2 = summarize(&rev);
    assert_eq!((s2.aircraft, s2.pairs, s2.jumps, s2.jitter), (2, 5, 2, 1));
    assert_eq!(summarize(&Reports::default()), Summary::default());
}

fn at_times(ms: &[i64]) -> Reports {
    let mut r = Reports::default();
    for &m in ms {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + m * 1_000_000;
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-ADSB-019
#[test]
fn frame_ranges_split_by_time_and_by_count_and_cover_every_row_once() {
    // A report exactly `frame_ms` after the first starts a new frame; one ms earlier does not.
    let r = at_times(&[0, 999, 1000, 1500, 1999, 2000, 2000, 5000]);
    assert_eq!(frame_ranges(&r, 1000), [(0, 2), (2, 5), (5, 7), (7, 8)]);
    assert_eq!(
        frame_ranges(&r, 1),
        [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 7), (7, 8)]
    );
    assert_eq!(frame_ranges(&r, 60_000), [(0, 8)]);
    // Out-of-range spans are clamped, never zero-length frames.
    assert_eq!(frame_ranges(&r, 0), frame_ranges(&r, 1));
    assert_eq!(frame_ranges(&r, u32::MAX), [(0, 8)]);
    assert!(frame_ranges(&Reports::default(), 1000).is_empty());
    // Count: many reports in one instant split at the per-frame maximum.
    let n = MAX_ROWS_PER_FRAME * 2 + 7;
    let r = at_times(&vec![0; n]);
    let m = MAX_ROWS_PER_FRAME;
    assert_eq!(frame_ranges(&r, 1000), [(0, m), (m, 2 * m), (2 * m, n)]);
    // Property: contiguous, non-empty, in order, covering every row, within the limits.
    let mut ms = Vec::new();
    let mut t = 0i64;
    for i in 0..5000i64 {
        t += (i * 7919) % 400;
        ms.push(t);
    }
    let r = at_times(&ms);
    for span in [1, 10, 250, 1000, 60_000] {
        let ranges = frame_ranges(&r, span);
        let mut next = 0;
        for &(a, b) in &ranges {
            assert_eq!(a, next);
            assert!(b > a && b - a <= MAX_ROWS_PER_FRAME);
            assert!(r.time_ns[b - 1] - r.time_ns[a] < i64::from(span) * 1_000_000);
            next = b;
        }
        assert_eq!(next, ms.len());
    }
}

// REQ: OA-ADSB-020
#[test]
fn payloads_of_any_grouping_decode_to_the_same_reports() {
    // An old clip (one frame of many seconds) and a new one (a frame per second) hold the same
    // reports; decoding either gives the same rows in the same order.
    let r = at_times(&(0..4000).map(|i| i * 10).collect::<Vec<_>>());
    let decode_all = |ranges: &[(usize, usize)]| {
        let mut out = Reports::default();
        for &(a, b) in ranges {
            let mut p = Vec::new();
            encode_payload(&mut p, &r, a, b).unwrap();
            assert_eq!(decode_payload(&p, &mut out).unwrap(), b - a);
        }
        out
    };
    let old = decode_all(&[(0, 4000)]);
    let new = decode_all(&frame_ranges(&r, DEFAULT_FRAME_MS));
    assert!(frame_ranges(&r, DEFAULT_FRAME_MS).len() > 30);
    assert_eq!(old, new);
    assert_eq!(old, r);
}

fn measured_row() -> Row {
    Row {
        icao: 0xabc123,
        time_ns: 1_700_000_000_250_000_000,
        lat_e7: 420_000_000,
        lon_e7: -880_000_000,
        baro_alt_ft: 35000,
        gs_dkt: 4800,
        track_ddeg: 2700,
        ws_kt: 62,
        wd_deg: 285,
        tas_kt: 470,
        ias_kt: 255,
        mach_milli: 812,
        mag_heading_cdeg: 26_987,
        true_heading_cdeg: 27_650,
        oat_c: -54,
        tat_c: -29,
        roll_cdeg: -1_072,
        track_rate_cdps: -3,
        ..ROW_EMPTY
    }
}

// REQ: OA-ADSB-021
#[test]
fn measurements_round_trip_at_the_documented_offsets() {
    let mut r = Reports::default();
    reports_push(&mut r, &measured_row());
    reports_push(&mut r, &ROW_EMPTY_IN_BOX);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 2).unwrap();
    let rec = &p[16..96];
    let u = |at: usize| u16::from_le_bytes([rec[at], rec[at + 1]]);
    let s = |at: usize| i16::from_le_bytes([rec[at], rec[at + 1]]);
    assert_eq!((u(57), u(59), u(61), u(63)), (62, 285, 470, 255));
    assert_eq!((u(65), u(67), u(69)), (812, 26_987, 27_650));
    assert_eq!((s(71), s(73), s(75), s(77)), (-54, -29, -1_072, -3));
    let mut back = Reports::default();
    assert_eq!(decode_payload(&p, &mut back).unwrap(), 2);
    assert_eq!(reports_row(&back, 0), Some(measured_row()));
    assert_eq!(reports_row(&back, 1), Some(ROW_EMPTY_IN_BOX));
    assert_eq!(back.ws_kt, [62, NONE_U16]);
    assert_eq!(back.oat_c, [-54, NONE_I16]);
    // A corrupt measurement is refused: direction 360, heading 360.00, roll beyond 180.
    for (at, v) in [(59usize, 360u16), (67, 36_000), (75, 18_001)] {
        let mut m = p.clone();
        m[16 + at..16 + at + 2].copy_from_slice(&v.to_le_bytes());
        let mut out = Reports::default();
        assert_eq!(
            decode_payload(&m, &mut out).unwrap_err(),
            PayloadError::BadRecord(0)
        );
        assert_eq!(reports_len(&out), 0);
    }
}

const ROW_EMPTY_IN_BOX: Row = Row {
    icao: 0xabc124,
    time_ns: 1_700_000_001_000_000_000,
    lat_e7: 420_000_000,
    lon_e7: -880_000_000,
    ..ROW_EMPTY
};

/// An `ADS1` payload as the first clips wrote it: 64-byte records, bytes 57 to 63 zero.
fn legacy_payload(r: &Reports) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"ADS1");
    v.extend_from_slice(&64u16.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&(reports_len(r) as u32).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    for i in 0..reports_len(r) {
        let mut p = Vec::new();
        let mut one = Reports::default();
        reports_push(&mut one, &reports_row(r, i).unwrap());
        encode_payload(&mut p, &one, 0, 1).unwrap();
        // The first 57 bytes of an ADS2 record are the ADS1 record.
        v.extend_from_slice(&p[16..16 + 57]);
        v.extend_from_slice(&[0; 7]);
    }
    v
}

// REQ: OA-ADSB-022
#[test]
fn legacy_ads1_payloads_still_decode_with_measurements_absent() {
    let r = sample_rows(40);
    let p = legacy_payload(&r);
    assert_eq!(p.len(), PAYLOAD_HEADER_LEN + 40 * LEGACY_RECORD_LEN);
    let mut back = Reports::default();
    assert_eq!(decode_payload(&p, &mut back).unwrap(), 40);
    assert_eq!(back, r);
    assert!(back.ws_kt.iter().all(|&x| x == NONE_U16));
    // Legacy rules still hold: a non-zero byte after 57 is refused; so is a 80-byte record
    // length under the old magic; the old count limit applies, not the new one.
    let mut m = p.clone();
    m[16 + 3 * 64 + 60] = 1;
    let mut out = Reports::default();
    assert_eq!(
        decode_payload(&m, &mut out).unwrap_err(),
        PayloadError::BadRecord(3)
    );
    let mut m = p.clone();
    m[4] = 80;
    assert_eq!(
        decode_payload(&m, &mut out).unwrap_err(),
        PayloadError::Layout
    );
    let mut m = p[..16].to_vec();
    m[8..12].copy_from_slice(&(LEGACY_MAX_ROWS as u32 + 1).to_le_bytes());
    assert_eq!(
        decode_payload(&m, &mut out).unwrap_err(),
        PayloadError::TooManyRows
    );
    let mut m = p[..16].to_vec();
    m[8..12].copy_from_slice(&(MAX_ROWS_PER_FRAME as u32 + 1).to_le_bytes());
    m.resize(16 + (MAX_ROWS_PER_FRAME + 1) * LEGACY_RECORD_LEN, 0);
    for at in (16..m.len()).step_by(LEGACY_RECORD_LEN) {
        m[at + 24..at + 32].copy_from_slice(&TIME_MIN_NS.to_le_bytes());
    }
    // Records are valid ADS1 records only if in range: latitude 0 and the earliest time are, so this decodes.
    assert_eq!(
        decode_payload(&m, &mut out).unwrap(),
        MAX_ROWS_PER_FRAME + 1
    );
    assert_eq!(reports_len(&out), MAX_ROWS_PER_FRAME + 1);
}

// REQ: OA-ADSB-027
#[test]
fn a_bad_record_does_not_refuse_the_frame_the_valid_ones_are_kept_and_the_bad_one_counted() {
    let r = sample_rows(6);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 6).unwrap();
    let rec = |i: usize| 16 + i * RECORD_LEN;
    let mut m = p.clone();
    m[rec(1) + 7] = 0x7f; // record 1: latitude out of range, no position
    m[rec(2) + 38] = 0x80; // record 2: unknown flag bits
    m[rec(3) + 39] = 14; // record 3: unknown source type
    m[rec(4) + 59] = 0x68; // record 4: wind direction 360 or more
    m[rec(4) + 60] = 0x01;
    m[rec(5) + 79] = 9; // record 5: reserved byte set
                        // Strict: the whole payload is refused at the first bad record (unchanged).
    let mut out = Reports::default();
    assert_eq!(
        decode_payload(&m, &mut out).unwrap_err(),
        PayloadError::BadRecord(1)
    );
    assert_eq!(reports_len(&out), 0);
    // Lenient: records 0, 3, 4 and 5 are kept (3 with source unknown, 4 with the direction
    // absent), 1 and 2 are dropped and counted; the layout is the one documented.
    let c = decode_payload_lenient(&m, &mut out).unwrap();
    assert_eq!(
        c,
        PayloadCounts {
            records: 6,
            kept: 4,
            dropped: 2,
            first_dropped: Some(1),
            unknown_source: 1,
            bad_time: 0,
            bad_measurement: 1,
            reserved_set: 1,
        }
    );
    assert_eq!(reports_row(&out, 0), reports_row(&r, 0));
    let r3 = reports_row(&out, 1).unwrap();
    assert_eq!(
        (r3.icao, r3.source),
        (reports_row(&r, 3).unwrap().icao, SOURCE_UNKNOWN)
    );
    assert_eq!(reports_row(&out, 2).unwrap().wd_deg, NONE_U16);
    assert_eq!(
        reports_row(&out, 3).unwrap().icao,
        reports_row(&r, 5).unwrap().icao
    );
    // A clean payload decodes the same by both paths; structural errors still refuse whole.
    let (mut a, mut b) = (Reports::default(), Reports::default());
    assert_eq!(decode_payload(&p, &mut a).unwrap(), 6);
    assert_eq!(decode_payload_lenient(&p, &mut b).unwrap().kept, 6);
    assert_eq!(a, b);
    let mut out = Reports::default();
    assert_eq!(
        decode_payload_lenient(&p[..p.len() - 1], &mut out).unwrap_err(),
        PayloadError::Length
    );
    let mut bad_magic = p.clone();
    bad_magic[0] = b'X';
    assert_eq!(
        decode_payload_lenient(&bad_magic, &mut out).unwrap_err(),
        PayloadError::Magic
    );
    assert_eq!(reports_len(&out), 0);
}

fn pair(dist_m: f64, dt_s: f64, category: u8, gs_a_kt: u16, gs_b_kt: u16) -> JumpPair {
    let dkt = |kt: u16| if kt == NONE_U16 { NONE_U16 } else { kt * 10 };
    JumpPair {
        dist_m,
        dt_s,
        slack_m: 200.0,
        category,
        gs_a_dkt: dkt(gs_a_kt),
        gs_b_dkt: dkt(gs_b_kt),
    }
}

const KT: f64 = 0.514_444_444_444;

// REQ: OA-ADSB-028
#[test]
fn the_speed_rule_passes_real_fast_aircraft_and_catches_position_jumps() {
    let v = |p: &JumpPair| jump_verdict(p, CAP_ORDINARY_KT);
    // 900 kt for 3 s, reporting 900 kt: plausible whatever the category says, even for an
    // airliner category, and for one that reports nothing about itself in a class that can.
    for cat in [0xa3u8, 0xa6, 0] {
        assert_eq!(
            v(&pair(900.0 * KT * 3.0, 3.0, cat, 900, 900)),
            JumpVerdict::Plausible
        );
    }
    assert_eq!(
        v(&pair(900.0 * KT * 3.0, 3.0, 0xa6, NONE_U16, NONE_U16)),
        JumpVerdict::Plausible
    );
    // One report with a ground speed is enough; the larger one counts.
    assert_eq!(
        v(&pair(900.0 * KT * 3.0, 3.0, 0xa3, NONE_U16, 900)),
        JumpVerdict::Plausible
    );
    // The 2,782 kt jump: caught with and without ground speed, in every class but space.
    for (cat, gs, want) in [
        (0xa3u8, 450u16, JumpVerdict::OverCap),
        (0xa3, NONE_U16, JumpVerdict::OverCap),
        (0xa6, 450, JumpVerdict::OverCap),
        (0xa6, NONE_U16, JumpVerdict::OverCap),
        (0, NONE_U16, JumpVerdict::OverCap),
    ] {
        assert_eq!(
            v(&pair(2782.0 * KT * 2.0, 2.0, cat, gs, gs)),
            want,
            "{cat:x} {gs}"
        );
    }
    // 1,500 kt is within the high-performance cap but not consistent with 450 kt reported.
    assert_eq!(
        v(&pair(1500.0 * KT * 2.0, 2.0, 0xa6, 450, 450)),
        JumpVerdict::InconsistentWithGroundSpeed
    );
    assert_eq!(
        v(&pair(1500.0 * KT * 2.0, 2.0, 0xa6, 1500, 1500)),
        JumpVerdict::Plausible
    );
    // No ground speed: the class ceiling decides. 700 kt is fine for an airliner class, not for
    // a light aircraft or a surface vehicle; 900 kt is over the airliner ceiling.
    assert_eq!(
        v(&pair(700.0 * KT * 5.0, 5.0, 0xa3, NONE_U16, NONE_U16)),
        JumpVerdict::Plausible
    );
    assert_eq!(
        v(&pair(700.0 * KT * 5.0, 5.0, 0xc2, NONE_U16, NONE_U16)),
        JumpVerdict::OverCeiling
    );
    assert_eq!(
        v(&pair(900.0 * KT * 5.0, 5.0, 0xa3, NONE_U16, NONE_U16)),
        JumpVerdict::OverCeiling
    );
    // A 300 kt aircraft that jumps 2 km in 5 s (780 kt, under every cap and the old fixed limit)
    // is caught by its own speed, and a 300 kt one that moved 4 km in 20 s (390 kt) is not.
    assert_eq!(
        v(&pair(2000.0, 5.0, 0xa3, 300, 300)),
        JumpVerdict::InconsistentWithGroundSpeed
    );
    assert_eq!(
        v(&pair(4000.0, 20.0, 0xa3, 300, 300)),
        JumpVerdict::Plausible
    );
    // Position noise within the allowance is never a jump, even at no elapsed time; otherwise
    // no time elapsed is an impossible displacement.
    assert_eq!(v(&pair(150.0, 0.0, 0xa3, 0, 0)), JumpVerdict::Plausible);
    assert!(is_jump(&pair(1000.0, 0.0, 0xa3, 900, 900), CAP_ORDINARY_KT));
    // The caps by class.
    assert_eq!(category_hard_cap_kt(0xa3, CAP_ORDINARY_KT), 1000.0);
    assert_eq!(category_hard_cap_kt(0xa6, CAP_ORDINARY_KT), 2000.0);
    assert_eq!(category_hard_cap_kt(0, CAP_ORDINARY_KT), 2000.0);
    assert_eq!(category_hard_cap_kt(0xb7, CAP_ORDINARY_KT), 4088.0);
    assert!(category_ceiling_kt(0xa5) < category_ceiling_kt(0xa6));
    // The summary uses the same rule: a 900 kt pair reporting 900 kt is not a jump.
    let mut r = Reports::default();
    for k in 0..4 {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + i64::from(k) * 3_000_000_000;
        row.lat_e7 = 400_000_000 + (f64::from(k) * 900.0 * KT * 3.0 / 0.011_119_5) as i32;
        row.lon_e7 = -900_000_000;
        row.gs_dkt = 9000;
        row.category = 0xa3;
        reports_push(&mut r, &row);
    }
    let s = summarize(&r);
    assert_eq!((s.pairs, s.jumps), (3, 0));
    r.gs_dkt.iter_mut().for_each(|g| *g = NONE_U16);
    assert_eq!(summarize(&r).jumps, 3);
}

// REQ: OA-ADSB-029
#[test]
fn a_cap_raising_category_must_persist_and_a_reserved_code_never_raises_the_cap() {
    let mut s = CategoryState::default();
    // No category changes nothing; an ordinary class is taken at once.
    assert_eq!(category_observe(&mut s, 0), 0);
    assert_eq!(category_observe(&mut s, 0xa3), 0xa3);
    // One or two reports of a cap-raising class do not move it; the third does.
    assert_eq!(category_observe(&mut s, 0xb7), 0xa3);
    assert_eq!(category_observe(&mut s, 0xb7), 0xa3);
    assert_eq!(category_observe(&mut s, 0xb7), 0xb7);
    // A report with no category in between does not break the count; a different one does.
    let mut s = CategoryState::default();
    for c in [0xb7u8, 0, 0xb7, 0xa6, 0xb7, 0xb7] {
        assert_eq!(category_observe(&mut s, c), 0, "{c:x}");
    }
    assert_eq!(category_observe(&mut s, 0xb7), 0xb7);
    // An ordinary class after it is taken at once and resets the candidate.
    assert_eq!(category_observe(&mut s, 0xa5), 0xa5);
    assert_eq!(category_observe(&mut s, 0xb7), 0xa5);
    // Reserved codes are ignored however often they are printed.
    let mut s = CategoryState::default();
    for _ in 0..10 {
        assert_eq!(category_observe(&mut s, 0xa8), 0);
    }
    assert!(category_is_reserved(0xa8) && category_is_reserved(0xb5) && category_is_reserved(0xd1));
    assert!(!category_is_reserved(0xb7) && !category_is_reserved(0));
    // A reserved code in a direct verdict gets the ordinary cap, not the high one.
    assert_eq!(category_hard_cap_kt(0xa8, CAP_ORDINARY_KT), 1000.0);
    assert_eq!(category_hard_cap_kt(0xb5, CAP_ORDINARY_KT), 1000.0);
    // The summary applies the same rule: one report printing B7 does not excuse a 2,782 kt jump.
    let mut r = Reports::default();
    for k in 0..6 {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = 1_700_000_000_000_000_000 + i64::from(k) * 2_000_000_000;
        let extra = if k == 3 { 2862.0 / 0.011_119_5 } else { 0.0 };
        row.lat_e7 = 400_000_000 + (f64::from(k) * 2.0 * 231.5 / 0.011_119_5 + extra) as i32;
        row.lon_e7 = -900_000_000;
        row.category = if k == 3 { 0xb7 } else { 0xa3 };
        reports_push(&mut r, &row);
    }
    // The jump out and the jump back; with B7 taken on the spot only the jump back counted.
    assert_eq!(summarize(&r).jumps, 2);
}

/// Replaces the report time of record `i` of a payload.
fn set_time(p: &mut [u8], i: usize, t: i64) {
    let at = 16 + i * RECORD_LEN + 24;
    p[at..at + 8].copy_from_slice(&t.to_le_bytes());
}

// REQ: OA-ADSB-171
#[test]
fn a_report_time_outside_2000_to_2100_is_refused_and_counted() {
    let r = sample_rows(5);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 5).unwrap();
    set_time(&mut p, 1, -4_700_000_000_000_000_000);
    set_time(&mut p, 2, 4_700_000_000_000_000_000);
    set_time(&mut p, 3, TIME_MIN_NS - 1);
    set_time(&mut p, 4, TIME_MAX_NS);
    let mut out = Reports::default();
    let c = decode_payload_lenient(&p, &mut out).unwrap();
    assert_eq!((c.records, c.kept, c.dropped, c.bad_time), (5, 1, 4, 4));
    assert_eq!(c.first_dropped, Some(1));
    // The limits themselves are in range; the strict decoder refuses the whole payload.
    set_time(&mut p, 1, TIME_MIN_NS);
    set_time(&mut p, 2, TIME_MAX_NS - 1);
    set_time(&mut p, 3, 1_700_000_000_000_000_000);
    set_time(&mut p, 4, 1_700_000_000_000_000_001);
    let mut ok = Reports::default();
    assert_eq!(decode_payload(&p, &mut ok).unwrap(), 5);
    set_time(&mut p, 4, i64::MAX);
    assert_eq!(
        decode_payload(&p, &mut ok).unwrap_err(),
        PayloadError::BadRecord(4)
    );
}
