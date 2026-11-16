#![forbid(unsafe_code)]
//! oa-json: a strict RFC 8259 JSON parser with hard bounds. Pure function of its input (RULESET
//! B1): one complete text in, one flat document out.
//!
//! * The output ([`Doc`]) is a flat arena of nodes in document (pre-)order, struct-of-arrays: a
//!   kind, an offset and length, and the index one past the node's subtree. A container's first
//!   child is the next node; the next sibling of a child is its subtree end. Nothing is a
//!   pointer, and nothing is recursive: the parser keeps open containers on an explicit stack
//!   bounded by [`Limits::max_depth`], and the accessors walk with indexes, so no input can
//!   overflow the stack.
//! * Strict: the RFC 8259 grammar exactly. No comments, no trailing commas, no leading zeros, no
//!   `+`, `NaN` or `Infinity`, single quotes, or unescaped control characters; whitespace is
//!   space, tab, line feed and carriage return only. Any value may be the top-level text (RFC
//!   8259 section 2). The input must be valid UTF-8 (checked up front, so an invalid byte
//!   anywhere is [`ErrorKind::InvalidUtf8`]); a byte-order mark is refused (it is not
//!   whitespace). Anything but whitespace after the value is [`ErrorKind::TrailingGarbage`].
//! * Numbers are kept as their source text ([`Doc::number_text`]); [`Doc::as_f64`] and
//!   [`Doc::as_i64`] convert on request and return `None` when the value does not fit (an
//!   `f64` that is not finite, an integer outside `i64`, or an integer-valued number written
//!   with a fraction or exponent for `as_i64`).
//! * Strings: escapes are decoded, including `\u` surrogate pairs. A lone surrogate (high not
//!   followed by a low, or a low alone) cannot be UTF-8 and is refused ([`ErrorKind::BadEscape`]).
//!   `\u0000` is accepted and yields a NUL byte. A string without escapes is a span of the
//!   input; one with escapes is decoded into a side pool owned by the document.
//! * Duplicate object keys are accepted and all kept in document order (RFC 8259 says names
//!   SHOULD be unique and leaves behaviour undefined); [`Doc::get`] returns the last one, as
//!   ECMAScript does. A caller that must refuse duplicates can walk [`Doc::members`].
//!
//! Bounds ([`Limits`], defaults below): input bytes, nesting depth, string bytes, node count.
//! Exceeding one returns an error naming it and the byte offset; nothing is truncated.
//!
//! Choices for the JSONTestSuite `i_` (implementation-defined) cases are listed in
//! `tests/jsontestsuite.rs` next to the table that enforces them. Cold, offline code: A7.2
//! applies to the document and limit records; the node columns are one row per value.

mod parse;

pub use parse::parse;

use std::fmt;

/// Limit: input bytes, 32 MiB. A day's largest real aircraft trace is a few MiB.
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
/// Limit: nesting depth of arrays and objects. Real data nests 3 deep; the depth only sizes the
/// explicit stack.
pub const MAX_DEPTH: usize = 64;
/// Limit: bytes of one string (raw, between its quotes), 1 MiB.
pub const MAX_STRING_BYTES: usize = 1024 * 1024;
/// Limit: nodes (values plus object keys) in one document, 4 Mi. At 13 bytes a node the
/// columns can never exceed about 55 MB.
pub const MAX_NODES: usize = 4 * 1024 * 1024;

/// The enforced limits, gathered so tests (and callers with tighter needs) can lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    /// Most raw bytes between a string's quotes, escapes counted as written (`\n` is two).
    pub max_string_bytes: usize,
    pub max_nodes: usize,
}

/// The limits above, as one value.
pub const DEFAULT_LIMITS: Limits = Limits {
    max_input_bytes: MAX_INPUT_BYTES,
    max_depth: MAX_DEPTH,
    max_string_bytes: MAX_STRING_BYTES,
    max_nodes: MAX_NODES,
};

/// What a node holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Null = 0,
    False = 1,
    True = 2,
    /// `off`/`len` span the number's text in the input.
    Number = 3,
    /// `off`/`len` span the decoded text in the input (no escapes).
    String = 4,
    /// `off`/`len` span the decoded text in the document's pool (it had escapes).
    StringPooled = 5,
    /// `len` is the number of elements.
    Array = 6,
    /// `len` is the number of members; the children are key, value, key, value...
    Object = 7,
}

/// Index of a node. `NONE` means "no such node".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NodeIx(pub u32);
const _: () = assert!(core::mem::size_of::<NodeIx>() == 4);

impl NodeIx {
    pub const NONE: NodeIx = NodeIx(u32::MAX);
    /// The top-level value of a document.
    pub const ROOT: NodeIx = NodeIx(0);
}

/// Why a text was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Input longer than [`Limits::max_input_bytes`].
    InputTooLarge,
    /// The input is not valid UTF-8; the offset is the first bad byte.
    InvalidUtf8,
    /// Arrays and objects nested deeper than [`Limits::max_depth`].
    DepthExceeded,
    /// More than [`Limits::max_nodes`] values.
    TooManyNodes,
    /// A string longer than [`Limits::max_string_bytes`].
    StringTooLong,
    /// The text ended before the value was complete (or is empty).
    UnexpectedEnd,
    /// A byte that the grammar does not allow here.
    UnexpectedByte,
    /// A malformed number.
    BadNumber,
    /// A bad escape sequence, or a lone surrogate.
    BadEscape,
    /// An unescaped control character (below 0x20) inside a string.
    ControlInString,
    /// Something other than whitespace after the top-level value.
    TrailingGarbage,
}

/// A refused text and the byte offset where it went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct JsonError {
    pub offset: usize,
    pub kind: ErrorKind,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "json: byte {}: ", self.offset)?;
        match self.kind {
            ErrorKind::InputTooLarge => write!(f, "input exceeds the size limit"),
            ErrorKind::InvalidUtf8 => write!(f, "invalid UTF-8"),
            ErrorKind::DepthExceeded => write!(f, "nesting exceeds the depth limit"),
            ErrorKind::TooManyNodes => write!(f, "more values than the node limit"),
            ErrorKind::StringTooLong => write!(f, "string exceeds the length limit"),
            ErrorKind::UnexpectedEnd => write!(f, "unexpected end of input"),
            ErrorKind::UnexpectedByte => write!(f, "unexpected byte"),
            ErrorKind::BadNumber => write!(f, "malformed number"),
            ErrorKind::BadEscape => write!(f, "bad escape or lone surrogate"),
            ErrorKind::ControlInString => write!(f, "control character in string"),
            ErrorKind::TrailingGarbage => write!(f, "text after the top-level value"),
        }
    }
}

impl std::error::Error for JsonError {}

/// A parsed document: borrows the input, owns the node columns. A subsystem under D5 (it keeps
/// the columns and the string pool consistent), so it keeps methods.
// A7.2: cold struct (one per parsed text), size guard waived
#[derive(Debug)]
pub struct Doc<'a> {
    pub(crate) input: &'a [u8],
    pub(crate) pool: Vec<u8>,
    pub(crate) kind: Vec<Kind>,
    pub(crate) off: Vec<u32>,
    pub(crate) len: Vec<u32>,
    /// One past the last node of this node's subtree (`i + 1` for scalars).
    pub(crate) end: Vec<u32>,
}

impl<'a> Doc<'a> {
    /// Number of nodes (values and object keys).
    pub fn node_count(&self) -> usize {
        self.kind.len()
    }

    /// The kind of a node, or `None` for an index out of range.
    pub fn kind(&self, n: NodeIx) -> Option<Kind> {
        self.kind.get(n.0 as usize).copied()
    }

    /// Elements of an array or members of an object (0 for anything else).
    pub fn len(&self, n: NodeIx) -> usize {
        match self.kind(n) {
            Some(Kind::Array | Kind::Object) => {
                self.len.get(n.0 as usize).copied().unwrap_or(0) as usize
            }
            _ => 0,
        }
    }

    /// True for an empty array or object, and for any scalar.
    pub fn is_empty(&self, n: NodeIx) -> bool {
        self.len(n) == 0
    }

    fn span(&self, n: NodeIx) -> Option<&[u8]> {
        let i = n.0 as usize;
        let off = *self.off.get(i)? as usize;
        let len = *self.len.get(i)? as usize;
        match self.kind.get(i)? {
            Kind::Number | Kind::String => self.input.get(off..off.checked_add(len)?),
            Kind::StringPooled => self.pool.get(off..off.checked_add(len)?),
            _ => None,
        }
    }

    /// The decoded text of a string node.
    pub fn str(&self, n: NodeIx) -> Option<&str> {
        match self.kind(n)? {
            Kind::String | Kind::StringPooled => std::str::from_utf8(self.span(n)?).ok(),
            _ => None,
        }
    }

    /// The source text of a number node.
    pub fn number_text(&self, n: NodeIx) -> Option<&str> {
        match self.kind(n)? {
            Kind::Number => std::str::from_utf8(self.span(n)?).ok(),
            _ => None,
        }
    }

    /// A number as `f64`; `None` if the node is not a number or the value is not finite.
    pub fn as_f64(&self, n: NodeIx) -> Option<f64> {
        let v: f64 = self.number_text(n)?.parse().ok()?;
        v.is_finite().then_some(v)
    }

    /// A number as `i64`; `None` if the node is not a number, is written with a fraction or an
    /// exponent, or is outside `i64`.
    pub fn as_i64(&self, n: NodeIx) -> Option<i64> {
        let t = self.number_text(n)?;
        if t.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
            return None;
        }
        t.parse().ok()
    }

    /// A boolean node's value.
    pub fn as_bool(&self, n: NodeIx) -> Option<bool> {
        match self.kind(n)? {
            Kind::True => Some(true),
            Kind::False => Some(false),
            _ => None,
        }
    }

    /// The node after the subtree of `n` (its next sibling, or the parent's end).
    pub fn skip(&self, n: NodeIx) -> NodeIx {
        match self.end.get(n.0 as usize) {
            Some(&e) => NodeIx(e),
            None => NodeIx::NONE,
        }
    }

    /// Elements of an array, in order. Empty for anything else.
    pub fn elements(&self, n: NodeIx) -> Children<'_, 'a> {
        let count = if self.kind(n) == Some(Kind::Array) {
            self.len(n)
        } else {
            0
        };
        Children {
            doc: self,
            next: NodeIx(n.0.wrapping_add(1)),
            left: count,
        }
    }

    /// Members of an object as (key, value) node pairs, in document order. Empty for anything
    /// else.
    pub fn members(&self, n: NodeIx) -> Members<'_, 'a> {
        let count = if self.kind(n) == Some(Kind::Object) {
            self.len(n)
        } else {
            0
        };
        Members {
            doc: self,
            next: NodeIx(n.0.wrapping_add(1)),
            left: count,
        }
    }

    /// The value of the member named `key`; with duplicate keys, the last one.
    pub fn get(&self, obj: NodeIx, key: &str) -> Option<NodeIx> {
        let mut found = None;
        for (k, v) in self.members(obj) {
            if self.str(k) == Some(key) {
                found = Some(v);
            }
        }
        found
    }
}

/// Iterator over the elements of an array.
// A7.2: cold struct, size guard waived
pub struct Children<'d, 'a> {
    doc: &'d Doc<'a>,
    next: NodeIx,
    left: usize,
}

impl Iterator for Children<'_, '_> {
    type Item = NodeIx;
    fn next(&mut self) -> Option<NodeIx> {
        if self.left == 0 {
            return None;
        }
        let here = self.next;
        self.next = self.doc.skip(here);
        self.left -= 1;
        Some(here)
    }
}

/// Iterator over the (key, value) pairs of an object.
// A7.2: cold struct, size guard waived
pub struct Members<'d, 'a> {
    doc: &'d Doc<'a>,
    next: NodeIx,
    left: usize,
}

impl Iterator for Members<'_, '_> {
    type Item = (NodeIx, NodeIx);
    fn next(&mut self) -> Option<(NodeIx, NodeIx)> {
        if self.left == 0 {
            return None;
        }
        let key = self.next;
        let val = self.doc.skip(key);
        self.next = self.doc.skip(val);
        self.left -= 1;
        Some((key, val))
    }
}
