#![forbid(unsafe_code)]
//! Durable shell operations for the bounded audit log. Event values and validation live in
//! `oa-audit-core`; this package owns fixed record encoding, hashing, verification and file I/O.

mod encoding;
pub mod shell;

pub use oa_audit_core::{
    Action, AuditError, AuditReceipt, Checkpoint, Event, Record, Role, DETAILS_CAPACITY,
    TARGET_CAPACITY,
};
