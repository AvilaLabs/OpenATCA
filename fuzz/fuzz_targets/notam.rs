#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_notam::{
    clear_notams, code_str, field_error, has_circle, has_field, has_geometry, new_decoder,
    new_notams, new_stats, notams_len, polygon_vertices, polygons_of, row_at, schedule_of, text_of,
    unknown_text, Decoder, Limits, Notams, Stats, ALL_FIELDS,
};
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    dec: Decoder,
    // The same decoder with the sealed window, so the whole-document pre-scan runs too.
    sealed: Decoder,
    rows: Notams,
    sealed_rows: Notams,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"),
        dec: new_decoder(Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz())),
        sealed: new_decoder(Limits::new(oa_sealed::sealed_drop_window())),
        rows: new_notams(16, 1 << 16, 32, 4096),
        sealed_rows: new_notams(16, 1 << 16, 32, 4096),
    });
}

fuzz_target!(|data: &[u8]| {
    // The scalar field parsers, on the raw bytes.
    let _ = (
        oa_notam::scalar::parse_iso_ms(data),
        oa_notam::scalar::parse_compact_ms(data),
        oa_notam::scalar::parse_eff_end(data),
        oa_notam::scalar::parse_uint(data, 9),
        oa_notam::scalar::parse_uuid(data),
        oa_notam::scalar::parse_code::<8>(data, b"-"),
        oa_notam::scalar::trim(data),
        oa_notam::scalar::parse_fl3(data),
        oa_notam::scalar::parse_limit(data),
        oa_notam::scalar::parse_coord(data),
        oa_notam::scalar::parse_scaled(data, 3),
    );
    let (lat, lon) = data.split_at(data.len() / 2);
    let _ = oa_notam::scalar::parse_lat_lon(lat, lon);

    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State {
            parser,
            dec,
            sealed,
            rows,
            sealed_rows,
        } = &mut *s;
        clear_notams(rows);
        clear_notams(sealed_rows);
        let mut stats: Stats = new_stats(64);
        let mut sealed_stats: Stats = new_stats(64);
        let a = oa_notam::decode_bytes(parser, dec, data, rows, &mut stats);
        let b = oa_notam::decode_bytes(parser, sealed, data, sealed_rows, &mut sealed_stats);
        // Oracle 1: no row the sealed decoder keeps has an observation time in the window.
        let w = oa_sealed::sealed_drop_window();
        for i in 0..notams_len(sealed_rows) {
            let r = row_at(sealed_rows, i).expect("a row below the length");
            assert!(
                !oa_time_window::contains(w, r.issued_ms)
                    && !oa_time_window::contains(w, r.canceled_ms),
                "kept row in the window"
            );
        }
        // Oracle 2: a document the sealed decoder did not drop decodes as with no window.
        match &b {
            Ok(o) if o.sealed_dropped => assert_eq!(notams_len(sealed_rows), 0),
            _ => {
                assert_eq!(format!("{a:?}"), format!("{b:?}"), "results differ");
                assert_eq!(stats, sealed_stats);
                assert_eq!(notams_len(rows), notams_len(sealed_rows));
                for i in 0..notams_len(rows) {
                    assert_eq!(row_at(rows, i), row_at(sealed_rows, i));
                }
            }
        }
        for u in &stats.unknown {
            let _ = unknown_text(u);
        }
        for i in 0..notams_len(rows) {
            let _ = row_at(rows, i);
            let _ = (text_of(rows, i), schedule_of(rows, i));
            let _ = (has_circle(rows, i), has_geometry(rows, i));
            for f in ALL_FIELDS {
                let _ = (has_field(rows, i, f), field_error(rows, i, f));
            }
            for p in polygons_of(rows, i) {
                let _ = polygon_vertices(rows, p);
            }
            if let Some(r) = row_at(rows, i) {
                let _ = code_str(&r.fir);
            }
        }
    });
});
