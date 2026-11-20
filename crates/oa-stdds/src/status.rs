//! The STDDS service-status messages (kind [`Kind::StddsStatus`]): the service name and its
//! status value. Pure functions (crate class shell).
//!
//! Four of the roots (`AirportDataServiceStatus`, `TerminalAutomationInformationServiceStatus`,
//! `TowerDepartureEventServiceStatus`, `SurfaceMovementEventServiceStatus`) carry the service's
//! state as the text of a direct `serviceStatus` child; the service name is the root name.
//! `STDDSStatus` is a tree of `p` elements with `k` (key) and `v` (value) attributes: every
//! top-level `p` whose key ends in `Status` or `State` and that has a value is one service
//! state, named by its key. The links, connections and counters below are not read. The values
//! are service states (`Normal`, `Degraded` and the like), not identifying; the names are
//! schema text.

use crate::kind::{attr_present, attr_value, children, in_message_ns, name_of, Kind, STATUS_ROOTS};
use crate::walk::{leaf_text, text_index, VALUE_BUF};
use crate::{FieldErrorKind, Stats, StdError};
use oa_xml::{Doc, ElemIx};

/// Most states kept per message; further ones are counted in [`StatusMessage::dropped`].
pub const MAX_STATES: usize = 32;
/// Longest name or value kept (bytes); a longer one is dropped and counted.
pub const MAX_TEXT: usize = 64;
// VALUE_BUF is also 64: a serviceStatus text longer than that is `TooLong` from the reader.

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
    /// States found but not kept (more than [`MAX_STATES`], or an empty or unreadable name or
    /// value).
    pub dropped: u64,
    /// States whose value text is longer than [`MAX_TEXT`], and whose key is.
    pub value_too_long: u64,
    pub key_too_long: u64,
}

struct Acc {
    states: Vec<StatusState>,
    dropped: u64,
    value_too_long: u64,
    key_too_long: u64,
}

impl Acc {
    fn keep(&mut self, name: &[u8], value: &[u8]) {
        if name.len() > MAX_TEXT {
            self.key_too_long += 1;
        } else if value.len() > MAX_TEXT {
            self.value_too_long += 1;
        } else if name.is_empty() || value.is_empty() || self.states.len() >= MAX_STATES {
            self.dropped += 1;
        } else {
            self.states.push(StatusState {
                name: String::from_utf8_lossy(name).into_owned(),
                value: String::from_utf8_lossy(value).into_owned(),
            });
        }
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
    let mut acc = Acc {
        states: Vec::new(),
        dropped: 0,
        value_too_long: 0,
        key_too_long: 0,
    };
    if root_static == "STDDSStatus" {
        // Buffers a little over the limit, so a text past it is seen as too long, not absent.
        let mut kb = [0u8; MAX_TEXT + 8];
        let mut vb = [0u8; MAX_TEXT + 8];
        for p in children(doc, root).filter(|&c| name_of(doc, c) == b"p" && in_message_ns(doc, c)) {
            let key = match attr_value(doc, p, b"k", &mut kb) {
                Some(k) => k,
                None => {
                    if attr_present(doc, p, b"k") {
                        acc.key_too_long += 1;
                    }
                    continue;
                }
            };
            if !(key.ends_with(b"Status") || key.ends_with(b"State")) {
                continue;
            }
            match attr_value(doc, p, b"v", &mut vb) {
                Some(v) => acc.keep(key, v),
                // A key with no value (a container such as a status block) is not a state.
                None if !attr_present(doc, p, b"v") => {}
                None => acc.value_too_long += 1,
            }
        }
    } else {
        let tx = text_index(doc);
        let mut buf = [0u8; VALUE_BUF];
        for c in children(doc, root)
            .filter(|&c| name_of(doc, c) == b"serviceStatus" && in_message_ns(doc, c))
        {
            match leaf_text(doc, &tx, c, &mut buf) {
                Ok(v) => acc.keep(root_name, v),
                Err(FieldErrorKind::TooLong) => acc.value_too_long += 1,
                Err(_) => acc.dropped += 1,
            }
        }
    }
    let m = StatusMessage {
        root: root_static,
        states: acc.states,
        dropped: acc.dropped,
        value_too_long: acc.value_too_long,
        key_too_long: acc.key_too_long,
    };
    stats.observe_status(&m);
    Ok(m)
}
