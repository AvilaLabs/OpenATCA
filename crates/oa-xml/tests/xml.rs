//! Tests for oa-xml. Fixtures are handwritten, modelled on the structure of SWIM messages only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_xml::{
    attr_needs_decode, decode_into, error_offset, resolve_attr_ns, resolve_elem_ns, span_bytes,
    span_eq_str, text_needs_decode, AttrIx, Doc, ElemIx, LimitKind, Limits, Ns, Parser, SpanKind,
    TextIx, XmlError,
};

/// The bytes of a span of a parsed document (always in range).
fn sb(input: &[u8], s: oa_xml::Span) -> &[u8] {
    span_bytes(input, s).unwrap()
}

fn parse_ok(src: &str) -> (Parser, Vec<u8>) {
    let mut p = Parser::new(Limits::default()).unwrap();
    p.parse(src.as_bytes()).unwrap();
    (p, src.as_bytes().to_vec())
}

fn err(src: &[u8]) -> XmlError {
    let mut p = Parser::new(Limits::default()).unwrap();
    p.parse(src).unwrap_err()
}

fn name<'a>(d: &Doc<'a>, e: usize) -> &'a [u8] {
    sb(d.input, d.elem_name[e])
}

const FLIGHT: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<fx:Message xmlns:fx=\"urn:example:flight\" xmlns=\"urn:example:default\" id='m1'>\n  \
<!-- a comment -->\n  \
<fx:Flight cs=\"ABC123\" note=\"a &amp; b &#65;&#x42;\">\n    \
<Origin>KORD</Origin>\n    \
<Dest><![CDATA[<K&D>]]></Dest>\n    \
<Empty/>\n  \
</fx:Flight>\n\
</fx:Message>\n";

// REQ: OA-XML-001
#[test]
fn xml_declaration_accepted_forms() {
    for src in [
        "<?xml version=\"1.0\"?><a/>",
        "<?xml version='1.0' encoding='UTF-8'?><a/>",
        "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?><a/>",
        "<?xml  version = \"1.0\"  ?>\n<a/>",
        "<a/>",
        "\u{feff}<?xml version=\"1.0\"?><a/>",
    ] {
        let mut p = Parser::new(Limits::default()).unwrap();
        assert!(p.parse(src.as_bytes()).is_ok(), "{src}");
    }
    assert!(matches!(
        err(b"<?xml version=\"1.1\"?><a/>"),
        XmlError::UnsupportedVersion { .. }
    ));
    assert!(matches!(
        err(b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>"),
        XmlError::UnsupportedEncoding { .. }
    ));
    assert!(matches!(
        err(b"<?xml encoding=\"UTF-8\"?><a/>"),
        XmlError::BadXmlDecl { .. }
    ));
    assert!(matches!(
        err(b"<?xml version=\"1.0\"<a/>"),
        XmlError::BadXmlDecl { .. }
    ));
    assert!(matches!(
        err(b"<?xml version=\"1.0\" standalone=\"maybe\"?><a/>"),
        XmlError::BadXmlDecl { .. }
    ));
}

// REQ: OA-XML-002
#[test]
fn accepts_elements_attributes_comments_cdata_whitespace() {
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p.parse(FLIGHT.as_bytes()).unwrap();
    assert_eq!(d.elem_name.len(), 5);
    assert_eq!(name(&d, 0), b"Message");
    assert_eq!(name(&d, 1), b"Flight");
    assert_eq!(name(&d, 2), b"Origin");
    assert_eq!(name(&d, 4), b"Empty");
    assert_eq!(sb(d.input, d.elem_prefix[1]), b"fx");
    // Root: two namespace declarations plus id, in order, quote styles preserved.
    assert_eq!(d.elem_attr_count[0], 3);
    let a = d.elem_first_attr[0].0 as usize;
    assert_eq!(sb(d.input, d.attr_name[a + 2]), b"id");
    assert_eq!(sb(d.input, d.attr_value[a + 2]), b"m1");
    // Text nodes: KORD and the CDATA; layout whitespace and comments are not stored.
    assert_eq!(d.text_span.len(), 2);
    assert_eq!(sb(d.input, d.text_span[0]), b"KORD");
    assert_eq!(sb(d.input, d.text_span[1]), b"<K&D>");
    assert!(!text_needs_decode(&d, TextIx(1)), "CDATA is literal");
    assert_eq!(d.text_owner[0], ElemIx(2));
    assert_eq!(d.text_owner[1], ElemIx(3));
}

// REQ: OA-XML-021
#[test]
fn tree_links_are_consistent() {
    let mut p = Parser::new(Limits::default()).unwrap();
    let d = p.parse(FLIGHT.as_bytes()).unwrap();
    assert_eq!(d.elem_parent[0], ElemIx::NONE);
    assert_eq!(d.elem_parent[1], ElemIx(0));
    assert_eq!(d.elem_first_child[0], ElemIx(1));
    assert_eq!(d.elem_first_child[1], ElemIx(2));
    assert_eq!(d.elem_next_sibling[2], ElemIx(3));
    assert_eq!(d.elem_next_sibling[3], ElemIx(4));
    assert_eq!(d.elem_next_sibling[4], ElemIx::NONE);
    assert_eq!(d.elem_next_sibling[1], ElemIx::NONE);
    assert_eq!(d.elem_first_child[4], ElemIx::NONE);
    // Walking first-child/next-sibling from the root visits every element once.
    let mut seen = vec![false; d.elem_name.len()];
    let mut stack = vec![ElemIx(0)];
    while let Some(e) = stack.pop() {
        assert!(!seen[e.0 as usize]);
        seen[e.0 as usize] = true;
        let mut c = d.elem_first_child[e.0 as usize];
        while c != ElemIx::NONE {
            assert_eq!(d.elem_parent[c.0 as usize], e);
            stack.push(c);
            c = d.elem_next_sibling[c.0 as usize];
        }
    }
    assert!(seen.iter().all(|&s| s));
}

// REQ: OA-XML-003
#[test]
fn character_references_and_predefined_entities() {
    let src = "<a v=\"&lt;&gt;&amp;&quot;&apos;&#65;&#x1F600;\">&lt;&#x42;</a>";
    let (mut p, bytes) = parse_ok(src);
    let d = p.parse(&bytes).unwrap();
    assert!(attr_needs_decode(&d, AttrIx(0)));
    assert!(text_needs_decode(&d, TextIx(0)));
    let mut out = [0u8; 64];
    let n = decode_into(d.attr_value[0], d.input, &mut out, SpanKind::AttrValue).unwrap();
    assert_eq!(&out[..n], "<>&\"'A\u{1F600}".as_bytes());
    let n = decode_into(d.text_span[0], d.input, &mut out, SpanKind::Text).unwrap();
    assert_eq!(&out[..n], b"<B");
    // A plain value carries no flag.
    let (mut p, bytes) = parse_ok("<a v=\"plain\">text</a>");
    let d = p.parse(&bytes).unwrap();
    assert!(!attr_needs_decode(&d, AttrIx(0)));
    assert!(!text_needs_decode(&d, TextIx(0)));
}

// REQ: OA-XML-004
#[test]
fn doctype_is_rejected_billion_laughs_and_xxe() {
    let laughs = b"<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [\n\
<!ENTITY lol \"lol\">\n<!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\n\
<!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\n]>\n<lolz>&lol3;</lolz>";
    assert!(matches!(err(laughs), XmlError::Doctype { at: 22 }));
    let xxe = b"<?xml version=\"1.0\"?>\n<!DOCTYPE foo [ <!ENTITY xxe SYSTEM \"file:///etc/passwd\"> ]>\n<foo>&xxe;</foo>";
    assert!(matches!(err(xxe), XmlError::Doctype { .. }));
    assert!(matches!(
        err(b"<!doctype a><a/>"),
        XmlError::Doctype { at: 0 }
    ));
    assert!(matches!(
        err(b"<a/><!DOCTYPE a>"),
        XmlError::Doctype { at: 4 }
    ));
    assert!(matches!(
        err(b"<a><!DOCTYPE a></a>"),
        XmlError::Doctype { at: 3 }
    ));
}

// REQ: OA-XML-005
#[test]
fn processing_instructions_are_rejected() {
    assert!(matches!(
        err(b"<?xml-stylesheet href=\"a\"?><a/>"),
        XmlError::ProcessingInstruction { at: 0 }
    ));
    assert!(matches!(
        err(b"<a><?pi data?></a>"),
        XmlError::ProcessingInstruction { at: 3 }
    ));
    assert!(matches!(
        err(b"<a/><?pi?>"),
        XmlError::ProcessingInstruction { at: 4 }
    ));
    // A second XML declaration is a processing instruction too.
    assert!(matches!(
        err(b"<?xml version=\"1.0\"?><?xml version=\"1.0\"?><a/>"),
        XmlError::ProcessingInstruction { .. }
    ));
}

// REQ: OA-XML-006
#[test]
fn undefined_and_malformed_entities_are_rejected() {
    assert!(matches!(
        err(b"<a>&nbsp;</a>"),
        XmlError::UndefinedEntity { at: 3 }
    ));
    assert!(matches!(
        err(b"<a v=\"&foo;\"/>"),
        XmlError::UndefinedEntity { at: 6 }
    ));
    assert!(matches!(err(b"<a>&amp</a>"), XmlError::BadEntity { .. }));
    assert!(matches!(err(b"<a>& </a>"), XmlError::BadEntity { .. }));
    assert!(matches!(err(b"<a>&#;</a>"), XmlError::BadEntity { .. }));
    assert!(matches!(err(b"<a>&#xZZ;</a>"), XmlError::BadEntity { .. }));
    assert!(matches!(err(b"<a>&#0;</a>"), XmlError::BadChar { .. }));
    assert!(matches!(err(b"<a>&#xD800;</a>"), XmlError::BadChar { .. }));
    assert!(matches!(
        err(b"<a>&#x110000;</a>"),
        XmlError::BadChar { .. }
    ));
    assert!(matches!(
        err(b"<a>&#99999999999;</a>"),
        XmlError::BadEntity { .. } | XmlError::BadChar { .. }
    ));
}

// REQ: OA-XML-007
#[test]
fn invalid_utf8_is_rejected_with_offset() {
    assert!(matches!(err(b"<a>\xFF</a>"), XmlError::Utf8 { at: 3 }));
    assert!(matches!(err(b"<a>\xC3</a>"), XmlError::Utf8 { at: 3 }));
    assert!(matches!(err(b"<a>\xC0\xAF</a>"), XmlError::Utf8 { at: 3 }));
    assert!(matches!(
        err(b"<a>\xED\xA0\x80</a>"),
        XmlError::Utf8 { at: 3 }
    ));
    assert!(matches!(err(b"<a>\xE2\x82"), XmlError::Utf8 { at: 3 }));
    // Valid multi-byte text is fine, in text, attributes and names.
    let (mut p, bytes) = parse_ok("<a b=\"\u{e9}\u{20ac}\">\u{1F600}</a>");
    assert!(p.parse(&bytes).is_ok());
}

// REQ: OA-XML-008
#[test]
fn characters_outside_char_production_are_rejected() {
    assert!(matches!(err(b"<a>\x01</a>"), XmlError::BadChar { at: 3 }));
    assert!(matches!(err(b"<a>\x00</a>"), XmlError::BadChar { at: 3 }));
    assert!(matches!(err(b"<a b=\"\x08\"/>"), XmlError::BadChar { .. }));
    assert!(matches!(
        err(b"<a><!-- \x0B --></a>"),
        XmlError::BadChar { .. }
    ));
    assert!(matches!(
        err(b"<a><![CDATA[\x1F]]></a>"),
        XmlError::BadChar { .. }
    ));
    assert!(matches!(
        err("<a>\u{fffe}</a>".as_bytes()),
        XmlError::BadChar { at: 3 }
    ));
    assert!(matches!(
        err("<a>\u{ffff}</a>".as_bytes()),
        XmlError::BadChar { at: 3 }
    ));
    assert!(matches!(err(b"<a>]]></a>"), XmlError::Syntax { .. }));
    // Tab, LF, CR and U+FFFD are legal.
    let (mut p, bytes) = parse_ok("<a>\t\r\n\u{fffd}x</a>");
    assert!(p.parse(&bytes).is_ok());
}

// REQ: OA-XML-009
#[test]
fn mismatched_and_unclosed_tags_are_rejected() {
    assert!(matches!(err(b"<a></b>"), XmlError::MismatchedTag { at: 3 }));
    assert!(matches!(
        err(b"<a><b></a></b>"),
        XmlError::MismatchedTag { at: 6 }
    ));
    assert!(matches!(
        err(b"<a:b xmlns:a=\"u\"></a:c>"),
        XmlError::MismatchedTag { .. }
    ));
    assert!(matches!(err(b"<a>"), XmlError::Unclosed { at: 3 }));
    assert!(matches!(err(b"<a><b></b>"), XmlError::Unclosed { at: 10 }));
    assert!(matches!(err(b"<a"), XmlError::UnexpectedEof { .. }));
    assert!(matches!(err(b"<a b=\"x"), XmlError::UnexpectedEof { .. }));
    assert!(matches!(err(b"<a></a"), XmlError::UnexpectedEof { at: 6 }));
    assert!(matches!(err(b"</a>"), XmlError::OutsideRoot { at: 0 }));
    assert!(matches!(err(b""), XmlError::NoRoot { at: 0 }));
    assert!(matches!(err(b"  \n"), XmlError::NoRoot { .. }));
    assert!(matches!(err(b"<a b=x/>"), XmlError::Syntax { .. }));
    assert!(matches!(err(b"<a b/>"), XmlError::Syntax { .. }));
    assert!(matches!(err(b"<a b=\"<\"/>"), XmlError::Syntax { .. }));
    assert!(matches!(
        err(b"<a b=\"1\"c=\"2\"/>"),
        XmlError::Syntax { .. }
    ));
    assert!(matches!(
        err(b"<a><!-- x -- y --></a>"),
        XmlError::Syntax { .. }
    ));
    assert!(matches!(
        err(b"<a><!-- x </a>"),
        XmlError::UnexpectedEof { .. }
    ));
    assert!(matches!(
        err(b"<a><![CDATA[ x </a>"),
        XmlError::UnexpectedEof { .. }
    ));
    assert!(matches!(err(b"<1a/>"), XmlError::Syntax { .. }));
    assert!(matches!(err(b"<a:b:c/>"), XmlError::Syntax { .. }));
}

// REQ: OA-XML-010
#[test]
fn duplicate_attributes_are_rejected() {
    assert!(matches!(
        err(b"<a x=\"1\" x=\"2\"/>"),
        XmlError::DuplicateAttribute { at: 9 }
    ));
    assert!(matches!(
        err(b"<a x=\"1\" y=\"2\" x='3'/>"),
        XmlError::DuplicateAttribute { .. }
    ));
    assert!(matches!(
        err(b"<a xmlns:p=\"u\" p:x=\"1\" p:x=\"2\"/>"),
        XmlError::DuplicateAttribute { .. }
    ));
    // Same local name, different prefix or none: not duplicates.
    let (mut p, bytes) = parse_ok("<a xmlns:p=\"u\" x=\"1\" p:x=\"2\"/>");
    assert!(p.parse(&bytes).is_ok());
}

// REQ: OA-XML-011
#[test]
fn content_outside_root_is_rejected() {
    assert!(matches!(err(b"<a/><b/>"), XmlError::OutsideRoot { at: 4 }));
    assert!(matches!(err(b"<a/>text"), XmlError::OutsideRoot { at: 4 }));
    assert!(matches!(err(b"text<a/>"), XmlError::OutsideRoot { at: 0 }));
    assert!(matches!(err(b"<a></a>x"), XmlError::OutsideRoot { .. }));
    assert!(matches!(
        err(b"<![CDATA[x]]><a/>"),
        XmlError::OutsideRoot { .. }
    ));
    // Whitespace and comments around the root are fine.
    let (mut p, bytes) = parse_ok("<!-- pre -->\n<a/>\n<!-- post -->\n");
    assert!(p.parse(&bytes).is_ok());
}

// REQ: OA-XML-012
#[test]
fn undeclared_prefix_is_rejected() {
    assert!(matches!(
        err(b"<p:a/>"),
        XmlError::UndeclaredPrefix { at: 1 }
    ));
    assert!(matches!(
        err(b"<a p:x=\"1\"/>"),
        XmlError::UndeclaredPrefix { at: 3 }
    ));
    // A declaration on a sibling or child does not leak outward.
    assert!(matches!(
        err(b"<a><b xmlns:p=\"u\"/><p:c/></a>"),
        XmlError::UndeclaredPrefix { .. }
    ));
    assert!(matches!(
        err(b"<a><b xmlns:p=\"u\"><p:c/></b><p:d/></a>"),
        XmlError::UndeclaredPrefix { .. }
    ));
    // Declared on the same element, an ancestor; `xml` is predefined.
    for src in [
        "<p:a xmlns:p=\"u\"/>",
        "<a xmlns:p=\"u\"><b><p:c p:x=\"1\"/></b></a>",
        "<a xml:lang=\"en\"/>",
        "<a p:x=\"1\" xmlns:p=\"u\"/>",
    ] {
        let mut p = Parser::new(Limits::default()).unwrap();
        assert!(p.parse(src.as_bytes()).is_ok(), "{src}");
    }
    assert!(matches!(err(b"<a xmlns:p=\"\"/>"), XmlError::Syntax { .. }));
    assert!(matches!(
        err(b"<a xmlns:xmlns=\"u\"/>"),
        XmlError::Syntax { .. }
    ));
}

// REQ: OA-XML-013
#[test]
fn namespace_resolution_follows_scope() {
    let src = "<r xmlns=\"urn:d\" xmlns:p=\"urn:p\">\
<p:a p:k=\"1\" k=\"2\" xml:lang=\"en\"><b xmlns=\"\"><c xmlns=\"urn:c\"/></b></p:a></r>";
    let (mut parser, bytes) = parse_ok(src);
    let d = parser.parse(&bytes).unwrap();
    let uri = |ns: Ns| match ns {
        Ns::Uri(s) => sb(d.input, s).to_vec(),
        Ns::None => b"<none>".to_vec(),
        Ns::Xml => b"<xml>".to_vec(),
    };
    assert_eq!(uri(resolve_elem_ns(&d, ElemIx(0))), b"urn:d");
    assert_eq!(uri(resolve_elem_ns(&d, ElemIx(1))), b"urn:p"); // p:a
    assert_eq!(uri(resolve_elem_ns(&d, ElemIx(2))), b"<none>"); // b: xmlns="" undeclares
    assert_eq!(uri(resolve_elem_ns(&d, ElemIx(3))), b"urn:c");
    let first = d.elem_first_attr[1].0;
    assert_eq!(uri(resolve_attr_ns(&d, ElemIx(1), AttrIx(first))), b"urn:p"); // p:k
    assert_eq!(
        uri(resolve_attr_ns(&d, ElemIx(1), AttrIx(first + 1))),
        b"<none>"
    ); // k
    assert_eq!(
        uri(resolve_attr_ns(&d, ElemIx(1), AttrIx(first + 2))),
        b"<xml>"
    );
    // Out-of-range indexes resolve to nothing rather than panicking.
    assert_eq!(resolve_elem_ns(&d, ElemIx(999)), Ns::None);
    assert_eq!(resolve_attr_ns(&d, ElemIx(999), AttrIx(999)), Ns::None);
}

// REQ: OA-XML-014
#[test]
fn decode_into_fills_caller_buffer() {
    let (mut p, bytes) = parse_ok("<a v=\"x&amp;y&#xe9;z\"/>");
    let d = p.parse(&bytes).unwrap();
    let mut out = [0u8; 16];
    let n = decode_into(d.attr_value[0], d.input, &mut out, SpanKind::AttrValue).unwrap();
    assert_eq!(&out[..n], "x&y\u{e9}z".as_bytes());
    // Exact fit works; one byte short fails and names an offset inside the span.
    let mut exact = [0u8; 6];
    assert_eq!(
        decode_into(d.attr_value[0], d.input, &mut exact, SpanKind::AttrValue).unwrap(),
        6
    );
    let mut small = [0u8; 5];
    match decode_into(d.attr_value[0], d.input, &mut small, SpanKind::AttrValue) {
        Err(XmlError::OutputTooSmall { at }) => assert!(at >= d.attr_value[0].start),
        other => panic!("{other:?}"),
    }
    let mut none = [0u8; 0];
    assert_eq!(
        decode_into(oa_xml_empty(), &bytes, &mut none, SpanKind::AttrValue).unwrap(),
        0
    );
    // A span holding a bad reference reports the input-relative offset.
    let bad = b"xx&foo;";
    let span = oa_xml_span(0, 7);
    assert!(matches!(
        decode_into(span, bad, &mut out, SpanKind::AttrValue),
        Err(XmlError::UndefinedEntity { at: 2 })
    ));
}

fn oa_xml_empty() -> oa_xml::Span {
    oa_xml_span(0, 0)
}
fn oa_xml_span(start: u32, len: u32) -> oa_xml::Span {
    oa_xml::Span { start, len }
}

// REQ: OA-XML-015
#[test]
fn span_eq_str_decodes_on_the_fly() {
    let (mut p, bytes) = parse_ok("<a v=\"A&amp;B&#x42;&lt;\"/>");
    let d = p.parse(&bytes).unwrap();
    let s = d.attr_value[0];
    assert_eq!(
        span_eq_str(s, d.input, "A&BB<", SpanKind::AttrValue),
        Ok(true)
    );
    assert_eq!(
        span_eq_str(s, d.input, "A&BB", SpanKind::AttrValue),
        Ok(false)
    );
    assert_eq!(
        span_eq_str(s, d.input, "A&BB<x", SpanKind::AttrValue),
        Ok(false)
    );
    assert_eq!(
        span_eq_str(s, d.input, "A&BBX", SpanKind::AttrValue),
        Ok(false)
    );
    assert_eq!(span_eq_str(s, d.input, "", SpanKind::AttrValue), Ok(false));
    assert_eq!(
        span_eq_str(oa_xml_empty(), d.input, "", SpanKind::AttrValue),
        Ok(true)
    );
    assert_eq!(
        span_eq_str(oa_xml_span(0, 7), b"xx&foo;", "xx", SpanKind::AttrValue),
        Err(XmlError::UndefinedEntity { at: 2 })
    );
}

fn nested(depth: usize) -> String {
    let mut s = String::new();
    for _ in 0..depth {
        s.push_str("<e>");
    }
    for _ in 0..depth {
        s.push_str("</e>");
    }
    s
}

fn limit_of(src: &[u8], l: Limits) -> Result<(), (LimitKind, u32)> {
    let mut p = Parser::new(l).unwrap();
    match p.parse(src) {
        Ok(_) => Ok(()),
        Err(XmlError::Limit { limit, at }) => Err((limit, at)),
        Err(other) => panic!("unexpected {other:?} for limits {l:?}"),
    }
}

// REQ: OA-XML-016
#[test]
fn every_limit_triggers_at_limit_plus_one_and_passes_at_limit() {
    let base = Limits::default();

    // Input bytes: padded with trailing whitespace.
    let l = Limits {
        max_input_bytes: 20,
        ..base
    };
    let pad = |n: usize| format!("<a/>{}", " ".repeat(n - 4));
    assert_eq!(limit_of(pad(20).as_bytes(), l), Ok(()));
    assert_eq!(
        limit_of(pad(21).as_bytes(), l),
        Err((LimitKind::InputBytes, 20))
    );

    // Depth.
    let l = Limits {
        max_depth: 5,
        ..base
    };
    assert_eq!(limit_of(nested(5).as_bytes(), l), Ok(()));
    assert_eq!(
        limit_of(nested(6).as_bytes(), l),
        Err((LimitKind::Depth, 15))
    );

    // Elements: a root with n-1 children.
    let kids = |n: usize| format!("<r>{}</r>", "<c/>".repeat(n - 1));
    let l = Limits {
        max_elements: 10,
        ..base
    };
    assert_eq!(limit_of(kids(10).as_bytes(), l), Ok(()));
    assert_eq!(
        limit_of(kids(11).as_bytes(), l),
        Err((LimitKind::Elements, 3 + 9 * 4))
    );

    // Attributes in total: elements with one attribute each.
    let attrs = |n: usize| format!("<r>{}</r>", "<c x=\"1\"/>".repeat(n));
    let l = Limits {
        max_attributes: 10,
        ..base
    };
    assert_eq!(limit_of(attrs(10).as_bytes(), l), Ok(()));
    assert!(matches!(
        limit_of(attrs(11).as_bytes(), l),
        Err((LimitKind::Attributes, _))
    ));

    // Attributes per element.
    let per = |n: usize| {
        let mut s = String::from("<r");
        for i in 0..n {
            s.push_str(&format!(" a{i}=\"v\""));
        }
        s.push_str("/>");
        s
    };
    let l = Limits {
        max_attrs_per_elem: 4,
        ..base
    };
    assert_eq!(limit_of(per(4).as_bytes(), l), Ok(()));
    assert!(matches!(
        limit_of(per(5).as_bytes(), l),
        Err((LimitKind::AttributesPerElement, _))
    ));

    // Name length, for elements, attributes and end tags.
    let l = Limits {
        max_name_len: 8,
        ..base
    };
    assert_eq!(limit_of(b"<abcdefgh/>", l), Ok(()));
    assert_eq!(
        limit_of(b"<abcdefghi/>", l),
        Err((LimitKind::NameLength, 1))
    );
    assert_eq!(limit_of(b"<a abcdefgh=\"1\"/>", l), Ok(()));
    assert_eq!(
        limit_of(b"<a abcdefghi=\"1\"/>", l),
        Err((LimitKind::NameLength, 3))
    );
    assert_eq!(limit_of(b"<a:b xmlns:a=\"u\"/>", l), Ok(()));
    assert_eq!(
        limit_of(b"<abcd:efgh/>", l),
        Err((LimitKind::NameLength, 1))
    );

    // Text nodes.
    let texts = |n: usize| format!("<r>{}</r>", "<c>t</c>".repeat(n));
    let l = Limits {
        max_text_nodes: 10,
        ..base
    };
    assert_eq!(limit_of(texts(10).as_bytes(), l), Ok(()));
    assert!(matches!(
        limit_of(texts(11).as_bytes(), l),
        Err((LimitKind::TextNodes, _))
    ));

    // Namespace bindings in scope: n declarations on the root.
    let decls = |n: usize| {
        let mut s = String::from("<r");
        for i in 0..n {
            s.push_str(&format!(" xmlns:p{i}=\"u\""));
        }
        s.push_str("/>");
        s
    };
    let l = Limits {
        max_ns_bindings: 4,
        ..base
    };
    assert_eq!(limit_of(decls(4).as_bytes(), l), Ok(()));
    assert_eq!(
        limit_of(decls(5).as_bytes(), l),
        Err((LimitKind::NsBindings, 2 + 4 * 13 + 1))
    );
    // The sum over open elements counts: 2 + 2 passes, 2 + 3 does not; siblings do not add up.
    assert_eq!(
        limit_of(
            b"<r xmlns:a=\"u\" xmlns:b=\"u\"><e xmlns:c=\"u\" xmlns:d=\"u\"/></r>",
            l
        ),
        Ok(())
    );
    assert!(matches!(
        limit_of(
            b"<r xmlns:a=\"u\" xmlns:b=\"u\"><e xmlns:c=\"u\" xmlns:d=\"u\" xmlns:e=\"u\"/></r>",
            l
        ),
        Err((LimitKind::NsBindings, _))
    ));
    assert_eq!(
        limit_of(b"<r><e xmlns:a=\"u\" xmlns:b=\"u\" xmlns:c=\"u\" xmlns:d=\"u\"/><e xmlns:a=\"u\" xmlns:b=\"u\" xmlns:c=\"u\" xmlns:d=\"u\"/></r>", l),
        Ok(())
    );

    // The defaults themselves: depth 64 passes, 65 fails.
    assert_eq!(limit_of(nested(64).as_bytes(), base), Ok(()));
    assert!(matches!(
        limit_of(nested(65).as_bytes(), base),
        Err((LimitKind::Depth, _))
    ));
}

// REQ: OA-XML-018
#[test]
fn every_error_carries_its_offset() {
    assert_eq!(error_offset(&err(b"<a></b>")), 3);
    assert_eq!(error_offset(&err(b"<a>&bogus;</a>")), 3);
    assert_eq!(error_offset(&err(b"<a b=\"1\" b=\"2\"/>")), 9);
    assert_eq!(error_offset(&err(b"<a>\xFF")), 3);
    let msg = err(b"<a>&bogus;</a>").to_string();
    assert!(msg.contains("byte 3"), "{msg}");
    let msg = err(b"<a><b/></a><c/>").to_string();
    assert!(msg.contains("outside the root"), "{msg}");
    let l = Limits {
        max_depth: 1,
        ..Limits::default()
    };
    let e = Parser::new(l).unwrap().parse(b"<a><b/></a>").unwrap_err();
    assert!(e.to_string().contains("max depth"));
}

// REQ: OA-XML-019
#[test]
fn parser_reuse_keeps_tables_fixed_and_results_independent() {
    let mut p = Parser::new(Limits::default()).unwrap();
    let cap = p.table_capacity();
    for _ in 0..3 {
        let d = p.parse(FLIGHT.as_bytes()).unwrap();
        assert_eq!(d.elem_name.len(), 5);
        assert!(attr_needs_decode(&d, AttrIx(4)) || attr_needs_decode(&d, AttrIx(3)));
        let d = p.parse(b"<z q=\"plain\"/>").unwrap();
        assert_eq!(d.elem_name.len(), 1);
        assert_eq!(d.attr_name.len(), 1);
        assert_eq!(d.text_span.len(), 0);
        assert!(
            !attr_needs_decode(&d, AttrIx(0)),
            "flags from the previous message must not leak"
        );
        // A failed parse followed by a good one.
        assert!(p.parse(b"<a><b></a>").is_err());
        assert!(p.parse(b"<ok/>").is_ok());
    }
    assert_eq!(p.table_capacity(), cap);
}

/// What a truncated valid message may report: input that ends inside markup is
/// `UnexpectedEof`; between complete markup with elements open, `Unclosed`; before any root,
/// `NoRoot`; inside a multi-byte character, `Utf8` at that character. Always at the end offset.
fn is_clean_truncation(e: &XmlError, cut: &[u8]) -> bool {
    let end = cut.len() as u32;
    match *e {
        XmlError::UnexpectedEof { at } | XmlError::Unclosed { at } | XmlError::NoRoot { at } => {
            at == end
        }
        XmlError::Utf8 { at } => std::str::from_utf8(cut).unwrap_err().valid_up_to() == at as usize,
        _ => false,
    }
}

fn corpus() -> Vec<Vec<u8>> {
    let mut v = vec![
        FLIGHT.as_bytes().to_vec(),
        b"<?xml version='1.0'?><a xmlns:x=\"u\" x:b=\"&#65;&lt;\"><!--c--><x:c>t&amp;<![CDATA[d]]></x:c><e/></a>"
            .to_vec(),
        b"<r><a b=\"1\" c='2'>text</a><!-- note --><d><e/></d></r>\n".to_vec(),
        "<m>\u{e9}\u{20ac}\u{1F600}&#x1F600;</m>".as_bytes().to_vec(),
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<!-- c --><a/>".to_vec(),
    ];
    v.push(nested(10).into_bytes());
    v.push(
        b"\xEF\xBB\xBF<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<!-- c -->\r\n\
<p:a xmlns:p=\"urn:u\" xmlns=\"urn:d\" b='&#x41;&amp;\t'><p:b>x&lt;&#0000065;y</p:b><![CDATA[z]]>\
<q></q></p:a>\r\n<!-- end -->\r\n"
            .to_vec(),
    );
    v.push(
        "<\u{e9}:\u{20ac} xmlns:\u{e9}=\"u\" \u{3b1}=\"1\"><\u{e9}:b/></\u{e9}:\u{20ac}>"
            .as_bytes()
            .to_vec(),
    );
    v
}

// REQ: OA-XML-017
#[test]
fn no_panic_on_truncation_and_byte_flips() {
    let mut p = Parser::new(Limits {
        max_input_bytes: 4096,
        max_depth: 16,
        max_elements: 64,
        max_attributes: 64,
        max_attrs_per_elem: 8,
        max_name_len: 32,
        max_text_nodes: 64,
        max_ns_bindings: 8,
    })
    .unwrap();
    let flips: [u8; 12] = [
        0x00, 0x01, b'<', b'>', b'&', b'"', b'\'', b'/', b'!', b'?', 0xC3, 0xFF,
    ];
    let mut runs = 0usize;
    let mut oks = 0usize;
    for msg in corpus() {
        assert!(p.parse(&msg).is_ok(), "corpus message must be valid");
        for cut in 0..=msg.len() {
            runs += 1;
            match p.parse(&msg[..cut]) {
                Ok(_) => oks += 1,
                Err(e) => assert!(
                    is_clean_truncation(&e, &msg[..cut]),
                    "cut {cut} of {:?}: {e:?}",
                    String::from_utf8_lossy(&msg)
                ),
            }
        }
        for i in 0..msg.len() {
            for &f in &flips {
                let mut m = msg.clone();
                m[i] = f;
                runs += 1;
                if p.parse(&m).is_ok() {
                    oks += 1;
                }
            }
        }
        // Single-byte deletion and doubled byte.
        for i in 0..msg.len() {
            let mut m = msg.clone();
            m.remove(i);
            runs += 1;
            let _ = p.parse(&m);
            let mut m = msg.clone();
            m.insert(i, msg[i]);
            runs += 1;
            let _ = p.parse(&m);
        }
    }
    assert!(runs > 5000);
    assert!(oks > 0 && oks < runs);
    // Arbitrary byte soup from a fixed generator.
    let mut x: u32 = 0x1234_5678;
    for _ in 0..2000 {
        let mut m = Vec::new();
        for _ in 0..(x % 96) {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            m.push((x >> 24) as u8);
        }
        let _ = p.parse(&m);
    }
}
