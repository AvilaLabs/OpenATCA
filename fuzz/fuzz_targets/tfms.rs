#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    /// The open window: nothing is dropped.
    open: oa_tfms::Rows,
    /// The sealed window, so the whole-document pre-scan runs too.
    sealed: oa_tfms::Rows,
}

fn rows(window: oa_sealed::DropWindow) -> oa_tfms::Rows {
    oa_tfms::new_rows(
        oa_tfms::DEFAULT_FLIGHT_CAPACITY,
        oa_tfms::DEFAULT_FLOW_CAPACITY,
        oa_tfms::DEFAULT_STRING_BYTES,
        window,
    )
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_tfms::default_limits()).expect("default limits are valid"),
        open: rows(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
        sealed: rows(oa_sealed::DropWindow::sealed()),
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
        oa_tfms::clear_rows(open);
        let _ = oa_tfms::decode_bytes(parser, data, open);
        for prefix in [false, true] {
            let Ok(doc) = parser.parse(data) else { continue };
            oa_tfms::clear_rows(open);
            oa_tfms::clear_rows(sealed);
            let (a, b) = if prefix {
                (
                    oa_tfms::decode_doc_prefix(&doc, open),
                    oa_tfms::decode_doc_prefix(&doc, sealed),
                )
            } else {
                (
                    oa_tfms::decode_doc(&doc, open),
                    oa_tfms::decode_doc(&doc, sealed),
                )
            };
            // Oracle 1: no row the sealed decoder keeps has an observation time in the window.
            let w = oa_sealed::DropWindow::sealed();
            let nf = oa_tfms::flights_len(&sealed.flights);
            for i in 0..nf {
                let r = oa_tfms::flight_at(&sealed.flights, i).expect("a row below the length");
                assert!(!w.contains(r.src_ms) && !w.contains(r.pos_ms), "kept flight in the window");
            }
            for i in 0..oa_tfms::flows_len(&sealed.flows) {
                let r = oa_tfms::flow_at(&sealed.flows, i).expect("a row below the length");
                assert!(!w.contains(r.src_ms), "kept flow in the window");
            }
            // Oracle 2: a document the sealed decoder did not drop decodes as with no window.
            match &b {
                Ok(o) if o.sealed_dropped > 0 => {
                    assert_eq!(nf + oa_tfms::flows_len(&sealed.flows), 0);
                    assert_eq!(sealed.strings_used, 0);
                }
                _ => {
                    assert_eq!(a, b, "sealed and open results differ");
                    assert_eq!(nf, oa_tfms::flights_len(&open.flights));
                    assert_eq!(
                        oa_tfms::flows_len(&open.flows),
                        oa_tfms::flows_len(&sealed.flows)
                    );
                    assert_eq!(open.strings_used, sealed.strings_used);
                    for i in 0..nf {
                        assert_eq!(
                            oa_tfms::flight_at(&open.flights, i),
                            oa_tfms::flight_at(&sealed.flights, i)
                        );
                    }
                }
            }
        }
    });
});
