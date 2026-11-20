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
                // The message time, the position time and the actual-event times a pre-scan reads.
                for t in [
                    r.src_ms,
                    r.pos_ms,
                    r.out_ms,
                    r.off_ms,
                    r.on_ms,
                    r.in_ms,
                    r.rwy_dep_ms,
                    r.rwy_arr_ms,
                    r.gate_dep_ms,
                    r.gate_arr_ms,
                    r.bnd_ms,
                ] {
                    assert!(!w.contains(t), "kept flight in the window");
                }
                // The estimate and coordination times are keys only when they are actual: an eta
                // or etd of type ACTUAL, a time of departure or arrival that is not an estimate,
                // and an ACTUAL coordination time. The creation time and the TMI update time are
                // keys always.
                use oa_tfms::rows::{flag, ttype};
                let actual = |ty: u8, t: i64| ty == ttype::ACTUAL && w.contains(t);
                assert!(!actual(r.eta_type, r.eta_ms), "kept actual eta in the window");
                assert!(!actual(r.etd_type, r.etd_ms), "kept actual etd in the window");
                assert!(
                    !actual(r.coord_type, r.coord_ms),
                    "kept actual coordination time in the window"
                );
                for (est, t) in [(flag::TOD_EST, r.tod_ms), (flag::TOA_EST, r.toa_ms)] {
                    let estimated = oa_tfms::flag_get(r.flags, est) == flag::TRUE;
                    assert!(estimated || !w.contains(t), "kept actual time of day in window");
                }
                assert!(!w.contains(r.created_ms), "kept creation time in the window");
                assert!(!w.contains(r.tmi_ms), "kept TMI update time in the window");
            }
            for i in 0..oa_tfms::flows_len(&sealed.flows) {
                let r = oa_tfms::flow_at(&sealed.flows, i).expect("a row below the length");
                assert!(
                    !w.contains(r.src_ms) && !w.contains(r.event_ms),
                    "kept flow in the window"
                );
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
