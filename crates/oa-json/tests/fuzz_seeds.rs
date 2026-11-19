//! The committed fuzz seeds (`fuzz/seeds/json`) through the entry points of the `json` fuzz
//! target. `ok-*` seeds must parse, `bad-*` seeds must be refused with an error, and every node
//! accessor must be safe on whatever parsed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_json::{Kind, NodeIx};

#[test]
fn every_committed_seed_parses_or_is_refused_as_named() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/json");
    let limits = oa_json::Limits {
        max_nodes: 1 << 16,
        ..oa_json::DEFAULT_LIMITS
    };
    let (mut accepted, mut refused) = (0, 0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let data = std::fs::read(path).unwrap();
        assert!(data.len() < 64 * 1024, "{name} is over 64 KiB");
        let parsed = oa_json::parse(&data, &limits);
        if let Ok(doc) = &parsed {
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
        if name.starts_with("ok-") {
            assert!(parsed.is_ok(), "{name} should parse: {:?}", parsed.err());
            accepted += 1;
        } else if name.starts_with("bad-") {
            assert!(parsed.is_err(), "{name} should be refused");
            refused += 1;
        } else {
            panic!("{name}: seed names start with ok- or bad-");
        }
    }
    eprintln!("json seeds: {accepted} accepted, {refused} refused");
    assert_eq!((accepted, refused), (32, 46));
}
