//! Trace decoding against hand-written fixtures that cover every field and flag. The row layout
//! is the one in the crate docs (readsb `sprintTracePoint`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_adsb::*;

const WIDE: Filter = Filter {
    lat_min_e7: -900_000_000,
    lat_max_e7: 900_000_000,
    lon_min_e7: -1_800_000_000,
    lon_max_e7: 1_800_000_000,
    from_ns: i64::MIN,
    to_ns: i64::MAX,
};

const FULL: &str = r#"{"icao":"3c66b0","r":"D-AIUP","t":"A320","dbFlags":0,"desc":"AIRBUS A-320","timestamp": 1663259853.016,"trace":[
[7016.59,49.263300,10.614239,25125,446.5,309.0,0,-2176,{"type":"adsb_icao","flight":"DLH7YA  ","alt_geom":25875,"ias":335,"tas":484,"mach":0.796,"track":309.00,"roll":-10.72,"baro_rate":-2176,"geom_rate":-2208,"squawk":"1000","category":"A3","nav_qnh":1012.8,"nic":8,"rc":186,"version":2,"nic_baro":1,"nac_p":8,"nac_v":0,"sil":3,"sil_type":"perhour","gva":2,"sda":2,"alert":0,"spi":0},"adsb_icao",25875,-2208,335,-10.7],
[7024.85,49.273589,10.593278,24825,446.0,306.6,0,-2176,null,"adsb_icao",25550,-2144,337,-1.6]]}"#;

fn run(json: &str, f: &Filter) -> (Result<(), TraceError>, Reports, Stats) {
    let mut out = Reports::default();
    let mut st = Stats::default();
    let r = decode_trace(json.as_bytes(), f, &mut out, &mut st);
    (r, out, st)
}

fn wrap(rows: &[&str]) -> String {
    format!(
        "{{\"icao\":\"abc123\",\"timestamp\":1700000000.000,\"trace\":[{}]}}",
        rows.join(",")
    )
}

fn row0(r: &Reports) -> Row {
    reports_row(r, 0).unwrap()
}

// REQ: OA-ADSB-001
#[test]
fn full_row_with_object_decodes_every_field() {
    let (res, out, st) = run(FULL, &WIDE);
    res.unwrap();
    assert_eq!((st.rows_seen, st.kept), (2, 2));
    let r = row0(&out);
    assert_eq!(r.icao, 0x3c66b0);
    assert_eq!(r.time_ns, 1_663_259_853_016_000_000 + 7_016_590_000_000);
    assert_eq!((r.lat_e7, r.lon_e7), (492_633_000, 106_142_390));
    assert_eq!((r.baro_alt_ft, r.geom_alt_ft), (25125, 25875));
    assert_eq!((r.gs_dkt, r.track_ddeg), (4465, 3090));
    assert_eq!(r.vrate_fpm, -2176);
    assert_eq!(r.flags, 0);
    assert_eq!(r.source, 0);
    assert_eq!(&r.callsign, b"DLH7YA\0\0");
    assert_eq!(r.squawk, 1000);
    assert_eq!(r.category, 0xa3);
    assert_eq!((r.nic, r.nac_p, r.nac_v, r.sil), (8, 8, 0, 3));
    assert_eq!(
        (r.sil_type, r.version, r.gva, r.sda),
        (SIL_PER_HOUR, 2, 2, 2)
    );
    // The second row has no object: those fields are absent; the rest decodes.
    let r2 = reports_row(&out, 1).unwrap();
    assert_eq!(r2.time_ns, 1_663_259_853_016_000_000 + 7_024_850_000_000);
    assert_eq!((r2.baro_alt_ft, r2.geom_alt_ft), (24825, 25550));
    assert_eq!((r2.callsign, r2.squawk, r2.category), ([0; 8], NONE_U16, 0));
    assert_eq!(
        (r2.nic, r2.nac_p, r2.nac_v, r2.sil),
        (NONE_U8, NONE_U8, NONE_U8, NONE_U8)
    );
    assert_eq!(
        (r2.sil_type, r2.version, r2.gva, r2.sda),
        (NONE_U8, NONE_U8, NONE_U8, NONE_U8)
    );
    assert_eq!(r2.source, 0);
}

// REQ: OA-ADSB-002
#[test]
fn every_flag_bit_and_altitude_form() {
    let rows = [
        // stale
        "[1,40.0,-90.0,10000,300.0,90.0,1,0,null,\"adsb_icao\",10300,null,null,null]",
        // new leg
        "[2,40.0,-90.0,10000,300.0,90.0,2,64,null,\"adsb_icao\",null,null,null,null]",
        // vertical rate is geometric
        "[3,40.0,-90.0,10000,300.0,90.0,4,-128,null,\"adsb_icao\",10300,-128,null,null]",
        // altitude is geometric: column 3 is the geometric altitude, no baro altitude
        "[4,40.0,-90.0,11000,300.0,90.0,8,0,null,\"adsb_icao\",11000,null,null,null]",
        // on the ground: \"ground\" altitude
        "[5,40.0,-90.0,\"ground\",12.5,271.5,0,null,null,\"adsb_icao\",null,null,null,null]",
        // all of stale, leg, rate, alt
        "[6,40.0,-90.0,12000,null,null,15,16,null,\"mlat\",null,null,null,null]",
        // no altitude at all
        "[7,40.0,-90.0,null,null,null,0,null,null,\"tisb_icao\"]",
    ];
    let (res, out, st) = run(&wrap(&rows), &WIDE);
    res.unwrap();
    assert_eq!(st.kept, 7);
    let r: Vec<Row> = (0..7).map(|i| reports_row(&out, i).unwrap()).collect();
    assert_eq!(r[0].flags, F_STALE);
    assert_eq!(r[1].flags, F_NEW_LEG);
    assert_eq!((r[1].vrate_fpm, r[1].geom_alt_ft), (64, NONE_I32));
    assert_eq!(r[2].flags, F_RATE_GEOM);
    assert_eq!((r[2].vrate_fpm, r[2].geom_alt_ft), (-128, 10300));
    assert_eq!(r[3].flags, F_ALT_GEOM);
    assert_eq!((r[3].baro_alt_ft, r[3].geom_alt_ft), (NONE_I32, 11000));
    assert_eq!(r[4].flags, F_GROUND);
    assert_eq!(
        (r[4].baro_alt_ft, r[4].gs_dkt, r[4].track_ddeg),
        (NONE_I32, 125, 2715)
    );
    assert_eq!(r[5].flags, 15);
    assert_eq!(
        (r[5].gs_dkt, r[5].track_ddeg, r[5].source),
        (NONE_U16, NONE_U16, 5)
    );
    assert_eq!(
        (r[6].baro_alt_ft, r[6].vrate_fpm, r[6].source),
        (NONE_I32, NONE_I32, 3)
    );
    // Times: the dt column is seconds after the base, kept to the millisecond.
    assert_eq!(r[0].time_ns, 1_700_000_001_000_000_000);
}

// REQ: OA-ADSB-003
#[test]
fn all_fourteen_source_types_and_old_nine_column_rows() {
    for (i, name) in SOURCE_NAMES.iter().enumerate() {
        let row = format!("[1,40.0,-90.0,100,1.0,1.0,0,0,null,\"{name}\"]");
        let (res, out, _) = run(&wrap(&[&row]), &WIDE);
        res.unwrap();
        assert_eq!(row0(&out).source, i as u8, "{name}");
    }
    assert_eq!(SOURCE_NAMES.len(), 14);
    // Files before 2022: nine columns, no source type; also 7 and 8 columns.
    let rows = [
        "[1,40.0,-90.0,100,1.0,1.0,0,0,null]",
        "[2,40.0,-90.0,100,1.0,1.0,0,0]",
        "[3,40.0,-90.0,100,1.0,1.0,0]",
    ];
    let (res, out, st) = run(&wrap(&rows), &WIDE);
    res.unwrap();
    assert_eq!(st.kept, 3);
    assert_eq!(row0(&out).source, NONE_U8);
    // An extra column (the TRACKS_UUID receiver id) is ignored.
    let (_, out, st) = run(
        &wrap(&["[1,40.0,-90.0,100,1.0,1.0,0,0,null,\"adsb_icao\",100,0,10,0.0,\"abcd1234\"]"]),
        &WIDE,
    );
    assert_eq!(st.kept, 1);
    assert_eq!(row0(&out).source, 0);
}

// REQ: OA-ADSB-004
#[test]
fn aircraft_object_fields_and_their_ranges() {
    let obj = |o: &str| format!("[1,40.0,-90.0,100,1.0,1.0,0,0,{o},\"adsb_icao\"]");
    let good = obj(
        r#"{"type":"adsb_icao","flight":"N123AB  ","squawk":"7700","category":"B1","nic":11,"nac_p":15,"nac_v":7,"sil":3,"sil_type":"persample","gva":3,"sda":3,"version":0,"unknown_key":[1,{"x":2}]}"#,
    );
    let (res, out, st) = run(&wrap(&[&good]), &WIDE);
    res.unwrap();
    assert_eq!(st.kept, 1);
    let r = row0(&out);
    assert_eq!(&r.callsign, b"N123AB\0\0");
    assert_eq!((r.squawk, r.category), (7700, 0xb1));
    assert_eq!((r.nic, r.nac_p, r.nac_v, r.sil), (11, 15, 7, 3));
    assert_eq!(
        (r.sil_type, r.gva, r.sda, r.version),
        (SIL_PER_SAMPLE, 3, 3, 0)
    );
    let (_, out, _) = run(
        &wrap(&[&obj(
            r#"{"sil_type":"unknown","flight":"        ","category":"c2"}"#,
        )]),
        &WIDE,
    );
    let r = row0(&out);
    assert_eq!(
        (r.sil_type, r.callsign, r.category),
        (SIL_UNKNOWN, [0; 8], 0xc2)
    );
    // Duplicate keys: the last one wins (oa-json policy).
    let (_, out, _) = run(&wrap(&[&obj(r#"{"nac_p":3,"nac_p":9}"#)]), &WIDE);
    assert_eq!(row0(&out).nac_p, 9);
    // Malformed fields refuse the row, counted as `object`.
    for bad in [
        r#"{"nic":12}"#,
        r#"{"nac_p":-1}"#,
        r#"{"nac_p":16}"#,
        r#"{"nac_v":8}"#,
        r#"{"sil":4}"#,
        r#"{"gva":4}"#,
        r#"{"sda":4}"#,
        r#"{"version":8}"#,
        r#"{"nic":"8"}"#,
        r#"{"nic":8.5}"#,
        r#"{"squawk":"1800"}"#,
        r#"{"squawk":"123"}"#,
        r#"{"squawk":1234}"#,
        r#"{"category":"A"}"#,
        r#"{"category":"ZZ"}"#,
        r#"{"flight":"TOOLONGNAME"}"#,
        r#"{"flight":7}"#,
        r#"{"sil_type":"bogus"}"#,
        r#"[1]"#,
        r#"7"#,
    ] {
        let (res, out, st) = run(&wrap(&[&obj(bad)]), &WIDE);
        res.unwrap();
        assert_eq!(
            (st.kept, st.rejected[REJ_OBJECT], reports_len(&out)),
            (0, 1, 0),
            "{bad}"
        );
    }
}

// REQ: OA-ADSB-005
#[test]
fn each_value_rule_refuses_the_row_and_counts_it_by_reason() {
    let base = |lat: &str, alt: &str, gs: &str, tr: &str, fl: &str, vr: &str, src: &str| {
        format!("[1,{lat},-90.0,{alt},{gs},{tr},{fl},{vr},null,{src}]")
    };
    let cases: [(String, usize); 14] = [
        (
            base("91.0", "100", "1.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_POSITION,
        ),
        (
            base("null", "100", "1.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_POSITION,
        ),
        (
            base(
                "40.0",
                "\"airborne\"",
                "1.0",
                "1.0",
                "0",
                "0",
                "\"adsb_icao\"",
            ),
            REJ_ALTITUDE,
        ),
        (
            base("40.0", "210000", "1.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_ALTITUDE,
        ),
        (
            base("40.0", "100.5", "1.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_ALTITUDE,
        ),
        (
            base("40.0", "100", "7000.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_SPEED,
        ),
        (
            base("40.0", "100", "-1.0", "1.0", "0", "0", "\"adsb_icao\""),
            REJ_SPEED,
        ),
        (
            base("40.0", "100", "1.0", "361.0", "0", "0", "\"adsb_icao\""),
            REJ_TRACK,
        ),
        (
            base("40.0", "100", "1.0", "1.0", "16", "0", "\"adsb_icao\""),
            REJ_FLAGS,
        ),
        (
            base("40.0", "100", "1.0", "1.0", "null", "0", "\"adsb_icao\""),
            REJ_FLAGS,
        ),
        (
            base("40.0", "100", "1.0", "1.0", "0", "300000", "\"adsb_icao\""),
            REJ_RATE,
        ),
        (
            base("40.0", "100", "1.0", "1.0", "0", "0", "\"adsb_icao_v2\""),
            REJ_SOURCE,
        ),
        (base("40.0", "100", "1.0", "1.0", "0", "0", "7"), REJ_SOURCE),
        ("[1,40.0,-90.0]".to_string(), REJ_SHAPE),
    ];
    for (row, why) in &cases {
        let (res, out, st) = run(&wrap(&[row]), &WIDE);
        res.unwrap();
        let mut want = [0u64; REJECT_KINDS];
        want[*why] = 1;
        assert_eq!(st.rejected, want, "{row}");
        assert_eq!(
            (st.rows_seen, st.kept, reports_len(&out)),
            (1, 0, 0),
            "{row}"
        );
    }
    // Time errors and non-array rows.
    for (row, why) in [
        ("[\"x\",40.0,-90.0,1,1,1,0]", REJ_TIME),
        ("[9999999999,40.0,-90.0,1,1,1,0]", REJ_TIME),
        ("5", REJ_SHAPE),
    ] {
        let (_, _, st) = run(&wrap(&[row]), &WIDE);
        assert_eq!(st.rejected[why], 1, "{row}");
    }
    // A bad row does not stop the others, and the names cover every reason.
    let (_, out, st) = run(
        &wrap(&["[1,95.0,-90.0,1,1,1,0]", "[2,40.0,-90.0,1,1,1,0]"]),
        &WIDE,
    );
    assert_eq!(
        (st.kept, st.rejected.iter().sum::<u64>(), reports_len(&out)),
        (1, 1, 1)
    );
    assert_eq!(REJECT_NAMES.len(), REJECT_KINDS);
    // Track 360.0 wraps to 0; the maximum ground speed is accepted.
    let (_, out, _) = run(&wrap(&["[1,40.0,-90.0,1,6000.0,360.0,0,0]"]), &WIDE);
    assert_eq!((row0(&out).gs_dkt, row0(&out).track_ddeg), (60000, 0));
}

// REQ: OA-ADSB-006
#[test]
fn box_and_time_window_filter_while_decoding() {
    let rows = [
        "[0,38.5,-93.5,1,1,1,0]",    // south-west corner: inside (edges included)
        "[1,46.5,-83.5,1,1,1,0]",    // north-east corner: inside
        "[2,38.4999,-90.0,1,1,1,0]", // just south
        "[3,46.5001,-90.0,1,1,1,0]", // just north
        "[4,40.0,-93.5001,1,1,1,0]", // just west
        "[5,40.0,-83.4999,1,1,1,0]", // just east: outside (lon -83.4999 > -83.5)
        "[6,42.0,-88.0,1,1,1,0]",    // inside
    ];
    let json = wrap(&rows);
    let (_, out, st) = run(&json, &ZAU_BOX);
    assert_eq!((st.kept, st.outside_box, st.rows_seen), (3, 4, 7));
    assert_eq!(out.time_ns[0], 1_700_000_000_000_000_000);
    // Time window [from, to): from inclusive, to exclusive; counted separately from the box.
    let f = Filter {
        from_ns: 1_700_000_001_000_000_000,
        to_ns: 1_700_000_006_000_000_000,
        ..ZAU_BOX
    };
    let (_, out, st) = run(&json, &f);
    assert_eq!((st.kept, st.outside_time, st.outside_box), (1, 2, 4));
    assert_eq!(out.time_ns, [1_700_000_001_000_000_000]);
    // A stricter box built from the same type.
    let f = Filter {
        lat_min_e7: 400_000_000,
        lat_max_e7: 430_000_000,
        ..ZAU_BOX
    };
    let (_, _, st) = run(&json, &f);
    assert_eq!(st.kept, 1);
    // Reference values of the ZAU box.
    assert_eq!(
        (
            ZAU_BOX.lat_min_e7,
            ZAU_BOX.lat_max_e7,
            ZAU_BOX.lon_min_e7,
            ZAU_BOX.lon_max_e7
        ),
        (385_000_000, 465_000_000, -935_000_000, -835_000_000)
    );
}

// REQ: OA-ADSB-007
#[test]
fn structural_errors_refuse_the_whole_trace_and_touch_nothing() {
    type Check = fn(&TraceError) -> bool;
    let cases: [(&str, Check); 11] = [
        ("[]", |e| *e == TraceError::NotObject),
        (r#"{"timestamp":1700000000.0,"trace":[]}"#, |e| {
            *e == TraceError::Field("icao")
        }),
        (r#"{"icao":5,"timestamp":1700000000.0,"trace":[]}"#, |e| {
            *e == TraceError::Field("icao")
        }),
        (
            r#"{"icao":"12345","timestamp":1700000000.0,"trace":[]}"#,
            |e| *e == TraceError::BadIcao,
        ),
        (
            r#"{"icao":"12345g","timestamp":1700000000.0,"trace":[]}"#,
            |e| *e == TraceError::BadIcao,
        ),
        (r#"{"icao":"abc123","trace":[]}"#, |e| {
            *e == TraceError::Field("timestamp")
        }),
        (r#"{"icao":"abc123","timestamp":12.0,"trace":[]}"#, |e| {
            *e == TraceError::BadTimestamp
        }),
        (r#"{"icao":"abc123","timestamp":1700000000.0}"#, |e| {
            *e == TraceError::Field("trace")
        }),
        (
            r#"{"icao":"abc123","timestamp":1700000000.0,"trace":{}}"#,
            |e| *e == TraceError::Field("trace"),
        ),
        (
            r#"{"icao":"abc123","timestamp":1700000000.0,"trace":[1,2"#,
            |e| matches!(e, TraceError::Json(_)),
        ),
        ("", |e| matches!(e, TraceError::Json(_))),
    ];
    for (json, check) in cases {
        let (res, out, st) = run(json, &WIDE);
        let e = res.unwrap_err();
        assert!(check(&e), "{json}: {e:?}");
        assert_eq!((reports_len(&out), st), (0, Stats::default()), "{json}");
        let _ = e.to_string();
    }
    // A non-ICAO address (readsb prints a tilde).
    let json = r#"{"icao":"~0a1b2c","timestamp":1700000000.0,"trace":[[0,40.0,-90.0,1,1,1,0,0]]}"#;
    let (res, out, _) = run(json, &WIDE);
    res.unwrap();
    assert_eq!(row0(&out).icao, 0x0a1b2c | NON_ICAO);
    // Uppercase hex is accepted.
    let json = r#"{"icao":"ABCDEF","timestamp":1700000000.0,"trace":[]}"#;
    assert_eq!(run(json, &WIDE).0, Ok(()));
}

// REQ: OA-ADSB-008
#[test]
fn row_count_bound_is_enforced() {
    let mut json = String::from(r#"{"icao":"abc123","timestamp":1700000000.0,"trace":["#);
    for i in 0..=MAX_TRACE_ROWS {
        if i > 0 {
            json.push(',');
        }
        json.push_str("[0,1,2,3,4,5,6]");
    }
    json.push_str("]}");
    let (res, out, st) = run(&json, &WIDE);
    assert_eq!(
        res.unwrap_err(),
        TraceError::TooManyRows(MAX_TRACE_ROWS + 1)
    );
    assert_eq!((reports_len(&out), st), (0, Stats::default()));
    // One row fewer is fine (every row here is in range for the wide box).
    json.truncate(json.len() - "]}".len() - ",[0,1,2,3,4,5,6]".len());
    json.push_str("]}");
    let (res, _, st) = run(&json, &WIDE);
    res.unwrap();
    assert_eq!(st.kept as usize, MAX_TRACE_ROWS);
    // A row with too many elements is refused alone.
    let long = format!("[{}]", vec!["0"; MAX_ROW_LEN + 1].join(","));
    let (_, _, st) = run(&wrap(&[&long]), &WIDE);
    assert_eq!(st.rejected[REJ_SHAPE], 1);
    // An object with too many members is refused as an object error.
    let members: Vec<String> = (0..=MAX_OBJECT_MEMBERS)
        .map(|i| format!("\"k{i}\":0"))
        .collect();
    let row = format!("[1,40.0,-90.0,1,1,1,0,0,{{{}}}]", members.join(","));
    let (_, _, st) = run(&wrap(&[&row]), &WIDE);
    assert_eq!(st.rejected[REJ_OBJECT], 1);
}

// REQ: OA-ADSB-009
#[test]
fn damaged_traces_never_panic() {
    let bytes = FULL.as_bytes();
    let mut seed = 0x1234_5678_9abc_def1u64;
    for cut in 0..bytes.len() {
        let mut out = Reports::default();
        let mut st = Stats::default();
        let _ = decode_trace(&bytes[..cut], &WIDE, &mut out, &mut st);
    }
    for _ in 0..5000 {
        let mut b = bytes.to_vec();
        for _ in 0..3 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % b.len() as u64) as usize;
            b[at] = (seed >> 40) as u8;
        }
        let mut out = Reports::default();
        let mut st = Stats::default();
        let _ = decode_trace(&b, &WIDE, &mut out, &mut st);
        // Whatever was kept is in range.
        for i in 0..reports_len(&out) {
            let r = reports_row(&out, i).unwrap();
            assert!((-900_000_000..=900_000_000).contains(&r.lat_e7));
            assert!(r.flags < 32);
        }
    }
}

const WIND_OBJ: &str = r#"{"type":"adsb_icao","ias":267,"tas":484,"mach":0.796,"wd":297,"ws":40,"oat":-30,"tat":1,"track":309.00,"track_rate":-0.53,"roll":-10.72,"mag_heading":304.28,"true_heading":308.02}"#;

// REQ: OA-ADSB-023
#[test]
fn wind_temperature_and_speed_measurements_are_per_row_and_never_carried() {
    let json = wrap(&[
        &format!("[1.0,40.0,-88.0,25125,446.5,309.0,0,-2176,{WIND_OBJ},\"adsb_icao\",25875,-2208,335,-10.7]"),
        "[2.0,40.1,-88.1,25000,446.0,309.0,0,-2176,null,\"adsb_icao\",25800,-2208,336,-9.5]",
        "[3.0,40.2,-88.2,24900,446.0,309.0,0,-2176,null,\"adsb_icao\"]",
    ]);
    let (res, out, st) = run(&json, &WIDE);
    res.unwrap();
    assert_eq!((st.kept, st.bad_measurements), (3, 0));
    let a = reports_row(&out, 0).unwrap();
    // The object's values override the row columns and carry more digits.
    assert_eq!((a.ws_kt, a.wd_deg), (40, 297));
    assert_eq!((a.tas_kt, a.ias_kt, a.mach_milli), (484, 267, 796));
    assert_eq!((a.oat_c, a.tat_c), (-30, 1));
    assert_eq!((a.mag_heading_cdeg, a.true_heading_cdeg), (30_428, 30_802));
    assert_eq!((a.roll_cdeg, a.track_rate_cdps), (-1_072, -53));
    // Second row: no object, so only the columns' indicated airspeed and roll; no wind carried.
    let b = reports_row(&out, 1).unwrap();
    assert_eq!((b.ias_kt, b.roll_cdeg), (336, -950));
    assert_eq!(
        (b.ws_kt, b.wd_deg, b.tas_kt),
        (NONE_U16, NONE_U16, NONE_U16)
    );
    assert_eq!(
        (b.oat_c, b.tat_c, b.track_rate_cdps),
        (NONE_I16, NONE_I16, NONE_I16)
    );
    assert_eq!((b.mach_milli, b.mag_heading_cdeg), (NONE_U16, NONE_U16));
    // Third row: a nine-column row has none of them.
    let c = reports_row(&out, 2).unwrap();
    assert_eq!(
        (c.ias_kt, c.roll_cdeg, c.ws_kt),
        (NONE_U16, NONE_I16, NONE_U16)
    );
}

// REQ: OA-ADSB-024
#[test]
fn malformed_measurements_are_left_absent_and_counted_without_refusing_the_row() {
    let obj = r#"{"ws":"fast","wd":361,"tas":-5,"oat":-300,"mach":9.9,"roll":181,"true_heading":400,"mag_heading":null,"tat":3,"ias":100}"#;
    let json = wrap(&[
        &format!(
            "[1.0,40.0,-88.0,25125,446.5,309.0,0,-2176,{obj},\"adsb_icao\",25875,-2208,\"x\",null]"
        ),
        r#"[2.0,40.0,-88.0,25125,446.5,309.0,0,-2176,{"wd":360,"true_heading":360,"mag_heading":359.996}]"#,
    ]);
    let (res, out, st) = run(&json, &WIDE);
    res.unwrap();
    // Both rows kept; the first counted once however many of its values were bad.
    assert_eq!((st.kept, st.bad_measurements), (2, 1));
    assert_eq!(st.rejected, [0; REJECT_KINDS]);
    let a = reports_row(&out, 0).unwrap();
    assert_eq!(
        (a.ws_kt, a.wd_deg, a.tas_kt),
        (NONE_U16, NONE_U16, NONE_U16)
    );
    assert_eq!(
        (a.oat_c, a.mach_milli, a.roll_cdeg),
        (NONE_I16, NONE_U16, NONE_I16)
    );
    assert_eq!(
        (a.true_heading_cdeg, a.mag_heading_cdeg),
        (NONE_U16, NONE_U16)
    );
    // The good ones in the same object survive.
    assert_eq!((a.tat_c, a.ias_kt), (3, 100));
    // 360 wraps to 0 and 359.996 rounds to 360.00, which wraps to 0.
    let b = reports_row(&out, 1).unwrap();
    assert_eq!(
        (b.wd_deg, b.true_heading_cdeg, b.mag_heading_cdeg),
        (0, 0, 0)
    );
    // Stats add.
    let mut t = Stats::default();
    stats_add(&mut t, &st);
    stats_add(&mut t, &st);
    assert_eq!(t.bad_measurements, 2);
}
