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
        let mut stats = oa_stdds::new_stats();
        let _ = oa_stdds::parse_message(parser, data, open, &mut stats);
        let _ = oa_stdds::parse_message(parser, data, sealed, &mut stats);
    });
});
