#![forbid(unsafe_code)]
//! oa-tfms: decoder for the FAA TFMS (Traffic Flow Management System) data service, the
//! `tfmDataService` messages delivered through SWIM SCDS: flight data messages (track
//! information, flight plans and amendments, departure and arrival, flight create and modify,
//! boundary crossings, oceanic reports) and flow information messages (restrictions,
//! advisories, FCAs and FEAs, airport configurations, TMI flight lists). The survey of the real
//! feed this is built on is `docs/TFMS_SURVEY.md`.
//!
//! * Core (pure, no I/O, no clock, no allocation after [`new_rows`]): [`time`] (strict ISO-8601
//!   to Unix ms), [`scalar`] (strict field parsers), [`rows`] (the two row records, their
//!   struct-of-arrays and the text table), [`fields`] (field and error types), [`schema`] (the
//!   static element tables), [`decode`] (a parsed [`oa_xml::Doc`] to rows) and [`stats`].
//! * Shell/offline: [`link`], which reports how many TFMS flights match SFDPS flights
//!   ([`oa_fixm::Events`]).
//!
//! The decoder follows the interface rules (INTERFACES.md): one message is decoded all or
//! nothing, a refused field rejects that message (its siblings are kept) and says which field and
//! where (IF-R5), a document whose structure is unreadable is refused whole, the
//! output has a stated capacity and reaching it is a reported reject (IF-R1), and unknown
//! elements are counted, not fatal, so a new FAA field neither breaks the adapter nor goes
//! unseen (IF-R6).

pub mod decode;
pub mod fields;
pub mod link;
pub mod rows;
pub mod scalar;
pub mod schema;
pub mod stats;
pub mod time;

pub use decode::{decode_bytes, decode_doc, decode_doc_prefix, default_limits, Outcome, NAMESPACE};
pub use fields::{
    error_kind_index, error_kind_name, field_name, reject_reason, CapKind, DocError, DocKind,
    Field, FieldError, FieldErrorKind, DOC_KIND_COUNT, DOC_KIND_NAMES, ERROR_KIND_COUNT,
    FIELD_COUNT, REJECT_CAP_FLIGHTS, REJECT_CAP_FLOWS, REJECT_CAP_STRINGS, REJECT_COUNT,
    REJECT_FIELD, REJECT_NAMES, REJECT_NOT_TFMS, REJECT_XML,
};
pub use link::{format_link_report, link_flights, LinkReport};
pub use rows::{
    clear_rows, flag_get, flight_at, flights_len, flow_at, flows_len, name_str, new_rows,
    push_flight, push_flow, text, truncate_flights, truncate_flows, truncate_rows, FlightRow,
    Flights, FlowRow, Flows, Rows, StrRef, BLANK_FLIGHT, BLANK_FLOW, DEFAULT_FLIGHT_CAPACITY,
    DEFAULT_FLOW_CAPACITY, DEFAULT_STRING_BYTES, NO_COORD, NO_NUM, NO_STR, NO_TIME,
};
pub use stats::{new_stats, observe, Stats};
pub use time::{parse_iso_ms, TimeError};
