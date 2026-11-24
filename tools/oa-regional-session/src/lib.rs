#![forbid(unsafe_code)]
//! Bounded normalized regional-session input and receipt-time staging. Class: shell (RULESET
//! B1, B3, B7). Runtime bridges use explicitly approved existing workspace components.

pub mod cli;
pub mod commands;
pub mod flight;
pub mod input;
pub mod msaw_bridge;
pub mod performance;
pub mod route;
pub mod recordings;
pub mod recordings_reader;
pub mod recordings_cli;
pub mod runtime;
pub mod session;
pub mod surveillance;

pub use input::{
    parse_event_line, EventKind, FlightOperation, HandoffOperation, Identity, InputError,
    NormalizedEvent, RouteFixInput, SourceKind, MAX_EVENT_LINE_BYTES,
};
pub use session::{
    flush_receipt_group, push_event, session_cursor_new, session_total_events,
    summarize_input_batch, FrameEvidence, ReceiptBatch, SessionCursor, SessionError,
    MAX_EVENTS_PER_RECEIPT, MAX_SESSION_EVENTS,
};
