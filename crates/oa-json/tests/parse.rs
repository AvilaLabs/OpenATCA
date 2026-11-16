//! Hand-written cases for oa-json, independent of any downloaded data.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_json::{parse, ErrorKind, Kind, Limits, NodeIx, DEFAULT_LIMITS};

fn ok(s: &str) -> oa_json::Doc<'_> {
    parse(s.as_bytes(), &DEFAULT_LIMITS).unwrap()
}

fn kind_of(s: &[u8]) -> ErrorKind {
    parse(s, &DEFAULT_LIMITS).unwrap_err().kind
}

// REQ: OA-JSON-001
#[test]
fn strict_grammar_cases() {
    for good in [
        "0",
        "-0",
        "1.5e+3",
        "-1E-2",
        "true",
        "false",
        "null",
        "\"\"",
        "[]",
        "{}",
        " [ 1 , 2 ] ",
        "{\"a\":{\"b\":[null]}}",
        "\t\r\n 7 \n",
    ] {
        assert!(parse(good.as_bytes(), &DEFAULT_LIMITS).is_ok(), "{good}");
    }
    for bad in [
        "",
        " ",
        "01",
        "+1",
        "1.",
        ".5",
        "-",
        "1e",
        "1e+",
        "NaN",
        "Infinity",
        "[1,]",
        "[,1]",
        "{\"a\":1,}",
        "{a:1}",
        "{'a':1}",
        "'a'",
        "[1 2]",
        "{\"a\" 1}",
        "{\"a\":}",
        "[1",
        "{\"a\":1",
        "tru",
        "nul",
        "truee",
        "[\u{000b}1]",
        "[1]x",
        "[] []",
        "/* */1",
        "[1]//x",
    ] {
        assert!(parse(bad.as_bytes(), &DEFAULT_LIMITS).is_err(), "{bad:?}");
    }
}

// REQ: OA-JSON-002
#[test]
fn every_bound_is_enforced_and_named() {
    let small = Limits {
        max_input_bytes: 24,
        max_depth: 3,
        max_string_bytes: 4,
        max_nodes: 5,
    };
    let e = |s: &str| parse(s.as_bytes(), &small).unwrap_err().kind;
    assert_eq!(e("[1,2,3,4,5,6,7,8,9,10,11,12]"), ErrorKind::InputTooLarge);
    // Exactly at the input bound (24 bytes) is accepted, one more is not.
    let at = format!("\"{}\"", "x".repeat(2));
    assert!(parse(
        format!("{at}{}", " ".repeat(24 - at.len())).as_bytes(),
        &small
    )
    .is_ok());
    assert_eq!(
        e(&format!("{at}{}", " ".repeat(25 - at.len()))),
        ErrorKind::InputTooLarge
    );
    assert_eq!(e("[[[[1]]]]"), ErrorKind::DepthExceeded);
    assert!(parse(b"[[[1]]]", &small).is_ok());
    assert_eq!(e("\"abcde\""), ErrorKind::StringTooLong);
    assert!(parse(b"\"abcd\"", &small).is_ok());
    assert_eq!(e("[1,2,3,4,5]"), ErrorKind::TooManyNodes);
    assert!(parse(b"[1,2,3,4]", &small).is_ok());
    // The same string bound counts escaped text by its raw length, quotes excluded, on both
    // paths: four raw bytes pass whether plain or escaped, five do not.
    assert_eq!(e("\"\\n\\n\\n\""), ErrorKind::StringTooLong);
    assert!(parse(b"\"\\nxy\"", &small).is_ok());
    assert!(parse(b"\"x\\ny\"", &small).is_ok());
    assert!(parse(b"\"xy\\n\"", &small).is_ok());
    assert_eq!(e("\"\\u0041x\""), ErrorKind::StringTooLong);
    assert_eq!(e("\"\\nxyz\""), ErrorKind::StringTooLong);
    assert_eq!(e("\"xyz\\n\""), ErrorKind::StringTooLong);
    assert!(parse(b"\"\\\\\\\\\"", &small).is_ok());
    assert_eq!(e("\"\\\\\\\\\\\\\""), ErrorKind::StringTooLong);
    // Object keys are nodes too: 2 members = 5 nodes with the object itself.
    assert!(parse(b"{\"a\":1,\"b\":2}", &small).is_ok());
    assert_eq!(e("{\"a\":1,\"b\":2,\"c\":3}"), ErrorKind::TooManyNodes);
}

// REQ: OA-JSON-003
#[test]
fn numbers_are_text_with_checked_conversions() {
    let d = ok("[12, -0, 1.50, 2e3, 1e999, 9223372036854775807, 9223372036854775808, -9223372036854775808]");
    let v: Vec<NodeIx> = d.elements(NodeIx::ROOT).collect();
    assert_eq!(d.number_text(v[2]), Some("1.50"));
    assert_eq!(d.as_i64(v[0]), Some(12));
    assert_eq!(d.as_i64(v[1]), Some(0));
    assert_eq!(d.as_i64(v[2]), None, "fraction is not an integer");
    assert_eq!(d.as_i64(v[3]), None, "exponent form is not an integer");
    assert_eq!(d.as_f64(v[3]), Some(2000.0));
    assert_eq!(d.as_f64(v[4]), None, "overflows to infinity");
    assert_eq!(d.as_i64(v[5]), Some(i64::MAX));
    assert_eq!(d.as_i64(v[6]), None);
    assert_eq!(d.as_i64(v[7]), Some(i64::MIN));
    assert_eq!(d.as_f64(NodeIx::ROOT), None, "not a number");
    assert_eq!(d.number_text(NodeIx::ROOT), None);
}

// REQ: OA-JSON-004
#[test]
fn strings_decode_escapes_and_surrogate_pairs() {
    let d = ok(r#"["plain","a\"b\\c\/d\b\f\n\r\t","\u00e9\u4E2D","\ud834\udd1e","\u0000x","é"]"#);
    let v: Vec<NodeIx> = d.elements(NodeIx::ROOT).collect();
    assert_eq!(d.kind(v[0]), Some(Kind::String));
    assert_eq!(d.kind(v[1]), Some(Kind::StringPooled));
    assert_eq!(d.str(v[0]), Some("plain"));
    assert_eq!(d.str(v[1]), Some("a\"b\\c/d\u{8}\u{c}\n\r\t"));
    assert_eq!(d.str(v[2]), Some("é中"));
    assert_eq!(d.str(v[3]), Some("\u{1d11e}"));
    assert_eq!(d.str(v[4]), Some("\0x"));
    assert_eq!(d.str(v[5]), Some("é"));
    for bad in [
        r#""\ud834""#,
        r#""\udd1e""#,
        r#""\ud834\u0041""#,
        r#""\x""#,
        r#""\u12""#,
        r#""\u12G4""#,
    ] {
        assert_eq!(kind_of(bad.as_bytes()), ErrorKind::BadEscape, "{bad}");
    }
    assert_eq!(kind_of(b"\"a\nb\""), ErrorKind::ControlInString);
    assert_eq!(kind_of(b"\"a\\nb\x01\""), ErrorKind::ControlInString);
    assert_eq!(kind_of(b"\"abc"), ErrorKind::UnexpectedEnd);
}

// REQ: OA-JSON-005
#[test]
fn utf8_bom_and_trailing_garbage_are_refused() {
    assert_eq!(kind_of(b"[\"\xff\"]"), ErrorKind::InvalidUtf8);
    assert_eq!(kind_of(b"\xc0\x80"), ErrorKind::InvalidUtf8);
    assert_eq!(kind_of(b"\xef\xbb\xbf{}"), ErrorKind::UnexpectedByte);
    assert_eq!(kind_of(b"{} x"), ErrorKind::TrailingGarbage);
    assert_eq!(kind_of(b"1 2"), ErrorKind::TrailingGarbage);
    let e = parse(b"[1, 2] x", &DEFAULT_LIMITS).unwrap_err();
    assert_eq!(e.offset, 7);
}

// REQ: OA-JSON-006
#[test]
fn duplicate_keys_are_kept_and_get_returns_the_last() {
    let d = ok(r#"{"a":1,"b":2,"a":3}"#);
    assert_eq!(d.len(NodeIx::ROOT), 3);
    let pairs: Vec<_> = d
        .members(NodeIx::ROOT)
        .map(|(k, _)| d.str(k).unwrap())
        .collect();
    assert_eq!(pairs, ["a", "b", "a"]);
    assert_eq!(d.as_i64(d.get(NodeIx::ROOT, "a").unwrap()), Some(3));
    assert_eq!(d.as_i64(d.get(NodeIx::ROOT, "b").unwrap()), Some(2));
    assert!(d.get(NodeIx::ROOT, "c").is_none());
}

// REQ: OA-JSON-007
#[test]
fn deep_nesting_cannot_overflow_the_stack() {
    // Depth far past anything recursive code survives, with the limit raised to allow it.
    let n = 1_000_000;
    let mut s = "[".repeat(n);
    s.push_str(&"]".repeat(n));
    let lim = Limits {
        max_depth: n,
        ..DEFAULT_LIMITS
    };
    let d = parse(s.as_bytes(), &lim).unwrap();
    assert_eq!(d.node_count(), n);
    assert_eq!(d.skip(NodeIx::ROOT), NodeIx(n as u32));
    assert_eq!(d.elements(NodeIx::ROOT).count(), 1);
    // The default limit refuses the same text without recursing.
    assert_eq!(kind_of(s.as_bytes()), ErrorKind::DepthExceeded);
}

// REQ: OA-JSON-009
#[test]
fn flat_arena_navigation() {
    let d = ok(r#"{"x":[1,[2,3],{"k":[]}],"y":"s","z":null}"#);
    assert_eq!(d.node_count(), 14);
    let x = d.get(NodeIx::ROOT, "x").unwrap();
    assert_eq!(d.kind(x), Some(Kind::Array));
    assert_eq!(d.len(x), 3);
    let kids: Vec<NodeIx> = d.elements(x).collect();
    assert_eq!(d.as_i64(kids[0]), Some(1));
    assert_eq!(d.len(kids[1]), 2);
    assert_eq!(d.kind(kids[2]), Some(Kind::Object));
    let k = d.get(kids[2], "k").unwrap();
    assert!(d.is_empty(k));
    assert_eq!(d.str(d.get(NodeIx::ROOT, "y").unwrap()), Some("s"));
    assert_eq!(d.kind(d.get(NodeIx::ROOT, "z").unwrap()), Some(Kind::Null));
    // Wrong-kind queries are empty, never panics.
    assert_eq!(d.elements(NodeIx::ROOT).count(), 0);
    assert_eq!(d.members(x).count(), 0);
    assert_eq!(d.kind(NodeIx(9999)), None);
    assert_eq!(d.as_bool(NodeIx(9999)), None);
    let b = ok("[true,false]");
    let v: Vec<_> = b.elements(NodeIx::ROOT).collect();
    assert_eq!(
        (b.as_bool(v[0]), b.as_bool(v[1])),
        (Some(true), Some(false))
    );
}

// REQ: OA-JSON-010
#[test]
fn damaged_input_never_panics() {
    let sample = br#"{"icao":"3c66b0","timestamp":1663259853.016,"trace":[[7016.59,49.2633,10.614239,25125,446.5,309.0,0,-2176,{"type":"adsb_icao","flight":"DLH7YA  ","sil_type":"perhour"},"adsb_icao",25875,-2208,335,-10.7],[1,2,"\u00e9\ud834\udd1e",null,true,false,-1.5e-3]]}"#;
    assert!(parse(sample, &DEFAULT_LIMITS).is_ok());
    for cut in 0..sample.len() {
        let _ = parse(&sample[..cut], &DEFAULT_LIMITS);
    }
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..20_000 {
        let mut b = sample.to_vec();
        for _ in 0..3 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % b.len() as u64) as usize;
            b[at] = (seed >> 32) as u8;
        }
        let _ = parse(&b, &DEFAULT_LIMITS);
    }
}
