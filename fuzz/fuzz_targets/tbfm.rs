#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    /// The open window: nothing is dropped.
    open: oa_tbfm::Rows,
    /// The sealed window, so the whole-document pre-scan runs too.
    sealed: oa_tbfm::Rows,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_tbfm::default_limits()).expect("default limits are valid"),
        open: oa_tbfm::new_rows(
            oa_tbfm::DEFAULT_ROW_CAPACITY,
            oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
        ),
        sealed: oa_tbfm::new_rows(oa_tbfm::DEFAULT_ROW_CAPACITY, oa_sealed::sealed_drop_window()),
    });
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State {
            parser,
            open,
            sealed,
        } = &mut *s;
        oa_tbfm::clear_rows(open);
        let _ = oa_tbfm::decode_bytes(parser, data, open);
        for prefix in [false, true] {
            let Ok(doc) = parser.parse(data) else { continue };
            oa_tbfm::clear_rows(open);
            oa_tbfm::clear_rows(sealed);
            let (a, b) = if prefix {
                (
                    oa_tbfm::decode_doc_prefix(&doc, open),
                    oa_tbfm::decode_doc_prefix(&doc, sealed),
                )
            } else {
                (
                    oa_tbfm::decode_doc(&doc, open),
                    oa_tbfm::decode_doc(&doc, sealed),
                )
            };
            // Oracle 1: no row the sealed decoder keeps has an observation time in the window.
            let w = oa_sealed::sealed_drop_window();
            for i in 0..oa_tbfm::rows_len(sealed) {
                let r = oa_tbfm::row_at(sealed, i).expect("a row below the length");
                assert!(!oa_time_window::contains(w, r.msg_ms) && !oa_time_window::contains(w, r.env_ms), "kept row in the window");
                assert!(!oa_time_window::contains(w, r.ctm_ms), "kept crossing time in the window");
            }
            // The outcome's envelope time is a key too, whatever the rows are.
            if let Ok(o) = &b {
                assert!(o.sealed_dropped > 0 || !oa_time_window::contains(w, o.env_ms), "kept envelope time");
            }
            // Oracle 2: a document the sealed decoder did not drop decodes as with no window.
            match &b {
                Ok(o) if o.sealed_dropped > 0 => assert_eq!(oa_tbfm::rows_len(sealed), 0),
                _ => {
                    assert_eq!(a, b, "sealed and open results differ");
                    assert_eq!(oa_tbfm::rows_len(open), oa_tbfm::rows_len(sealed));
                    for i in 0..oa_tbfm::rows_len(open) {
                        assert_eq!(oa_tbfm::row_at(open, i), oa_tbfm::row_at(sealed, i));
                    }
                }
            }
        }
    });
});
