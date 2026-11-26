#![deny(unsafe_code)]
//! Provisional C ABI shell for selected existing `oa-*` L0 operations, including bounded text parsing.
//!
//! This is an experimental binding boundary, not a stable API. The only unsafe Rust is the
//! pointer adaptation in `ffi.rs`; parser, math, integrity, and geodesy operations remain in their
//! existing `oa-*` crates, with safe flat-output conversion in `text.rs`.

#[cfg(panic = "abort")]
compile_error!("the provisional C ABI requires panic=unwind for catch_unwind containment");

mod ffi;
mod text;

pub use ffi::{
    oa_core_abi_version, oa_core_error_message, oa_csv_parse, oa_geo_polygon_contains_wgs84,
    oa_geo_polygon_edge_count, oa_geo_polygon_free, oa_geo_polygon_new_wgs84, oa_geo_wgs84_direct,
    oa_geo_wgs84_geodesic_rhumb_separation, oa_geo_wgs84_inverse, oa_geo_wgs84_rhumb_direct,
    oa_geo_wgs84_rhumb_inverse, oa_geo_wgs84_rhumb_point_at_fraction, oa_json_parse,
    oa_math_f32_binary, oa_math_f32_compare, oa_math_f32_is_sign_negative, oa_math_f32_ternary,
    oa_math_f32_unary, oa_math_f64_binary, oa_math_f64_compare, oa_math_f64_is_sign_negative,
    oa_math_f64_powi, oa_math_f64_sin_cos, oa_math_f64_ternary, oa_math_f64_unary,
    oa_sha256_digest, oa_sha256_hex, oa_xml_decode_span, oa_xml_parse, OaErrorInfo, OaPolygon,
};
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
