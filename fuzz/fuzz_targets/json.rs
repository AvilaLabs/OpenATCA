#![no_main]
use libfuzzer_sys::fuzz_target;
use oa_json::{Kind, NodeIx};

fuzz_target!(|data: &[u8]| {
    let limits = oa_json::Limits {
        max_nodes: 1 << 16,
        ..oa_json::DEFAULT_LIMITS
    };
    if let Ok(doc) = oa_json::parse(data, &limits) {
        for i in 0..doc.node_count().min(4096) {
            let n = NodeIx(i as u32);
            match doc.kind(n) {
                Some(Kind::Number) => {
                    let _ = (doc.as_f64(n), doc.as_i64(n), doc.number_text(n));
                }
                Some(Kind::String | Kind::StringPooled) => {
                    let _ = doc.str(n);
                }
                Some(Kind::Object) => {
                    for _ in doc.members(n) {}
                    let _ = doc.get(n, "icao");
                }
                Some(Kind::Array) => for _ in doc.elements(n) {},
                _ => {}
            }
        }
    }
});
