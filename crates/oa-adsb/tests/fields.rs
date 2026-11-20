//! The payload record field by field, the payload size limits, frame splitting and the counts
//! that add across traces.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::*;

/// A row with a distinct, non-default, valid value in every field.
fn full_row() -> Row {
    Row {
        time_ns: 1_700_000_123_456_789_012,
        icao: 0x00ab_cdef,
        lat_e7: 412_345_678,
        lon_e7: -876_543_210,
        baro_alt_ft: 35_025,
        geom_alt_ft: 35_150,
        vrate_fpm: -1_792,
        gs_dkt: 4_567,
        track_ddeg: 2_713,
        squawk: 0o4321,
        flags: F_GROUND | F_NEW_LEG,
        source: 5,
        callsign: *b"N123AB\0\0",
        category: 0xa3,
        nic: 9,
        nac_p: 10,
        nac_v: 2,
        sil: 3,
        sil_type: SIL_PER_SAMPLE,
        version: 2,
        gva: 1,
        sda: 2,
        nic_baro: 1,
        ws_kt: 41,
        wd_deg: 271,
        tas_kt: 483,
        ias_kt: 311,
        mach_milli: 812,
        mag_heading_cdeg: 12_345,
        true_heading_cdeg: 23_456,
        oat_c: -52,
        tat_c: -31,
        roll_cdeg: -1_072,
        track_rate_cdps: 321,
    }
}

fn payload_of(row: &Row) -> Vec<u8> {
    let mut r = Reports::default();
    reports_push(&mut r, row);
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 0, 1).unwrap();
    p
}

fn header(magic: &[u8; 4], rec: u16, count: u32) -> Vec<u8> {
    let mut h = magic.to_vec();
    h.extend_from_slice(&rec.to_le_bytes());
    h.extend_from_slice(&0u16.to_le_bytes());
    h.extend_from_slice(&count.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h
}

fn decoded(p: &[u8]) -> Row {
    let mut out = Reports::default();
    assert_eq!(decode_payload(p, &mut out).unwrap(), 1);
    reports_row(&out, 0).unwrap()
}

// REQ: OA-ADSB-154
#[test]
fn every_field_of_an_ads2_record_round_trips() {
    let want = full_row();
    let p = payload_of(&want);
    // The bytes sit at the documented offsets.
    let rec = &p[16..];
    assert_eq!(i32::from_le_bytes(rec[16..20].try_into().unwrap()), 35_150);
    assert_eq!(i32::from_le_bytes(rec[20..24].try_into().unwrap()), -1_792);
    assert_eq!(&rec[48..57], &[0xa3, 9, 10, 2, 3, SIL_PER_SAMPLE, 2, 1, 2]);
    let got = decoded(&p);
    assert_eq!(got, want);
    assert_eq!(got.geom_alt_ft, 35_150);
    assert_eq!(got.vrate_fpm, -1_792);
    assert_eq!(got.category, 0xa3);
    assert_eq!(got.nic, 9);
    assert_eq!(got.nac_p, 10);
    assert_eq!(got.nac_v, 2);
    assert_eq!(got.sil, 3);
    assert_eq!(got.sil_type, SIL_PER_SAMPLE);
    assert_eq!(got.version, 2);
    assert_eq!(got.gva, 1);
    assert_eq!(got.sda, 2);
    assert_eq!(got.nic_baro, 1);
    assert_eq!(got.callsign, *b"N123AB\0\0");
    assert_eq!(got.ws_kt, 41);
    assert_eq!(got.track_rate_cdps, 321);
}

// REQ: OA-ADSB-155
#[test]
fn every_field_of_an_ads1_record_decodes_and_the_measurements_are_absent() {
    let want = full_row();
    let p2 = payload_of(&want);
    let mut p1 = header(b"ADS1", LEGACY_RECORD_LEN as u16, 1);
    p1.extend_from_slice(&p2[16..16 + 57]);
    p1.resize(16 + LEGACY_RECORD_LEN, 0);
    let got = decoded(&p1);
    let expect = Row {
        ws_kt: NONE_U16,
        wd_deg: NONE_U16,
        tas_kt: NONE_U16,
        ias_kt: NONE_U16,
        mach_milli: NONE_U16,
        mag_heading_cdeg: NONE_U16,
        true_heading_cdeg: NONE_U16,
        oat_c: NONE_I16,
        tat_c: NONE_I16,
        roll_cdeg: NONE_I16,
        track_rate_cdps: NONE_I16,
        nic_baro: NONE_U8,
        ..want
    };
    assert_eq!(got, expect);
    assert_eq!(got.geom_alt_ft, 35_150);
    assert_eq!(got.vrate_fpm, -1_792);
    assert_eq!(got.category, 0xa3);
    assert_eq!((got.nic, got.nac_p, got.nac_v), (9, 10, 2));
    assert_eq!((got.sil, got.sil_type), (3, SIL_PER_SAMPLE));
    assert_eq!((got.version, got.gva, got.sda), (2, 1, 2));
    // A reserved byte set after offset 57 is an anomaly, ignored.
    let mut dirty = p1.clone();
    *dirty.last_mut().unwrap() = 9;
    let mut out = Reports::default();
    let c = decode_payload_lenient(&dirty, &mut out).unwrap();
    assert_eq!((c.kept, c.reserved_set), (1, 1));
    assert_eq!(reports_row(&out, 0).unwrap(), expect);
}

// REQ: OA-ADSB-157
#[test]
fn every_anomaly_in_one_record_shows_in_its_own_count() {
    let mut p = payload_of(&full_row());
    let rec = 16;
    p[rec + 39] = 200; // unknown source type
    p[rec + 59..rec + 61].copy_from_slice(&400u16.to_le_bytes()); // wind direction
    p[rec + 79] = 9; // reserved (above the NIC_baro codes)
    let mut out = Reports::default();
    let c = decode_payload_lenient(&p, &mut out).unwrap();
    assert_eq!(
        (c.kept, c.unknown_source, c.bad_measurement, c.reserved_set),
        (1, 1, 1, 1)
    );
    let row = reports_row(&out, 0).unwrap();
    assert_eq!(row.source, SOURCE_UNKNOWN);
    assert_eq!(row.wd_deg, NONE_U16);
    assert_eq!(row.ws_kt, 41);
    // Strict decoding refuses it.
    assert_eq!(
        decode_payload(&p, &mut Reports::default()),
        Err(PayloadError::BadRecord(0))
    );
    // Each pair of the three, on its own.
    let one = |src: bool, meas: bool, res: bool| {
        let mut p = payload_of(&full_row());
        if src {
            p[rec + 39] = 200;
        }
        if meas {
            p[rec + 71 + 4..rec + 71 + 6].copy_from_slice(&20_000i16.to_le_bytes());
        }
        if res {
            p[rec + 79] = 9;
        }
        let c = decode_payload_lenient(&p, &mut Reports::default()).unwrap();
        (c.unknown_source, c.bad_measurement, c.reserved_set)
    };
    assert_eq!(one(true, true, false), (1, 1, 0));
    assert_eq!(one(true, false, true), (1, 0, 1));
    assert_eq!(one(false, true, true), (0, 1, 1));
    assert_eq!(one(true, false, false), (1, 0, 0));
    assert_eq!(one(false, true, false), (0, 1, 0));
    assert_eq!(one(false, false, true), (0, 0, 1));
    assert_eq!(one(false, false, false), (0, 0, 0));
    // Both headings and the roll bad at once is still one bad record.
    let mut p = payload_of(&full_row());
    p[rec + 57 + 10..rec + 57 + 12].copy_from_slice(&40_000u16.to_le_bytes());
    p[rec + 57 + 12..rec + 57 + 14].copy_from_slice(&40_000u16.to_le_bytes());
    let mut out = Reports::default();
    let c = decode_payload_lenient(&p, &mut out).unwrap();
    assert_eq!(c.bad_measurement, 1);
    let row = reports_row(&out, 0).unwrap();
    assert_eq!(
        (row.mag_heading_cdeg, row.true_heading_cdeg),
        (NONE_U16, NONE_U16)
    );
}

/// Gives every record of a payload of `rec`-byte records the earliest accepted time.
fn with_times(mut p: Vec<u8>, rec: usize) -> Vec<u8> {
    for at in (16..p.len()).step_by(rec) {
        if let Some(b) = p.get_mut(at + 24..at + 32) {
            b.copy_from_slice(&TIME_MIN_NS.to_le_bytes());
        }
    }
    p
}

fn sample(n: usize, time: impl Fn(usize) -> i64) -> Reports {
    let mut r = Reports::default();
    for i in 0..n {
        let mut row = ROW_EMPTY;
        row.icao = 0xa00001;
        row.time_ns = TIME_MIN_NS + time(i);
        row.lat_e7 = 400_000_000;
        row.lon_e7 = -900_000_000;
        reports_push(&mut r, &row);
    }
    r
}

// REQ: OA-ADSB-156
#[test]
fn payload_size_limits_are_exact_for_both_layouts() {
    // At the limit is accepted, one over is refused, for the header alone and with records.
    let mut ok = header(b"ADS2", 80, MAX_ROWS_PER_FRAME as u32);
    ok.resize(PAYLOAD_HEADER_LEN + MAX_ROWS_PER_FRAME * RECORD_LEN, 0);
    let ok = with_times(ok, RECORD_LEN);
    let mut out = Reports::default();
    let c = decode_payload_lenient(&ok, &mut out).unwrap();
    assert_eq!(
        (c.records, c.kept),
        (MAX_ROWS_PER_FRAME, MAX_ROWS_PER_FRAME)
    );
    let over = header(b"ADS2", 80, MAX_ROWS_PER_FRAME as u32 + 1);
    assert_eq!(
        decode_payload_lenient(&over, &mut Reports::default()).unwrap_err(),
        PayloadError::TooManyRows
    );
    let mut ok = header(b"ADS1", 64, LEGACY_MAX_ROWS as u32);
    ok.resize(PAYLOAD_HEADER_LEN + LEGACY_MAX_ROWS * LEGACY_RECORD_LEN, 0);
    let ok = with_times(ok, LEGACY_RECORD_LEN);
    let c = decode_payload_lenient(&ok, &mut Reports::default()).unwrap();
    assert_eq!(c.kept, LEGACY_MAX_ROWS);
    let over = header(b"ADS1", 64, LEGACY_MAX_ROWS as u32 + 1);
    assert_eq!(
        decode_payload_lenient(&over, &mut Reports::default()).unwrap_err(),
        PayloadError::TooManyRows
    );
    // The length must be exactly header plus count records, short or long.
    for (magic, rec) in [(b"ADS2", 80usize), (b"ADS1", 64)] {
        let mut p = header(magic, rec as u16, 3);
        p.resize(16 + 3 * rec, 0);
        let p = with_times(p, rec);
        assert!(decode_payload(&p, &mut Reports::default()).is_ok());
        let mut long = p.clone();
        long.push(0);
        let mut short = p.clone();
        short.pop();
        assert_eq!(
            decode_payload(&long, &mut Reports::default()),
            Err(PayloadError::Length)
        );
        assert_eq!(
            decode_payload(&short, &mut Reports::default()),
            Err(PayloadError::Length)
        );
        // Header only, and one byte short of the header.
        assert_eq!(
            decode_payload(&p[..15], &mut Reports::default()),
            Err(PayloadError::Length)
        );
        assert_eq!(
            decode_payload(&p[..16], &mut Reports::default()),
            Err(PayloadError::Length)
        );
    }
    // A header alone for zero records is valid.
    assert_eq!(
        decode_payload(&header(b"ADS2", 80, 0), &mut Reports::default()),
        Ok(0)
    );
    // The record length of the other layout is a layout error.
    let mut p = header(b"ADS2", 64, 0);
    assert_eq!(
        decode_payload(&p, &mut Reports::default()),
        Err(PayloadError::Layout)
    );
    p = header(b"ADS1", 80, 0);
    assert_eq!(
        decode_payload(&p, &mut Reports::default()),
        Err(PayloadError::Layout)
    );
}

// REQ: OA-ADSB-156
#[test]
fn encode_payload_counts_its_rows_and_refuses_bad_ranges() {
    let r = sample(MAX_ROWS_PER_FRAME + 1, |i| i as i64);
    let mut p = Vec::new();
    // Exactly the most is fine, one more is refused and writes nothing.
    encode_payload(&mut p, &r, 0, MAX_ROWS_PER_FRAME).unwrap();
    assert_eq!(
        p.len(),
        PAYLOAD_HEADER_LEN + MAX_ROWS_PER_FRAME * RECORD_LEN
    );
    let mut q = Vec::new();
    assert_eq!(
        encode_payload(&mut q, &r, 0, MAX_ROWS_PER_FRAME + 1),
        Err(PayloadError::TooManyRows)
    );
    assert!(q.is_empty());
    // A range that does not start at 0: the count is `to - from`, the first row is `from`.
    let mut p = Vec::new();
    encode_payload(&mut p, &r, 3, 10).unwrap();
    assert_eq!(u32::from_le_bytes(p[8..12].try_into().unwrap()), 7);
    assert_eq!(p.len(), 16 + 7 * 80);
    let mut back = Reports::default();
    assert_eq!(decode_payload(&p, &mut back).unwrap(), 7);
    assert_eq!(back.time_ns, [3, 4, 5, 6, 7, 8, 9].map(|i| TIME_MIN_NS + i));
    // Empty, reversed and past-the-end ranges; the end may equal the length.
    let r = sample(5, |i| i as i64);
    for (a, b) in [(2, 2), (3, 2), (0, 6), (5, 5), (6, 7)] {
        let mut q = Vec::new();
        assert_eq!(
            encode_payload(&mut q, &r, a, b),
            Err(PayloadError::Range),
            "{a}..{b}"
        );
        assert!(q.is_empty());
    }
    let mut q = Vec::new();
    encode_payload(&mut q, &r, 4, 5).unwrap();
    assert_eq!(q.len(), 16 + 80);
    // The Display text of an error is not empty.
    assert!(!PayloadError::Range.to_string().is_empty());
}

// REQ: OA-ADSB-156
#[test]
fn frames_split_on_the_time_span_and_on_the_row_limit() {
    // Span: 0, 0.999 ms, 1 ms, 1.5 ms, 2.5 ms; 1 ms frames start at 0 and 1 ms and 2.5 ms.
    let t = [0i64, 999_999, 1_000_000, 1_500_000, 2_500_000];
    let r = sample(5, |i| t[i]);
    assert_eq!(frame_ranges(&r, 1), [(0, 2), (2, 4), (4, 5)]);
    // 0 is clamped to the 1 ms minimum, an over-large span to 60 s.
    assert_eq!(frame_ranges(&r, 0), [(0, 2), (2, 4), (4, 5)]);
    assert_eq!(frame_ranges(&r, 3), [(0, 5)]);
    let r = sample(3, |i| i as i64 * 40_000_000_000);
    assert_eq!(frame_ranges(&r, u32::MAX), [(0, 2), (2, 3)]);
    assert_eq!(frame_ranges(&r, 60_000), [(0, 2), (2, 3)]);
    assert_eq!(frame_ranges(&r, 59_999), [(0, 2), (2, 3)]);
    // Ranges start where the previous ended, from a first row that is not 0.
    let r = sample(7, |i| i as i64 * 10_000_000);
    assert_eq!(frame_ranges(&r, 20), [(0, 2), (2, 4), (4, 6), (6, 7)]);
    // Row limit: the 50 001st row of one instant starts a new frame; exactly the limit does not.
    let r = sample(MAX_ROWS_PER_FRAME, |_| 5);
    assert_eq!(frame_ranges(&r, 1000), [(0, MAX_ROWS_PER_FRAME)]);
    let r = sample(MAX_ROWS_PER_FRAME + 1, |_| 5);
    assert_eq!(
        frame_ranges(&r, 1000),
        [
            (0, MAX_ROWS_PER_FRAME),
            (MAX_ROWS_PER_FRAME, MAX_ROWS_PER_FRAME + 1)
        ]
    );
    assert!(frame_ranges(&Reports::default(), 1000).is_empty());
}

fn stat_with(seed: u64) -> Stats {
    let mut s = Stats {
        traces_read: seed,
        traces_refused: seed * 2,
        rows_seen: seed * 3,
        outside_box: seed * 5,
        outside_time: seed * 7,
        reserved_address: seed * 11,
        kept: seed * 13,
        bad_measurements: seed * 17,
        unknown_overflow: seed * 19,
        ..Stats::default()
    };
    for (i, x) in s.rejected.iter_mut().enumerate() {
        *x = seed * (23 + i as u64);
    }
    for (i, x) in s.unknown_fields.iter_mut().enumerate() {
        *x = seed * (100 + i as u64);
    }
    s
}

fn slot_count(s: &Stats, field: usize, text: &str) -> u64 {
    s.unknown_values
        .iter()
        .filter(|v| v.count > 0 && usize::from(v.field) == field && unknown_value_text(v) == text)
        .map(|v| v.count)
        .sum()
}

// REQ: OA-ADSB-158
#[test]
fn unknown_values_accumulate_per_field_and_text() {
    let mut s = Stats::default();
    stats_note_unknown(&mut s, FLD_CATEGORY, "Z9");
    stats_note_unknown(&mut s, FLD_CATEGORY, "Z9");
    stats_note_unknown(&mut s, FLD_CATEGORY, "Z");
    stats_note_unknown(&mut s, FLD_CATEGORY, "Z98");
    stats_note_unknown(&mut s, FLD_NIC, "Z9");
    assert_eq!(s.unknown_fields[FLD_CATEGORY], 4);
    assert_eq!(s.unknown_fields[FLD_NIC], 1);
    assert_eq!(slot_count(&s, FLD_CATEGORY, "Z9"), 2);
    assert_eq!(slot_count(&s, FLD_CATEGORY, "Z"), 1);
    assert_eq!(slot_count(&s, FLD_CATEGORY, "Z98"), 1);
    assert_eq!(slot_count(&s, FLD_NIC, "Z9"), 1);
    assert_eq!(s.unknown_values.iter().filter(|v| v.count > 0).count(), 4);
    assert_eq!(s.unknown_overflow, 0);
    // An empty text is a value of its own, and is kept apart from "Z".
    stats_note_unknown(&mut s, FLD_CATEGORY, "");
    stats_note_unknown(&mut s, FLD_CATEGORY, "");
    assert_eq!(slot_count(&s, FLD_CATEGORY, ""), 2);
    // A field outside the table is not counted per field, and does not panic.
    stats_note_unknown(&mut s, FIELD_KINDS, "x");
    assert_eq!(s.unknown_fields.iter().sum::<u64>(), 7);
}

// REQ: OA-ADSB-158
#[test]
fn long_text_is_cut_on_a_character_boundary() {
    let mut s = Stats::default();
    let exact = "a".repeat(UNKNOWN_TEXT_LEN);
    stats_note_unknown(&mut s, FLD_FLIGHT, &exact);
    stats_note_unknown(&mut s, FLD_FLIGHT, &format!("{exact}bcd"));
    // Both are the same 24 bytes.
    assert_eq!(slot_count(&s, FLD_FLIGHT, &exact), 2);
    // 1 + 12 two-byte characters: the cut at 24 falls inside the last one, so 23 bytes stay.
    let wide = format!("a{}", "\u{e9}".repeat(12));
    assert_eq!(wide.len(), 25);
    stats_note_unknown(&mut s, FLD_SQUAWK, &wide);
    let kept = format!("a{}", "\u{e9}".repeat(11));
    assert_eq!(slot_count(&s, FLD_SQUAWK, &kept), 1);
    // Shorter than the limit is untouched.
    stats_note_unknown(&mut s, FLD_SQUAWK, "short");
    assert_eq!(slot_count(&s, FLD_SQUAWK, "short"), 1);
}

// REQ: OA-ADSB-158
#[test]
fn the_value_table_holds_32_pairs_and_counts_the_rest_as_overflow() {
    let mut s = Stats::default();
    for i in 0..MAX_UNKNOWN_VALUES {
        stats_note_unknown(&mut s, FLD_SQUAWK, &format!("v{i}"));
    }
    assert_eq!(s.unknown_overflow, 0);
    assert!(s.unknown_values.iter().all(|v| v.count == 1));
    // A known pair still counts in its slot; a new one overflows.
    stats_note_unknown(&mut s, FLD_SQUAWK, "v0");
    stats_note_unknown(&mut s, FLD_SQUAWK, "v31");
    assert_eq!(s.unknown_overflow, 0);
    assert_eq!(slot_count(&s, FLD_SQUAWK, "v0"), 2);
    assert_eq!(slot_count(&s, FLD_SQUAWK, "v31"), 2);
    stats_note_unknown(&mut s, FLD_SQUAWK, "new");
    stats_note_unknown(&mut s, FLD_SQUAWK, "new");
    assert_eq!(s.unknown_overflow, 2);
    assert_eq!(s.unknown_fields[FLD_SQUAWK], 36);
    assert_eq!(slot_count(&s, FLD_SQUAWK, "new"), 0);
}

// REQ: OA-ADSB-158
#[test]
fn adding_stats_sums_every_count_and_merges_the_value_tables() {
    let mut a = stat_with(1);
    let mut b = stat_with(10);
    stats_note_unknown(&mut a, FLD_CATEGORY, "Q1");
    stats_note_unknown(&mut a, FLD_CATEGORY, "Q1");
    stats_note_unknown(&mut b, FLD_CATEGORY, "Q1");
    stats_note_unknown(&mut b, FLD_CATEGORY, "Q1");
    stats_note_unknown(&mut b, FLD_CATEGORY, "Q1");
    stats_note_unknown(&mut b, FLD_GVA, "Q2");
    let (ca, cb) = (a, b);
    stats_add(&mut a, &b);
    assert_eq!(a.traces_read, 11);
    assert_eq!(a.traces_refused, 22);
    assert_eq!(a.rows_seen, 33);
    assert_eq!(a.outside_box, 55);
    assert_eq!(a.outside_time, 77);
    assert_eq!(a.reserved_address, 121);
    assert_eq!(a.kept, 143);
    assert_eq!(a.bad_measurements, 187);
    assert_eq!(a.unknown_overflow, 209);
    for i in 0..REJECT_KINDS {
        assert_eq!(a.rejected[i], ca.rejected[i] + cb.rejected[i], "{i}");
    }
    for i in 0..FIELD_KINDS {
        assert_eq!(
            a.unknown_fields[i],
            ca.unknown_fields[i] + cb.unknown_fields[i],
            "{i}"
        );
    }
    assert_eq!(slot_count(&a, FLD_CATEGORY, "Q1"), 5);
    assert_eq!(slot_count(&a, FLD_GVA, "Q2"), 1);
    // The source is unchanged, and adding nothing changes nothing.
    assert_eq!(b, cb);
    let before = a;
    stats_add(&mut a, &Stats::default());
    assert_eq!(a, before);
    // Adding twice adds twice.
    let mut c = Stats::default();
    stats_add(&mut c, &cb);
    stats_add(&mut c, &cb);
    assert_eq!(c.kept, 260);
    assert_eq!(slot_count(&c, FLD_CATEGORY, "Q1"), 6);
}

// REQ: OA-ADSB-158
#[test]
fn adding_stats_overflows_into_the_overflow_count() {
    let mut a = Stats::default();
    for i in 0..MAX_UNKNOWN_VALUES {
        stats_note_unknown(&mut a, FLD_SQUAWK, &format!("v{i}"));
    }
    let mut b = Stats::default();
    for _ in 0..5 {
        stats_note_unknown(&mut b, FLD_SQUAWK, "extra");
    }
    b.unknown_overflow = 3;
    stats_add(&mut a, &b);
    assert_eq!(a.unknown_overflow, 5 + 3);
    assert_eq!(slot_count(&a, FLD_SQUAWK, "extra"), 0);
    // A pair the table has gets the whole count.
    let mut c = Stats::default();
    for _ in 0..4 {
        stats_note_unknown(&mut c, FLD_SQUAWK, "v7");
    }
    stats_add(&mut a, &c);
    assert_eq!(slot_count(&a, FLD_SQUAWK, "v7"), 5);
    assert_eq!(a.unknown_overflow, 8);
}

// REQ: OA-ADSB-155
#[test]
fn nic_baro_is_the_last_byte_plus_one_and_a_clip_without_it_reads_as_absent() {
    for (v, byte) in [(0u8, 1u8), (1, 2), (NONE_U8, 0)] {
        let mut row = full_row();
        row.nic_baro = v;
        let p = payload_of(&row);
        assert_eq!(p[16 + 79], byte);
        assert_eq!(decoded(&p).nic_baro, v);
    }
    // A byte above 2 is not a NIC_baro: absent, and the record is still kept.
    let mut p = payload_of(&full_row());
    p[16 + 79] = 3;
    let mut out = Reports::default();
    let c = decode_payload_lenient(&p, &mut out).unwrap();
    assert_eq!((c.kept, c.reserved_set), (1, 1));
    assert_eq!(reports_row(&out, 0).unwrap().nic_baro, NONE_U8);
}

// REQ: OA-ADSB-160
#[test]
fn a_new_clip_is_ads3_an_old_clip_ads2_reads_without_nic_baro_and_an_old_magic_with_byte_79_set_is_reserved(
) {
    use oa_adsb::PREVIOUS_PAYLOAD_MAGIC;
    // New clip: the magic is ADS3, which a reader of ADS1 and ADS2 only refuses by its magic, and
    // NIC_baro 0 and 1 are carried at byte 79 as 1 and 2.
    let mut want = full_row();
    want.nic_baro = 1;
    let p = payload_of(&want);
    assert_eq!(&p[..4], b"ADS3");
    assert_eq!(PAYLOAD_MAGIC, *b"ADS3");
    assert_ne!(&p[..4], &PREVIOUS_PAYLOAD_MAGIC[..]);
    assert_ne!(&p[..4], &LEGACY_PAYLOAD_MAGIC[..]);
    assert_eq!(p[16 + 79], 2);
    assert_eq!(decoded(&p).nic_baro, 1);
    want.nic_baro = 0;
    let p = payload_of(&want);
    assert_eq!((p[16 + 79], decoded(&p).nic_baro), (1, 0));
    want.nic_baro = NONE_U8;
    let p = payload_of(&want);
    assert_eq!((p[16 + 79], decoded(&p).nic_baro), (0, NONE_U8));
    // Old clip: ADS2 with byte 79 zero reads, NIC_baro absent, nothing counted.
    let mut old = p.clone();
    old[..4].copy_from_slice(b"ADS2");
    let mut out = Reports::default();
    let c = decode_payload_lenient(&old, &mut out).unwrap();
    assert_eq!((c.kept, c.reserved_set), (1, 0));
    assert_eq!(reports_row(&out, 0).unwrap().nic_baro, NONE_U8);
    // Old magic with byte 79 set: counted as a reserved byte, and NOT read as NIC_baro.
    for v in [1u8, 2, 9] {
        let mut dirty = old.clone();
        dirty[16 + 79] = v;
        let mut out = Reports::default();
        let c = decode_payload_lenient(&dirty, &mut out).unwrap();
        assert_eq!((c.kept, c.reserved_set), (1, 1), "byte 79 = {v}");
        assert_eq!(
            reports_row(&out, 0).unwrap().nic_baro,
            NONE_U8,
            "byte 79 = {v}"
        );
        assert_eq!(
            decode_payload(&dirty, &mut Reports::default()),
            Err(PayloadError::BadRecord(0))
        );
    }
    // New clip, a byte above the codes: reserved, NIC_baro absent.
    let mut bad = payload_of(&want);
    bad[16 + 79] = 9;
    let mut out = Reports::default();
    let c = decode_payload_lenient(&bad, &mut out).unwrap();
    assert_eq!((c.kept, c.reserved_set), (1, 1));
    assert_eq!(reports_row(&out, 0).unwrap().nic_baro, NONE_U8);
}
