#![forbid(unsafe_code)]
//! oa-nam: the North American (NAM) Common Coordination interface between area control centres,
//! NAS-IC-21009205 Revision G (15 December 2022), and the session layer ERAM runs it over,
//! NAS-IC-82422100 (ERAM / NAM Non-US ACC System ICD, 7 May 2019) with NAS-IR-82422100. It is
//! the message protocol ERAM speaks with the adjacent Canadian and Mexican (and Cuban,
//! Dominican, Central American) centres. No sockets, no clock: the shell supplies bytes and
//! the time.
//!
//! # Classification (B1, B7)
//!
//! The crate is `split`. [`core`](crate::core) holds [`session`], a pure function of its inputs
//! that allocates nothing and holds fixed, named capacities (C1, C2), checked under a counting
//! allocator (C5), and the `Copy` identity types it uses (`core::identity`, re-exported by
//! [`model`]). The `shell` modules (the codec: [`field`], [`message`], [`model`], [`schema`],
//! [`frame`], [`lrm`], [`error`], [`transport_epoch`], [`workload`]) are adapters under the shell and
//! offline rules (D1, C6, as the other decoders of this repository). They are a pure function
//! of bytes and values, but they allocate per message (`Vec`, `String`), so they are not
//! real-time core and are not held to C1 or C3. Their structs are cold under A7.2.
//!
//! # Layers
//!
//! * [`field`] and [`message`]: the 25 NAM messages of the core set (Part II Table 2), plus the
//!   public ICAO Appendix 3 DLA field message, as plain
//!   data ([`model`]), parsed and encoded from one table of definitions ([`schema`]).
//!   [`parse`] is strict and canonical; [`encode`] refuses an invalid value and checks that its
//!   output parses back to the same value. Encoding what [`parse`] accepted gives the same
//!   bytes (the variants the ICD allows are stored in the model so they round-trip, see the
//!   README).
//! * [`frame`]: the four-hex-digit length header of the transport.
//! * [`session`]: the INACTIVE / ACTIVATING / ACTIVE / DEACTIVATING state machine as a pure
//!   function `(config, session, event, now) -> (session, actions)`, with message-number and
//!   acknowledgement bookkeeping.
//! * [`lrm`]: the LRM error code tables. [`workload`]: the ICD's rate, size and latency figures.
//!
//! # Refusal (E5)
//!
//! Input that breaks a rule is refused with a [`NamError`] that carries the Table A-1 code and
//! field a receiving system would put in its LRM ([`lrm::lrm`]); nothing is repaired. The
//! only tolerance is [`parse_tolerant`], which drops spaces beside a hyphen (the ICD's own
//! examples have them) and reports that it did.
//!
//! # Bounds
//!
//! A message is 17 to 1800 bytes ([`MIN_MESSAGE_LEN`], [`MAX_MESSAGE_LEN`]); a route holds at
//! most [`MAX_ROUTE_ELEMENTS`] elements; Field 22 at most [`MAX_AMENDMENTS`]; the session keeps
//! at most [`session::MAX_PENDING`] unanswered messages and returns at most
//! [`session::MAX_ACTIONS`] actions per step.

// The width of `usize` is fixed by construction: no 32-bit build of the core (B10).
#[cfg(not(target_pointer_width = "64"))]
compile_error!("this crate supports 64-bit targets only");

pub mod core;
pub mod shell;

pub use crate::core::session;
pub use crate::shell::{
    error, field, frame, item15, lrm, message, model, schema, transport_epoch, workload,
};

pub use error::{ErrorKind, NamError, NO_OFFSET};
pub use field::{MAX_F31_PLAIN, MAX_F31_TEXT, MAX_MESSAGE_LEN, MAX_ROUTE_ELEMENTS};
pub use message::{
    encode, encode_with, parse, parse_tolerant, parse_tolerant_with, parse_with, peek_kind,
    Deviations, MAX_AMENDMENTS, MIN_MESSAGE_LEN,
};
pub use model::*;
