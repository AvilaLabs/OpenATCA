#![forbid(unsafe_code)]
//! Bounded normalized regional-session input and receipt-time staging. Class: shell (RULESET
//! B1, B3, B7). Runtime bridges use explicitly approved existing workspace components.

pub mod cli;
pub mod commands;
pub mod conflict_probe;
pub mod flight;
pub mod input;
pub mod migration_amendment;
pub mod migration_full_day;
pub mod migration_full_day_runner;
pub mod migration_g3;
pub mod migration_shadow;
pub mod migration_snapshot;
// Pending F1 integration; must remain excluded from qualified landings until edge approval.
pub mod msaw_bridge;
pub mod nam_handoff;
pub mod operator;
pub mod performance;
pub mod recordings;
pub mod recordings_cli;
pub mod recordings_reader;
pub mod recordings_stream;
pub mod route;
pub mod runtime;
pub mod session;
pub mod surveillance;

pub use input::{
    parse_event_line, EventKind, FlightOperation, HandoffOperation, Identity, InputError,
    NormalizedEvent, RouteFixInput, SourceKind, MAX_EVENT_LINE_BYTES,
};
pub use runtime::{
    regional_new_with_profile, regional_profile_limits, RegionalRunProfile,
    DEFAULT_REGIONAL_RUN_PROFILE,
};
pub use session::{
    flush_receipt_group, push_event, session_cursor_new, session_cursor_new_with_limit,
    session_total_events, summarize_input_batch, FrameEvidence, ReceiptBatch, SessionCursor,
    SessionError, MAX_EVENTS_PER_RECEIPT, MAX_RECORDED_PROFILE_EVENTS, MAX_SESSION_EVENTS,
};
