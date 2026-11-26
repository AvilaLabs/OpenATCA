//! Safe, caller-owned serialization adapters for the existing bounded text parsers.
//!
//! This module contains no FFI pointers or unsafe code. Each call builds ordinary Rust output
//! vectors, bounded by the native aggregate budget, and retains no input or parser state.

#![forbid(unsafe_code)]

use std::io::Cursor;

use oa_csv::{CsvErrorKind, Limits as CsvLimits, Reader as CsvReader};
use oa_json::{self, ErrorKind as JsonErrorKind, Kind as JsonKind, NodeIx};
use oa_xml::{
    attr_needs_decode, ns_uri_bytes, text_needs_decode, LimitKind, Limits as XmlLimits,
    Parser as XmlParser, SpanKind,
};

pub const MODULE_XML: u32 = 1;
pub const MODULE_JSON: u32 = 2;
pub const MODULE_CSV: u32 = 3;

pub const MAX_CSV_INPUT_BYTES: usize = u32::MAX as usize;
pub const MAX_FLAT_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaTextError {
    pub module: u32,
    pub kind: u32,
    pub position: u64,
    pub detail_a: u64,
    pub detail_b: u64,
}
const _: () = assert!(core::mem::size_of::<OaTextError>() == 32);
const _: () = assert!(core::mem::align_of::<OaTextError>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaJsonNode {
    pub kind: u32,
    pub first_child: u32,
    pub child_count: u32,
    pub subtree_end: u32,
    pub value_offset: u32,
    pub value_length: u32,
}
const _: () = assert!(core::mem::size_of::<OaJsonNode>() == 24);
const _: () = assert!(core::mem::align_of::<OaJsonNode>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaJsonCounts {
    pub node_count: u64,
    pub value_bytes: u64,
}
const _: () = assert!(core::mem::size_of::<OaJsonCounts>() == 16);
const _: () = assert!(core::mem::align_of::<OaJsonCounts>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlSpan {
    pub start: u32,
    pub length: u32,
}
const _: () = assert!(core::mem::size_of::<OaXmlSpan>() == 8);
const _: () = assert!(core::mem::align_of::<OaXmlSpan>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlElement {
    pub name: OaXmlSpan,
    pub prefix: OaXmlSpan,
    pub parent: u32,
    pub first_attribute: u32,
    pub attribute_count: u32,
    pub first_child: u32,
    pub next_sibling: u32,
    pub namespace_head: u32,
}
const _: () = assert!(core::mem::size_of::<OaXmlElement>() == 40);
const _: () = assert!(core::mem::align_of::<OaXmlElement>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlAttribute {
    pub name: OaXmlSpan,
    pub prefix: OaXmlSpan,
    pub raw_value: OaXmlSpan,
    pub owner_element: u32,
    pub flags: u32,
}
const _: () = assert!(core::mem::size_of::<OaXmlAttribute>() == 32);
const _: () = assert!(core::mem::align_of::<OaXmlAttribute>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlText {
    pub raw_value: OaXmlSpan,
    pub owner_element: u32,
    pub flags: u32,
}
const _: () = assert!(core::mem::size_of::<OaXmlText>() == 16);
const _: () = assert!(core::mem::align_of::<OaXmlText>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlNamespace {
    pub owner_element: u32,
    pub prefix: OaXmlSpan,
    pub uri_offset: u32,
    pub uri_length: u32,
    pub next_for_element: u32,
}
const _: () = assert!(core::mem::size_of::<OaXmlNamespace>() == 24);
const _: () = assert!(core::mem::align_of::<OaXmlNamespace>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaXmlCounts {
    pub elements: u64,
    pub attributes: u64,
    pub text_nodes: u64,
    pub namespaces: u64,
    pub namespace_uri_bytes: u64,
}
const _: () = assert!(core::mem::size_of::<OaXmlCounts>() == 40);
const _: () = assert!(core::mem::align_of::<OaXmlCounts>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCsvRow {
    pub physical_line: u64,
    pub first_field: u32,
    pub field_count: u32,
}
const _: () = assert!(core::mem::size_of::<OaCsvRow>() == 16);
const _: () = assert!(core::mem::align_of::<OaCsvRow>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCsvField {
    pub value_offset: u32,
    pub value_length: u32,
}
const _: () = assert!(core::mem::size_of::<OaCsvField>() == 8);
const _: () = assert!(core::mem::align_of::<OaCsvField>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaCsvCounts {
    pub rows: u64,
    pub fields: u64,
    pub value_bytes: u64,
}
const _: () = assert!(core::mem::size_of::<OaCsvCounts>() == 24);
const _: () = assert!(core::mem::align_of::<OaCsvCounts>() == 8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Parser,
    ResourceLimit,
    Allocation,
    Internal,
    InvalidArgument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterFailure {
    pub kind: FailureKind,
    pub parser: OaTextError,
    /// Module-specific counts, copied to the corresponding public count record on failure.
    pub counts: [u64; 5],
}

impl AdapterFailure {
    fn invalid_argument(module: u32) -> Self {
        Self {
            kind: FailureKind::InvalidArgument,
            parser: OaTextError {
                module,
                ..OaTextError::default()
            },
            counts: [0; 5],
        }
    }
    fn resource(module: u32, counts: [u64; 5]) -> Self {
        Self {
            kind: FailureKind::ResourceLimit,
            parser: OaTextError {
                module,
                ..OaTextError::default()
            },
            counts,
        }
    }

    fn allocation(module: u32, counts: [u64; 5]) -> Self {
        Self {
            kind: FailureKind::Allocation,
            parser: OaTextError {
                module,
                ..OaTextError::default()
            },
            counts,
        }
    }

    fn internal(module: u32, counts: [u64; 5]) -> Self {
        Self {
            kind: FailureKind::Internal,
            parser: OaTextError {
                module,
                ..OaTextError::default()
            },
            counts,
        }
    }

    fn parser(parser: OaTextError, counts: [u64; 5]) -> Self {
        Self {
            kind: FailureKind::Parser,
            parser,
            counts,
        }
    }
}

pub struct JsonFlat {
    pub nodes: Vec<OaJsonNode>,
    pub pool: Vec<u8>,
}

pub struct XmlFlat {
    pub elements: Vec<OaXmlElement>,
    pub attributes: Vec<OaXmlAttribute>,
    pub texts: Vec<OaXmlText>,
    pub namespaces: Vec<OaXmlNamespace>,
    pub namespace_uris: Vec<u8>,
}

pub struct CsvFlat {
    pub rows: Vec<OaCsvRow>,
    pub fields: Vec<OaCsvField>,
    pub pool: Vec<u8>,
}

fn parse_json_doc(input: &[u8]) -> Result<oa_json::Doc<'_>, AdapterFailure> {
    oa_json::parse(input, &oa_json::DEFAULT_LIMITS).map_err(|error| {
        AdapterFailure::parser(
            OaTextError {
                module: MODULE_JSON,
                kind: json_error_kind(error.kind),
                position: error.offset as u64,
                detail_a: 0,
                detail_b: 0,
            },
            [0; 5],
        )
    })
}

fn measure_json(doc: &oa_json::Doc<'_>) -> Result<OaJsonCounts, AdapterFailure> {
    let node_count = doc.node_count();
    let mut value_bytes = 0usize;
    for index in 0..node_count {
        let index_u32 =
            u32::try_from(index).map_err(|_| AdapterFailure::internal(MODULE_JSON, [0; 5]))?;
        let node = NodeIx(index_u32);
        match doc
            .kind(node)
            .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, [0; 5]))?
        {
            JsonKind::String | JsonKind::StringPooled => {
                let text = doc
                    .str(node)
                    .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, [0; 5]))?;
                value_bytes = value_bytes
                    .checked_add(text.len())
                    .ok_or_else(|| AdapterFailure::resource(MODULE_JSON, [0; 5]))?;
            }
            JsonKind::Number => {
                let text = doc
                    .number_text(node)
                    .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, [0; 5]))?;
                value_bytes = value_bytes
                    .checked_add(text.len())
                    .ok_or_else(|| AdapterFailure::resource(MODULE_JSON, [0; 5]))?;
            }
            _ => {}
        }
    }
    let node_count =
        u64::try_from(node_count).map_err(|_| AdapterFailure::resource(MODULE_JSON, [0; 5]))?;
    let value_bytes =
        u64::try_from(value_bytes).map_err(|_| AdapterFailure::resource(MODULE_JSON, [0; 5]))?;
    let output_bytes = node_count
        .checked_mul(core::mem::size_of::<OaJsonNode>() as u64)
        .and_then(|n| n.checked_add(value_bytes))
        .ok_or_else(|| AdapterFailure::resource(MODULE_JSON, [node_count, 0, value_bytes, 0, 0]))?;
    let counts = [node_count, value_bytes, 0, 0, 0];
    if output_bytes > MAX_FLAT_OUTPUT_BYTES {
        return Err(AdapterFailure::resource(MODULE_JSON, counts));
    }
    Ok(OaJsonCounts {
        node_count,
        value_bytes,
    })
}

pub fn json_counts(input: &[u8]) -> Result<OaJsonCounts, AdapterFailure> {
    let doc = parse_json_doc(input)?;
    measure_json(&doc)
}

pub fn parse_json(input: &[u8]) -> Result<JsonFlat, AdapterFailure> {
    let doc = parse_json_doc(input)?;
    let counts = measure_json(&doc)?;
    let node_count = usize::try_from(counts.node_count).map_err(|_| {
        AdapterFailure::resource(
            MODULE_JSON,
            [counts.node_count, counts.value_bytes, 0, 0, 0],
        )
    })?;
    let value_bytes = usize::try_from(counts.value_bytes).map_err(|_| {
        AdapterFailure::resource(
            MODULE_JSON,
            [counts.node_count, counts.value_bytes, 0, 0, 0],
        )
    })?;
    let count_array = [counts.node_count, counts.value_bytes, 0, 0, 0];

    let mut nodes = Vec::new();
    let mut pool = Vec::new();
    nodes
        .try_reserve_exact(node_count)
        .map_err(|_| AdapterFailure::allocation(MODULE_JSON, count_array))?;
    pool.try_reserve_exact(value_bytes)
        .map_err(|_| AdapterFailure::allocation(MODULE_JSON, count_array))?;
    for index in 0..node_count {
        let index_u32 =
            u32::try_from(index).map_err(|_| AdapterFailure::internal(MODULE_JSON, count_array))?;
        let node = NodeIx(index_u32);
        let kind = doc
            .kind(node)
            .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, count_array))?;
        let members = doc.len(node);
        let direct_children = match kind {
            JsonKind::Array => members,
            JsonKind::Object => members
                .checked_mul(2)
                .ok_or_else(|| AdapterFailure::resource(MODULE_JSON, count_array))?,
            _ => 0,
        };
        let first_child = if direct_children == 0 {
            u32::MAX
        } else {
            index_u32
                .checked_add(1)
                .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, count_array))?
        };
        let subtree_end = doc.skip(node).0;
        let (value_offset, value_length) = match kind {
            JsonKind::String | JsonKind::StringPooled => {
                let bytes = doc
                    .str(node)
                    .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, count_array))?
                    .as_bytes();
                let offset = u32::try_from(pool.len())
                    .map_err(|_| AdapterFailure::resource(MODULE_JSON, count_array))?;
                let length = u32::try_from(bytes.len())
                    .map_err(|_| AdapterFailure::resource(MODULE_JSON, count_array))?;
                pool.extend_from_slice(bytes);
                (offset, length)
            }
            JsonKind::Number => {
                let bytes = doc
                    .number_text(node)
                    .ok_or_else(|| AdapterFailure::internal(MODULE_JSON, count_array))?
                    .as_bytes();
                let offset = u32::try_from(pool.len())
                    .map_err(|_| AdapterFailure::resource(MODULE_JSON, count_array))?;
                let length = u32::try_from(bytes.len())
                    .map_err(|_| AdapterFailure::resource(MODULE_JSON, count_array))?;
                pool.extend_from_slice(bytes);
                (offset, length)
            }
            _ => (0, 0),
        };
        nodes.push(OaJsonNode {
            kind: json_kind(kind),
            first_child,
            child_count: u32::try_from(direct_children)
                .map_err(|_| AdapterFailure::resource(MODULE_JSON, count_array))?,
            subtree_end,
            value_offset,
            value_length,
        });
    }
    if pool.len() != value_bytes {
        return Err(AdapterFailure::internal(MODULE_JSON, count_array));
    }
    Ok(JsonFlat { nodes, pool })
}

fn new_xml_parser() -> Result<XmlParser, AdapterFailure> {
    XmlParser::new(XmlLimits::default()).map_err(|error| xml_failure(error, [0; 5]))
}

fn measure_xml(doc: &oa_xml::Doc<'_>) -> Result<OaXmlCounts, AdapterFailure> {
    let counts = OaXmlCounts {
        elements: u64::try_from(doc.elem_name.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, [0; 5]))?,
        attributes: u64::try_from(doc.attr_name.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, [0; 5]))?,
        text_nodes: u64::try_from(doc.text_span.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, [0; 5]))?,
        namespaces: u64::try_from(doc.ns_uri.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, [0; 5]))?,
        namespace_uri_bytes: 0,
    };
    let mut uri_bytes = 0usize;
    for span in doc.ns_uri {
        let uri = ns_uri_bytes(doc, *span)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, xml_count_array(counts)))?;
        uri_bytes = uri_bytes
            .checked_add(uri.len())
            .ok_or_else(|| AdapterFailure::resource(MODULE_XML, xml_count_array(counts)))?;
    }
    let counts = OaXmlCounts {
        namespace_uri_bytes: u64::try_from(uri_bytes)
            .map_err(|_| AdapterFailure::resource(MODULE_XML, xml_count_array(counts)))?,
        ..counts
    };
    let count_array = xml_count_array(counts);
    let output_bytes = counts
        .elements
        .checked_mul(core::mem::size_of::<OaXmlElement>() as u64)
        .and_then(|n| {
            n.checked_add(
                counts
                    .attributes
                    .checked_mul(core::mem::size_of::<OaXmlAttribute>() as u64)?,
            )
        })
        .and_then(|n| {
            n.checked_add(
                counts
                    .text_nodes
                    .checked_mul(core::mem::size_of::<OaXmlText>() as u64)?,
            )
        })
        .and_then(|n| {
            n.checked_add(
                counts
                    .namespaces
                    .checked_mul(core::mem::size_of::<OaXmlNamespace>() as u64)?,
            )
        })
        .and_then(|n| n.checked_add(counts.namespace_uri_bytes))
        .ok_or_else(|| AdapterFailure::resource(MODULE_XML, count_array))?;
    if output_bytes > MAX_FLAT_OUTPUT_BYTES {
        return Err(AdapterFailure::resource(MODULE_XML, count_array));
    }
    Ok(counts)
}

pub fn xml_counts(input: &[u8]) -> Result<OaXmlCounts, AdapterFailure> {
    let mut parser = new_xml_parser()?;
    let doc = parser
        .parse(input)
        .map_err(|error| xml_failure(error, [0; 5]))?;
    measure_xml(&doc)
}

pub fn parse_xml(input: &[u8]) -> Result<XmlFlat, AdapterFailure> {
    let mut parser = new_xml_parser()?;
    let doc = parser
        .parse(input)
        .map_err(|error| xml_failure(error, [0; 5]))?;
    let counts = measure_xml(&doc)?;
    let count_array = xml_count_array(counts);
    let uri_bytes = usize::try_from(counts.namespace_uri_bytes)
        .map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
    let mut elements = Vec::new();
    let mut attributes = Vec::new();
    let mut texts = Vec::new();
    let mut namespaces = Vec::new();
    let mut namespace_uris = Vec::new();
    elements
        .try_reserve_exact(doc.elem_name.len())
        .map_err(|_| AdapterFailure::allocation(MODULE_XML, count_array))?;
    attributes
        .try_reserve_exact(doc.attr_name.len())
        .map_err(|_| AdapterFailure::allocation(MODULE_XML, count_array))?;
    texts
        .try_reserve_exact(doc.text_span.len())
        .map_err(|_| AdapterFailure::allocation(MODULE_XML, count_array))?;
    namespaces
        .try_reserve_exact(doc.ns_uri.len())
        .map_err(|_| AdapterFailure::allocation(MODULE_XML, count_array))?;
    namespace_uris
        .try_reserve_exact(uri_bytes)
        .map_err(|_| AdapterFailure::allocation(MODULE_XML, count_array))?;
    for i in 0..doc.elem_name.len() {
        let name = *doc
            .elem_name
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let prefix = *doc
            .elem_prefix
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let parent = doc
            .elem_parent
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let first_attribute = doc
            .elem_first_attr
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let attribute_count = *doc
            .elem_attr_count
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let first_child = doc
            .elem_first_child
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let next_sibling = doc
            .elem_next_sibling
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let namespace_head = doc
            .elem_ns_head
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        elements.push(OaXmlElement {
            name: xml_span(name),
            prefix: xml_span(prefix),
            parent: parent.0,
            first_attribute: first_attribute.0,
            attribute_count,
            first_child: first_child.0,
            next_sibling: next_sibling.0,
            namespace_head: namespace_head.0,
        });
    }
    for i in 0..doc.attr_name.len() {
        let name = *doc
            .attr_name
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let prefix = *doc
            .attr_prefix
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let value = *doc
            .attr_value
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let index =
            u32::try_from(i).map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
        attributes.push(OaXmlAttribute {
            name: xml_span(name),
            prefix: xml_span(prefix),
            raw_value: xml_span(value),
            owner_element: u32::MAX,
            flags: u32::from(attr_needs_decode(&doc, oa_xml::AttrIx(index))),
        });
    }
    for (element_ix, element) in doc
        .elem_first_attr
        .iter()
        .zip(doc.elem_attr_count)
        .enumerate()
    {
        let (first, count) = element;
        let owner = u32::try_from(element_ix)
            .map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
        let end = (first.0 as usize)
            .checked_add(*count as usize)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        for attribute_ix in first.0 as usize..end {
            let attribute = attributes
                .get_mut(attribute_ix)
                .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
            attribute.owner_element = owner;
        }
    }
    for i in 0..doc.text_span.len() {
        let raw_value = *doc
            .text_span
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let owner_element = doc
            .text_owner
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let index =
            u32::try_from(i).map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
        texts.push(OaXmlText {
            raw_value: xml_span(raw_value),
            owner_element: owner_element.0,
            flags: u32::from(text_needs_decode(&doc, oa_xml::TextIx(index))),
        });
    }
    for i in 0..doc.ns_uri.len() {
        let span = *doc
            .ns_uri
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let uri = ns_uri_bytes(&doc, span)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let offset = u32::try_from(namespace_uris.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
        let length = u32::try_from(uri.len())
            .map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
        namespace_uris.extend_from_slice(uri);
        let prefix = *doc
            .ns_prefix
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let next = doc
            .ns_next
            .get(i)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        namespaces.push(OaXmlNamespace {
            owner_element: u32::MAX,
            prefix: xml_span(prefix),
            uri_offset: offset,
            uri_length: length,
            next_for_element: next.0,
        });
    }
    for element_ix in 0..doc.elem_ns_head.len() {
        let element = doc
            .elem_ns_head
            .get(element_ix)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let parent = doc
            .elem_parent
            .get(element_ix)
            .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        let inherited = if parent.0 == u32::MAX {
            oa_xml::NsIx::NONE
        } else {
            *doc.elem_ns_head
                .get(parent.0 as usize)
                .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?
        };
        let mut current = *element;
        while current != inherited && current.0 != u32::MAX {
            let owner = u32::try_from(element_ix)
                .map_err(|_| AdapterFailure::resource(MODULE_XML, count_array))?;
            let namespace = namespaces
                .get_mut(current.0 as usize)
                .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
            if namespace.owner_element != u32::MAX {
                return Err(AdapterFailure::internal(MODULE_XML, count_array));
            }
            namespace.owner_element = owner;
            current = *doc
                .ns_next
                .get(current.0 as usize)
                .ok_or_else(|| AdapterFailure::internal(MODULE_XML, count_array))?;
        }
        if current != inherited {
            return Err(AdapterFailure::internal(MODULE_XML, count_array));
        }
    }
    Ok(XmlFlat {
        elements,
        attributes,
        texts,
        namespaces,
        namespace_uris,
    })
}

pub fn decode_xml_span(
    input: &[u8],
    start: u32,
    length: u32,
    kind: u32,
    output: &mut [u8],
) -> Result<usize, AdapterFailure> {
    let span_kind = match kind {
        1 => SpanKind::Text,
        2 => SpanKind::AttrValue,
        _ => return Err(AdapterFailure::invalid_argument(MODULE_XML)),
    };
    let span = oa_xml::Span { start, len: length };
    oa_xml::decode_into(span, input, output, span_kind).map_err(|error| xml_failure(error, [0; 5]))
}

pub fn parse_csv(input: &[u8]) -> Result<CsvFlat, AdapterFailure> {
    let expected = csv_counts_for_input(input)?;
    let rows_capacity = usize::try_from(expected.rows)
        .map_err(|_| AdapterFailure::resource(MODULE_CSV, csv_count_array(expected)))?;
    let fields_capacity = usize::try_from(expected.fields)
        .map_err(|_| AdapterFailure::resource(MODULE_CSV, csv_count_array(expected)))?;
    let bytes_capacity = usize::try_from(expected.value_bytes)
        .map_err(|_| AdapterFailure::resource(MODULE_CSV, csv_count_array(expected)))?;
    let counts = csv_count_array(expected);
    let mut reader = new_csv_reader(input)?;
    let header_fields = reader.header().names();
    if input.len() > MAX_CSV_INPUT_BYTES {
        return Err(AdapterFailure::resource(MODULE_CSV, [0; 5]));
    }
    let mut rows = Vec::new();
    let mut fields = Vec::new();
    let mut pool = Vec::new();
    rows.try_reserve_exact(rows_capacity)
        .map_err(|_| AdapterFailure::allocation(MODULE_CSV, counts))?;
    fields
        .try_reserve_exact(fields_capacity)
        .map_err(|_| AdapterFailure::allocation(MODULE_CSV, counts))?;
    pool.try_reserve_exact(bytes_capacity)
        .map_err(|_| AdapterFailure::allocation(MODULE_CSV, counts))?;
    fields_and_pool(
        &mut fields,
        &mut pool,
        header_fields.iter().map(String::as_str),
    )?;
    rows.push(OaCsvRow {
        physical_line: 1,
        first_field: 0,
        field_count: u32::try_from(header_fields.len()).map_err(|_| {
            AdapterFailure::resource(MODULE_CSV, csv_counts(rows.len(), fields.len(), pool.len()))
        })?,
    });
    loop {
        let record = match reader.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => {
                return Err(csv_failure(
                    error,
                    csv_counts(rows.len(), fields.len(), pool.len()),
                ))
            }
        };
        let first_field = u32::try_from(fields.len()).map_err(|_| {
            AdapterFailure::resource(MODULE_CSV, csv_counts(rows.len(), fields.len(), pool.len()))
        })?;
        let field_count = u32::try_from(record.len()).map_err(|_| {
            AdapterFailure::resource(MODULE_CSV, csv_counts(rows.len(), fields.len(), pool.len()))
        })?;
        for index in 0..record.len() {
            let value = record
                .field(index)
                .ok_or_else(|| AdapterFailure::internal(MODULE_CSV, counts))?;
            push_csv_value(&mut fields, &mut pool, value)?;
        }
        rows.push(OaCsvRow {
            physical_line: record.line(),
            first_field,
            field_count,
        });
    }
    if rows.len() != rows_capacity
        || fields.len() != fields_capacity
        || pool.len() != bytes_capacity
    {
        return Err(AdapterFailure::internal(MODULE_CSV, counts));
    }
    Ok(CsvFlat { rows, fields, pool })
}

fn new_csv_reader(input: &[u8]) -> Result<CsvReader<Cursor<&[u8]>>, AdapterFailure> {
    if input.len() > MAX_CSV_INPUT_BYTES {
        return Err(AdapterFailure::resource(MODULE_CSV, [0; 5]));
    }
    CsvReader::with_limits("memory", Cursor::new(input), CsvLimits::default())
        .map_err(|error| csv_failure(error, [0; 5]))
}

pub fn csv_counts_for_input(input: &[u8]) -> Result<OaCsvCounts, AdapterFailure> {
    let mut reader = new_csv_reader(input)?;
    let header = reader.header().names();
    let mut rows = 1u64;
    let mut fields =
        u64::try_from(header.len()).map_err(|_| AdapterFailure::resource(MODULE_CSV, [0; 5]))?;
    let mut value_bytes = 0u64;
    for value in header {
        value_bytes = value_bytes
            .checked_add(
                u64::try_from(value.len())
                    .map_err(|_| AdapterFailure::resource(MODULE_CSV, [0; 5]))?,
            )
            .ok_or_else(|| {
                AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
            })?;
    }
    loop {
        let record = match reader.next_record() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => return Err(csv_failure(error, [rows, fields, value_bytes, 0, 0])),
        };
        rows = rows.checked_add(1).ok_or_else(|| {
            AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
        })?;
        fields = fields
            .checked_add(u64::try_from(record.len()).map_err(|_| {
                AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
            })?)
            .ok_or_else(|| {
                AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
            })?;
        for index in 0..record.len() {
            let value = record.field(index).ok_or_else(|| {
                AdapterFailure::internal(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
            })?;
            value_bytes = value_bytes
                .checked_add(u64::try_from(value.len()).map_err(|_| {
                    AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
                })?)
                .ok_or_else(|| {
                    AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
                })?;
        }
        let total = rows
            .checked_mul(core::mem::size_of::<OaCsvRow>() as u64)
            .and_then(|n| {
                n.checked_add(fields.checked_mul(core::mem::size_of::<OaCsvField>() as u64)?)
            })
            .and_then(|n| n.checked_add(value_bytes))
            .ok_or_else(|| {
                AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0])
            })?;
        if total > MAX_FLAT_OUTPUT_BYTES {
            return Err(AdapterFailure::resource(
                MODULE_CSV,
                [rows, fields, value_bytes, 0, 0],
            ));
        }
    }
    let total = rows
        .checked_mul(core::mem::size_of::<OaCsvRow>() as u64)
        .and_then(|n| n.checked_add(fields.checked_mul(core::mem::size_of::<OaCsvField>() as u64)?))
        .and_then(|n| n.checked_add(value_bytes))
        .ok_or_else(|| AdapterFailure::resource(MODULE_CSV, [rows, fields, value_bytes, 0, 0]))?;
    if total > MAX_FLAT_OUTPUT_BYTES {
        return Err(AdapterFailure::resource(
            MODULE_CSV,
            [rows, fields, value_bytes, 0, 0],
        ));
    }
    Ok(OaCsvCounts {
        rows,
        fields,
        value_bytes,
    })
}

fn fields_and_pool<'a>(
    fields: &mut Vec<OaCsvField>,
    pool: &mut Vec<u8>,
    values: impl Iterator<Item = &'a str>,
) -> Result<(), AdapterFailure> {
    for value in values {
        push_csv_value(fields, pool, value)?;
    }
    Ok(())
}

fn push_csv_value(
    fields: &mut Vec<OaCsvField>,
    pool: &mut Vec<u8>,
    value: &str,
) -> Result<(), AdapterFailure> {
    let counts = csv_counts(fields.len(), fields.len(), pool.len());
    let offset =
        u32::try_from(pool.len()).map_err(|_| AdapterFailure::resource(MODULE_CSV, counts))?;
    let length =
        u32::try_from(value.len()).map_err(|_| AdapterFailure::resource(MODULE_CSV, counts))?;
    pool.extend_from_slice(value.as_bytes());
    fields.push(OaCsvField {
        value_offset: offset,
        value_length: length,
    });
    Ok(())
}

fn csv_counts(rows: usize, fields: usize, bytes: usize) -> [u64; 5] {
    [rows as u64, fields as u64, bytes as u64, 0, 0]
}

fn csv_count_array(counts: OaCsvCounts) -> [u64; 5] {
    [counts.rows, counts.fields, counts.value_bytes, 0, 0]
}

fn xml_count_array(counts: OaXmlCounts) -> [u64; 5] {
    [
        counts.elements,
        counts.attributes,
        counts.text_nodes,
        counts.namespaces,
        counts.namespace_uri_bytes,
    ]
}

fn xml_span(span: oa_xml::Span) -> OaXmlSpan {
    OaXmlSpan {
        start: span.start,
        length: span.len,
    }
}

fn json_kind(kind: JsonKind) -> u32 {
    match kind {
        JsonKind::Null => 1,
        JsonKind::False => 2,
        JsonKind::True => 3,
        JsonKind::Number => 4,
        JsonKind::String | JsonKind::StringPooled => 5,
        JsonKind::Array => 6,
        JsonKind::Object => 7,
    }
}

fn json_error_kind(kind: JsonErrorKind) -> u32 {
    match kind {
        JsonErrorKind::InputTooLarge => 1,
        JsonErrorKind::InvalidUtf8 => 2,
        JsonErrorKind::DepthExceeded => 3,
        JsonErrorKind::TooManyNodes => 4,
        JsonErrorKind::StringTooLong => 5,
        JsonErrorKind::UnexpectedEnd => 6,
        JsonErrorKind::UnexpectedByte => 7,
        JsonErrorKind::BadNumber => 8,
        JsonErrorKind::BadEscape => 9,
        JsonErrorKind::ControlInString => 10,
        JsonErrorKind::TrailingGarbage => 11,
    }
}

fn xml_failure(error: oa_xml::XmlError, counts: [u64; 5]) -> AdapterFailure {
    let (kind, position, detail_a) = match error {
        oa_xml::XmlError::Limit { limit, at } => (1, at, limit_code(limit)),
        oa_xml::XmlError::Utf8 { at } => (2, at, 0),
        oa_xml::XmlError::BadChar { at } => (3, at, 0),
        oa_xml::XmlError::Doctype { at } => (4, at, 0),
        oa_xml::XmlError::ProcessingInstruction { at } => (5, at, 0),
        oa_xml::XmlError::BadXmlDecl { at } => (6, at, 0),
        oa_xml::XmlError::UnsupportedVersion { at } => (7, at, 0),
        oa_xml::XmlError::UnsupportedEncoding { at } => (8, at, 0),
        oa_xml::XmlError::UndefinedEntity { at } => (9, at, 0),
        oa_xml::XmlError::BadEntity { at } => (10, at, 0),
        oa_xml::XmlError::MismatchedTag { at } => (11, at, 0),
        oa_xml::XmlError::Unclosed { at } => (12, at, 0),
        oa_xml::XmlError::UnexpectedEof { at } => (13, at, 0),
        oa_xml::XmlError::DuplicateAttribute { at } => (14, at, 0),
        oa_xml::XmlError::UndeclaredPrefix { at } => (15, at, 0),
        oa_xml::XmlError::NoRoot { at } => (16, at, 0),
        oa_xml::XmlError::OutsideRoot { at } => (17, at, 0),
        oa_xml::XmlError::Syntax { at, .. } => (18, at, 0),
        oa_xml::XmlError::OutputTooSmall { at } => (19, at, 0),
        oa_xml::XmlError::SpanOutOfRange { at } => (20, at, 0),
        oa_xml::XmlError::LimitsInvalid { limit, at } => (21, at, limit_code(limit)),
    };
    AdapterFailure::parser(
        OaTextError {
            module: MODULE_XML,
            kind,
            position: position as u64,
            detail_a,
            detail_b: 0,
        },
        counts,
    )
}

fn csv_failure(error: oa_csv::CsvError, counts: [u64; 5]) -> AdapterFailure {
    let (kind, detail_a, detail_b) = match error.kind {
        CsvErrorKind::Io(_) => (1, 0, 0),
        CsvErrorKind::MissingHeader => (2, 0, 0),
        CsvErrorKind::EmptyHeaderName { index } => (3, index as u64, 0),
        CsvErrorKind::DuplicateHeader(_) => (4, 0, 0),
        CsvErrorKind::MissingColumn(_) => (5, 0, 0),
        CsvErrorKind::FieldCountMismatch { expected, found } => (6, expected as u64, found as u64),
        CsvErrorKind::QuoteInUnquotedField => (7, 0, 0),
        CsvErrorKind::CharacterAfterClosingQuote => (8, 0, 0),
        CsvErrorKind::BareCarriageReturn => (9, 0, 0),
        CsvErrorKind::UnterminatedQuote => (10, 0, 0),
        CsvErrorKind::InvalidUtf8 => (11, 0, 0),
        CsvErrorKind::LimitExceeded { max, .. } => (12, max as u64, 0),
    };
    AdapterFailure::parser(
        OaTextError {
            module: MODULE_CSV,
            kind,
            position: error.line,
            detail_a,
            detail_b,
        },
        counts,
    )
}

fn limit_code(kind: LimitKind) -> u64 {
    match kind {
        LimitKind::InputBytes => 1,
        LimitKind::Depth => 2,
        LimitKind::Elements => 3,
        LimitKind::Attributes => 4,
        LimitKind::AttributesPerElement => 5,
        LimitKind::NameLength => 6,
        LimitKind::TextNodes => 7,
        LimitKind::NsBindings => 8,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn json_flat_nodes_and_lexical_numbers_are_stable() {
        let flat = parse_json(br#"{"a":[1,"x"]}"#).expect("synthetic JSON parses");
        assert_eq!(flat.nodes.len(), 5);
        assert_eq!(flat.nodes[0].kind, 7);
        assert_eq!(flat.nodes[0].child_count, 2);
        assert_eq!(flat.nodes[0].subtree_end, 5);
        assert_eq!(flat.nodes[1].kind, 5);
        assert_eq!(flat.nodes[2].kind, 6);
        assert_eq!(flat.nodes[2].child_count, 2);
        assert_eq!(flat.nodes[3].kind, 4);
        assert_eq!(flat.nodes[4].kind, 5);
        assert_eq!(flat.pool, b"a1x");
    }

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn json_refusal_preserves_parser_kind_and_position() {
        let failure = parse_json(br#"{"x":"\q"}"#)
            .err()
            .expect("bad JSON escape refuses");
        assert_eq!(failure.kind, FailureKind::Parser);
        assert_eq!(failure.parser.module, MODULE_JSON);
        assert_eq!(failure.parser.kind, 9);
        assert_eq!(failure.parser.position, 7);
    }

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn xml_flat_spans_namespaces_and_decoded_values_are_preserved() {
        let input = b"<r xmlns='urn:x'><x a='&amp;'>v&amp;</x></r>";
        let flat = parse_xml(input).expect("synthetic XML parses");
        assert_eq!(flat.elements.len(), 2);
        // Namespace declarations remain visible as raw attributes as well as namespace records.
        assert_eq!(flat.attributes.len(), 2);
        assert_eq!(flat.texts.len(), 1);
        assert_eq!(flat.namespaces.len(), 1);
        assert_eq!(flat.namespace_uris, b"urn:x");
        let attr = flat
            .attributes
            .get(1)
            .expect("entity-valued attribute exists");
        assert_eq!(attr.flags, 1);
        let text = flat.texts.first().expect("text node exists");
        assert_eq!(text.flags, 1);
        let mut decoded = [0; 3];
        let written = decode_xml_span(
            input,
            text.raw_value.start,
            text.raw_value.length,
            1,
            &mut decoded,
        )
        .expect("XML text span decodes");
        assert_eq!(&decoded[..written], b"v&");
    }

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn xml_doctype_refusal_has_parser_error() {
        let failure = parse_xml(b"<!DOCTYPE r><r/>")
            .err()
            .expect("DTD is unsupported");
        assert_eq!(failure.kind, FailureKind::Parser);
        assert_eq!(failure.parser.module, MODULE_XML);
        assert_eq!(failure.parser.kind, 4);
    }

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn csv_rows_include_header_and_quote_decoding() {
        let flat = parse_csv(b"id,name\r\n1,\"A,B\"\r\n").expect("synthetic CSV parses");
        assert_eq!(flat.rows.len(), 2);
        assert_eq!(flat.rows.first().expect("header row exists").field_count, 2);
        assert_eq!(flat.rows.get(1).expect("data row exists").physical_line, 2);
        assert_eq!(flat.fields.len(), 4);
        assert_eq!(flat.pool, b"idname1A,B");
    }

    // REQ: OA-FFI-001, OA-FFI-002
    #[test]
    fn csv_refusal_preserves_physical_line_and_kind() {
        let failure = parse_csv(b"id,name\n1,\"bad\n")
            .err()
            .expect("unclosed quote refuses");
        assert_eq!(failure.kind, FailureKind::Parser);
        assert_eq!(failure.parser.module, MODULE_CSV);
        assert_eq!(failure.parser.kind, 10);
        assert_eq!(failure.parser.position, 2);
        assert_eq!(failure.counts[0], 1);
    }
}
