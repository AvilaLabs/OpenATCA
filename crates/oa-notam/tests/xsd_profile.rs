//! Source-pinned emitted-profile checks. This is deliberately not an XSD validation engine.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_notam::*;
use oa_xml::{resolve_elem_ns, span_bytes, ElemIx, Ns, Parser};
use oa_zip::Archive;
use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

const AIXM_5_1_ZIP: &[u8] = include_bytes!("../testdata/schema/aixm_5_1_xsd.zip");
const AIXM_5_1_SHA256: &str = "e14090244c66c38bd459b954b896faeeb7f7828f135164cf0546ae1c1f69c221";
const AIXM_NS: &[u8] = b"http://www.aixm.aero/schema/5.1";
const MESSAGE_NS: &[u8] = b"http://www.aixm.aero/schema/5.1/message";
const EVENT_NS: &[u8] = b"http://www.aixm.aero/schema/5.1/event";
const GML_NS: &[u8] = b"http://www.opengis.net/gml/3.2";
const FNSE_NS: &[u8] = b"http://www.aixm.aero/schema/5.1/extensions/FAA/FNSE";
const EVENT_FEATURES: &[u8] = include_bytes!("../testdata/schema/event_5_1_features.xsd");
const EVENT_DATATYPES: &[u8] = include_bytes!("../testdata/schema/event_5_1_datatypes.xsd");
const FNSE_FEATURES: &[u8] = include_bytes!("../testdata/schema/fnse_5_1_features.xsd");
const FNSE_DATATYPES: &[u8] = include_bytes!("../testdata/schema/fnse_5_1_datatypes.xsd");

const EVENT_FEATURES_SHA256: &str =
    "f8cc720b319647a3120f4d568683b62f1b5dd0d0323b6b8178c7761aedbe09e0";
const EVENT_DATATYPES_SHA256: &str =
    "dfc8d8076d692dbab6c8d173b63ba423cbebcebb02da1330153f00ad250f699a";
const FNSE_FEATURES_SHA256: &str =
    "79221077feba07e0f376554cf1c46330844af760e03c710dceaab6b7ee7be35f";
const FNSE_DATATYPES_SHA256: &str =
    "151fd279be1f675420be2e0daab51f2ec8823e46a885c0fb51fc1be6e68841d8";

struct PinnedSchemas {
    basic_message: String,
    aixm_features: String,
    aixm_common: String,
    temporal: String,
    geometry: String,
    event_features: String,
    event_datatypes: String,
    fnse_features: String,
    fnse_datatypes: String,
}

fn pinned_schemas() -> PinnedSchemas {
    let manifest = include_str!("../testdata/schema/aixm_5_1_xsd.source.json");
    assert!(manifest.contains(AIXM_5_1_SHA256));
    assert_eq!(
        oa_sha256::to_hex(&oa_sha256::sha256(AIXM_5_1_ZIP)),
        AIXM_5_1_SHA256
    );

    let archive = Archive::parse(AIXM_5_1_ZIP).expect("pinned AIXM 5.1 schema archive");
    let extract = |path: &str| {
        String::from_utf8(archive.extract(path).expect("schema entry with CRC check"))
            .expect("official XSD is UTF-8")
    };
    PinnedSchemas {
        basic_message: extract("xsd/message/AIXM_BasicMessage.xsd"),
        aixm_features: extract("xsd/AIXM_Features.xsd"),
        aixm_common: extract("xsd/AIXM_AbstractGML_ObjectTypes.xsd"),
        temporal: extract("xsd/ISO_19136_Schemas/temporal.xsd"),
        geometry: extract("xsd/ISO_19136_Schemas/geometryBasic2d.xsd"),
        event_features: pinned_text(EVENT_FEATURES, EVENT_FEATURES_SHA256),
        event_datatypes: pinned_text(EVENT_DATATYPES, EVENT_DATATYPES_SHA256),
        fnse_features: pinned_text(FNSE_FEATURES, FNSE_FEATURES_SHA256),
        fnse_datatypes: pinned_text(FNSE_DATATYPES, FNSE_DATATYPES_SHA256),
    }
}

fn pinned_text(bytes: &[u8], expected_sha256: &str) -> String {
    assert_eq!(
        oa_sha256::to_hex(&oa_sha256::sha256(bytes)),
        expected_sha256
    );
    String::from_utf8(bytes.to_vec()).expect("official XSD is UTF-8")
}

fn encode_source(source: &str) -> String {
    let source = schema_supported(source);
    let (rows, _) = ok(&source);
    let mut output = vec![0_u8; MAX_ENCODED_BYTES];
    let len = encode_row(&rows, 0, &mut output).expect("bounded profile fixture encodes");
    String::from_utf8(output[..len].to_vec()).expect("encoder emits UTF-8")
}

fn span_text(input: &[u8], span: oa_xml::Span) -> &str {
    std::str::from_utf8(span_bytes(input, span).expect("parser-owned span"))
        .expect("schema and encoder names are UTF-8")
}

fn qname(doc: &oa_xml::Doc<'_>, element: usize) -> String {
    let prefix = doc
        .elem_prefix
        .get(element)
        .map(|span| span_text(doc.input, *span))
        .unwrap_or("");
    let local = doc
        .elem_name
        .get(element)
        .map(|span| span_text(doc.input, *span))
        .unwrap_or("");
    if prefix.is_empty() {
        local.to_owned()
    } else {
        format!("{prefix}:{local}")
    }
}

fn direct_children(doc: &oa_xml::Doc<'_>, parent: usize) -> Vec<usize> {
    let parent = u32::try_from(parent).expect("test XML element index fits parser index");
    doc.elem_parent
        .iter()
        .enumerate()
        .filter_map(|(index, owner)| (owner.0 == parent).then_some(index))
        .collect()
}

fn attribute<'a>(doc: &'a oa_xml::Doc<'_>, element: usize, want: &str) -> Option<&'a str> {
    let first = doc.elem_first_attr.get(element)?.0 as usize;
    let end = first.checked_add(*doc.elem_attr_count.get(element)? as usize)?;
    (first..end).find_map(|index| {
        let prefix = doc
            .attr_prefix
            .get(index)
            .map(|span| span_text(doc.input, *span))?;
        let local = doc
            .attr_name
            .get(index)
            .map(|span| span_text(doc.input, *span))?;
        let name = if prefix.is_empty() {
            local.to_owned()
        } else {
            format!("{prefix}:{local}")
        };
        (name == want)
            .then(|| {
                doc.attr_value
                    .get(index)
                    .map(|span| span_text(doc.input, *span))
            })
            .flatten()
    })
}

fn direct_text(doc: &oa_xml::Doc<'_>, element: usize) -> String {
    doc.text_owner
        .iter()
        .enumerate()
        .filter(|(_, owner)| owner.0 == element as u32)
        .filter_map(|(index, _)| doc.text_span.get(index))
        .map(|span| span_text(doc.input, *span))
        .collect()
}

fn namespace_is(doc: &oa_xml::Doc<'_>, element: usize, expected: &[u8]) {
    let element_ix = ElemIx(u32::try_from(element).expect("test element index fits parser index"));
    match resolve_elem_ns(doc, element_ix) {
        Ns::Uri(span) => assert_eq!(span_bytes(doc.input, span).ok(), Some(expected)),
        _ => panic!("element {} has no declared namespace", qname(doc, element)),
    }
}

fn assert_schema_facts(schemas: &PinnedSchemas) {
    // AIXM_BasicMessage.xsd: AIXMBasicMessagePropertyGroup is a sequence containing one or
    // more hasMember values; omitted minOccurs means the XSD default 1.
    assert!(schemas
        .basic_message
        .contains("group name=\"AIXMBasicMessagePropertyGroup\""));
    let has_member = xsd_tag(&schemas.basic_message, "element name=\"hasMember\"");
    assert!(has_member.contains("maxOccurs=\"unbounded\""));
    assert!(
        !has_member.contains("minOccurs="),
        "hasMember defaults to minOccurs=1"
    );

    // AIXM_AbstractGML_ObjectTypes.xsd puts validTime before interpretation, then optional
    // sequenceNumber and correctionNumber. Omitted minOccurs on validTime means 1.
    assert!(schemas
        .aixm_common
        .contains("element ref=\"gml:validTime\""));
    assert!(schemas
        .aixm_common
        .contains("element ref=\"aixm:interpretation\""));
    assert!(schemas
        .aixm_common
        .contains("element ref=\"aixm:sequenceNumber\" minOccurs=\"0\""));
    assert!(schemas
        .aixm_common
        .contains("element ref=\"aixm:correctionNumber\" minOccurs=\"0\""));
    let time_slice_type = complex_type_body(&schemas.aixm_common, "AbstractAIXMTimeSliceType");
    let mut previous = 0;
    for marker in [
        "element ref=\"aixm:interpretation\"",
        "element ref=\"aixm:sequenceNumber\" minOccurs=\"0\"",
        "element ref=\"aixm:correctionNumber\" minOccurs=\"0\"",
    ] {
        let position = time_slice_type
            .find(marker)
            .expect("AIXM time-slice sequence member");
        assert!(
            position >= previous,
            "AIXM time-slice fields follow XSD order"
        );
        previous = position;
    }
    for value in ["BASELINE", "SNAPSHOT", "TEMPDELTA", "PERMDELTA"] {
        assert!(schemas
            .aixm_common
            .contains(&format!("enumeration value=\"{value}\"")));
    }

    // AIXM_Features.xsd declares the emitted feature globals and required timeSlice property.
    for feature in [
        "AirportHeliport",
        "Runway",
        "RunwayDirection",
        "RunwayElement",
        "Taxiway",
        "TaxiwayElement",
        "Navaid",
        "Unit",
        "ObstacleArea",
        "VerticalStructure",
        "RadarSystem",
        "PrimarySurveillanceRadar",
    ] {
        assert!(
            schemas.aixm_features.contains(&format!(
                "element name=\"{feature}\" type=\"aixm:{feature}Type\""
            )),
            "global AIXM declaration for {feature}"
        );
        let timeslice = xsd_tag(
            &schemas.aixm_features,
            &format!("element name=\"timeSlice\" type=\"aixm:{feature}TimeSlicePropertyType\""),
        );
        assert!(timeslice.contains("maxOccurs=\"unbounded\""));
        assert!(
            !timeslice.contains("minOccurs="),
            "{feature} timeSlice defaults to minOccurs=1"
        );
        assert!(schemas
            .aixm_features
            .contains(&format!("group ref=\"aixm:{feature}PropertyGroup\"")));
    }

    // These are the optional feature properties retained by the encoder. Their order is read
    // from each official PropertyGroup and checked against emitted child order.
    for (feature, properties) in [
        (
            "AirportHeliport",
            &["designator", "locationIndicatorICAO"][..],
        ),
        ("Runway", &["designator"][..]),
        ("RunwayDirection", &["designator"][..]),
        ("RunwayElement", &["extent"][..]),
        ("Taxiway", &[][..]),
        ("TaxiwayElement", &["extent"][..]),
        ("Navaid", &["type"][..]),
        ("Unit", &["designator"][..]),
        ("ObstacleArea", &[][..]),
        ("VerticalStructure", &[][..]),
        ("RadarSystem", &[][..]),
        ("PrimarySurveillanceRadar", &[][..]),
    ] {
        let group_name = format!("<group name=\"{feature}PropertyGroup\">");
        let start = schemas
            .aixm_features
            .find(&group_name)
            .expect("official property group")
            + group_name.len();
        let end = schemas.aixm_features[start..]
            .find("</group>")
            .map(|offset| start + offset)
            .expect("closed XSD property group");
        let group = schemas.aixm_features.get(start..end).expect("group slice");
        let mut previous = 0usize;
        for property in properties {
            let fact = format!("element name=\"{property}\"");
            let position = group.find(&fact).expect("retained property in XSD group");
            let property_tag = xsd_tag(group, &fact);
            assert!(
                position >= previous,
                "retained properties follow XSD sequence order"
            );
            previous = position;
            assert!(property_tag.contains("minOccurs=\"0\""));
        }
    }

    // GML temporal.xsd enumerates indeterminate positions; the emitted unknown value is one of
    // these values and is not interchangeable with before/after/now.
    for value in ["after", "before", "now", "unknown"] {
        assert!(schemas
            .temporal
            .contains(&format!("enumeration value=\"{value}\"")));
    }
    for position in ["beginPosition", "endPosition"] {
        let tag = xsd_tag(
            &schemas.temporal,
            &format!("element name=\"{position}\" type=\"gml:TimePositionType\""),
        );
        assert!(
            !tag.contains("minOccurs="),
            "TimePeriod {position} choice defaults to minOccurs=1"
        );
    }
    // GML geometryBasic2d.xsd requires four positions in a LinearRing when positions are used;
    // its accompanying type documentation requires coincident first and last coordinates.
    assert!(schemas
        .geometry
        .contains("choice minOccurs=\"4\" maxOccurs=\"unbounded\""));
    assert!(schemas
        .geometry
        .contains("first and last coordinates shall be coincident"));

    assert_event_fnse_schema_facts(schemas);
}

fn assert_event_fnse_schema_facts(schemas: &PinnedSchemas) {
    let event_source = include_str!("../testdata/schema/aixm_event_fnse.source.json");
    let fnse_source = include_str!("../testdata/schema/fnse_5_1.source.json");
    assert!(event_source.contains(EVENT_FEATURES_SHA256));
    assert!(event_source.contains(EVENT_DATATYPES_SHA256));
    assert!(event_source.contains("https://aixm.aero/schema/5.1/event/Event_Features.xsd"));
    assert!(fnse_source.contains(FNSE_FEATURES_SHA256));
    assert!(fnse_source.contains(FNSE_DATATYPES_SHA256));
    assert!(
        fnse_source.contains("https://aixm.aero/schema/5.1/extensions/FAA/FNSE/FNSE_Features.xsd")
    );
    assert!(schemas.event_features.contains(&format!(
        "targetNamespace=\"{}\"",
        String::from_utf8_lossy(EVENT_NS)
    )));
    assert!(schemas.event_features.contains("version=\"5.1-d\""));
    assert!(schemas
        .event_features
        .contains("element name=\"Event\" type=\"event:EventType\""));
    assert!(schemas
        .event_features
        .contains("element name=\"EventTimeSlice\" type=\"event:EventTimeSliceType\""));
    let event_time_slice = xsd_tag(
        &schemas.event_features,
        "element name=\"timeSlice\" type=\"event:EventTimeSlicePropertyType\"",
    );
    assert!(event_time_slice.contains("maxOccurs=\"unbounded\""));
    assert!(!event_time_slice.contains("minOccurs="));
    assert!(schemas
        .event_features
        .contains("group ref=\"event:EventPropertyGroup\""));
    assert!(schemas
        .event_features
        .contains("group ref=\"event:NOTAMPropertyGroup\""));
    let event_extension = xsd_tag(
        &schemas.event_features,
        "element name=\"extension\" minOccurs=\"0\" maxOccurs=\"unbounded\"",
    );
    assert!(event_extension.contains("minOccurs=\"0\""));
    assert!(schemas.event_features.contains(
        "element name=\"textNOTAM\" type=\"event:NOTAMPropertyType\" nillable=\"true\" minOccurs=\"0\""
    ));
    assert!(schemas.event_datatypes.contains(&format!(
        "targetNamespace=\"{}\"",
        String::from_utf8_lossy(EVENT_NS)
    )));
    for value in ["N", "R", "C"] {
        assert!(schemas
            .event_datatypes
            .contains(&format!("enumeration value=\"{value}\"")));
    }

    assert!(schemas.fnse_features.contains(&format!(
        "targetNamespace=\"{}\"",
        String::from_utf8_lossy(FNSE_NS)
    )));
    assert!(schemas.fnse_features.contains("version=\"5.1-a\""));
    assert!(schemas.fnse_features.contains(
        "element name=\"EventExtension\" type=\"fnse:EventExtensionType\" substitutionGroup=\"event:AbstractEventExtension\""
    ));
    let extension_group = group_body(&schemas.fnse_features, "EventExtensionPropertyGroup");
    assert_eq!(extension_group.match_indices("<element name=").count(), 1);
    assert!(extension_group.contains("element name=\"classification\""));
    assert!(extension_group.contains("minOccurs=\"0\""));
    for absent in ["canceled", "xovernotamID", "icaoLocation"] {
        assert!(
            !schemas
                .fnse_features
                .contains(&format!("element name=\"{absent}\"")),
            "published FNSE schema does not declare {absent}"
        );
    }
    assert!(schemas.fnse_datatypes.contains(&format!(
        "targetNamespace=\"{}\"",
        String::from_utf8_lossy(FNSE_NS)
    )));
    for value in ["INTL", "DOM", "MIL", "LMIL", "FDC"] {
        assert!(schemas
            .fnse_datatypes
            .contains(&format!("enumeration value=\"{value}\"")));
    }
    assert!(schemas
        .fnse_datatypes
        .contains("pattern value=\"OTHER(:(\\w|_){1,58})?\""));
}

fn group_body<'a>(schema: &'a str, group_name: &str) -> &'a str {
    let marker = format!("<group name=\"{group_name}\">");
    let start = schema.find(&marker).expect("official XSD group") + marker.len();
    let end = schema[start..]
        .find("</group>")
        .map(|offset| start + offset)
        .expect("closed XSD group");
    schema.get(start..end).expect("XSD group slice")
}

fn complex_type_body<'a>(schema: &'a str, type_name: &str) -> &'a str {
    let marker = format!("<complexType name=\"{type_name}\"");
    let tag_start = schema.find(&marker).expect("official XSD complex type");
    let tag_end = schema[tag_start..]
        .find('>')
        .map(|offset| tag_start + offset + 1)
        .expect("closed XSD complex-type tag");
    let start = tag_end;
    let end = schema[start..]
        .find("</complexType>")
        .map(|offset| start + offset)
        .expect("closed XSD complex type");
    schema.get(start..end).expect("XSD complex-type slice")
}

fn xsd_tag<'a>(schema: &'a str, marker: &str) -> &'a str {
    let start = schema.find(marker).expect("pinned XSD fact");
    let relative_end = schema[start..].find('>').expect("closed XSD element tag");
    schema
        .get(start..start + relative_end + 1)
        .expect("XSD tag slice")
}

fn assert_emitted_profile(xml: &str, schemas: &PinnedSchemas) {
    let mut parser = Parser::new(Default::default()).expect("bounded XML parser");
    let doc = parser
        .parse(xml.as_bytes())
        .expect("well-formed emitted XML");
    assert_eq!(doc.elem_name.len(), doc.elem_parent.len());
    assert_eq!(qname(&doc, 0), "message:AIXMBasicMessage");
    namespace_is(&doc, 0, MESSAGE_NS);

    let root_children = direct_children(&doc, 0);
    assert!(!root_children.is_empty());
    assert!(root_children
        .iter()
        .all(|&child| qname(&doc, child) == "message:hasMember"));
    assert!(schemas.basic_message.contains("maxOccurs=\"unbounded\""));

    let mut seen_ids = BTreeSet::new();
    for element in 0..doc.elem_name.len() {
        if let Some(id) = attribute(&doc, element, "gml:id") {
            assert!(matches!(
                id.chars().next(),
                Some(c) if c.is_ascii_alphabetic() || c == '_'
            ));
            assert!(id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')));
            assert!(seen_ids.insert(id.to_owned()), "duplicate gml:id {id}");
        }
        match qname(&doc, element).as_str() {
            name if name.starts_with("message:") => namespace_is(&doc, element, MESSAGE_NS),
            name if name.starts_with("event:") => namespace_is(&doc, element, EVENT_NS),
            name if name.starts_with("fnse:") => namespace_is(&doc, element, FNSE_NS),
            name if name.starts_with("aixm:") => namespace_is(&doc, element, AIXM_NS),
            name if name.starts_with("gml:") => namespace_is(&doc, element, GML_NS),
            _ => {}
        }
    }

    for (member_index, &member) in root_children.iter().enumerate() {
        let payload = direct_children(&doc, member);
        assert_eq!(payload.len(), 1, "one feature/event per hasMember");
        if member_index == 0 {
            assert_eq!(qname(&doc, payload[0]), "event:Event");
            assert_emitted_event_profile(&doc, payload[0], schemas);
            continue;
        }

        let feature_name = qname(&doc, payload[0]);
        let Some(feature) = feature_name.strip_prefix("aixm:") else {
            panic!("feature member has unexpected element {feature_name}");
        };
        assert!(schemas
            .aixm_features
            .contains(&format!("name=\"{feature}\" type=\"aixm:{feature}Type\"")));
        let feature_children = direct_children(&doc, payload[0]);
        assert_eq!(
            feature_children.len(),
            1,
            "one required timeSlice for {feature}"
        );
        assert_eq!(qname(&doc, feature_children[0]), "aixm:timeSlice");
        let slice_wrapper = direct_children(&doc, feature_children[0]);
        assert_eq!(slice_wrapper.len(), 1);
        assert_eq!(
            qname(&doc, slice_wrapper[0]),
            format!("aixm:{feature}TimeSlice")
        );

        let slice_fields = direct_children(&doc, slice_wrapper[0]);
        assert!(slice_fields.len() >= 2);
        assert_eq!(qname(&doc, slice_fields[0]), "gml:validTime");
        assert_eq!(qname(&doc, slice_fields[1]), "aixm:interpretation");
        let interpretation = direct_text(&doc, slice_fields[1]);
        assert!(matches!(
            interpretation.as_str(),
            "BASELINE" | "SNAPSHOT" | "PERMDELTA" | "TEMPDELTA"
        ));
        let tail: Vec<String> = slice_fields
            .iter()
            .skip(2)
            .map(|child| qname(&doc, *child))
            .collect();
        let mut properties = Vec::new();
        let mut property_phase = false;
        let mut sequence_seen = false;
        let mut correction_seen = false;
        for name in tail {
            match name.as_str() {
                "aixm:sequenceNumber" if !property_phase && !sequence_seen && !correction_seen => {
                    sequence_seen = true;
                }
                "aixm:correctionNumber" if !property_phase && !correction_seen => {
                    correction_seen = true;
                }
                "aixm:designator" | "aixm:type" | "aixm:locationIndicatorICAO" | "aixm:extent" => {
                    property_phase = true;
                    properties.push(name);
                }
                _ => panic!("unreviewed or out-of-order emitted AIXM property {name}"),
            }
        }
        let expected_properties: &[&str] = match feature {
            "AirportHeliport" => &["aixm:designator", "aixm:locationIndicatorICAO"],
            "Runway" | "RunwayDirection" | "Unit" => &["aixm:designator"],
            "RunwayElement" | "TaxiwayElement" => &["aixm:extent"],
            "Navaid" => &["aixm:type"],
            "Taxiway"
            | "ObstacleArea"
            | "VerticalStructure"
            | "RadarSystem"
            | "PrimarySurveillanceRadar" => &[],
            _ => panic!("unreviewed emitted feature {feature}"),
        };
        let observed_properties: Vec<&str> = properties.iter().map(String::as_str).collect();
        assert_eq!(observed_properties, expected_properties);
    }

    for element in 0..doc.elem_name.len() {
        match qname(&doc, element).as_str() {
            "gml:TimePeriod" => {
                let children = direct_children(&doc, element);
                assert_eq!(children.len(), 2);
                assert_eq!(qname(&doc, children[0]), "gml:beginPosition");
                assert_eq!(qname(&doc, children[1]), "gml:endPosition");
                assert!(!direct_text(&doc, children[0]).is_empty());
                let end = direct_text(&doc, children[1]);
                if let Some(indeterminate) = attribute(&doc, children[1], "indeterminatePosition") {
                    assert_eq!(indeterminate, "unknown");
                    assert!(end.is_empty());
                    assert!(schemas.temporal.contains("enumeration value=\"unknown\""));
                } else {
                    assert!(!end.is_empty());
                }
            }
            "gml:TimeInstant" => {
                let children = direct_children(&doc, element);
                assert_eq!(children.len(), 1);
                assert_eq!(qname(&doc, children[0]), "gml:timePosition");
                assert!(!direct_text(&doc, children[0]).is_empty());
            }
            "gml:posList" => {
                let mut surface = element;
                loop {
                    let parent = doc.elem_parent.get(surface).expect("posList ancestor").0;
                    surface = usize::try_from(parent).expect("element index is bounded");
                    if qname(&doc, surface) == "aixm:ElevatedSurface" {
                        break;
                    }
                }
                assert_eq!(attribute(&doc, surface, "srsDimension"), Some("2"));
                assert_eq!(
                    attribute(&doc, surface, "srsName"),
                    Some("urn:ogc:def:crs:EPSG::4326")
                );
                let text = direct_text(&doc, element);
                let coordinates: Vec<f64> = text
                    .split_ascii_whitespace()
                    .map(|part| part.parse::<f64>().expect("schema decimal coordinate"))
                    .collect();
                assert!(coordinates.len() >= 8 && coordinates.len().is_multiple_of(2));
                assert_eq!(coordinates.first(), coordinates.get(coordinates.len() - 2));
                assert_eq!(coordinates.get(1), coordinates.last());
                assert!(coordinates.iter().all(|coordinate| coordinate.is_finite()));
            }
            _ => {}
        }
    }
}

fn assert_emitted_event_profile(doc: &oa_xml::Doc<'_>, event: usize, schemas: &PinnedSchemas) {
    let children = direct_children(doc, event);
    assert_eq!(children.len(), 3);
    assert_eq!(qname(doc, children[0]), "gml:identifier");
    assert_eq!(qname(doc, children[1]), "gml:boundedBy");
    assert_eq!(qname(doc, children[2]), "event:timeSlice");
    assert!(schemas
        .event_features
        .contains("element name=\"Event\" type=\"event:EventType\""));

    let wrapper = direct_children(doc, children[2]);
    assert_eq!(wrapper.len(), 1);
    assert_eq!(qname(doc, wrapper[0]), "event:EventTimeSlice");
    let time_slice_fields = direct_children(doc, wrapper[0]);
    assert!(time_slice_fields.len() >= 3);
    assert_eq!(qname(doc, time_slice_fields[0]), "gml:validTime");
    assert_eq!(qname(doc, time_slice_fields[1]), "aixm:interpretation");

    // AbstractAIXMTimeSliceType's inherited optional metadata must precede the
    // EventPropertyGroup and its trailing extension. Accept either optional
    // member individually, but only in the declared sequence and at most once.
    let mut property_start = 2;
    if time_slice_fields
        .get(property_start)
        .is_some_and(|&child| qname(doc, child) == "aixm:sequenceNumber")
    {
        property_start += 1;
    }
    if time_slice_fields
        .get(property_start)
        .is_some_and(|&child| qname(doc, child) == "aixm:correctionNumber")
    {
        property_start += 1;
    }

    let event_group = group_body(&schemas.event_features, "EventPropertyGroup");
    let mut previous = 0usize;
    let mut extension_seen = false;
    let mut seen_properties = BTreeSet::new();
    for child in time_slice_fields.iter().skip(property_start) {
        let name = qname(doc, *child);
        if name == "event:extension" {
            extension_seen = true;
            assert_emitted_fnse_profile(doc, *child, schemas);
            continue;
        }
        assert!(
            !extension_seen,
            "Event extension follows EventPropertyGroup"
        );
        let local = name
            .strip_prefix("event:")
            .expect("Event property namespace");
        let marker = format!("element name=\"{local}\"");
        let position = event_group
            .find(&marker)
            .unwrap_or_else(|| panic!("Event schema has no emitted property {name}"));
        let property_tag = xsd_tag(event_group, &marker);
        if !property_tag.contains("maxOccurs=\"unbounded\"") {
            assert!(
                seen_properties.insert(name.clone()),
                "duplicate singleton {name}"
            );
        }
        assert!(
            position >= previous,
            "Event property order follows XSD sequence"
        );
        previous = position;
        assert!(property_tag.contains("minOccurs=\"0\""));
    }

    let notam = (0..doc.elem_name.len())
        .find(|&index| qname(doc, index) == "event:NOTAM")
        .expect("emitted Event contains NOTAM object");
    let notam_group = group_body(&schemas.event_features, "NOTAMPropertyGroup");
    let mut previous = 0usize;
    for child in direct_children(doc, notam) {
        let name = qname(doc, child);
        let local = name
            .strip_prefix("event:")
            .expect("NOTAM element namespace");
        let marker = format!("element name=\"{local}\"");
        let position = notam_group
            .find(&marker)
            .unwrap_or_else(|| panic!("Event schema has no emitted NOTAM property {name}"));
        assert!(position >= previous, "NOTAM properties follow XSD sequence");
        previous = position;
        assert!(xsd_tag(notam_group, &marker).contains("minOccurs=\"0\""));
    }

    for element in 0..doc.elem_name.len() {
        if qname(doc, element) == "event:Event" {
            assert!(schemas
                .event_features
                .contains("element name=\"Event\" type=\"event:EventType\""));
        }
    }
}

fn profile_accepts(xml: &str, schemas: &PinnedSchemas) -> bool {
    catch_unwind(AssertUnwindSafe(|| assert_emitted_profile(xml, schemas))).is_ok()
}

fn remove_first_element(xml: &str, opening: &str, closing: &str) -> String {
    let start = xml.find(opening).expect("fixture element start");
    let end_start = xml[start..]
        .find(closing)
        .map(|offset| start + offset + closing.len())
        .expect("fixture element end");
    format!("{}{}", &xml[..start], &xml[end_start..])
}

fn assert_emitted_fnse_profile(doc: &oa_xml::Doc<'_>, wrapper: usize, schemas: &PinnedSchemas) {
    let extension = direct_children(doc, wrapper);
    assert_eq!(extension.len(), 1);
    assert_eq!(qname(doc, extension[0]), "fnse:EventExtension");
    let children = direct_children(doc, extension[0]);
    assert!(children.len() <= 1);
    for child in children {
        assert_eq!(qname(doc, child), "fnse:classification");
        let group = group_body(&schemas.fnse_features, "EventExtensionPropertyGroup");
        assert!(group.contains("element name=\"classification\""));
        let value = direct_text(doc, child);
        assert!(matches!(value.as_str(), "INTL" | "DOM" | "MIL"));
        assert!(schemas
            .fnse_datatypes
            .contains(&format!("enumeration value=\"{value}\"")));
    }
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_emission_matches_the_pinned_xsd_profile_subset() {
    let schemas = pinned_schemas();
    assert_schema_facts(&schemas);

    let single_features = [
        airport(),
        runway(),
        direction(),
        runway_element(RING),
        taxiway(),
        taxiway_element("30.30 -90.30 30.30 -90.40 30.40 -90.40 30.30 -90.30"),
        navaid(),
        unit(),
        obstacle_area(),
        vertical_structure(),
        radar_system(),
        primary_radar(),
    ];
    for feature in &single_features {
        assert_emitted_profile(&encode_source(&msg(&[feature])), &schemas);
    }

    // The combined fixture exercises every retained scalar/extension branch, the full feature
    // sequence, repeated polygons, sequence/correction metadata, and canonical member order.
    assert_emitted_profile(&encode_source(&full()), &schemas);
    assert_emitted_profile(&encode_source(&msg(&[])), &schemas);

    for feature_interpretation in ["BASELINE", "SNAPSHOT", "TEMPDELTA", "PERMDELTA"] {
        let feature = airport();
        let source = msg(&[&feature]).replace(
            "<aixm:interpretation>SNAPSHOT</aixm:interpretation>",
            &format!("<aixm:interpretation>{feature_interpretation}</aixm:interpretation>"),
        );
        assert_emitted_profile(&encode_source(&source), &schemas);
    }

    let airport_feature = airport();
    let sliced = msg(&[&airport_feature]).replace(
        "<aixm:interpretation>SNAPSHOT</aixm:interpretation>",
        "<aixm:interpretation>SNAPSHOT</aixm:interpretation><aixm:sequenceNumber>3</aixm:sequenceNumber><aixm:correctionNumber>1</aixm:correctionNumber>",
    );
    assert_emitted_profile(&encode_source(&sliced), &schemas);

    for notam_type in ["N", "R", "C"] {
        let source = rep(
            &msg(&[]),
            "<event:type>N</event:type>",
            &format!("<event:type>{notam_type}</event:type>"),
        );
        assert_emitted_profile(&encode_source(&source), &schemas);
    }

    for classification in ["INTL", "DOM", "MIL"] {
        let source = rep(
            &msg(&[]),
            "<fnse:classification>DOM</fnse:classification>",
            &format!("<fnse:classification>{classification}</fnse:classification>"),
        );
        assert_emitted_profile(&encode_source(&source), &schemas);
    }

    let escaped = rep(
        &msg(&[]),
        "<event:text>RWY 09/27 CLSD</event:text>",
        "<event:text>A &amp; B &lt;C&gt;</event:text>",
    );
    assert_emitted_profile(&encode_source(&escaped), &schemas);

    let estimated = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>203003061830EST</event:effectiveEnd>",
    );
    assert_emitted_profile(&encode_source(&estimated), &schemas);

    let permanent = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>PERM</event:effectiveEnd>",
    );
    assert_emitted_profile(&encode_source(&permanent), &schemas);
    let permanent_unknown = rep(
        &permanent,
        "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>",
        "",
    );
    assert_emitted_profile(&encode_source(&permanent_unknown), &schemas);
}

#[test]
fn emitted_profile_guard_rejects_namespace_order_and_required_field_mutations() {
    let schemas = pinned_schemas();
    assert_schema_facts(&schemas);
    let emitted = encode_source(&msg(&[]));
    assert_emitted_profile(&emitted, &schemas);

    let wrong_namespace = emitted.replace(
        "xmlns:event=\"http://www.aixm.aero/schema/5.1/event\"",
        "xmlns:event=\"urn:wrong-event-namespace\"",
    );
    assert_ne!(wrong_namespace, emitted);
    assert!(!profile_accepts(&wrong_namespace, &schemas));

    let reversed_metadata = emitted.replace(
        "<aixm:sequenceNumber>1</aixm:sequenceNumber><aixm:correctionNumber>0</aixm:correctionNumber>",
        "<aixm:correctionNumber>0</aixm:correctionNumber><aixm:sequenceNumber>1</aixm:sequenceNumber>",
    );
    assert_ne!(reversed_metadata, emitted);
    assert!(!profile_accepts(&reversed_metadata, &schemas));

    let missing_identifier = remove_first_element(
        &emitted,
        "<gml:identifier codeSpace=\"urn:uuid:\">",
        "</gml:identifier>",
    );
    assert_ne!(missing_identifier, emitted);
    assert!(!profile_accepts(&missing_identifier, &schemas));
}
