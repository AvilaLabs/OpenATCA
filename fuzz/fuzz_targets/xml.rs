#![no_main]
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;

thread_local! {
    static PARSER: RefCell<oa_xml::Parser> = RefCell::new(
        oa_xml::Parser::new(oa_xml::Limits::default()).expect("default limits are valid"));
}

fuzz_target!(|data: &[u8]| {
    PARSER.with(|p| {
        let mut p = p.borrow_mut();
        if let Ok(doc) = p.parse(data) {
            for e in 0..doc.elem_name.len() {
                let _ = oa_xml::resolve_elem_ns(&doc, oa_xml::ElemIx(e as u32));
            }
        }
    });
});
