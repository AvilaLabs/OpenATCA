#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    decoder: oa_fixm::Decoder,
    // The same decoder with the sealed window, so the whole-document pre-scan runs too.
    sealed: oa_fixm::Decoder,
    events: oa_fixm::Events,
}

thread_local! {
    static STATE: RefCell<State> = {
        let limits = oa_fixm::Limits::new(oa_sealed::DropWindow::empty_for_tests_and_fuzz());
        RefCell::new(State {
            parser: oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"),
            decoder: oa_fixm::Decoder::new(limits),
            sealed: oa_fixm::Decoder::new(oa_fixm::Limits::new(oa_sealed::DropWindow::sealed())),
            events: oa_fixm::new_events(256, 256 * 1024),
        })
    };
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State {
            parser,
            decoder,
            sealed,
            events,
        } = &mut *s;
        oa_fixm::clear_events(events);
        if let Ok(doc) = parser.parse(data) {
            let _ = decoder.decode(&doc, data, events);
            oa_fixm::clear_events(events);
            let _ = decoder.decode_prefix(&doc, data, events);
            oa_fixm::clear_events(events);
            let _ = sealed.decode_prefix(&doc, data, events);
        }
    });
});
