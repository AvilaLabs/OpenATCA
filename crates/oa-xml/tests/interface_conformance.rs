//! Source-pinned public interface boundaries that this generic XML parser does and does not cover.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_xml::{Limits, Parser, XmlError};

const REGISTRY: &str = include_str!("../../../validation/interface-conformance-v1.json");

// REQ: OA-IFACE-001
#[test]
fn public_asdi_and_tfmdi_sources_are_pinned_without_claiming_feed_conformance() {
    for marker in [
        "FAA-ASDI-XML-1.8",
        "ASDI-FD-001",
        "3a8213a27253db858b03af7b3fcd306253e294bea181829d0afe5221b56d2f67",
        "FAA-TFMDI-XML-1.7",
        "ETMS-TFMDI-003",
        "5321d99452d9e3578e3ad0ba7979050661761247f7ebbbfb3e12b3a4bd9f6ca8",
        "ASDI-packet-and-schema-codec-not-implemented",
        "TFMDI-profile-not-implemented",
        "oa-tfms tests exercise SWIM TFMData namespaces and message structures, not ETMS-TFMDI-003",
    ] {
        assert!(REGISTRY.contains(marker), "registry lacks {marker}");
    }
}

// REQ: OA-IFACE-001
#[test]
fn generic_xml_subset_accepts_well_formed_namespaced_payload_and_refuses_doctype() {
    // Synthetic XML demonstrates only the parser's generic XML 1.0 surface. The ASDI and TFMDI
    // schemas, packet framing, compression, field requirements, and message sets are not checked.
    let input = br#"<?xml version="1.0"?><asdiOutput xmlns="http://tfm.faa.gov/tfms/TFMS_XIS"><message><id>SYNTHETIC</id></message></asdiOutput>"#;
    let mut parser = Parser::new(Limits::default()).unwrap();
    let doc = parser.parse(input).unwrap();
    assert_eq!(doc.elem_name.len(), 3);

    let mut parser = Parser::new(Limits::default()).unwrap();
    let err = parser
        .parse(b"<!DOCTYPE asdiOutput><asdiOutput/>")
        .unwrap_err();
    assert!(matches!(err, XmlError::Doctype { at: 0 }));
}
