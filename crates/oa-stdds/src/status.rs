//! The STDDS service-status messages (kind [`Kind::StddsStatus`]): the service name and its
//! status value. Core (pure).
//!
//! Four of the roots (`AirportDataServiceStatus`, `TerminalAutomationInformationServiceStatus`,
//! `TowerDepartureEventServiceStatus`, `SurfaceMovementEventServiceStatus`) carry the service's
//! state as the text of a direct `serviceStatus` child; the service name is the root name.
//! `STDDSStatus` is a tree of `p` elements with `k` (key) and `v` (value) attributes: every
//! top-level `p` whose key ends in `Status` or `State` and that has a value is one service
//! state, named by its key. The links, connections and counters below are not read. The values
//! are service states (`Normal`, `Degraded` and the like), not identifying; the names are
//! schema text.

use crate::kind::{attr_value, children, name_of, Kind, STATUS_ROOTS};
use crate::walk::{leaf_text, text_index, VALUE_BUF};
use crate::{Stats, StdError};
use oa_xml::{Doc, ElemIx};

/// Most states kept per message; further ones are counted in [`StatusMessage::dropped`].
pub const MAX_STATES: usize = 32;
/// Longest name or value kept (bytes); a longer one is dropped and counted.
pub const MAX_TEXT: usize = 64;

/// One service state: `name` (schema text) and its `value`.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusState {
    pub name: String,
    pub value: String,
}

/// A parsed status message.
// A7.2: cold struct, size guard waived
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusMessage {
    /// The root element's name, one of [`STATUS_ROOTS`].
    pub root: &'static str,
    pub states: Vec<StatusState>,
    /// States found but not kept (too many, or a name or value too long or unreadable).
    pub dropped: u64,
}

fn keep(states: &mut Vec<StatusState>, dropped: &mut u64, name: &[u8], value: &[u8]) {
    let ok = !name.is_empty()
        && !value.is_empty()
        && name.len() <= MAX_TEXT
        && value.len() <= MAX_TEXT
        && states.len() < MAX_STATES;
    if ok {
        states.push(StatusState {
            name: String::from_utf8_lossy(name).into_owned(),
            value: String::from_utf8_lossy(value).into_owned(),
        });
    } else {
        *dropped += 1;
    }
}

/// Parses a message that [`crate::classify`] called [`Kind::StddsStatus`]. Counts into `stats`.
pub fn parse_status(
    doc: &Doc<'_>,
    _limits: &crate::Limits,
    stats: &mut Stats,
) -> Result<StatusMessage, StdError> {
    let k = crate::classify(doc);
    if k != Kind::StddsStatus {
        let e = StdError::WrongKind { kind: k };
        stats.observe_error(&e);
        return Err(e);
    }
    let root = ElemIx(0);
    let root_name = name_of(doc, root);
    let root_static = STATUS_ROOTS
        .iter()
        .copied()
        .find(|r| r.as_bytes() == root_name)
        .unwrap_or("?");
    let mut states = Vec::new();
    let mut dropped = 0u64;
    if root_static == "STDDSStatus" {
        let mut kb = [0u8; MAX_TEXT + 1];
        let mut vb = [0u8; MAX_TEXT + 1];
        for p in children(doc, root).filter(|&c| name_of(doc, c) == b"p") {
            let Some(key) = attr_value(doc, p, b"k", &mut kb) else {
                continue;
            };
            if !(key.ends_with(b"Status") || key.ends_with(b"State")) {
                continue;
            }
            match attr_value(doc, p, b"v", &mut vb) {
                Some(v) => keep(&mut states, &mut dropped, key, v),
                None => {
                    // A key with no value (a container such as a status block) is not a state; a
                    // value too long for the buffer is dropped and counted.
                }
            }
        }
    } else {
        let tx = text_index(doc);
        let mut buf = [0u8; VALUE_BUF];
        for c in children(doc, root).filter(|&c| name_of(doc, c) == b"serviceStatus") {
            match leaf_text(doc, &tx, c, &mut buf) {
                Ok(v) => keep(&mut states, &mut dropped, root_name, v),
                Err(_) => dropped += 1,
            }
        }
    }
    let m = StatusMessage {
        root: root_static,
        states,
        dropped,
    };
    stats.observe_status(&m);
    Ok(m)
}
