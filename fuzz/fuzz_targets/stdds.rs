#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    // One with no window (every decode path runs) and one with the sealed window (the
    // whole-message pre-scan runs and drops).
    open: oa_stdds::Limits,
    sealed: oa_stdds::Limits,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"),
        open: oa_stdds::Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
        sealed: oa_stdds::Limits::new(oa_sealed::DropWindow::sealed()),
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
        let (mut st_open, mut st_sealed) = (oa_stdds::new_stats(), oa_stdds::new_stats());
        let a = oa_stdds::parse_message(parser, data, open, &mut st_open);
        let b = oa_stdds::parse_message(parser, data, sealed, &mut st_sealed);
        let w = oa_sealed::DropWindow::sealed();
        // Oracle 1: no record the sealed decoder keeps has an observation time in the window.
        match &b {
            Ok(oa_stdds::Message::Tais(t)) => {
                for r in &t.records {
                    assert!(
                        !w.contains(r.mrt_time_ms) && !w.contains(r.rec_safa_receipt_ms),
                        "kept record in the window"
                    );
                }
            }
            Ok(oa_stdds::Message::Smes(t)) => {
                for r in &t.records {
                    assert!(!w.contains(r.time_ms), "kept report in the window");
                }
            }
            _ => {}
        }
        // Oracle 2: a message the sealed decoder did not drop decodes as with no window.
        if st_sealed.sealed_window_dropped == 0 {
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "results differ");
            assert_eq!(st_open, st_sealed);
        } else {
            assert_eq!(st_sealed.records, 0);
        }
    });
});
