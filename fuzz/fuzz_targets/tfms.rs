#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    rows: oa_tfms::Rows,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_tfms::default_limits()).expect("default limits are valid"),
        rows: oa_tfms::new_rows(
            oa_tfms::DEFAULT_FLIGHT_CAPACITY,
            oa_tfms::DEFAULT_FLOW_CAPACITY,
            oa_tfms::DEFAULT_STRING_BYTES,
        ),
    });
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State { parser, rows } = &mut *s;
        oa_tfms::clear_rows(rows);
        let _ = oa_tfms::decode_bytes(parser, data, rows);
        oa_tfms::clear_rows(rows);
        if let Ok(doc) = parser.parse(data) {
            let _ = oa_tfms::decode_doc_prefix(&doc, rows);
        }
        for i in 0..oa_tfms::flights_len(&rows.flights) {
            let _ = oa_tfms::flight_at(&rows.flights, i);
        }
    });
});
