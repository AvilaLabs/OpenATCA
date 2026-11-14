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
    assert_eq!(&p[..4], b"ADS1");
    assert_eq!(u16::from_le_bytes([p[4], p[5]]), 64);
    assert_eq!(u32::from_le_bytes([p[8], p[9], p[10], p[11]]), 50);
    // First record: icao at 0, lat at 4, time at 24, callsign at 40, nac_p at 50.
    let rec = &p[16..80];
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
    assert!(rec[57..].iter().all(|&b| b == 0));
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
    for (at, v) in [(4usize, 0x7fu8), (38, 0x80), (39, 14), (60, 1)] {
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
