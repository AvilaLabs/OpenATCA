#![forbid(unsafe_code)]
//! oa-notam: decodes FAA SWIM FNS (Federal NOTAM System) messages, AIXM 5.1 `AIXMBasicMessage`
//! with the FAA event extension, into flat NOTAM rows. The survey of the real feed this is built
//! on is `docs/FNS_SURVEY.md`.
//!
//! * Core (pure, no I/O, no clock, no allocation after [`new_decoder`] and [`new_notams`]):
//!   [`scalar`] (strict parsers), [`rows`] (the row record and its struct-of-arrays, with
//!   shared polygon vertices and a bounded text table), [`fields`] (field, feature and error
//!   types), [`decode`] (a parsed [`oa_xml::Doc`] to a row) and [`stats`].
//! * Shell/offline: [`xcheck`], which resolves the rows' locations, airports, runways and
//!   navaids against an [`oa_airspace::model::Model`] and tests the polygons against its ARTCC
//!   boundary.
//!
//! The decoder follows the interface rules (INTERFACES.md): one document is decoded all or
//! nothing, the output has stated capacities and reaching one is a reported refusal (IF-R1),
//! malformed fields are visible and never defaulted (IF-R5), and unknown elements are counted,
//! not fatal, so a new FAA field neither breaks the adapter nor goes unseen (IF-R6).

pub mod decode;
pub mod encode;
pub mod fields;
mod known;
pub mod rows;
pub mod scalar;
pub mod stats;
pub mod xcheck;

pub use decode::{decode_bytes, decode_doc, new_decoder, Decoder, Limits, Outcome};
pub use encode::{encode_row, EncodeError, MAX_ENCODED_BYTES};
pub use fields::{
    feat_name, field_name, reject_index, CapacityKind, DocError, Field, Reject, ALL_FIELDS,
    ALT_UNLIMITED, CLASS_DOM, CLASS_INTL, CLASS_MIL, FEAT_AIRPORT, FEAT_COUNT, FEAT_NAMES,
    FEAT_NAVAID, FEAT_OBSTACLE_AREA, FEAT_PRIMARY_RADAR, FEAT_RADAR_SYSTEM, FEAT_RUNWAY,
    FEAT_RUNWAY_DIRECTION, FEAT_RUNWAY_ELEMENT, FEAT_TAXIWAY, FEAT_TAXIWAY_ELEMENT, FEAT_UNIT,
    FEAT_VERTICAL_STRUCTURE, FIELD_COUNT, FIELD_NAMES, FLAG_END_UNKNOWN, FLAG_ESTIMATED,
    FLAG_PERMANENT, INTERPRETATION_BASELINE, INTERPRETATION_PERMDELTA, INTERPRETATION_SNAPSHOT,
    INTERPRETATION_TEMPDELTA, PURPOSE_B, PURPOSE_K, PURPOSE_M, PURPOSE_N, PURPOSE_O, REF_AGL,
    REF_FL, REF_MSL, REJECT_COUNT, REJECT_NAMES, REJECT_SLOTS, REJECT_SLOT_NAMES, SCOPE_A, SCOPE_E,
    SCOPE_K, SCOPE_W, SLICE_CORRECTION_ERROR, SLICE_CORRECTION_PRESENT, SLICE_INTERPRETATION_ERROR,
    SLICE_SEQUENCE_ERROR, SLICE_SEQUENCE_PRESENT, TRAFFIC_I, TRAFFIC_K, TRAFFIC_V, TYPE_CANCEL,
    TYPE_NEW, TYPE_REPLACE,
};
pub use rows::{
    clear_notams, code_str, default_notams, feature_at, feature_slices_len, field_bit, field_error,
    has_circle, has_field, has_geometry, mark, new_notams, new_notams_with_feature_capacity,
    notams_len, polygon_vertices, polygons_of, push_feature_slice, push_row, row_at, schedule_of,
    set_feature_slice, text_of, truncate_notams, FeatureSlice, Mark, Notams, Row, BLANK_ROW,
    DEFAULT_FEATURES_PER_ROW, FEATURE_ERROR_CORRECTION, FEATURE_ERROR_DESIGNATOR,
    FEATURE_ERROR_INTERPRETATION, FEATURE_ERROR_SEQUENCE, FEATURE_ERROR_TIME,
    FEATURE_META_CORRECTION, FEATURE_META_SEQUENCE, FEATURE_TIME_BEGIN_PRESENT,
    FEATURE_TIME_END_PRESENT, FEATURE_TIME_INSTANT, FEATURE_TIME_INSTANT_PRESENT,
    FEATURE_TIME_PERIOD,
};
pub use stats::{new_stats, unknown_text, Stats, UnknownPath, MAX_PATH_TEXT};
pub use xcheck::{cross_check, format_cross_check, CrossCheck, PolyCheck, Scope, Tally};
