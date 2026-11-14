//! Adversarial review of oa-xml. Every test here began as a demonstration of a defect found in
//! review (the FINDING comments say what it was); each asserts the behaviour the specification
//! or REQUIREMENTS.md demands and passes now that the defect is fixed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::{Duration, Instant};

use oa_xml::{
    decode_into, limit_name, resolve_attr_ns, resolve_elem_ns, span_bytes, span_eq_str,
    validate_limits, AttrIx, ElemIx, LimitKind, Limits, Ns, Parser, Span, SpanKind, XmlError,
};

fn parses(src: &[u8]) -> Result<(), XmlError> {
    let mut p = Parser::new(Limits::default()).unwrap();
    p.parse(src).map(|_| ())
}

/// The most namespace bindings the default limits allow in scope.
const FULL: usize = 64;

/// A document of 8 nested elements, each declaring 8 prefixes (`FULL` = 64 bindings live at
/// the deepest level, the most the default limit admits), then `leaves` leaf elements built by
/// `leaf`. The review's original (63 levels of 64, 4,032 bindings) is now refused outright.
fn ns_churn_doc(leaves: usize, leaf: &str) -> String {
    let mut s = String::from("<r");
    for lvl in 0..8 {
        if lvl > 0 {
            s.push_str("<e");
        }
        for k in 0..8 {
            s.push_str(&format!(" xmlns:p{lvl}_{k}=\"u\""));
        }
        s.push('>');
    }
    for _ in 0..leaves {
        s.push_str(leaf);
    }
    for lvl in (0..8).rev() {
        s.push_str(if lvl > 0 { "</e>" } else { "</r>" });
    }
    s
}

/// A leaf with 64 prefixed attributes of distinct names, all in the outermost namespace.
fn wide_leaf(prefixed: bool) -> String {
    let mut s = String::from("<l");
    for k in 0..64 {
        let pre = if prefixed { "p0_0:" } else { "" };
        s.push_str(&format!(" {pre}x{k}=\"\""));
    }
    s.push_str("/>");
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

/// Best-of-5 nanoseconds per input byte of parsing `doc`.
fn ns_per_byte(p: &mut Parser, doc: &str) -> f64 {
    let t = best_of(5, || {
        p.parse(doc.as_bytes()).unwrap();
    });
    t.as_nanos() as f64 / doc.len() as f64
}

// FINDING F1 (medium): duplicate attributes are detected by qualified name only. Namespaces in
// XML 1.0 section 6.3 forbids two attributes with the same expanded name, so `a:x` and `b:x`
// bound to one URI must be rejected (expat rejects it). Fixed: after the element's
// declarations are known, prefixed attributes are also compared by (URI bytes, local name),
// wherever the declarations sit on the element; the qualified-name check stays. Cost per
// element is O(a^2) in its attributes, a <= 64 by default.
// Repro: br#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:x="2"/>"#
// REQ: OA-XML-010
#[test]
fn f1_duplicate_attribute_by_expanded_name() {
    let r = parses(br#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:x="2"/>"#);
    assert!(
        matches!(r, Err(XmlError::DuplicateAttribute { at: 35 })),
        "{r:?}"
    );
    // Declarations after the attributes, and an inherited declaration, count too.
    let r = parses(br#"<r a:x="1" b:x="2" xmlns:a="u" xmlns:b="u"/>"#);
    assert!(
        matches!(r, Err(XmlError::DuplicateAttribute { .. })),
        "{r:?}"
    );
    let r = parses(br#"<r xmlns:a="u"><e xmlns:b="u" a:x="1" b:x="2"/></r>"#);
    assert!(
        matches!(r, Err(XmlError::DuplicateAttribute { .. })),
        "{r:?}"
    );
    // Different URIs, different local names, or an unprefixed twin are not duplicates.
    assert_eq!(
        parses(br#"<r xmlns:a="u" xmlns:b="v" a:x="1" b:x="2"/>"#),
        Ok(())
    );
    assert_eq!(
        parses(br#"<r xmlns:a="u" xmlns:b="u" a:x="1" b:y="2"/>"#),
        Ok(())
    );
    assert_eq!(parses(br#"<r xmlns:a="u" x="1" a:x="2"/>"#), Ok(()));
}

// FINDING F2 (medium): `bound()` scanned every live namespace binding linearly for each
// prefixed element name and each prefixed attribute, so with the old limits (up to 4032 live
// bindings) 40,000 tiny leaves took 50-100x the time per byte of the same message without the
// prefix, and 64 prefixed attributes per leaf about 100x. Fixed: `max_ns_bindings` (default
// 64) bounds the live bindings, the binding table has exactly that capacity, so a lookup is at
// most 64 comparisons. Here: the worst case the default admits (64 live bindings, the prefix
// declared outermost so it is found last) against the same message without the prefix.
// REQ: OA-XML-022
#[test]
fn f2_prefix_lookup_is_not_linear_in_bindings() {
    let mut p = Parser::new(Limits::default()).unwrap();
    assert_eq!(p.limits().max_ns_bindings, FULL);
    let hot = ns_churn_doc(40_000, "<l p0_0:x=\"\"/>");
    let control = ns_churn_doc(40_000, "<l x=\"\"/>");
    let hot_pb = ns_per_byte(&mut p, &hot);
    let ctl_pb = ns_per_byte(&mut p, &control);
    eprintln!(
        "F2 narrow: {hot_pb:.1} ns/byte vs control {ctl_pb:.1}: x{:.2}",
        hot_pb / ctl_pb
    );
    assert!(
        hot_pb < 10.0 * ctl_pb,
        "{hot_pb:.1} ns/byte vs control {ctl_pb:.1} ns/byte"
    );
    // 64 prefixed attributes per leaf: a lookup each, and the expanded-name duplicate check.
    let hot = ns_churn_doc(1_000, &wide_leaf(true));
    let control = ns_churn_doc(1_000, &wide_leaf(false));
    let hot_pb = ns_per_byte(&mut p, &hot);
    let ctl_pb = ns_per_byte(&mut p, &control);
    eprintln!(
        "F2 wide: {hot_pb:.1} ns/byte vs control {ctl_pb:.1}: x{:.2}",
        hot_pb / ctl_pb
    );
    assert!(
        hot_pb < 10.0 * ctl_pb,
        "{hot_pb:.1} ns/byte vs control {ctl_pb:.1} ns/byte"
    );
    // The limit itself: 64 in scope parse, the 65th is refused naming the limit and its offset.
    assert_eq!(parses(ns_churn_doc(1, "<l/>").as_bytes()), Ok(()));
    let over = ns_churn_doc(1, "<l xmlns:more=\"u\"/>");
    let at = over.find(" xmlns:more").unwrap() as u32 + 1;
    let r = parses(over.as_bytes());
    assert_eq!(
        r,
        Err(XmlError::Limit {
            limit: LimitKind::NsBindings,
            at
        })
    );
    assert!(r
        .unwrap_err()
        .to_string()
        .contains(limit_name(LimitKind::NsBindings)));
}

// FINDING F2b (low): the same cost on the consumer side. `resolve_attr_ns` / `resolve_elem_ns`
// walked the ancestor chain scanning every attribute of every ancestor, about 10 us per call
// at depth 64 with 64 declarations per level. Fixed: each element carries the head of a chain
// of the declarations in scope (Doc::elem_ns_head), so a call follows at most 64 links.
// REQ: OA-XML-022, OA-XML-013
#[test]
fn f2b_resolution_cost_is_bounded() {
    let s = ns_churn_doc(20_000, "<l p0_0:x=\"\"/>");
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p.parse(s.as_bytes()).unwrap();
    // Correct first: the outermost prefix resolves, from the deepest leaf.
    let leaf = ElemIx(d.elem_name.len() as u32 - 1);
    let a = AttrIx(d.elem_first_attr[leaf.0 as usize].0);
    assert!(
        matches!(resolve_attr_ns(&d, leaf, a), Ns::Uri(u) if span_bytes(d.input, u).unwrap() == b"u")
    );
    assert_eq!(resolve_elem_ns(&d, leaf), Ns::None);
    let t = Instant::now();
    let mut found = 0usize;
    for e in 0..d.elem_name.len() {
        let a = AttrIx(d.elem_first_attr[e].0);
        if matches!(resolve_attr_ns(&d, ElemIx(e as u32), a), Ns::Uri(_)) {
            found += 1;
        }
    }
    let per_call = t.elapsed().as_nanos() / d.elem_name.len() as u128;
    assert_eq!(found, 20_000);
    // 10 us per call before; at most 64 chain links now (the bound is for the slower debug
    // build too).
    let bound = if cfg!(debug_assertions) {
        25_000
    } else {
        1_000
    };
    eprintln!("F2b: {per_call} ns per lookup (bound {bound})");
    assert!(per_call < bound, "{per_call} ns per lookup");
}

// FINDING F3 (low): character references with many leading zeros are well-formed (the digit
// string is unbounded in the grammar) but `parse_entity` only looked 12 bytes ahead for the
// `;`, so `&#x0000000041;` (value 0x41, 10 digits) was rejected as malformed. Fixed: the body
// is scanned to its `;` bounded only by the input, accumulated with the value capped, so any
// value above 0x10FFFF is BadChar however it is written.
// Repro: b"<r>&#x0000000041;</r>"
// REQ: OA-XML-003
#[test]
fn f3_leading_zero_char_reference() {
    assert_eq!(parses(b"<r>&#x0000000041;</r>"), Ok(()));
    assert_eq!(parses(b"<r>&#0000000000065;</r>"), Ok(()));
    let zeros = "0".repeat(100_000);
    assert_eq!(parses(format!("<r>&#x{zeros}41;</r>").as_bytes()), Ok(()));
    let src = format!("<r>&#{zeros}65;</r>");
    let mut out = [0u8; 4];
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p.parse(src.as_bytes()).unwrap();
    let n = decode_into(d.text_span[0], d.input, &mut out, SpanKind::Text).unwrap();
    assert_eq!(&out[..n], b"A");
    // Too big however written: BadChar, including past u32 and with leading zeros.
    for big in [
        "&#x110000;",
        "&#1114112;",
        "&#xFFFFFFFFFFFFFFFFFF;",
        "&#00000000000000000000000000001114112;",
    ] {
        let r = parses(format!("<r>{big}</r>").as_bytes());
        assert!(
            matches!(r, Err(XmlError::BadChar { at: 3 })),
            "{big}: {r:?}"
        );
    }
    // Still BadEntity for a bad digit, even after an overflow, and for an empty body.
    for bad in ["&#xFFFFFFFFFFFFFFFFFFZ;", "&#;", "&#x;", "&#12a;"] {
        let r = parses(format!("<r>{bad}</r>").as_bytes());
        assert!(
            matches!(r, Err(XmlError::BadEntity { at: 3 })),
            "{bad}: {r:?}"
        );
    }
    // Still the existing rejections: NUL, surrogates, noncharacters.
    for bad in [
        "&#0;", "&#x0000;", "&#xD800;", "&#xFFFE;", "&#xFFFF;", "&#xB;",
    ] {
        let r = parses(format!("<r>{bad}</r>").as_bytes());
        assert!(
            matches!(r, Err(XmlError::BadChar { at: 3 })),
            "{bad}: {r:?}"
        );
    }
}

// FINDING F4 (medium): name characters were "any non-ASCII", checked only for UTF-8 validity,
// so U+FFFE and U+FFFF were accepted inside element, attribute and end-tag names. That broke
// OA-XML-008 (no character outside Char, raw), and also admitted non-name characters such as
// U+00D7, U+00B7 at the start, U+0085 and U+2028 (expat rejects them). Fixed: the XML 1.0
// fifth-edition NameStartChar / NameChar productions exactly, decoding code points, with the
// ASCII byte table kept as the fast path.
// Note on U+FEFF: the review expected it rejected, but U+FEFF lies in the production's
// [#xFDF0-#xFFFD] range, so the exact production accepts it inside a name; it is checked as
// accepted, and U+0085 (outside every range) takes its place in the rejected list.
// Repro: "<a\u{fffe}b/>"
// REQ: OA-XML-023, OA-XML-008
#[test]
fn f4_names_reject_noncharacters_and_non_name_chars() {
    assert!(
        parses("<a\u{fffe}b/>".as_bytes()).is_err(),
        "U+FFFE in element name"
    );
    assert!(
        parses("<a\u{ffff}/>".as_bytes()).is_err(),
        "U+FFFF in element name"
    );
    assert!(
        parses("<a x\u{fffe}=\"1\"/>".as_bytes()).is_err(),
        "U+FFFE in attribute name"
    );
    assert!(
        parses("<a\u{d7}b/>".as_bytes()).is_err(),
        "U+00D7 (multiplication sign)"
    );
    assert!(
        parses("<a\u{2028}b/>".as_bytes()).is_err(),
        "U+2028 line separator"
    );
    assert!(parses("<a\u{85}b/>".as_bytes()).is_err(), "U+0085");
    // Noncharacters are BadChar, in every kind of name and at the start.
    for src in [
        "<a\u{fffe}/>",
        "<\u{ffff}a/>",
        "<a \u{fffe}x=\"1\"/>",
        "<a><b/></b\u{ffff}></a>",
    ] {
        let r = parses(src.as_bytes());
        assert!(matches!(r, Err(XmlError::BadChar { .. })), "{src:?}: {r:?}");
    }
    // The boundaries of the productions, as start characters and as inner characters.
    let start_ok = [
        0xC0, 0xD6, 0xD8, 0xF6, 0xF8, 0x2FF, 0x370, 0x37D, 0x37F, 0x1FFF, 0x200C, 0x200D, 0x2070,
        0x218F, 0x2C00, 0x2FEF, 0x3001, 0xD7FF, 0xF900, 0xFDCF, 0xFDF0, 0xFFFD, 0x10000, 0xEFFFF,
    ];
    let start_bad = [
        0x80, 0xB7, 0xBF, 0xD7, 0xF7, 0x300, 0x36F, 0x37E, 0x2000, 0x200B, 0x200E, 0x203F, 0x2040,
        0x206F, 0x2190, 0x2BFF, 0x2FF0, 0x3000, 0xE000, 0xF8FF, 0xFDD0, 0xFDEF, 0xFFFE, 0xFFFF,
        0xF0000,
    ];
    for c in start_ok {
        let ch = char::from_u32(c).unwrap();
        assert_eq!(
            parses(format!("<{ch}/>").as_bytes()),
            Ok(()),
            "start U+{c:04X}"
        );
        assert_eq!(
            parses(format!("<a{ch}/>").as_bytes()),
            Ok(()),
            "inner U+{c:04X}"
        );
    }
    for c in start_bad {
        let Some(ch) = char::from_u32(c) else {
            continue;
        };
        assert!(
            parses(format!("<{ch}/>").as_bytes()).is_err(),
            "start U+{c:04X}"
        );
    }
    // Inner-only characters: U+00B7, U+0300-U+036F, U+203F-U+2040.
    for c in [0xB7, 0x300, 0x36F, 0x203F, 0x2040] {
        let ch = char::from_u32(c).unwrap();
        assert_eq!(
            parses(format!("<a{ch}/>").as_bytes()),
            Ok(()),
            "inner U+{c:04X}"
        );
        assert!(
            parses(format!("<{ch}a/>").as_bytes()).is_err(),
            "start U+{c:04X}"
        );
    }
    assert_eq!(
        parses("<a\u{feff}b/>".as_bytes()),
        Ok(()),
        "U+FEFF is in [#xFDF0-#xFFFD]"
    );
    // Real names work in prefixes, attributes and end tags; a multi-byte name round-trips.
    assert_eq!(
        parses("<\u{e9}:\u{20ac} xmlns:\u{e9}=\"u\" \u{3b1}=\"1\"></\u{e9}:\u{20ac}>".as_bytes()),
        Ok(())
    );
}

// FINDING F5 (low): Namespaces in XML 1.0 section 3 reserves the `xml` prefix (bindable only to
// the XML namespace, which in turn may be bound to no other prefix nor be the default) and the
// `http://www.w3.org/2000/xmlns/` namespace (bindable to nothing). All of these were accepted.
// Fixed in the declaration pass of `start_tag`.
// REQ: OA-XML-024
#[test]
fn f5_reserved_namespace_bindings() {
    let bad: [&[u8]; 7] = [
        br#"<r xmlns:xml="urn:x"/>"#,
        br#"<r xmlns="http://www.w3.org/XML/1998/namespace"/>"#,
        br#"<r xmlns:a="http://www.w3.org/2000/xmlns/"/>"#,
        br#"<r xmlns:a="http://www.w3.org/XML/1998/namespace"/>"#,
        br#"<r xmlns="http://www.w3.org/2000/xmlns/"/>"#,
        br#"<r xmlns:xml="http://www.w3.org/2000/xmlns/"/>"#,
        br#"<r><e xmlns:p="http://www.w3.org/XML/1998/namespace"/></r>"#,
    ];
    for b in bad {
        let r = parses(b);
        assert!(
            matches!(r, Err(XmlError::Syntax { .. })),
            "{}: {r:?}",
            String::from_utf8_lossy(b)
        );
    }
    // The one allowed use: `xml` bound to its own namespace (and it still resolves as `xml`).
    let ok = br#"<r xmlns:xml="http://www.w3.org/XML/1998/namespace" xml:lang="en"/>"#;
    assert_eq!(parses(ok), Ok(()));
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p.parse(ok).unwrap();
    assert_eq!(resolve_attr_ns(&d, ElemIx(0), AttrIx(1)), Ns::Xml);
    // Near misses are ordinary URIs.
    assert_eq!(
        parses(br#"<r xmlns:a="http://www.w3.org/XML/1998/namespace/"/>"#),
        Ok(())
    );
}

// FINDING F6 (low): input that ends inside an end tag or an attribute was reported as a generic
// `Syntax` error instead of `UnexpectedEof` (OA-XML-009 names "input that ends inside markup";
// `<a` and `<a><!-- x` reported UnexpectedEof, so the family was inconsistent). Fixed
// everywhere the parser can run out of input inside markup: tags, attributes, end tags,
// comments, CDATA, references, the XML declaration, the openings of `<!--`, `<![CDATA[` and
// `<?xml`. Input that stops between complete markup with elements open is `Unclosed`, and
// before any root `NoRoot`, both at the end offset. The sweep in xml.rs asserts this for every
// truncation of every corpus message.
// REQ: OA-XML-009, OA-XML-017
#[test]
fn f6_truncation_is_unexpected_eof() {
    let cases: [&[u8]; 27] = [
        b"<a></",
        b"<a></a",
        b"<a b",
        b"<a b=",
        b"<a b=\"",
        b"<a b=\"x\"/",
        b"<a b=\"&am",
        b"<a><",
        b"<a><!",
        b"<a><!-",
        b"<a><!--x--",
        b"<a><![CDATA",
        b"<a>&",
        b"<a>&am",
        b"<a:",
        b"<a x:",
        b"<a/><!",
        b"<a/><!-",
        b"<?xml version=\"1.0\"?",
        b"<?xml version=",
        b"<?xml vers",
        b"<?xml",
        b"<?x",
        b"<?xml version=\"1.0\" encodi",
        b"<ab></a",
        b"<?xml version=\"1.0\" standalone=\"y",
        b"<!-",
    ];
    for c in cases {
        let r = parses(c);
        assert_eq!(
            r,
            Err(XmlError::UnexpectedEof { at: c.len() as u32 }),
            "{}",
            String::from_utf8_lossy(c)
        );
    }
    // Not markup: the elements are simply still open, or there is no root yet.
    assert_eq!(parses(b"<a>"), Err(XmlError::Unclosed { at: 3 }));
    assert_eq!(parses(b"<a>text"), Err(XmlError::Unclosed { at: 7 }));
    assert_eq!(parses(b"<!-- c -->"), Err(XmlError::NoRoot { at: 10 }));
    // A full-length mismatch is still a mismatch, not a cut.
    assert!(matches!(
        parses(b"<ab></ac"),
        Err(XmlError::MismatchedTag { .. })
    ));
}

// FINDING F7 (low, documented deviation from XML 1.0 sections 2.11 and 3.3.3): line ends and
// attribute whitespace were not normalised, so a value decoded with `decode_into` kept `\r\n`
// (should be `\n`) and a tab or newline in an attribute value (should become a space). A
// consumer comparing multi-line values with `span_eq_str` saw different strings from every
// conforming parser. Fixed on decode, not in the spans: `decode_into` and `span_eq_str` take a
// `SpanKind`; the entity flags also mark values that normalisation would change.
// REQ: OA-XML-025
#[test]
fn f7_line_end_normalisation() {
    let src = b"x\r\ny\rz";
    let mut out = [0u8; 16];
    let span = Span {
        start: 0,
        len: src.len() as u32,
    };
    let n = decode_into(span, src, &mut out, SpanKind::Text).unwrap();
    assert_eq!(&out[..n], b"x\ny\nz");
    assert_eq!(span_eq_str(span, src, "x\ny\nz", SpanKind::Text), Ok(true));
    // Attribute values: each of tab, LF, CR and CRLF is one space; references are not
    // normalised, so `&#10;`, `&#13;` and `&#9;` stay as written.
    let src = b"a\tb\nc\rd\r\ne&#10;f&#13;g&#9;h";
    let span = Span {
        start: 0,
        len: src.len() as u32,
    };
    let n = decode_into(span, src, &mut out, SpanKind::AttrValue).unwrap();
    assert_eq!(&out[..n], b"a b c d e\nf\rg\th");
    assert_eq!(
        span_eq_str(span, src, "a b c d e\nf\rg\th", SpanKind::AttrValue),
        Ok(true)
    );
    // In text the same bytes keep tab and LF, and `\r\n` is one `\n`.
    let n = decode_into(span, src, &mut out, SpanKind::Text).unwrap();
    assert_eq!(&out[..n], b"a\tb\nc\nd\ne\nf\rg\th");
    // The parser flags what needs the decode: a lone tab in text does not, in an attribute it does.
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p
        .parse(b"<r a=\"1\t2\" b=\"plain\" c=\"\r\">t\tu<e>v\r\nw</e><e>plain</e></r>")
        .unwrap();
    let flag = |i: usize| oa_xml::attr_needs_decode(&d, AttrIx(i as u32));
    assert!(flag(0) && !flag(1) && flag(2));
    let tflag = |i: usize| oa_xml::text_needs_decode(&d, oa_xml::TextIx(i as u32));
    assert!(!tflag(0) && tflag(1) && !tflag(2));
    // CRLF line ends between elements are layout, not text nodes.
    let d = p.parse(b"<r>\r\n<e/>\r\n</r>").unwrap();
    assert_eq!(d.text_span.len(), 0);
    // The fast path allocates nothing: a literal value costs one scan and no heap use, since
    // the function has no allocating call (checked by the signature: slices in, slice out).
    let n = decode_into(
        Span { start: 0, len: 3 },
        b"abc",
        &mut out,
        SpanKind::AttrValue,
    )
    .unwrap();
    assert_eq!(&out[..n], b"abc");
}

// FINDING F8 (low): `decode_into` wrote the bytes decoded before an error into `out` and then
// returned Err (both for `OutputTooSmall` and for a bad reference), leaving a partial result in
// the caller's buffer. Fixed: all or nothing, by validating and measuring in a first pass and
// writing in a second (restoring would need a saved copy of the buffer, so it would allocate).
// Repro: src = b"abc&amp;def&bad;"
// REQ: OA-XML-014
#[test]
fn f8_decode_into_no_partial_write_on_error() {
    let src = b"abc&amp;def&bad;";
    let mut out = [b'#'; 32];
    let r = decode_into(
        Span {
            start: 0,
            len: src.len() as u32,
        },
        src,
        &mut out,
        SpanKind::Text,
    );
    assert_eq!(r, Err(XmlError::UndefinedEntity { at: 11 }));
    assert_eq!(out, [b'#'; 32], "buffer modified despite error");

    let src2 = b"abc&amp;d";
    let mut small = [b'#'; 4];
    let r = decode_into(
        Span {
            start: 0,
            len: src2.len() as u32,
        },
        src2,
        &mut small,
        SpanKind::Text,
    );
    assert_eq!(r, Err(XmlError::OutputTooSmall { at: 8 }));
    assert_eq!(small, [b'#'; 4], "buffer modified despite OutputTooSmall");

    // Success still fills exactly the decoded length and leaves the rest alone.
    let mut big = [b'#'; 12];
    let n = decode_into(
        Span {
            start: 0,
            len: src2.len() as u32,
        },
        src2,
        &mut big,
        SpanKind::Text,
    )
    .unwrap();
    assert_eq!((&big[..n], big[n]), (&b"abc&d"[..], b'#'));
}

// FINDING F9 (low): `span_eq_str` returned Ok(false) at the first mismatch without validating
// the rest of the span, while `decode_into` of the same span returned Err for the bad
// reference. Fixed: `span_eq_str` does not stop at a mismatch; it walks the whole span, so it
// returns the same error as `decode_into` would (it has no output buffer, so never
// `OutputTooSmall`).
// Repro: span over b"xyz&bad;" compared with "q".
// REQ: OA-XML-015
#[test]
fn f9_span_eq_and_decode_agree_on_bad_reference() {
    let src = b"xyz&bad;";
    let span = Span {
        start: 0,
        len: src.len() as u32,
    };
    let d = decode_into(span, src, &mut [0u8; 16], SpanKind::Text);
    let e = span_eq_str(span, src, "q", SpanKind::Text);
    assert_eq!(d.map(|_| ()), e.map(|_| ()));
    assert_eq!(e, Err(XmlError::UndefinedEntity { at: 3 }));
    for bad in [&b"a&#xZ;"[..], b"a&#x110000;", b"a&am", b"ok&amp;&lt"] {
        let span = Span {
            start: 0,
            len: bad.len() as u32,
        };
        let d = decode_into(span, bad, &mut [0u8; 16], SpanKind::Text);
        let e = span_eq_str(span, bad, "zzz", SpanKind::Text);
        assert!(d.is_err());
        assert_eq!(
            d.map(|_| ()),
            e.map(|_| ()),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

// FINDING F10 (low): a span that does not lie inside the input was treated as the empty string
// by `decode_into` (Ok(0)), `span_eq_str` (equal to "") and `span_bytes`, instead of being
// refused (RULESET E5). A span from another message, or a stale one, silently compared equal
// to "". Fixed: `SpanOutOfRange` from every public function that takes a span.
// REQ: OA-XML-026
#[test]
fn f10_out_of_range_span_is_refused() {
    let src = b"abc";
    for span in [
        Span { start: 5, len: 100 },
        Span { start: 0, len: 4 },
        Span { start: 3, len: 1 },
        Span {
            start: u32::MAX,
            len: u32::MAX,
        },
        Span {
            start: 2,
            len: u32::MAX,
        },
    ] {
        let want = Err(XmlError::SpanOutOfRange { at: span.start });
        assert_eq!(decode_into(span, src, &mut [0u8; 16], SpanKind::Text), want);
        assert_eq!(
            decode_into(span, src, &mut [0u8; 16], SpanKind::AttrValue),
            want
        );
        assert_eq!(
            span_eq_str(span, src, "", SpanKind::Text).map(|_| ()),
            want.map(|_: usize| ())
        );
        assert_eq!(
            span_bytes(src, span),
            Err(XmlError::SpanOutOfRange { at: span.start })
        );
    }
    // Spans at the edge are fine, including the empty span at the end.
    for (start, len, want) in [(0, 3, "abc"), (3, 0, ""), (1, 2, "bc"), (0, 0, "")] {
        let span = Span { start, len };
        assert_eq!(span_bytes(src, span), Ok(want.as_bytes()));
        assert_eq!(span_eq_str(span, src, want, SpanKind::Text), Ok(true));
    }
    assert!(XmlError::SpanOutOfRange { at: 5 }
        .to_string()
        .contains("outside the input"));
}

// FINDING F11 (low): `Parser::new` panicked with "capacity overflow" for extreme limits
// (`max_depth * max_attrs_per_elem` saturated to usize::MAX; `max_elements` = usize::MAX),
// rather than returning an error (RULESET E3). Configuration only, never input. Fixed:
// `Parser::new` returns Result; every limit has a hard ceiling (`Limits::CEILING`) and the
// zero rules, both `LimitsInvalid`; tables are reserved with `try_reserve_exact`.
// REQ: OA-XML-027
#[test]
fn f11_parser_new_does_not_panic_on_extreme_limits() {
    let r = std::panic::catch_unwind(|| {
        Parser::new(Limits {
            max_depth: usize::MAX,
            max_attrs_per_elem: 2,
            ..Limits::default()
        })
        .map(|_| ())
    });
    assert_eq!(
        r.unwrap(),
        Err(XmlError::LimitsInvalid {
            limit: LimitKind::Depth,
            at: 0
        })
    );
    let d = Limits::default();
    let c = Limits::CEILING;
    // Every field: the ceiling is accepted (checked without allocating), one over is refused,
    // and a zero is refused where it cannot work.
    assert_eq!(validate_limits(&c), Ok(()));
    assert_eq!(validate_limits(&d), Ok(()));
    let cases: [(LimitKind, Limits, Limits, bool); 8] = [
        (
            LimitKind::InputBytes,
            Limits {
                max_input_bytes: u32::MAX as usize + 1,
                ..c
            },
            Limits {
                max_input_bytes: 0,
                ..d
            },
            true,
        ),
        (
            LimitKind::Depth,
            Limits {
                max_depth: c.max_depth + 1,
                ..c
            },
            Limits { max_depth: 0, ..d },
            true,
        ),
        (
            LimitKind::Elements,
            Limits {
                max_elements: c.max_elements + 1,
                ..c
            },
            Limits {
                max_elements: 0,
                ..d
            },
            true,
        ),
        (
            LimitKind::Attributes,
            Limits {
                max_attributes: c.max_attributes + 1,
                ..c
            },
            Limits {
                max_attributes: 0,
                ..d
            },
            false,
        ),
        (
            LimitKind::AttributesPerElement,
            Limits {
                max_attrs_per_elem: c.max_attrs_per_elem + 1,
                ..c
            },
            Limits {
                max_attrs_per_elem: 0,
                ..d
            },
            false,
        ),
        (
            LimitKind::NameLength,
            Limits {
                max_name_len: c.max_name_len + 1,
                ..c
            },
            Limits {
                max_name_len: 0,
                ..d
            },
            true,
        ),
        (
            LimitKind::TextNodes,
            Limits {
                max_text_nodes: c.max_text_nodes + 1,
                ..c
            },
            Limits {
                max_text_nodes: 0,
                ..d
            },
            false,
        ),
        (
            LimitKind::NsBindings,
            Limits {
                max_ns_bindings: c.max_ns_bindings + 1,
                ..c
            },
            Limits {
                max_ns_bindings: 0,
                ..d
            },
            false,
        ),
    ];
    for (kind, over, zero, zero_invalid) in cases {
        let want = Err(XmlError::LimitsInvalid { limit: kind, at: 0 });
        assert_eq!(validate_limits(&over), want, "{kind:?} over its ceiling");
        assert_eq!(
            Parser::new(over).map(|_| ()),
            want,
            "{kind:?} over its ceiling"
        );
        if zero_invalid {
            assert_eq!(validate_limits(&zero), want, "{kind:?} zero");
        } else {
            // Zero attributes, attributes per element, text nodes or bindings is a real
            // configuration (a feed that allows none), and it works.
            assert!(Parser::new(zero).is_ok(), "{kind:?} zero");
        }
    }
    assert!(Parser::new(Limits {
        max_elements: usize::MAX,
        ..d
    })
    .is_err());
    let e = Parser::new(Limits {
        max_name_len: 0,
        ..d
    })
    .map(|_| ())
    .unwrap_err();
    assert!(e.to_string().contains("max name length"), "{e}");
    // Zero attributes still parses a document that has none, and refuses one that has some.
    let mut p = Parser::new(Limits {
        max_attributes: 0,
        max_attrs_per_elem: 0,
        ..d
    })
    .unwrap();
    assert!(p.parse(b"<a><b/></a>").is_ok());
    assert!(matches!(
        p.parse(b"<a b=\"1\"/>"),
        Err(XmlError::Limit { .. })
    ));
}
