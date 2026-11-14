//! Adversarial review of oa-xml. Every test here demonstrates a defect and is `#[ignore]`d so the
//! suite stays green until the defect is fixed; run them with `cargo test -p oa-xml --test
//! adversarial -- --ignored`. Each asserts the behaviour the specification or REQUIREMENTS.md
//! demands, so a fix turns the test green and the `#[ignore]` can be removed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::{Duration, Instant};

use oa_xml::{
    decode_into, resolve_attr_ns, span_eq_str, AttrIx, ElemIx, Limits, Ns, Parser, Span, XmlError,
};

fn parses(src: &[u8]) -> Result<(), XmlError> {
    let mut p = Parser::new(Limits::default());
    p.parse(src).map(|_| ())
}

/// A document of 63 nested elements, each declaring 64 prefixes (4032 bindings live at the
/// deepest level), then `leaves` leaf elements built by `leaf`. About 67 KB of header.
fn ns_churn_doc(leaves: usize, leaf: &str) -> String {
    let mut s = String::from("<r");
    for lvl in 0..63 {
        if lvl > 0 {
            s.push_str("<e");
        }
        for k in 0..64 {
            s.push_str(&format!(" xmlns:p{lvl}_{k}=\"u\""));
        }
        s.push('>');
    }
    for _ in 0..leaves {
        s.push_str(leaf);
    }
    for lvl in (0..63).rev() {
        s.push_str(if lvl > 0 { "</e>" } else { "</r>" });
    }
    s
}

fn best_of(n: usize, mut f: impl FnMut()) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..n {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed());
    }
    best
}

// FINDING F1 (medium): duplicate attributes are detected by qualified name only. Namespaces in
// XML 1.0 section 6.3 forbids two attributes with the same expanded name, so `a:x` and `b:x`
// bound to one URI must be rejected (expat rejects it). parse.rs:811-824.
// Repro: br#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:x="2"/>"#
#[test]
#[ignore = "F1: duplicate attribute by expanded name accepted"]
fn f1_duplicate_attribute_by_expanded_name() {
    let r = parses(br#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:x="2"/>"#);
    assert!(
        matches!(r, Err(XmlError::DuplicateAttribute { .. })),
        "{r:?}"
    );
}

// FINDING F2 (medium): `bound()` scans every live namespace binding linearly for each prefixed
// element name and each prefixed attribute, so cost is (live bindings, up to max_depth *
// max_attrs_per_elem = 4032) x (prefixed names). 40,000 tiny leaves in a 627 KB message take
// about 160 ms against about 3 ms for the same message without the prefix (50-100x per byte);
// 64 prefixed attributes per leaf reaches about 320 ns per byte against 3 for ordinary input.
// Bounded by the limits, but IF-R1/G5 want a stated, small worst case. parse.rs:592-601, called
// from 698 and 710.
// Repro: ns_churn_doc(40_000, "<l p0_0:x=\"\"/>") versus the same with `x=""`.
#[test]
#[ignore = "F2: prefix lookup is O(live bindings) per use"]
fn f2_prefix_lookup_is_not_linear_in_bindings() {
    let hot = ns_churn_doc(40_000, "<l p0_0:x=\"\"/>");
    let control = ns_churn_doc(40_000, "<l x=\"\"/>");
    let mut p = Parser::new(Limits::default());
    let t_hot = best_of(3, || {
        p.parse(hot.as_bytes()).unwrap();
    });
    let t_ctl = best_of(3, || {
        p.parse(control.as_bytes()).unwrap();
    });
    // Per byte, the prefixed document may cost at most 8x the control.
    let hot_pb = t_hot.as_nanos() as f64 / hot.len() as f64;
    let ctl_pb = t_ctl.as_nanos() as f64 / control.len() as f64;
    assert!(
        hot_pb < 8.0 * ctl_pb,
        "{hot_pb:.1} ns/byte vs control {ctl_pb:.1} ns/byte"
    );
}

// FINDING F2b (low): the same cost on the consumer side. `resolve_attr_ns` / `resolve_elem_ns`
// walk the ancestor chain scanning every attribute of every ancestor, about 10 us per call at
// depth 64 with 64 declarations per level: 20,000 lookups take about 190 ms. ns.rs:34-67.
#[test]
#[ignore = "F2b: namespace resolution costs O(depth x attrs) per call"]
fn f2b_resolution_cost_is_bounded() {
    let s = ns_churn_doc(20_000, "<l p0_0:x=\"\"/>");
    let mut p = Parser::new(Limits::default());
    let d = p.parse(s.as_bytes()).unwrap();
    let t = Instant::now();
    for e in 0..d.elem_name.len() {
        let a = AttrIx(d.elem_first_attr[e].0);
        let _ = resolve_attr_ns(&d, ElemIx(e as u32), a);
    }
    let per_call = t.elapsed().as_nanos() / d.elem_name.len() as u128;
    assert!(per_call < 1_000, "{per_call} ns per lookup");
}

// FINDING F3 (low): character references with many leading zeros are well-formed (the digit
// string is unbounded in the grammar) but `parse_entity` only looks 12 bytes ahead for the `;`,
// so `&#x0000000041;` (value 0x41, 10 digits) is rejected as malformed. decode.rs:21-23.
// Repro: b"<r>&#x0000000041;</r>"
#[test]
#[ignore = "F3: char reference with 10+ leading zeros rejected"]
fn f3_leading_zero_char_reference() {
    assert_eq!(parses(b"<r>&#x0000000041;</r>"), Ok(()));
    assert_eq!(parses(b"<r>&#0000000000065;</r>"), Ok(()));
}

// FINDING F4 (medium): name characters are "any non-ASCII", checked only for UTF-8 validity, so
// U+FFFE and U+FFFF are accepted inside element, attribute and end-tag names. That breaks
// OA-XML-008 (no character outside Char, raw), and also admits non-name characters such as
// U+00D7, U+00B7 at the start, U+0085, U+2028 and U+FEFF (expat rejects them). parse.rs:168-186.
// Repro: "<a\u{fffe}b/>"
#[test]
#[ignore = "F4: U+FFFE / U+FFFF accepted in names (OA-XML-008); non-name chars accepted"]
fn f4_names_reject_noncharacters_and_non_name_chars() {
    assert!(parses("<a\u{fffe}b/>".as_bytes()).is_err(), "U+FFFE in element name");
    assert!(parses("<a\u{ffff}/>".as_bytes()).is_err(), "U+FFFF in element name");
    assert!(
        parses("<a x\u{fffe}=\"1\"/>".as_bytes()).is_err(),
        "U+FFFE in attribute name"
    );
    assert!(parses("<a\u{d7}b/>".as_bytes()).is_err(), "U+00D7 (multiplication sign)");
    assert!(parses("<a\u{2028}b/>".as_bytes()).is_err(), "U+2028 line separator");
    assert!(parses("<a\u{feff}b/>".as_bytes()).is_err(), "U+FEFF");
}

// FINDING F5 (low): Namespaces in XML 1.0 section 3 reserves the `xml` prefix (bindable only to
// the XML namespace, which in turn may be bound to no other prefix nor be the default) and the
// `http://www.w3.org/2000/xmlns/` namespace (bindable to nothing). All of these are accepted.
// parse.rs:665-683.
#[test]
#[ignore = "F5: reserved namespace bindings accepted"]
fn f5_reserved_namespace_bindings() {
    let bad: [&[u8]; 3] = [
        br#"<r xmlns:xml="urn:x"/>"#,
        br#"<r xmlns="http://www.w3.org/XML/1998/namespace"/>"#,
        br#"<r xmlns:a="http://www.w3.org/2000/xmlns/"/>"#,
    ];
    for b in bad {
        assert!(parses(b).is_err(), "{}", String::from_utf8_lossy(b));
    }
}

// FINDING F6 (low): input that ends inside an end tag or an attribute is reported as a generic
// `Syntax` error instead of `UnexpectedEof` (OA-XML-009 names "input that ends inside markup";
// `<a` and `<a><!-- x` do report UnexpectedEof, so the family is inconsistent). parse.rs:569-589
// and 765-799.
#[test]
#[ignore = "F6: truncation inside end tag / attribute reported as Syntax, not UnexpectedEof"]
fn f6_truncation_is_unexpected_eof() {
    let cases: [&[u8]; 4] = [b"<a></", b"<a></a", b"<a b", b"<a b="];
    for c in cases {
        let r = parses(c);
        assert!(
            matches!(r, Err(XmlError::UnexpectedEof { .. })),
            "{}: {r:?}",
            String::from_utf8_lossy(c)
        );
    }
}

// FINDING F7 (low, documented deviation from XML 1.0 sections 2.11 and 3.3.3): line ends and
// attribute whitespace are not normalised, so a value decoded with `decode_into` keeps `\r\n`
// (should be `\n`) and a tab or newline in an attribute value (should become a space). A
// consumer comparing multi-line values with `span_eq_str` sees different strings from every
// conforming parser.
#[test]
#[ignore = "F7: no end-of-line / attribute-value normalisation in decode_into"]
fn f7_line_end_normalisation() {
    let src = b"x\r\ny\rz";
    let mut out = [0u8; 16];
    let n = decode_into(Span { start: 0, len: src.len() as u32 }, src, &mut out).unwrap();
    assert_eq!(&out[..n], b"x\ny\nz");
}

// FINDING F8 (low): `decode_into` writes the bytes decoded before an error into `out` and then
// returns Err (both for `OutputTooSmall` and for a bad reference), leaving a partial result in
// the caller's buffer. A caller that ignores the error, or reuses the buffer, sees a truncated
// value. decode.rs:62-96. Repro: src = b"abc&amp;def&bad;"
#[test]
#[ignore = "F8: decode_into leaves a partial write on error"]
fn f8_decode_into_no_partial_write_on_error() {
    let src = b"abc&amp;def&bad;";
    let mut out = [b'#'; 32];
    let r = decode_into(Span { start: 0, len: src.len() as u32 }, src, &mut out);
    assert!(r.is_err());
    assert_eq!(out, [b'#'; 32], "buffer modified despite error");

    let src2 = b"abc&amp;d";
    let mut small = [b'#'; 4];
    let r = decode_into(Span { start: 0, len: src2.len() as u32 }, src2, &mut small);
    assert!(matches!(r, Err(XmlError::OutputTooSmall { .. })));
    assert_eq!(small, [b'#'; 4], "buffer modified despite OutputTooSmall");
}

// FINDING F9 (low): `span_eq_str` returns Ok(false) at the first mismatch without validating
// the rest of the span, while `decode_into` of the same span returns Err for the bad reference.
// The two disagree on the same span. decode.rs:100-125.
// Repro: span over b"xyz&bad;" compared with "q".
#[test]
#[ignore = "F9: span_eq_str and decode_into disagree on a span with a bad reference"]
fn f9_span_eq_and_decode_agree_on_bad_reference() {
    let src = b"xyz&bad;";
    let span = Span { start: 0, len: src.len() as u32 };
    let d = decode_into(span, src, &mut [0u8; 16]);
    let e = span_eq_str(span, src, "q");
    assert_eq!(d.is_err(), e.is_err(), "decode {d:?} vs eq {e:?}");
}

// FINDING F10 (low): a span that does not lie inside the input is treated as the empty string by
// both `decode_into` (Ok(0)) and `span_eq_str` (equal to ""), instead of being refused (RULESET
// E5). A span from another message, or a stale one, silently compares equal to "". lib.rs:293.
#[test]
#[ignore = "F10: out-of-range span silently decodes as empty"]
fn f10_out_of_range_span_is_refused() {
    let src = b"abc";
    let span = Span { start: 5, len: 100 };
    assert!(decode_into(span, src, &mut [0u8; 16]).is_err());
    assert!(span_eq_str(span, src, "").is_err());
}

// FINDING F11 (low): `Parser::new` panics with "capacity overflow" for extreme limits
// (`max_depth * max_attrs_per_elem` saturates to usize::MAX; `max_elements` = usize::MAX),
// rather than returning an error (RULESET E3). Configuration only, never input. parse.rs:32-56.
#[test]
#[ignore = "F11: Parser::new panics on extreme limits"]
fn f11_parser_new_does_not_panic_on_extreme_limits() {
    let r = std::panic::catch_unwind(|| {
        let _ = Parser::new(Limits {
            max_depth: usize::MAX,
            max_attrs_per_elem: 2,
            ..Limits::default()
        });
    });
    assert!(r.is_ok(), "Parser::new panicked");
    let _ = Ns::None;
}
