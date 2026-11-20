#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

struct State {
    parser: oa_xml::Parser,
    /// The open window: nothing is dropped.
    open: oa_fixm::Decoder,
    /// The sealed window, so the whole-document pre-scan runs too.
    sealed: oa_fixm::Decoder,
    events: oa_fixm::Events,
    sealed_events: oa_fixm::Events,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        parser: oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"),
        open: oa_fixm::Decoder::new(oa_fixm::Limits::new(
            oa_sealed::DropWindow::empty_for_tests_and_fuzz(),
        )),
        sealed: oa_fixm::Decoder::new(oa_fixm::Limits::new(oa_sealed::DropWindow::sealed())),
        events: oa_fixm::new_events(256, 256 * 1024),
        sealed_events: oa_fixm::new_events(256, 256 * 1024),
    });
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let State {
            parser,
            open,
            sealed,
            events,
            sealed_events,
        } = &mut *s;
        let Ok(doc) = parser.parse(data) else { return };
        let w = oa_sealed::DropWindow::sealed();
        for prefix in [false, true] {
            oa_fixm::clear_events(events);
            oa_fixm::clear_events(sealed_events);
            let (a, b) = if prefix {
                (
                    open.decode_prefix(&doc, data, events),
                    sealed.decode_prefix(&doc, data, sealed_events),
                )
            } else {
                (
                    open.decode(&doc, data, events),
                    sealed.decode(&doc, data, sealed_events),
                )
            };
            // Oracle 1: no event the sealed decoder keeps has an observation time in the window.
            for i in 0..sealed_events.len {
                assert!(
                    !w.contains(sealed_events.ts_ms[i]) && !w.contains(sealed_events.pos_time_ms[i]),
                    "kept event in the window"
                );
            }
            // Oracle 2: a document the sealed decoder did not drop decodes as with no window.
            match &b {
                Ok(sm) if sm.sealed_window_dropped > 0 => {
                    assert_eq!((sealed_events.len, sealed_events.strings_used), (0, 0));
                }
                _ => {
                    assert_eq!(format!("{a:?}"), format!("{b:?}"), "results differ");
                    assert_eq!(events.len, sealed_events.len);
                    assert_eq!(events.ts_ms, sealed_events.ts_ms);
                    assert_eq!(events.pos_time_ms, sealed_events.pos_time_ms);
                    assert_eq!(events.strings_used, sealed_events.strings_used);
                }
            }
        }
    });
});
