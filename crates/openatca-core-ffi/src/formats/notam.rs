//! Bounded FAA FNS AIXM 5.1 profile adaptation over `oa-notam`.
//!
//! Safe adapter only. XML parsing, field interpretation, nil/error semantics and geometry all
//! remain in the existing decoder. The C layer owns pointer validation and copies the completed
//! result only after every caller capacity has been checked.
#![forbid(unsafe_code)]

use oa_notam::{
    decode_bytes, feature_slices_len, new_decoder, new_notams_with_feature_capacity, new_stats,
    DocError, FeatureSlice, Limits as NotamLimits, Notams, Row, Stats, DEFAULT_FEATURES_PER_ROW,
};
use oa_time_window::DropWindow;
use oa_xml::{Limits as XmlLimits, Parser, XmlError};

pub const MAX_INPUT_BYTES: usize = 1 << 20;
pub const MAX_UNKNOWN_PATHS: usize = 128;
pub const MAX_WORKING_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 8192;
pub const MAX_POLYGON_VERTICES: usize = 4096;

pub const STATUS_OK: u32 = 0;
pub const STATUS_INVALID_ARGUMENT: u32 = 1;
pub const STATUS_BUFFER_TOO_SMALL: u32 = 6;
pub const STATUS_INTERNAL_PANIC: u32 = 7;
pub const STATUS_ALLOCATION_FAILED: u32 = 11;
pub const STATUS_INPUT_LIMIT: u32 = 42;
pub const STATUS_XML_REFUSAL: u32 = 43;
pub const STATUS_DOCUMENT_REFUSAL: u32 = 44;
pub const STATUS_ELEMENT_LIMIT: u32 = 45;
pub const STATUS_CAPACITY: u32 = 46;
pub const STATUS_SEALED_DROP: u32 = 47;
pub const STATUS_MEMORY_BUDGET: u32 = 48;
pub const OA_STATUS_NOTAM_INPUT_LIMIT: u32 = STATUS_INPUT_LIMIT;
pub const OA_STATUS_NOTAM_XML_REFUSAL: u32 = STATUS_XML_REFUSAL;
pub const OA_STATUS_NOTAM_DOCUMENT_REFUSAL: u32 = STATUS_DOCUMENT_REFUSAL;
pub const OA_STATUS_NOTAM_ELEMENT_LIMIT: u32 = STATUS_ELEMENT_LIMIT;
pub const OA_STATUS_NOTAM_CAPACITY: u32 = STATUS_CAPACITY;
pub const OA_STATUS_NOTAM_SEALED_DROP: u32 = STATUS_SEALED_DROP;
pub const OA_STATUS_NOTAM_MEMORY_BUDGET: u32 = STATUS_MEMORY_BUDGET;

/// One completed FNS event row. Layout is identical to the existing 272-byte `oa-notam::Row`.
pub type OaNotamRow = Row;

/// One feature time-slice with explicit kind, source-presence and error bits.
pub type OaNotamFeature = FeatureSlice;

/// Polygon span into the separate `OaNotamVertex` output array.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaNotamPolygon {
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub feature_index: u32,
    pub kind: u8,
    pub reserved: [u8; 3],
}
const _: () = assert!(core::mem::size_of::<OaNotamPolygon>() == 16);
const _: () = assert!(core::mem::align_of::<OaNotamPolygon>() == 4);

/// One WGS-84 polygon vertex, latitude and longitude in signed 1e-7 degrees.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OaNotamVertex {
    pub lat_e7: i32,
    pub lon_e7: i32,
}
const _: () = assert!(core::mem::size_of::<OaNotamVertex>() == 8);

/// One bounded unknown XML path and its count.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OaNotamUnknownPath {
    pub hash: u64,
    pub count: u64,
    pub is_attribute: u8,
    pub text_len: u8,
    pub reserved: [u8; 6],
    pub text: [u8; oa_notam::MAX_PATH_TEXT],
}
const _: () = assert!(core::mem::size_of::<OaNotamUnknownPath>() == 184);

impl Default for OaNotamUnknownPath {
    fn default() -> Self {
        Self {
            hash: 0,
            count: 0,
            is_attribute: 0,
            text_len: 0,
            reserved: [0; 6],
            text: [0; oa_notam::MAX_PATH_TEXT],
        }
    }
}

/// Counts and refusal detail for a single FNS document.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OaNotamSummary {
    pub status: u32,
    /// Decoder whole-document refusal class; 0 means no whole-document refusal.
    pub refusal_kind: u32,
    /// XML parser byte offset, or `u64::MAX` when the refusal has no source offset.
    pub source_offset: u64,
    pub row_count: u64,
    pub feature_count: u64,
    pub polygon_count: u64,
    pub vertex_count: u64,
    pub string_bytes: u64,
    pub unknown_path_count: u64,
    pub unknown_elements: u64,
    pub unknown_attributes: u64,
    pub unknown_overflow: u64,
    pub sealed_dropped: u64,
    pub field_errors: [u64; oa_notam::FIELD_COUNT],
    pub rejects: [u64; oa_notam::REJECT_SLOTS],
    pub features: [u64; oa_notam::FEAT_COUNT],
}
const _: () = assert!(core::mem::size_of::<OaNotamSummary>() == 512);

impl Default for OaNotamSummary {
    fn default() -> Self {
        Self {
            status: STATUS_OK,
            refusal_kind: 0,
            source_offset: u64::MAX,
            row_count: 0,
            feature_count: 0,
            polygon_count: 0,
            vertex_count: 0,
            string_bytes: 0,
            unknown_path_count: 0,
            unknown_elements: 0,
            unknown_attributes: 0,
            unknown_overflow: 0,
            sealed_dropped: 0,
            field_errors: [0; oa_notam::FIELD_COUNT],
            rejects: [0; oa_notam::REJECT_SLOTS],
            features: [0; oa_notam::FEAT_COUNT],
        }
    }
}

/// Prepared result. The raw-pointer module reads this only after capacity preflight.
pub(crate) struct PreparedNotam {
    pub(crate) summary: OaNotamSummary,
    pub(crate) data: Notams,
    pub(crate) stats: Stats,
}

/// Compute a conservative ceiling for all parser, decoder and output allocations before any of
/// them are constructed. The 16 MiB allowance includes 1 MiB for fixed tables and allocator
/// rounding; vertices are bounded by one third of XML bytes because a retained pair needs at
/// least the three source bytes `0 0`.
pub fn estimated_working_bytes(input_len: usize) -> Option<usize> {
    if input_len > MAX_INPUT_BYTES {
        return None;
    }
    let xml = XmlLimits::default();
    let span = core::mem::size_of::<oa_xml::Span>();
    let elem_ix = core::mem::size_of::<oa_xml::ElemIx>();
    let attr_ix = core::mem::size_of::<oa_xml::AttrIx>();
    let ns_ix = core::mem::size_of::<oa_xml::NsIx>();
    let bit_words = |count: usize| count.div_ceil(64).checked_mul(core::mem::size_of::<u64>());

    let element_tables = xml.max_elements.checked_mul(
        span.checked_mul(2)?
            .checked_add(elem_ix.checked_mul(3)?)?
            .checked_add(attr_ix)?
            .checked_add(core::mem::size_of::<u32>())?
            .checked_add(ns_ix)?,
    )?;
    let attribute_tables = xml
        .max_attributes
        .checked_mul(span.checked_mul(5)?.checked_add(ns_ix)?)?
        .checked_add(bit_words(xml.max_attributes)?)?
        .checked_add(xml.max_attrs_per_elem.checked_mul(span)?)?;
    let text_tables = xml
        .max_text_nodes
        .checked_mul(span.checked_add(elem_ix)?)?
        .checked_add(bit_words(xml.max_text_nodes)?)?;
    let ns_buffer = xml.max_ns_bindings.checked_mul(2048)?;
    let parser_slack = 256 * 1024;
    let parser_bytes = element_tables
        .checked_add(attribute_tables)?
        .checked_add(text_tables)?
        .checked_add(ns_buffer)?
        .checked_add(parser_slack)?;
    let decoder_bytes = xml.max_elements.checked_mul(18)?.checked_add(256 * 1024)?;
    let features = xml.max_elements.checked_mul(48)?;
    let polygons = xml.max_elements.checked_mul(16)?;
    let vertices = (input_len / 3).checked_mul(core::mem::size_of::<OaNotamVertex>())?;
    let result_bytes = features
        .checked_add(polygons)?
        .checked_add(vertices)?
        .checked_add(2 * MAX_TEXT_BYTES)?
        .checked_add(MAX_UNKNOWN_PATHS.checked_mul(core::mem::size_of::<OaNotamUnknownPath>())?)?
        .checked_add(core::mem::size_of::<OaNotamRow>())?
        .checked_add(512 * 1024)?;
    parser_bytes
        .checked_add(decoder_bytes)?
        .checked_add(result_bytes)
}

pub(crate) fn prepare(input: &[u8], drop_from_ms: i64, drop_to_ms: i64) -> PreparedNotam {
    let mut summary = OaNotamSummary::default();
    if drop_from_ms > drop_to_ms {
        summary.status = STATUS_INVALID_ARGUMENT;
        summary.refusal_kind = 18;
        return empty_prepared(summary);
    }
    if input.len() > MAX_INPUT_BYTES {
        summary.status = STATUS_INPUT_LIMIT;
        return empty_prepared(summary);
    }
    let Some(working_bytes) = estimated_working_bytes(input.len()) else {
        summary.status = STATUS_MEMORY_BUDGET;
        return empty_prepared(summary);
    };
    if working_bytes > MAX_WORKING_BYTES {
        summary.status = STATUS_MEMORY_BUDGET;
        return empty_prepared(summary);
    }

    let parser = match Parser::new(XmlLimits::default()) {
        Ok(value) => value,
        Err(_) => {
            summary.status = STATUS_ALLOCATION_FAILED;
            return empty_prepared(summary);
        }
    };
    let mut parser = parser;
    let mut limits = NotamLimits::new(DropWindow::new(drop_from_ms, drop_to_ms));
    limits.max_poly_vertices = MAX_POLYGON_VERTICES;
    let mut decoder = new_decoder(limits);
    let vertex_capacity = input.len() / 3;
    let mut data = new_notams_with_feature_capacity(
        1,
        MAX_TEXT_BYTES * 2,
        XmlLimits::default().max_elements,
        vertex_capacity,
        XmlLimits::default().max_elements,
    );
    let mut stats = new_stats(MAX_UNKNOWN_PATHS);
    match decode_bytes(&mut parser, &mut decoder, input, &mut data, &mut stats) {
        Ok(outcome) if outcome.sealed_dropped => {
            summary.status = STATUS_SEALED_DROP;
            summary.sealed_dropped = 1;
        }
        Ok(outcome) => {
            summary.row_count = u64::from(data.len as u32);
            summary.feature_count = feature_slices_len(&data) as u64;
            summary.polygon_count = data.poly_vlen.len() as u64;
            summary.vertex_count = data.vtx_lat.len() as u64;
            summary.string_bytes = data.strings_used as u64;
            summary.refusal_kind = 0;
            let _ = outcome;
        }
        Err(error) => {
            let (status, kind, offset) = refusal(&error);
            summary.status = status;
            summary.refusal_kind = kind;
            summary.source_offset = offset;
        }
    }
    summary.unknown_path_count = stats.unknown.len() as u64;
    summary.unknown_elements = stats.unknown_elements;
    summary.unknown_attributes = stats.unknown_attributes;
    summary.unknown_overflow = stats.unknown_overflow;
    summary.field_errors = stats.field_errors;
    summary.rejects = stats.rejects;
    summary.features = stats.features;
    PreparedNotam {
        summary,
        data,
        stats,
    }
}

fn empty_prepared(summary: OaNotamSummary) -> PreparedNotam {
    PreparedNotam {
        summary,
        data: new_notams_with_feature_capacity(1, 0, 0, 0, 0),
        stats: new_stats(0),
    }
}

fn refusal(error: &DocError) -> (u32, u32, u64) {
    match error {
        DocError::Xml(xml) => (STATUS_XML_REFUSAL, 1, xml_offset(xml)),
        DocError::Rejected(reason) => (STATUS_DOCUMENT_REFUSAL, 2 + *reason as u32, u64::MAX),
        DocError::TooManyElements { .. } => (STATUS_ELEMENT_LIMIT, 16, u64::MAX),
        DocError::Capacity(kind) => (STATUS_CAPACITY, 17 + *kind as u32, u64::MAX),
    }
}

fn xml_offset(error: &XmlError) -> u64 {
    match error {
        XmlError::Limit { at, .. }
        | XmlError::Utf8 { at }
        | XmlError::BadChar { at }
        | XmlError::Doctype { at }
        | XmlError::ProcessingInstruction { at }
        | XmlError::BadXmlDecl { at }
        | XmlError::UnsupportedVersion { at }
        | XmlError::UnsupportedEncoding { at }
        | XmlError::UndefinedEntity { at }
        | XmlError::BadEntity { at }
        | XmlError::MismatchedTag { at }
        | XmlError::Unclosed { at }
        | XmlError::UnexpectedEof { at }
        | XmlError::DuplicateAttribute { at }
        | XmlError::UndeclaredPrefix { at }
        | XmlError::NoRoot { at }
        | XmlError::OutsideRoot { at }
        | XmlError::Syntax { at, .. }
        | XmlError::OutputTooSmall { at }
        | XmlError::SpanOutOfRange { at }
        | XmlError::LimitsInvalid { at, .. } => u64::from(*at),
    }
}

pub(crate) fn row(data: &Notams) -> Option<OaNotamRow> {
    oa_notam::row_at(data, 0)
}

pub(crate) fn feature(data: &Notams, index: usize) -> Option<OaNotamFeature> {
    oa_notam::feature_at(data, index)
}

pub(crate) fn polygon(data: &Notams, index: usize) -> Option<OaNotamPolygon> {
    Some(OaNotamPolygon {
        first_vertex: *data.poly_vstart.get(index)?,
        vertex_count: *data.poly_vlen.get(index)?,
        feature_index: *data.poly_feature.get(index)?,
        kind: *data.poly_kind.get(index)?,
        reserved: [0; 3],
    })
}

pub(crate) fn vertex(data: &Notams, index: usize) -> Option<OaNotamVertex> {
    Some(OaNotamVertex {
        lat_e7: *data.vtx_lat.get(index)?,
        lon_e7: *data.vtx_lon.get(index)?,
    })
}

pub(crate) fn unknown(stats: &Stats, index: usize) -> Option<OaNotamUnknownPath> {
    let value = stats.unknown.get(index)?;
    Some(OaNotamUnknownPath {
        hash: value.hash,
        count: value.count,
        is_attribute: u8::from(value.is_attr),
        text_len: value.len,
        reserved: [0; 6],
        text: value.text,
    })
}

const _: () = assert!(DEFAULT_FEATURES_PER_ROW == 16);
