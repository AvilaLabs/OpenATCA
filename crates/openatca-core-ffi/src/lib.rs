#![deny(unsafe_code)]
//! Provisional C ABI shell for selected existing `oa-*` L0 operations, including bounded
//! compression, archive metadata, and text parsing.
//!
//! This is an experimental binding boundary, not a stable API. The only unsafe Rust is the
//! pointer adaptation in `ffi.rs`; parser, math, integrity, and geodesy operations remain in their
//! existing `oa-*` crates, with safe text and ASTERIX projection adapters.

#[cfg(panic = "abort")]
compile_error!("the provisional C ABI requires panic=unwind for catch_unwind containment");

mod ffi;
mod formats;
mod text;

pub use ffi::{
    oa_asterix_decode_block, oa_cifp_parse, oa_core_abi_version, oa_core_error_message,
    oa_csv_parse, oa_deflate_raw_compress, oa_deflate_raw_inflate, oa_geo_polygon_contains_wgs84,
    oa_geo_polygon_edge_count, oa_geo_polygon_free, oa_geo_polygon_new_wgs84, oa_geo_wgs84_direct,
    oa_geo_wgs84_geodesic_rhumb_separation, oa_geo_wgs84_inverse, oa_geo_wgs84_rhumb_direct,
    oa_geo_wgs84_rhumb_inverse, oa_geo_wgs84_rhumb_point_at_fraction, oa_grib2_decode_message,
    oa_grib2_inspect_message, oa_json_parse, oa_math_f32_binary, oa_math_f32_compare,
    oa_math_f32_is_sign_negative, oa_math_f32_ternary, oa_math_f32_unary, oa_math_f64_binary,
    oa_math_f64_compare, oa_math_f64_is_sign_negative, oa_math_f64_powi, oa_math_f64_sin_cos,
    oa_math_f64_ternary, oa_math_f64_unary, oa_notam_fns_decode, oa_sha256_digest, oa_sha256_hex,
    oa_tar_parse, oa_xml_decode_span, oa_xml_parse, oa_zip_extract_entry, oa_zip_list,
    OaCifpBuffers, OaErrorInfo, OaNotamBuffers, OaPolygon, OaTarEntry, OA_STATUS_BUFFER_TOO_SMALL,
    OA_STATUS_INVALID_ARGUMENT,
};
pub use formats::asterix::{
    OaAsterixCounts, OaAsterixError, OaAsterixPosition, OaAsterixRecord, OaAsterixVertical,
};
pub use formats::cifp::{
    OaCifpAirport, OaCifpAirspace, OaCifpAirway, OaCifpAirwayFix, OaCifpAltitude, OaCifpBoundary,
    OaCifpContinuation, OaCifpCounts, OaCifpCycleCount, OaCifpError, OaCifpFixRef, OaCifpHeader,
    OaCifpHeaderText, OaCifpLeg, OaCifpLevelOfService, OaCifpLocalizer, OaCifpMeta, OaCifpMora,
    OaCifpNdb, OaCifpPosition, OaCifpProcedure, OaCifpRoute, OaCifpRunway, OaCifpSectionCount,
    OaCifpSpan, OaCifpVhfNavaid, OaCifpWaypoint,
};
pub use formats::grib2::{OaGrib2Field, OaGrib2Time};
pub use formats::notam::{
    OaNotamFeature, OaNotamPolygon, OaNotamRow, OaNotamSummary, OaNotamUnknownPath, OaNotamVertex,
    OA_STATUS_NOTAM_CAPACITY, OA_STATUS_NOTAM_DOCUMENT_REFUSAL, OA_STATUS_NOTAM_ELEMENT_LIMIT,
    OA_STATUS_NOTAM_INPUT_LIMIT, OA_STATUS_NOTAM_MEMORY_BUDGET, OA_STATUS_NOTAM_SEALED_DROP,
    OA_STATUS_NOTAM_XML_REFUSAL,
};
pub use formats::zip::OaZipEntry;
pub use text::{
    OaCsvCounts, OaCsvField, OaCsvRow, OaJsonCounts, OaJsonNode, OaTextError, OaXmlAttribute,
    OaXmlCounts, OaXmlElement, OaXmlNamespace, OaXmlSpan, OaXmlText,
};

#[cfg(test)]
mod tests {
    use super::OaErrorInfo;

    // REQ: OA-FFI-001
    #[test]
    fn provisional_header_matches_fixed_error_record_layout() {
        assert_eq!(core::mem::size_of::<OaErrorInfo>(), 16);
        assert_eq!(core::mem::align_of::<OaErrorInfo>(), 8);
    }
}
