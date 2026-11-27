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
    OA_STATUS_DEFLATE_RATIO_LIMIT = 16,
    OA_STATUS_DEFLATE_ENCODE_OUTPUT_LIMIT = 17,
    OA_STATUS_TAR_BAD_HEADER = 18,
    OA_STATUS_TAR_TRUNCATED = 19,
    OA_STATUS_TAR_LIMIT = 20,
    OA_STATUS_TAR_NAME_TOO_LONG = 21,
    OA_STATUS_TAR_READ_ERROR = 22,
    OA_STATUS_NOTAM_INPUT_LIMIT = 42,
    OA_STATUS_NOTAM_XML_REFUSAL = 43,
    OA_STATUS_NOTAM_DOCUMENT_REFUSAL = 44,
    OA_STATUS_NOTAM_ELEMENT_LIMIT = 45,
    OA_STATUS_NOTAM_CAPACITY = 46,
    OA_STATUS_NOTAM_SEALED_DROP = 47,
    OA_STATUS_NOTAM_MEMORY_BUDGET = 48,
    OA_STATUS_ZIP_FORMAT = 32,
    OA_STATUS_ZIP_TRUNCATED = 33,
    OA_STATUS_ZIP_UNSUPPORTED_METHOD = 34,
    OA_STATUS_ZIP_ENCRYPTED = 35,
    OA_STATUS_ZIP_UNSAFE_PATH = 36,
    OA_STATUS_ZIP_LIMIT = 37,
    OA_STATUS_ZIP_INFLATE_OR_SIZE = 38,
    OA_STATUS_ZIP_CRC = 39,
    OA_STATUS_ZIP_BAD_ENTRY_INDEX = 40,
    OA_STATUS_GRIB2_FORMAT = 80,
    OA_STATUS_GRIB2_UNSUPPORTED = 81,
    OA_STATUS_GRIB2_LIMIT = 82
};

#define OA_NOTAM_MAX_INPUT_BYTES UINT64_C(1048576)
#define OA_NOTAM_MAX_WORKING_BYTES UINT64_C(16777216)
#define OA_NOTAM_MAX_TEXT_BYTES UINT64_C(8192)
#define OA_NOTAM_MAX_POLYGON_VERTICES UINT64_C(4096)
#define OA_NOTAM_FIELD_COUNT UINT64_C(31)
#define OA_NOTAM_REJECT_SLOT_COUNT UINT64_C(9)
#define OA_NOTAM_FEATURE_KIND_COUNT UINT64_C(12)
#define OA_NOTAM_UNKNOWN_PATH_MAX UINT64_C(128)
#define OA_NOTAM_STATUS_BUFFER_TOO_SMALL OA_STATUS_BUFFER_TOO_SMALL

enum {
    OA_NOTAM_FIELD_SERIES = 0, OA_NOTAM_FIELD_NUMBER = 1, OA_NOTAM_FIELD_YEAR = 2,
    OA_NOTAM_FIELD_TYPE = 3, OA_NOTAM_FIELD_REFERENCE = 4, OA_NOTAM_FIELD_LOCATION = 5,
    OA_NOTAM_FIELD_FIR = 6, OA_NOTAM_FIELD_ICAO_LOCATION = 7, OA_NOTAM_FIELD_ISSUED = 8,
    OA_NOTAM_FIELD_START = 9, OA_NOTAM_FIELD_END = 10, OA_NOTAM_FIELD_SCHEDULE = 11,
    OA_NOTAM_FIELD_TEXT = 12, OA_NOTAM_FIELD_SELECTION = 13, OA_NOTAM_FIELD_SCENARIO = 14,
    OA_NOTAM_FIELD_CLASSIFICATION = 15, OA_NOTAM_FIELD_TRAFFIC = 16,
    OA_NOTAM_FIELD_PURPOSE = 17, OA_NOTAM_FIELD_SCOPE = 18, OA_NOTAM_FIELD_CANCELED = 19,
    OA_NOTAM_FIELD_CROSS_REFERENCE = 20, OA_NOTAM_FIELD_CENTRE = 21,
    OA_NOTAM_FIELD_RADIUS = 22, OA_NOTAM_FIELD_FLOOR = 23, OA_NOTAM_FIELD_CEILING = 24,
    OA_NOTAM_FIELD_POLYGONS = 25, OA_NOTAM_FIELD_AIRPORT = 26,
    OA_NOTAM_FIELD_AIRPORT_ICAO = 27, OA_NOTAM_FIELD_RUNWAY = 28,
    OA_NOTAM_FIELD_NAVAID = 29, OA_NOTAM_FIELD_UNIT = 30
};
#define OA_NOTAM_FIELD_MASK(field) (UINT64_C(1) << (field))
enum {
    OA_NOTAM_FEATURE_AIRPORT = 1, OA_NOTAM_FEATURE_RUNWAY = 2,
    OA_NOTAM_FEATURE_RUNWAY_DIRECTION = 3, OA_NOTAM_FEATURE_RUNWAY_ELEMENT = 4,
    OA_NOTAM_FEATURE_TAXIWAY = 5, OA_NOTAM_FEATURE_TAXIWAY_ELEMENT = 6,
    OA_NOTAM_FEATURE_NAVAID = 7, OA_NOTAM_FEATURE_UNIT = 8,
    OA_NOTAM_FEATURE_OBSTACLE_AREA = 9, OA_NOTAM_FEATURE_VERTICAL_STRUCTURE = 10,
    OA_NOTAM_FEATURE_RADAR_SYSTEM = 11, OA_NOTAM_FEATURE_PRIMARY_RADAR = 12
};

/* Stable only within this provisional, profile-specific preview. Fixed-width values retain
 * oa-notam's source codes; present/error masks distinguish absent, invalid, and populated fields. */
typedef struct OaNotamRow {
    uint64_t present, errors;
    int64_t issued_ms, start_ms, end_ms, canceled_ms;
    uint32_t flags, number, ref_number, sched_off, sched_len, text_off, text_len, radius_m;
    uint32_t poly_first, poly_count, feature_first, feature_count, feat_mask;
    int32_t lat_e7, lon_e7, floor_ft, ceil_ft;
    uint16_t year, ref_year, scenario;
    uint8_t ntype, class_code, traffic, purpose, scope, floor_ref, ceil_ref, feat_kind;
    uint8_t interpretation, slice_meta, slice_errors;
    uint32_t sequence_number, correction_number;
    uint8_t event_id[16], series[12], ref_series[12], xover[16], feat_desig[12], runway[12];
    uint8_t airport[8], navaid[8], unit[8], selection[5], location[4], fir[4];
    uint8_t icao_location[4], airport_icao[4];
} OaNotamRow;

typedef struct OaNotamFeature {
    uint8_t kind, time_kind, interpretation, meta, time_meta, errors, reserved[2];
    int64_t begin_ms, end_ms;
    uint32_t sequence_number, correction_number;
    uint8_t designator[12], designator_len, icao[4], icao_len;
} OaNotamFeature;

typedef struct OaNotamPolygon {
    uint32_t first_vertex, vertex_count, feature_index;
    uint8_t kind, reserved[3];
} OaNotamPolygon;

typedef struct OaNotamVertex { int32_t lat_e7, lon_e7; } OaNotamVertex;

typedef struct OaNotamUnknownPath {
    uint64_t hash, count;
    uint8_t is_attribute, text_len, reserved[6], text[160];
} OaNotamUnknownPath;

typedef struct OaNotamSummary {
    uint32_t status, refusal_kind;
    uint64_t source_offset, row_count, feature_count, polygon_count, vertex_count, string_bytes;
    uint64_t unknown_path_count, unknown_elements, unknown_attributes, unknown_overflow, sealed_dropped;
    uint64_t field_errors[31], rejects[9], features[12];
} OaNotamSummary;

typedef struct OaNotamBuffers {
    OaNotamRow *row; uint64_t row_capacity;
    OaNotamFeature *features; uint64_t feature_capacity;
    OaNotamPolygon *polygons; uint64_t polygon_capacity;
    OaNotamVertex *vertices; uint64_t vertex_capacity;
    uint8_t *strings; uint64_t string_capacity;
    OaNotamUnknownPath *unknown_paths; uint64_t unknown_path_capacity;
} OaNotamBuffers;

#define OA_DEFLATE_MAX_INPUT_BYTES UINT64_C(67108864)
#define OA_DEFLATE_MAX_OUTPUT_BYTES UINT64_C(268435456)
#define OA_DEFLATE_ENCODE_MAX_OUTPUT_BYTES UINT64_C(134217728)
#define OA_TAR_MAX_INPUT_BYTES UINT64_C(67108864)
#define OA_TAR_MAX_ENTRIES UINT64_C(4096)
#define OA_TAR_MAX_NAME_POOL_BYTES UINT64_C(16777216)
#define OA_ZIP_MAX_INPUT_BYTES UINT64_C(1073741824)
#define OA_ZIP_MAX_ENTRIES UINT64_C(10000)
#define OA_ZIP_MAX_ENTRY_BYTES UINT64_C(268435456)
#define OA_ZIP_MAX_TOTAL_BYTES UINT64_C(1073741824)
#define OA_ZIP_MAX_NAME_BYTES UINT64_C(4096)
#define OA_ZIP_MAX_NAME_POOL_BYTES UINT64_C(40960000)
#define OA_ZIP_MAX_RATIO UINT64_C(200)
#define OA_ZIP_RATIO_EXEMPT_BYTES UINT64_C(1048576)
#define OA_GRIB2_MAX_MESSAGE_BYTES UINT64_C(268435456)
#define OA_GRIB2_MAX_FIELDS UINT64_C(16)
#define OA_GRIB2_MAX_MESSAGE_POINTS UINT64_C(67108864)

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

/* FNS profile decoder. Required policy endpoints are a caller-owned Unix-ms half-open interval;
 * equal endpoints explicitly mean no filtering. Null buffers queries required capacities. */
OaStatus oa_notam_fns_decode(const uint8_t *input, uint64_t input_length,
    int64_t drop_from_ms, int64_t drop_to_ms, const OaNotamBuffers *buffers,
    OaNotamSummary *summary, OaErrorInfo *error);

#define OA_ASTERIX_MAX_INPUT_BYTES UINT64_C(65535)
#define OA_ASTERIX_MAX_RECORDS UINT64_C(21844)
enum {
    OA_ASTERIX_EDITION_NONE = 0,
    OA_ASTERIX_CAT021_EDITION_24 = 24,
    OA_ASTERIX_CAT021_EDITION_27 = 27
};
#define OA_ASTERIX_FIELD_SOURCE (UINT64_C(1) << 0)
#define OA_ASTERIX_FIELD_ADDRESS (UINT64_C(1) << 1)
#define OA_ASTERIX_FIELD_IDENT (UINT64_C(1) << 2)
#define OA_ASTERIX_FIELD_TRACK (UINT64_C(1) << 3)
#define OA_ASTERIX_FIELD_TIME (UINT64_C(1) << 4)
#define OA_ASTERIX_FIELD_POSITION_0 (UINT64_C(1) << 5)
#define OA_ASTERIX_FIELD_POSITION_1 (UINT64_C(1) << 6)
#define OA_ASTERIX_FIELD_VELOCITY (UINT64_C(1) << 7)
#define OA_ASTERIX_FIELD_MODE3A (UINT64_C(1) << 8)
#define OA_ASTERIX_FIELD_VERTICAL (UINT64_C(1) << 9)
enum {
    OA_ASTERIX_POSITION_GEODETIC = 1,
    OA_ASTERIX_POSITION_POLAR = 2,
    OA_ASTERIX_POSITION_CARTESIAN = 3,
    OA_ASTERIX_SCALE_CAT021_POS = 1,
    OA_ASTERIX_SCALE_CAT021_POS_HIGH = 2,
    OA_ASTERIX_SCALE_CAT062_POS = 3,
    OA_ASTERIX_SCALE_CAT048_POLAR = 4,
    OA_ASTERIX_SCALE_CAT048_CART = 5,
    OA_ASTERIX_SCALE_CAT062_CART = 6,
    OA_ASTERIX_VELOCITY_CAT021_GROUND = 1,
    OA_ASTERIX_VELOCITY_CAT062_CART = 2
};
enum {
    OA_ASTERIX_VERTICAL_GEOMETRIC_HEIGHT = 1,
    OA_ASTERIX_VERTICAL_FLIGHT_LEVEL = 2,
    OA_ASTERIX_VERTICAL_SELECTED_ALTITUDE = 3,
    OA_ASTERIX_VERTICAL_FINAL_SELECTED_ALTITUDE = 4,
    OA_ASTERIX_VERTICAL_BARO_RATE = 5,
    OA_ASTERIX_VERTICAL_GEOMETRIC_RATE = 6,
    OA_ASTERIX_VERTICAL_SCALE_6_25_FT = 1,
    OA_ASTERIX_VERTICAL_SCALE_0_25_FL = 2,
    OA_ASTERIX_VERTICAL_SCALE_25_FT = 3,
    OA_ASTERIX_VERTICAL_SCALE_6_25_FPM = 4
};
enum {
    OA_ASTERIX_VERTICAL_RANGE_EXCEEDED = 1 << 0,
    OA_ASTERIX_VERTICAL_NOT_VALIDATED = 1 << 1,
    OA_ASTERIX_VERTICAL_GARBLED = 1 << 2,
    OA_ASTERIX_VERTICAL_QNH = 1 << 3,
    OA_ASTERIX_VERTICAL_SOURCE_PROVIDED = 1 << 4,
    OA_ASTERIX_VERTICAL_MANAGE = 1 << 5,
    OA_ASTERIX_VERTICAL_HOLD = 1 << 6,
    OA_ASTERIX_VERTICAL_APPROACH = 1 << 7
};
enum {
    OA_ASTERIX_ERROR_BUFFER_TOO_SMALL = 1001,
    OA_ASTERIX_ERROR_INVALID_ARGUMENT = 1002,
    OA_ASTERIX_ERROR_LENGTH_TOO_LARGE = 1003,
    OA_ASTERIX_ERROR_ALLOCATION_FAILED = 1004,
    OA_ASTERIX_ERROR_INTERNAL_PANIC = 1005
};
typedef struct OaAsterixPosition {
    uint16_t item;
    uint8_t kind, scale;
    int32_t a_raw, b_raw;
} OaAsterixPosition;
typedef struct OaAsterixVertical {
    uint16_t item;
    uint8_t subitem, kind, scale, flags, auxiliary, reserved;
    int32_t raw;
} OaAsterixVertical;
typedef struct OaAsterixRecord {
    uint8_t category, cat021_edition, source_sac, source_sic;
    uint32_t reserved0;
    uint64_t present_frn_mask, raw_untyped_frn_mask, typed_unprojected_frn_mask;
    uint64_t present_field_mask;
    uint32_t target_address;
    uint8_t target_ident[8];
    uint16_t track_number, time_item;
    uint32_t time_raw;
    uint8_t position_count, velocity_kind, velocity_flags, mode3a_flags;
    OaAsterixPosition positions[2];
    int32_t velocity_a_raw, velocity_b_raw;
    uint16_t mode3a_code, reserved3;
    uint32_t raw_untyped_subitem_mask, reserved4, reserved5;
    uint8_t vertical_count, reserved6[7];
    OaAsterixVertical vertical[6];
} OaAsterixRecord;
typedef struct OaAsterixCounts {
    uint8_t category, edition;
    uint16_t reserved;
    uint8_t reserved_bytes[4];
    uint64_t consumed_bytes, required_record_count, written_record_count;
} OaAsterixCounts;
typedef struct OaAsterixError {
    uint32_t status, kind;
    uint8_t category, subitem;
    uint16_t item;
    uint32_t record;
    uint8_t reserved_bytes[8];
    uint64_t offset;
} OaAsterixError;

/* `status` repeats the generic return status; `kind` 1–23 maps to oa-asterix ErrorKind variants.
 * The generic status values 23–31 are currently unassigned. */

#define OA_CIFP_MAX_INPUT_BYTES UINT64_C(134217728)
#define OA_CIFP_MAX_PHYSICAL_LINES UINT64_C(600000)
#define OA_CIFP_MAX_SOURCE_BYTES UINT64_C(256)
#define OA_CIFP_MAX_OUTPUT_BYTES UINT64_C(805306368)
#define OA_CIFP_NO_OFFSET UINT64_MAX
/* `present` bit assignments; masks are row-family specific. */
#define OA_CIFP_PRESENT_IATA (UINT64_C(1) << 0)
#define OA_CIFP_PRESENT_LONGEST_RUNWAY (UINT64_C(1) << 1)
#define OA_CIFP_PRESENT_MAGVAR (UINT64_C(1) << 2)
#define OA_CIFP_PRESENT_ELEVATION (UINT64_C(1) << 3)
#define OA_CIFP_PRESENT_TRANSITION_ALT (UINT64_C(1) << 4)
#define OA_CIFP_PRESENT_TRANSITION_LEVEL (UINT64_C(1) << 5)
#define OA_CIFP_PRESENT_PUBLIC_MILITARY (UINT64_C(1) << 6)
#define OA_CIFP_PRESENT_NORTH_REFERENCE (UINT64_C(1) << 7)
#define OA_CIFP_PRESENT_BEARING (UINT64_C(1) << 8)
#define OA_CIFP_PRESENT_ELLIPSOID_HEIGHT (UINT64_C(1) << 9)
#define OA_CIFP_PRESENT_THRESHOLD_ELEVATION (UINT64_C(1) << 10)
#define OA_CIFP_PRESENT_TCH (UINT64_C(1) << 11)
#define OA_CIFP_PRESENT_WIDTH (UINT64_C(1) << 12)
#define OA_CIFP_PRESENT_LOCALIZER_IDENT (UINT64_C(1) << 13)
#define OA_CIFP_PRESENT_LOCALIZER_CATEGORY (UINT64_C(1) << 14)
#define OA_CIFP_PRESENT_GLIDE_POSITION (UINT64_C(1) << 15)
#define OA_CIFP_PRESENT_GLIDE_ANGLE (UINT64_C(1) << 16)
#define OA_CIFP_PRESENT_STATION_DECLINATION (UINT64_C(1) << 17)
#define OA_CIFP_PRESENT_GLIDE_ELEVATION (UINT64_C(1) << 18)
#define OA_CIFP_PRESENT_AIRPORT_IDENT (UINT64_C(1) << 19)
#define OA_CIFP_PRESENT_AIRPORT_REGION (UINT64_C(1) << 20)
#define OA_CIFP_PRESENT_FREQUENCY (UINT64_C(1) << 21)
#define OA_CIFP_PRESENT_VOR_POSITION (UINT64_C(1) << 22)
#define OA_CIFP_PRESENT_DME_IDENT (UINT64_C(1) << 23)
#define OA_CIFP_PRESENT_DME_POSITION (UINT64_C(1) << 24)
#define OA_CIFP_PRESENT_DME_ELEVATION (UINT64_C(1) << 25)
#define OA_CIFP_PRESENT_FIGURE_OF_MERIT (UINT64_C(1) << 26)
#define OA_CIFP_PRESENT_POSITION (UINT64_C(1) << 27)
#define OA_CIFP_PRESENT_USAGE (UINT64_C(1) << 28)
#define OA_CIFP_PRESENT_WAYPOINT_TYPE (UINT64_C(1) << 29)
#define OA_CIFP_PRESENT_ALTITUDE_1 (UINT64_C(1) << 30)
#define OA_CIFP_PRESENT_ALTITUDE_2 (UINT64_C(1) << 31)
#define OA_CIFP_PRESENT_LEVEL (UINT64_C(1) << 33)
#define OA_CIFP_PRESENT_OUTBOUND_COURSE (UINT64_C(1) << 34)
#define OA_CIFP_PRESENT_DISTANCE (UINT64_C(1) << 35)
#define OA_CIFP_PRESENT_INBOUND_COURSE (UINT64_C(1) << 36)
#define OA_CIFP_PRESENT_MAX_ALTITUDE (UINT64_C(1) << 37)
#define OA_CIFP_PRESENT_FIX (UINT64_C(1) << 38)
#define OA_CIFP_PRESENT_TURN_DIRECTION (UINT64_C(1) << 39)
#define OA_CIFP_PRESENT_RNP (UINT64_C(1) << 40)
#define OA_CIFP_PRESENT_RECOMMENDED_NAVAID (UINT64_C(1) << 41)
#define OA_CIFP_PRESENT_ARC_RADIUS (UINT64_C(1) << 42)
#define OA_CIFP_PRESENT_THETA (UINT64_C(1) << 43)
#define OA_CIFP_PRESENT_RHO (UINT64_C(1) << 44)
#define OA_CIFP_PRESENT_COURSE (UINT64_C(1) << 45)
#define OA_CIFP_PRESENT_DISTANCE_OR_TIME (UINT64_C(1) << 46)
#define OA_CIFP_PRESENT_ALT_DESC (UINT64_C(1) << 47)
#define OA_CIFP_PRESENT_TRANSITION_ALTITUDE (UINT64_C(1) << 48)
#define OA_CIFP_PRESENT_SPEED_LIMIT (UINT64_C(1) << 49)
#define OA_CIFP_PRESENT_VERTICAL_ANGLE (UINT64_C(1) << 50)
#define OA_CIFP_PRESENT_CENTER_FIX (UINT64_C(1) << 51)
#define OA_CIFP_PRESENT_GNSS_FMS (UINT64_C(1) << 52)
#define OA_CIFP_PRESENT_SPEED_DESC (UINT64_C(1) << 53)
#define OA_CIFP_PRESENT_CLASSIFICATION (UINT64_C(1) << 54)
#define OA_CIFP_PRESENT_TIME_CODE (UINT64_C(1) << 55)
#define OA_CIFP_PRESENT_LOWER (UINT64_C(1) << 56)
#define OA_CIFP_PRESENT_UPPER (UINT64_C(1) << 57)
#define OA_CIFP_PRESENT_ARC_ORIGIN (UINT64_C(1) << 59)
#define OA_CIFP_PRESENT_ARC_DISTANCE (UINT64_C(1) << 60)
#define OA_CIFP_PRESENT_ARC_BEARING (UINT64_C(1) << 61)
#define OA_CIFP_PRESENT_LONGEST_RUNWAY_SURFACE (UINT64_C(1) << 63)
typedef struct OaCifpSpan { uint64_t offset, length; } OaCifpSpan;
typedef struct OaCifpPosition { double lat, lon; } OaCifpPosition;
typedef struct OaCifpMeta { uint64_t line; uint8_t cycle_year, cycle_release, reserved[6]; OaCifpSpan customer_area, file_record_number; } OaCifpMeta;
typedef struct OaCifpAltitude { uint8_t kind, reserved[3]; int32_t value; } OaCifpAltitude;
typedef struct OaCifpFixRef { OaCifpSpan ident, region; uint8_t section, subsection, reserved[6]; } OaCifpFixRef;
typedef struct OaCifpAirport { OaCifpMeta meta; uint64_t present; OaCifpSpan ident, icao_region, iata, datum, name; OaCifpPosition position; double magnetic_variation; int32_t elevation_ft; uint32_t longest_runway_ft; OaCifpAltitude transition_altitude, transition_level; uint8_t ifr_capable, longest_runway_surface, public_military, north_reference, reserved[4]; } OaCifpAirport;
typedef struct OaCifpRunway { OaCifpMeta meta; uint64_t present; OaCifpSpan airport_ident, airport_region, ident, localizer_ident; OaCifpPosition threshold; uint32_t length_ft, displaced_threshold_ft; double magnetic_bearing, ellipsoid_height_m; int32_t threshold_elevation_ft; uint32_t threshold_crossing_height_ft, width_ft; uint8_t tch_indicator, localizer_category, reserved[6], reserved2[4]; } OaCifpRunway;
typedef struct OaCifpLocalizer { OaCifpMeta meta; uint64_t present; OaCifpSpan airport_ident, airport_region, ident, runway_ident; OaCifpPosition localizer_position, glide_slope_position; uint8_t category, reserved0[7]; double frequency_mhz, bearing, localizer_width_deg, glide_slope_angle_deg, station_declination; uint32_t glide_slope_tch_ft; int32_t glide_slope_elevation_ft; } OaCifpLocalizer;
typedef struct OaCifpVhfNavaid { OaCifpMeta meta; uint64_t present; OaCifpSpan airport_ident, airport_region, ident, icao_region, dme_ident, datum, name; OaCifpPosition vor_position, dme_position; double frequency_mhz, station_declination; int32_t dme_elevation_ft; uint8_t class[5], figure_of_merit, reserved[2], reserved2[4]; } OaCifpVhfNavaid;
typedef struct OaCifpNdb { OaCifpMeta meta; uint64_t present; OaCifpSpan airport_ident, airport_region, ident, icao_region, class, datum, name; OaCifpPosition position; double frequency_khz, magnetic_variation; uint8_t terminal, reserved[7]; } OaCifpNdb;
typedef struct OaCifpWaypoint { OaCifpMeta meta; uint64_t present; OaCifpSpan area, area_region, ident, icao_region, waypoint_type, usage, datum, name; OaCifpPosition position; double magnetic_variation; uint8_t kind, reserved[7]; } OaCifpWaypoint;
typedef struct OaCifpAirway { OaCifpSpan customer_area, ident; uint64_t first_line, fix_start, fix_count; uint8_t route_type, level, reserved[6]; } OaCifpAirway;
typedef struct OaCifpAirwayFix { OaCifpMeta meta; OaCifpSpan route_ident; OaCifpFixRef fix; uint32_t sequence; uint8_t description[4], route_type, level, reserved0[6]; uint64_t present; double outbound_course, distance_nm, inbound_course; OaCifpAltitude min_altitude, min_altitude_2, max_altitude; } OaCifpAirwayFix;
typedef struct OaCifpProcedure { OaCifpSpan customer_area, airport_ident, airport_region, ident; uint64_t first_line, route_start, route_count; uint8_t kind, reserved[7]; } OaCifpProcedure;
typedef struct OaCifpRoute { OaCifpSpan transition; uint64_t first_line, leg_start, leg_count; uint8_t route_type, reserved[7]; } OaCifpRoute;
typedef struct OaCifpLeg { OaCifpMeta meta; uint64_t present; OaCifpSpan airport_ident, airport_region, ident, transition, rnp_code; OaCifpFixRef fix, recommended_navaid, center_fix; uint32_t sequence; uint8_t description[4], kind, route_type, turn_direction, path_terminator, altitude_description, gnss_fms_indication, speed_limit_description, reserved0; double arc_radius_nm, theta_deg, rho_nm, course_deg, distance_or_time_value; uint8_t distance_or_time_kind, reserved1[7]; OaCifpAltitude altitude1, altitude2, transition_altitude; uint32_t speed_limit_kt; uint8_t reserved2[4]; double vertical_angle_deg; } OaCifpLeg;
typedef struct OaCifpContinuation { OaCifpMeta meta; uint64_t owner_index; OaCifpSpan text; uint8_t number, application_type, owner_kind, reserved[5]; } OaCifpContinuation;
typedef struct OaCifpLevelOfService { uint64_t continuation_index; OaCifpSpan name; uint8_t authorized, reserved[7]; } OaCifpLevelOfService;
typedef struct OaCifpAirspace { OaCifpMeta meta; uint64_t present; OaCifpSpan icao_region, designation, name; uint64_t boundary_start, boundary_count, continuation_start, continuation_count; uint8_t kind, airspace_type, classification, multiple_code, level, time_code, reserved[2]; OaCifpAltitude lower; uint8_t lower_unit, upper_unit, reserved2[6]; OaCifpAltitude upper; } OaCifpAirspace;
typedef struct OaCifpBoundary { OaCifpPosition position, arc_origin; uint32_t sequence; uint8_t via, end, present, reserved; double arc_distance_nm, arc_bearing_deg; uint64_t source_line; } OaCifpBoundary;
typedef struct OaCifpMora { OaCifpMeta meta; int16_t start_lat, start_lon; uint32_t present_mask, cells_ft[30]; } OaCifpMora;
typedef struct OaCifpHeader { OaCifpSpan file_name, version, creation_date, creation_time, data_supplier, crc; uint32_t record_length; uint8_t reserved0[4]; uint64_t record_count; uint8_t cycle_year, cycle_release, production_flag, crc_verified, reserved[4]; } OaCifpHeader;
typedef struct OaCifpHeaderText { OaCifpSpan text; } OaCifpHeaderText;
typedef struct OaCifpSectionCount { OaCifpSpan section; uint64_t count; uint8_t supported, reserved[7]; } OaCifpSectionCount;
typedef struct OaCifpCycleCount { uint64_t count; uint8_t year, release, reserved[6]; } OaCifpCycleCount;
typedef struct OaCifpError { uint32_t status, kind; uint64_t line; uint32_t first_column, last_column; uint64_t limit, actual, previous, found; uint32_t code, flags; uint8_t field[32], value[132], reason[64], reserved[4]; } OaCifpError;
typedef struct OaCifpCounts { uint64_t airports, runways, localizers, vhf_navaids, ndbs, waypoints, moras, airspaces, airways, airway_fixes, procedures, routes, legs, continuations, levels_of_service, boundaries, header_records, sections, cycles, errors, errors_dropped, strings_bytes, output_bytes; uint8_t is_clean, reserved[7]; } OaCifpCounts;
typedef struct OaCifpBuffers { OaCifpHeader *header; uint64_t header_capacity; OaCifpHeaderText *header_text; uint64_t header_text_capacity; OaCifpAirport *airports; uint64_t airports_capacity; OaCifpRunway *runways; uint64_t runways_capacity; OaCifpLocalizer *localizers; uint64_t localizers_capacity; OaCifpVhfNavaid *vhf_navaids; uint64_t vhf_navaids_capacity; OaCifpNdb *ndbs; uint64_t ndbs_capacity; OaCifpWaypoint *waypoints; uint64_t waypoints_capacity; OaCifpMora *moras; uint64_t moras_capacity; OaCifpAirspace *airspaces; uint64_t airspaces_capacity; OaCifpAirway *airways; uint64_t airways_capacity; OaCifpAirwayFix *airway_fixes; uint64_t airway_fixes_capacity; OaCifpProcedure *procedures; uint64_t procedures_capacity; OaCifpRoute *routes; uint64_t routes_capacity; OaCifpLeg *legs; uint64_t legs_capacity; OaCifpContinuation *continuations; uint64_t continuations_capacity; OaCifpLevelOfService *levels_of_service; uint64_t levels_of_service_capacity; OaCifpBoundary *boundaries; uint64_t boundaries_capacity; OaCifpSectionCount *sections; uint64_t sections_capacity; OaCifpCycleCount *cycles; uint64_t cycles_capacity; OaCifpError *errors; uint64_t errors_capacity; uint8_t *strings; uint64_t strings_capacity; } OaCifpBuffers;
uint32_t oa_cifp_parse(const uint8_t *input, uint64_t input_length, const uint8_t *source, uint64_t source_length, const OaCifpBuffers *buffers, OaCifpCounts *counts, OaCifpError *parse_error, OaErrorInfo *error);

enum { OA_TEXT_XML = 1, OA_TEXT_JSON = 2, OA_TEXT_CSV = 3 };
/* Parser kind is module-specific; offsets are byte positions except CSV's physical line. */
typedef struct OaTextError {
    uint32_t module;
    uint32_t kind;
    uint64_t position;
    uint64_t detail_a;
    uint64_t detail_b;
} OaTextError;
/* Name offsets index oa_tar_parse's UTF-8 byte pool; reserved bytes are always zero. */
typedef struct OaTarEntry {
    uint64_t name_offset, name_length, size, mtime, header_offset;
    uint8_t typeflag; /* oa-tar reported flag: NUL normalizes to '0'; unknown flags are kept. */
    uint8_t reserved[7];
} OaTarEntry;
/* ZIP entry name spans index the UTF-8 pool; flags/method are preserved from the central record. */
typedef struct OaZipEntry {
    uint64_t name_offset, name_length, compressed_size, uncompressed_size, local_offset;
    uint32_t crc32;
    uint16_t flags, method;
    uint8_t zip64, utf8_name_flag;
    uint8_t reserved[6]; /* Named padding; always zero. */
} OaZipEntry;
/* GRIB2 fixed layouts. Fields describe one decoded source field; values are contiguous f32
 * in message order, with bitmap-missing points represented by NaN and counted explicitly. */
typedef struct OaGrib2Time {
    uint16_t year;
    uint8_t month, day, hour, minute, second, reserved;
} OaGrib2Time;
typedef struct OaGrib2Field {
    uint8_t discipline, category, number, named_parameter;
    uint16_t grid_template, product_template, packing_template;
    uint8_t level_kind, forecast_time_unit;
    OaGrib2Time reference_time;
    uint32_t forecast_time;
    float level_value;
    uint8_t interval_present, interval_ranges, interval_statistic, reserved0;
    OaGrib2Time interval_end;
    uint32_t nx, ny;
    uint8_t grid_flags, scan_mode;
    int8_t latitude_row_direction;
    uint8_t global_longitude;
    double first_latitude_deg, first_longitude_deg, i_increment_deg, j_increment_deg;
    double lambert_lad_deg, lambert_lov_deg, lambert_dx_m, lambert_dy_m;
    double lambert_latin1_deg, lambert_latin2_deg, earth_radius_m;
    uint64_t value_offset, value_count, missing_count;
} OaGrib2Field;
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
 * Compresses one raw DEFLATE stream using oa-deflate's existing level clamp policy: requested
 * levels below 1 use 1, above 9 use 9. actual_level reports the applied value. Input is capped
 * at 64 MiB and output at 128 MiB. A zero output capacity queries the exact output length. Output
 * bytes are copied only after compression completes and passes max_output; failures preserve the
 * output buffer and length metadata. Success and BUFFER_TOO_SMALL consume all input bytes.
 */
uint32_t oa_deflate_raw_compress(const uint8_t *input, uint64_t input_length,
                                 uint32_t requested_level, uint64_t max_output,
                                 uint8_t *output, uint64_t output_capacity,
                                 uint64_t *output_length, uint64_t *consumed,
                                 uint32_t *actual_level, OaErrorInfo *error);

/*
 * Metadata-only tar reader. The complete archive is validated with oa-tar DEFAULT_LIMITS,
 * a 64 MiB input cap and caller-selected max_entries <= 4096. Names are returned as unchanged UTF-8 bytes in the
 * caller's pool; duplicates and unknown typeflags are preserved (NUL normalizes to '0'). No payload bytes or filesystem
 * extraction are exposed. Capacity query validates the whole archive and returns exact counts
 * only on BUFFER_TOO_SMALL; all other failures leave arrays and size metadata unchanged.
 */
uint32_t oa_tar_parse(const uint8_t *input, uint64_t input_length, uint32_t max_entries,
                      OaTarEntry *entries, uint64_t entry_capacity, uint64_t *entry_count,
                      uint8_t *name_pool, uint64_t name_pool_capacity,
                      uint64_t *name_pool_length, OaErrorInfo *error);

/*
 * Bounded read-only ZIP metadata and selected-member extraction. Uses oa-zip's existing defaults:
 * 1 GiB input/archive and total uncompressed bytes, 10,000 entries, 256 MiB per entry, ratio 200
 * except entries <=1 MiB, and 4096-byte names. Names use oa-zip's strict profile: bit-11 names
 * must be UTF-8; names without bit 11 must be ASCII. Unsupported legacy high-bit names refuse.
 * `oa_zip_list` validates the entire directory; a short output capacity returns exact counts but
 * writes no arrays. `oa_zip_extract_entry` selects by zero-based directory index and accepts an
 * output capacity no greater than 256 MiB. Its size query reports the validated declared size
 * only; local-header, stream, size and CRC integrity is established only by successful fill.
 * Extraction is to caller memory only, never to a path. All output is transactional on failures.
 */
uint32_t oa_zip_list(const uint8_t *input, uint64_t input_length,
                     OaZipEntry *entries, uint64_t entry_capacity, uint64_t *entry_count,
                     uint8_t *name_pool, uint64_t name_pool_capacity,
                     uint64_t *name_pool_length, OaErrorInfo *error);
uint32_t oa_zip_extract_entry(const uint8_t *input, uint64_t input_length, uint64_t entry_index,
                              uint8_t *output, uint64_t output_capacity,
                              uint64_t *output_length, OaErrorInfo *error);
/* Exact-subset GRIB2 bindings over the existing oa-grib2 decoder. One complete message only;
 * structural inspection reports required arrays without decoding values. The existing decoder
 * supports grid templates 3.0/3.30, products 4.0/4.8, packing 5.0/5.2/5.3/5.40 and bitmap reuse.
 * Message cap 256 MiB, 16 fields, 16 Mi points/field, 64 Mi total points, work budget 4 Mi +
 * 64 points/input byte. Query counts with zero capacities; short capacity returns BUFFER_TOO_SMALL
 * and exact sizes without writing arrays. Decode is transactional. NaN marks missing bitmap data.
 */
uint32_t oa_grib2_inspect_message(const uint8_t *input, uint64_t input_length,
                                  uint64_t *required_fields, uint64_t *required_values,
                                  OaErrorInfo *error);
uint32_t oa_grib2_decode_message(const uint8_t *input, uint64_t input_length,
                                 OaGrib2Field *fields, uint64_t fields_capacity,
                                 float *values, uint64_t values_capacity,
                                 uint64_t *required_fields, uint64_t *required_values,
                                 OaErrorInfo *error);


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

/* One ASTERIX block at a time. CAT021 selector is 24 or 27; CAT048/CAT062 selector is 0.
 * Input and output arrays are caller-owned. A null records pointer with capacity zero is a count
 * query; BUFFER_TOO_SMALL reports required count and consumed block bytes but writes no rows.
 * Parse failures preserve counts/rows and populate OaAsterixError. On success rows and then counts
 * are written. Present masks use FRN-1 and field bits above; numeric quantities remain raw source
 * counts. Time-of-day has no inferred date. All regions including OaErrorInfo are disjoint. */
uint32_t oa_asterix_decode_block(const uint8_t *input, uint64_t input_length,
                                 uint8_t cat021_edition, OaAsterixRecord *records,
                                 uint64_t record_capacity, OaAsterixCounts *counts,
                                 OaAsterixError *parse_error, OaErrorInfo *error);

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
