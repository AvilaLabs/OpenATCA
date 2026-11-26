/*
 * PROVISIONAL / PREVIEW / NOT VALIDATED.
 *
 * This hand-written header describes the implemented provisional L0 ABI. Names,
 * status assignments and layout are experimental and not frozen; do not treat it as a stable or
 * published API before independent ABI review.
 *
 * Pointer preconditions: callers provide valid, correctly aligned, readable input regions and
 * writable output locations for the duration of the call. Memory regions used by a call (including
 * input, output, error and message storage) must not overlap unless a function explicitly says
 * otherwise. Inputs are borrowed only for the call and are never retained; a returned polygon
 * handle owns its prepared data and must be freed exactly once. Null required pointers are
 * rejected. The library cannot validate arbitrary C addresses or prove the caller's capacity
 * claim. Invalid caller memory can fault or invoke undefined behavior; panic containment cannot
 * make invalid pointers safe.
 */
#ifndef OPENATCA_H
#define OPENATCA_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OA_CORE_ABI_VERSION_PROVISIONAL UINT32_C(1)

/* Provisional values; status 12 is the bounded text-parser refusal. */
typedef uint32_t OaStatus;
enum {
    OA_STATUS_OK = 0,
    OA_STATUS_INVALID_ARGUMENT = 1,
    OA_STATUS_NON_FINITE = 2,
    OA_STATUS_LATITUDE_OUT_OF_RANGE = 3,
    OA_STATUS_LONGITUDE_OUT_OF_RANGE = 4,
    OA_STATUS_NO_CONVERGENCE = 5,
    OA_STATUS_BUFFER_TOO_SMALL = 6,
    OA_STATUS_INTERNAL_PANIC = 7,
    OA_STATUS_BAD_PARAMETER = 8,
    OA_STATUS_LENGTH_TOO_LARGE = 9,
    OA_STATUS_BAD_OPERATION = 10,
    OA_STATUS_ALLOCATION_FAILED = 11,
    OA_STATUS_PARSE_ERROR = 12,
    OA_STATUS_DEFLATE_ERROR = 13,
    OA_STATUS_DEFLATE_TRUNCATED = 14,
    OA_STATUS_DEFLATE_OUTPUT_LIMIT = 15,
    OA_STATUS_DEFLATE_RATIO_LIMIT = 16
};

#define OA_DEFLATE_MAX_INPUT_BYTES UINT64_C(67108864)
#define OA_DEFLATE_MAX_OUTPUT_BYTES UINT64_C(268435456)

typedef uint32_t OaLocationKind;
enum {
    OA_LOCATION_NONE = 0,
    OA_LOCATION_ARGUMENT = 1,
    OA_LOCATION_ELEMENT = 2
};

/* Fixed-width proposed layout: 16 bytes, 8-byte alignment on supported 64-bit targets. */
typedef struct OaErrorInfo {
    uint32_t code;          /* Must equal the returned OaStatus. */
    uint32_t location_kind; /* NONE, ARGUMENT, or ELEMENT. */
    uint64_t location;      /* Zero-based argument/element index; raw value is preserved. */
} OaErrorInfo;

enum { OA_TEXT_XML = 1, OA_TEXT_JSON = 2, OA_TEXT_CSV = 3 };
/* Parser kind is module-specific; offsets are byte positions except CSV's physical line. */
typedef struct OaTextError {
    uint32_t module;
    uint32_t kind;
    uint64_t position;
    uint64_t detail_a;
    uint64_t detail_b;
} OaTextError;
typedef struct OaJsonNode {
    uint32_t kind, first_child, child_count, subtree_end, value_offset, value_length;
} OaJsonNode;
typedef struct OaJsonCounts { uint64_t node_count, value_bytes; } OaJsonCounts;
typedef struct OaXmlSpan { uint32_t start, length; } OaXmlSpan;
typedef struct OaXmlElement {
    OaXmlSpan name, prefix;
    uint32_t parent, first_attribute, attribute_count, first_child, next_sibling, namespace_head;
} OaXmlElement;
typedef struct OaXmlAttribute {
    OaXmlSpan name, prefix, raw_value;
    uint32_t owner_element, flags;
} OaXmlAttribute;
typedef struct OaXmlText { OaXmlSpan raw_value; uint32_t owner_element, flags; } OaXmlText;
typedef struct OaXmlNamespace {
    uint32_t owner_element;
    OaXmlSpan prefix;
    uint32_t uri_offset, uri_length, next_for_element;
} OaXmlNamespace;
typedef struct OaXmlCounts {
    uint64_t elements, attributes, text_nodes, namespaces, namespace_uri_bytes;
} OaXmlCounts;
typedef struct OaCsvRow { uint64_t physical_line; uint32_t first_field, field_count; } OaCsvRow;
typedef struct OaCsvField { uint32_t value_offset, value_length; } OaCsvField;
typedef struct OaCsvCounts { uint64_t rows, fields, value_bytes; } OaCsvCounts;

enum {
    OA_JSON_NULL = 1, OA_JSON_FALSE = 2, OA_JSON_TRUE = 3, OA_JSON_NUMBER = 4,
    OA_JSON_STRING = 5, OA_JSON_ARRAY = 6, OA_JSON_OBJECT = 7
};
enum { OA_XML_SPAN_TEXT = 1, OA_XML_SPAN_ATTRIBUTE_VALUE = 2 };

/*
 * Stateless WGS-84 operations. Angles are degrees; distance is metres. Azimuth is clockwise
 * from north, in (-180, 180] on output. Outputs are written only on success. Output addresses,
 * including `error`, must be non-null, writable and pairwise non-overlapping. The exact status
 * mapping remains provisional pending API review.
 */
uint32_t oa_core_abi_version(void);

uint32_t oa_geo_wgs84_inverse(
    double lat1_deg,
    double lon1_deg,
    double lat2_deg,
    double lon2_deg,
    double *distance_m,
    double *azimuth1_deg,
    double *azimuth2_deg,
    OaErrorInfo *error);

uint32_t oa_geo_wgs84_direct(
    double lat1_deg,
    double lon1_deg,
    double azimuth1_deg,
    double distance_m,
    double *lat2_deg,
    double *lon2_deg,
    double *azimuth2_deg,
    OaErrorInfo *error);

typedef struct OaPolygon OaPolygon;

/* Operation codes for the source-defined scalar operation groups below. */
enum {
    OA_MATH_U_SIN = 1, OA_MATH_U_COS = 2, OA_MATH_U_TAN = 3,
    OA_MATH_U_ATAN = 4, OA_MATH_U_ASIN = 5, OA_MATH_U_ACOS = 6,
    OA_MATH_U_EXP = 7, OA_MATH_U_LN = 8, OA_MATH_U_CBRT = 9,
    OA_MATH_U_CANON_NAN = 10, OA_MATH_U_SQRT = 11,
    OA_MATH_U_FLOOR = 12, OA_MATH_U_CEIL = 13, OA_MATH_U_TRUNC = 14,
    OA_MATH_U_ROUND = 15, OA_MATH_U_SIGNUM_F64 = 16
};
enum {
    OA_MATH_B_ATAN2 = 1, OA_MATH_B_POW = 2, OA_MATH_B_HYPOT = 3,
    OA_MATH_B_REM_EUCLID = 4, OA_MATH_B_MIN_F64 = 5,
    OA_MATH_B_MAX_F64 = 6, OA_MATH_B_COPYSIGN_F64 = 7
};
enum { OA_MATH_T_CLAMP_F64 = 1 };
enum { OA_MATH_F32_U_CANON_NAN = 1, OA_MATH_F32_U_SIGNUM = 2 };
enum {
    OA_MATH_F32_B_COPYSIGN = 1, OA_MATH_F32_B_MIN = 2,
    OA_MATH_F32_B_MAX = 3
};
enum { OA_MATH_F32_T_CLAMP = 1 };

/* Math results follow the selected Rust operation, including IEEE NaN/inf/signed-zero behavior. */
uint32_t oa_math_f64_unary(uint32_t operation, double value, double *output, OaErrorInfo *error);
uint32_t oa_math_f64_binary(uint32_t operation, double left, double right, double *output, OaErrorInfo *error);
uint32_t oa_math_f64_ternary(uint32_t operation, double value, double lower, double upper, double *output, OaErrorInfo *error);
uint32_t oa_math_f64_powi(double value, int32_t exponent, double *output, OaErrorInfo *error);
uint32_t oa_math_f64_sin_cos(double value, double *sine, double *cosine, OaErrorInfo *error);
uint32_t oa_math_f64_compare(double left, double right, int32_t *ordering, OaErrorInfo *error);
uint32_t oa_math_f64_is_sign_negative(double value, uint32_t *result, OaErrorInfo *error);
uint32_t oa_math_f32_unary(uint32_t operation, float value, float *output, OaErrorInfo *error);
uint32_t oa_math_f32_binary(uint32_t operation, float left, float right, float *output, OaErrorInfo *error);
uint32_t oa_math_f32_ternary(uint32_t operation, float value, float lower, float upper, float *output, OaErrorInfo *error);
uint32_t oa_math_f32_compare(float left, float right, int32_t *ordering, OaErrorInfo *error);
uint32_t oa_math_f32_is_sign_negative(float value, uint32_t *result, OaErrorInfo *error);

/* WGS-84 rhumb operations; all coordinates/courses are degrees, distances metres. */
uint32_t oa_geo_wgs84_rhumb_inverse(double lat1_deg, double lon1_deg, double lat2_deg, double lon2_deg,
                                    double *distance_m, double *course_deg, OaErrorInfo *error);
uint32_t oa_geo_wgs84_rhumb_direct(double lat1_deg, double lon1_deg, double course_deg, double distance_m,
                                   double *lat2_deg, double *lon2_deg, OaErrorInfo *error);
uint32_t oa_geo_wgs84_rhumb_point_at_fraction(double lat1_deg, double lon1_deg, double lat2_deg, double lon2_deg,
                                              double fraction, double *lat_out_deg, double *lon_out_deg,
                                              OaErrorInfo *error);
uint32_t oa_geo_wgs84_geodesic_rhumb_separation(double lat1_deg, double lon1_deg, double lat2_deg, double lon2_deg,
                                                uint32_t samples, double *max_m, double *at_fraction,
                                                double *geodesic_m, double *rhumb_m, OaErrorInfo *error);

/* Polygon input arrays are borrowed for creation only; the opaque immutable handle owns its prepared edges. */
uint32_t oa_geo_polygon_new_wgs84(const double *latitudes, const double *longitudes, uint64_t count,
                                  OaPolygon **output, OaErrorInfo *error);
uint32_t oa_geo_polygon_contains_wgs84(const OaPolygon *polygon, double latitude, double longitude,
                                       uint32_t *inside, OaErrorInfo *error);
uint32_t oa_geo_polygon_edge_count(const OaPolygon *polygon, uint64_t *output, OaErrorInfo *error);
void oa_geo_polygon_free(OaPolygon *polygon);

/* One-shot integrity digest. Empty input permits a NULL pointer; other pointers have fixed extents. */
uint32_t oa_sha256_digest(const uint8_t *input, uint64_t length, uint8_t digest[32], OaErrorInfo *error);
uint32_t oa_sha256_hex(const uint8_t digest[32], uint8_t output[64], OaErrorInfo *error);

/*
 * Inflates one raw DEFLATE stream with an explicit output limit. Input is capped at 64 MiB,
 * output at 256 MiB, and the existing ZIP 200:1 ratio limit applies above its 1 MiB exemption.
 * Bytes after the final block are not consumed. Output is transactional: malformed, truncated,
 * output-limit, ratio-limit, allocation and invalid-argument failures write no decoded bytes.
 * A zero output capacity with a null output pointer queries the exact output length and consumed
 * input; this returns BUFFER_TOO_SMALL when the stream produces bytes. On that result the exact
 * lengths are written; on success they describe the copied output. Other failures leave length
 * outputs unchanged. Input, output, length records and error must be valid and pairwise disjoint.
 */
uint32_t oa_deflate_raw_inflate(const uint8_t *input, uint64_t input_length,
                                uint64_t max_output, uint8_t *output,
                                uint64_t output_capacity, uint64_t *output_length,
                                uint64_t *consumed, OaErrorInfo *error);

/*
 * Bounded text parsers. Call once with zero capacities to query exact counts; this returns
 * BUFFER_TOO_SMALL without writing arrays. Retry with those capacities. Parser refusal is
 * PARSE_ERROR and reports the source-specific kind/position. Output rows and pools are caller-owned.
 * XML spans point into immutable input; CSV rows include the required header at index zero.
 * Inputs and every output region, including count/error records, must be pairwise non-overlapping.
 */
uint32_t oa_json_parse(const uint8_t *input, uint64_t input_length,
                       OaJsonNode *nodes, uint64_t node_capacity,
                       uint8_t *value_pool, uint64_t value_capacity,
                       OaJsonCounts *counts, OaTextError *parse_error, OaErrorInfo *error);
uint32_t oa_xml_parse(const uint8_t *input, uint64_t input_length,
                      OaXmlElement *elements, uint64_t element_capacity,
                      OaXmlAttribute *attributes, uint64_t attribute_capacity,
                      OaXmlText *texts, uint64_t text_capacity,
                      OaXmlNamespace *namespaces, uint64_t namespace_capacity,
                      uint8_t *namespace_uri_pool, uint64_t namespace_uri_capacity,
                      OaXmlCounts *counts, OaTextError *parse_error, OaErrorInfo *error);
uint32_t oa_xml_decode_span(const uint8_t *input, uint64_t input_length,
                            uint32_t start, uint32_t length, uint32_t span_kind,
                            uint8_t *output, uint64_t output_capacity, uint64_t *written,
                            OaTextError *parse_error, OaErrorInfo *error);
uint32_t oa_csv_parse(const uint8_t *input, uint64_t input_length,
                      OaCsvRow *rows, uint64_t row_capacity,
                      OaCsvField *fields, uint64_t field_capacity,
                      uint8_t *value_pool, uint64_t value_capacity,
                      OaCsvCounts *counts, OaTextError *parse_error, OaErrorInfo *error);

/*
 * Copies a deterministic UTF-8 status message into caller storage. `required` is mandatory and
 * reports bytes including the trailing NUL. With capacity zero, buffer may be NULL for a size
 * query. Insufficient capacity writes no message bytes and returns BUFFER_TOO_SMALL. `required`
 * is still written for that result. Provisional.
 */
uint32_t oa_core_error_message(
    uint32_t code,
    uint8_t *buffer,
    uint64_t capacity,
    uint64_t *required);

#ifdef __cplusplus
}
#endif

#endif /* OPENATCA_H */
