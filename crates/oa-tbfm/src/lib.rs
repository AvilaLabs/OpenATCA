#![forbid(unsafe_code)]
//! oa-tbfm: decoder for the FAA TBFM (Time Based Flow Management) metering publication, the
//! `tbfmmeteringpublication:1.1.0` messages delivered through SWIM SCDS. The survey of the real
//! feed this is built on is `docs/TBFM_SURVEY.md`.
//!
//! * Core (pure, no I/O, no clock, no allocation after [`new_rows`]): [`time`] (strict ISO-8601
//!   to Unix ms), [`rows`] (the row record and its struct-of-arrays), [`fields`] (field and
//!   error types), [`decode`] (a parsed [`oa_xml::Doc`] to rows) and [`stats`].
//! * Shell/offline: [`xcheck`], which resolves the rows' meter fixes and airports against an
//!   [`oa_airspace::model::Model`].
//!
//! The decoder follows the interface rules (INTERFACES.md): one message is decoded all or
//! nothing, a refused field rejects the message and says which field and where (IF-R5), the
//! output has a stated capacity and reaching it is a reported reject (IF-R1), and unknown
//! elements are counted, not fatal, so a new FAA field neither breaks the adapter nor goes
//! unseen (IF-R6).

pub mod decode;
pub mod fields;
pub mod rows;
pub mod stats;
pub mod time;
pub mod xcheck;

pub use decode::{decode_bytes, decode_doc, decode_doc_prefix, default_limits, Outcome, NAMESPACE};
pub use fields::{
    error_kind_name, field_name, reject_reason, DocError, Field, FieldError, FieldErrorKind,
    MsgKind, FIELD_COUNT, MSG_KIND_COUNT, MSG_KIND_NAMES, REJECT_CAPACITY, REJECT_COUNT,
    REJECT_FIELD, REJECT_NAMES, REJECT_NOT_TBFM, REJECT_XML,
};
pub use rows::{
    clear_rows, is_cleared, name_str, new_rows, push_row, row_at, rows_len, sch_get, truncate_rows,
    Row, Rows, BLANK_ROW, DEFAULT_ROW_CAPACITY, NO_BCN, NO_NUM, NO_TIME,
};
pub use stats::{new_stats, observe, Stats};
pub use time::{parse_iso_ms, TimeError};
pub use xcheck::{cross_check, format_cross_check, CrossCheck, Scope, Tally};
