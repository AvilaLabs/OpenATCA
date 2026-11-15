#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    decoder: oa_fixm::Decoder,
    events: oa_fixm::Events,
}

thread_local! {
    static STATE: RefCell<State> = {
        let limits = oa_fixm::Limits::default();
        RefCell::new(State {
            parser: oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"),
            decoder: oa_fixm::Decoder::new(limits),
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
            events,
        } = &mut *s;
        oa_fixm::clear_events(events);
        if let Ok(doc) = parser.parse(data) {
            let _ = decoder.decode(&doc, data, events);
            oa_fixm::clear_events(events);
            let _ = decoder.decode_prefix(&doc, data, events);
        }
    });
});
