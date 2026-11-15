#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    rows: oa_tbfm::Rows,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_tbfm::default_limits()).expect("default limits are valid"),
        rows: oa_tbfm::new_rows(oa_tbfm::DEFAULT_ROW_CAPACITY),
    });
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State { parser, rows } = &mut *s;
        oa_tbfm::clear_rows(rows);
        let _ = oa_tbfm::decode_bytes(parser, data, rows);
        oa_tbfm::clear_rows(rows);
        if let Ok(doc) = parser.parse(data) {
            let _ = oa_tbfm::decode_doc_prefix(&doc, rows);
        }
        for i in 0..oa_tbfm::rows_len(rows) {
            let _ = oa_tbfm::row_at(rows, i);
        }
    });
});
