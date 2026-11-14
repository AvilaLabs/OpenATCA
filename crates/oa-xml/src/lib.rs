#![forbid(unsafe_code)]
//! oa-xml: a bounded, zero-dependency XML 1.0 parser for FAA SWIM messages (FIXM, AIXM, TFM and
//! TBFM XML). Shell-side input adapter code, but a pure function of its input: one complete
//! message in, one flat document out (RULESET B1).
//!
//! * The output ([`Doc`]) is struct-of-arrays and stores no strings: every name, attribute value
//!   and text is a [`Span`] (start, length) into the caller's input. Elements, attributes and
//!   text nodes are in document order; an element's attributes are contiguous.
//! * A [`Parser`] is created once with [`Limits`]; it preallocates every table at those limits
//!   and [`Parser::parse`] reuses them, so nothing allocates after construction (RULESET C1).
//!   Exceeding any limit returns [`XmlError::Limit`] naming the limit and the byte offset; a
//!   message is never truncated or partly accepted (INTERFACES IF-R1).
//! * One pass over the input, O(n), no recursion: open elements live on an explicit stack
//!   bounded by the depth limit.
//! * Accepted: the XML declaration (version 1.0, UTF-8 or no encoding), elements, attributes in
//!   either quote style, namespace declarations, comments, CDATA, character references and the
//!   five predefined entities. Rejected, each with its own error: DOCTYPE/DTD (which blocks
//!   entity expansion and external entities), other processing instructions, undefined
//!   entities, invalid UTF-8, characters outside the XML 1.0 `Char` production, mismatched or
//!   unclosed tags, duplicate attributes, content outside the root element, and undeclared
//!   namespace prefixes.
//!
//! Decisions where the specification leaves room: whitespace-only text between markup is not
//! stored as a text node; line endings and attribute whitespace are not normalised (spans are
//! raw bytes); name characters are ASCII letters, digits, `_ - .` and any non-ASCII character
//! (a superset of the XML 1.0 name productions, applied after UTF-8 validation); a namespace
//! URI is the raw attribute value; duplicate attributes are detected by qualified name, not by
//! expanded name. Entity-bearing values are decoded on demand by [`decode_into`] and
//! [`span_eq_str`].

mod decode;
mod ns;
mod parse;

pub use decode::{decode_into, span_eq_str};
pub use ns::{resolve_attr_ns, resolve_elem_ns, Ns};
pub use parse::Parser;

use std::fmt;

/// A byte range (start, length) into the input message.
// Budget: one per name, prefix, value and text, held in quantity: two u32 and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}
const _: () = assert!(core::mem::size_of::<Span>() == 8);

/// Index of an element row. `NONE` means "no such element".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ElemIx(pub u32);
const _: () = assert!(core::mem::size_of::<ElemIx>() == 4);

impl ElemIx {
    pub const NONE: ElemIx = ElemIx(u32::MAX);
}

/// Index of an attribute row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct AttrIx(pub u32);
const _: () = assert!(core::mem::size_of::<AttrIx>() == 4);

/// Index of a text row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct TextIx(pub u32);
const _: () = assert!(core::mem::size_of::<TextIx>() == 4);

/// Open element during parsing: one per level of depth.
// Budget: at most `max_depth` (64 by default) live at once; five u32.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct Frame {
    pub elem: ElemIx,
    pub last_child: ElemIx,
    pub qname_start: u32,
    pub qname_len: u32,
    pub ns_mark: u32,
}
const _: () = assert!(core::mem::size_of::<Frame>() == 20);

/// A namespace declaration in scope during parsing.
// Budget: at most `max_depth * max_attrs_per_elem` live at once; two spans.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct Binding {
    pub prefix: Span,
    pub uri: Span,
}
const _: () = assert!(core::mem::size_of::<Binding>() == 16);

/// Hard bounds on one message. Defaults come from the measured SWIM feeds (60 s samples,
/// 2026-11-14: median 0.5-4 KB, p99 337 KB, max 367 KB), with the input limit about 3x the
/// largest message seen. The element, attribute and text limits are above what 1 MiB of real
/// data produces.
// A7.2: cold struct, size guard waived
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_elements: usize,
    pub max_attributes: usize,
    pub max_attrs_per_elem: usize,
    pub max_name_len: usize,
    pub max_text_nodes: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_input_bytes: 1 << 20,
            max_depth: 64,
            max_elements: 50_000,
            max_attributes: 100_000,
            max_attrs_per_elem: 64,
            max_name_len: 256,
            max_text_nodes: 50_000,
        }
    }
}

/// Which limit was exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitKind {
    InputBytes,
    Depth,
    Elements,
    Attributes,
    AttributesPerElement,
    NameLength,
    TextNodes,
}

/// Every way parsing or decoding can fail. Every variant carries the byte offset in the input
/// at which the problem was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XmlError {
    /// A configured limit was exceeded; `at` is where the offending item starts (for the input
    /// limit, the limit itself).
    Limit { limit: LimitKind, at: u32 },
    /// The input is not valid UTF-8 from this offset.
    Utf8 { at: u32 },
    /// A character outside the XML 1.0 `Char` production (raw, or from a character reference).
    BadChar { at: u32 },
    /// A DOCTYPE (and so any DTD) is never accepted.
    Doctype { at: u32 },
    /// A processing instruction other than the XML declaration.
    ProcessingInstruction { at: u32 },
    /// A malformed XML declaration.
    BadXmlDecl { at: u32 },
    /// The XML declaration names a version other than 1.0.
    UnsupportedVersion { at: u32 },
    /// The XML declaration names an encoding other than UTF-8.
    UnsupportedEncoding { at: u32 },
    /// `&name;` where `name` is not one of the five predefined entities.
    UndefinedEntity { at: u32 },
    /// A malformed entity or character reference.
    BadEntity { at: u32 },
    /// An end tag that does not match the open element.
    MismatchedTag { at: u32 },
    /// The input ended with elements still open.
    Unclosed { at: u32 },
    /// The input ended inside markup.
    UnexpectedEof { at: u32 },
    /// The same attribute name twice on one element.
    DuplicateAttribute { at: u32 },
    /// A namespace prefix with no in-scope declaration.
    UndeclaredPrefix { at: u32 },
    /// No root element.
    NoRoot { at: u32 },
    /// Text or markup other than whitespace and comments before or after the root element.
    OutsideRoot { at: u32 },
    /// Malformed markup not covered above.
    Syntax { at: u32, what: &'static str },
    /// The caller's output buffer is too small (decode only).
    OutputTooSmall { at: u32 },
}

/// The display name of a limit.
pub fn limit_name(k: LimitKind) -> &'static str {
    match k {
        LimitKind::InputBytes => "max input bytes",
        LimitKind::Depth => "max depth",
        LimitKind::Elements => "max elements",
        LimitKind::Attributes => "max attributes",
        LimitKind::AttributesPerElement => "max attributes per element",
        LimitKind::NameLength => "max name length",
        LimitKind::TextNodes => "max text nodes",
    }
}

/// The byte offset carried by an error.
pub fn error_offset(e: &XmlError) -> u32 {
    match *e {
        XmlError::Limit { at, .. }
        | XmlError::Utf8 { at }
        | XmlError::BadChar { at }
        | XmlError::Doctype { at }
        | XmlError::ProcessingInstruction { at }
        | XmlError::BadXmlDecl { at }
        | XmlError::UnsupportedVersion { at }
        | XmlError::UnsupportedEncoding { at }
        | XmlError::UndefinedEntity { at }
        | XmlError::BadEntity { at }
        | XmlError::MismatchedTag { at }
        | XmlError::Unclosed { at }
        | XmlError::UnexpectedEof { at }
        | XmlError::DuplicateAttribute { at }
        | XmlError::UndeclaredPrefix { at }
        | XmlError::NoRoot { at }
        | XmlError::OutsideRoot { at }
        | XmlError::Syntax { at, .. }
        | XmlError::OutputTooSmall { at } => at,
    }
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = error_offset(self);
        match self {
            XmlError::Limit { limit, .. } => {
                write!(f, "limit exceeded: {} (at byte {at})", limit_name(*limit))
            }
            XmlError::Utf8 { .. } => write!(f, "invalid UTF-8 at byte {at}"),
            XmlError::BadChar { .. } => {
                write!(
                    f,
                    "character outside the XML 1.0 Char production at byte {at}"
                )
            }
            XmlError::Doctype { .. } => write!(f, "DOCTYPE/DTD not accepted at byte {at}"),
            XmlError::ProcessingInstruction { .. } => {
                write!(f, "processing instruction not accepted at byte {at}")
            }
            XmlError::BadXmlDecl { .. } => write!(f, "malformed XML declaration at byte {at}"),
            XmlError::UnsupportedVersion { .. } => {
                write!(f, "XML version other than 1.0 at byte {at}")
            }
            XmlError::UnsupportedEncoding { .. } => {
                write!(f, "encoding other than UTF-8 at byte {at}")
            }
            XmlError::UndefinedEntity { .. } => write!(f, "undefined entity at byte {at}"),
            XmlError::BadEntity { .. } => write!(f, "malformed entity reference at byte {at}"),
            XmlError::MismatchedTag { .. } => write!(f, "mismatched end tag at byte {at}"),
            XmlError::Unclosed { .. } => write!(f, "unclosed element at byte {at}"),
            XmlError::UnexpectedEof { .. } => write!(f, "unexpected end of input at byte {at}"),
            XmlError::DuplicateAttribute { .. } => write!(f, "duplicate attribute at byte {at}"),
            XmlError::UndeclaredPrefix { .. } => {
                write!(f, "undeclared namespace prefix at byte {at}")
            }
            XmlError::NoRoot { .. } => write!(f, "no root element at byte {at}"),
            XmlError::OutsideRoot { .. } => {
                write!(f, "content outside the root element at byte {at}")
            }
            XmlError::Syntax { what, .. } => write!(f, "{what} at byte {at}"),
            XmlError::OutputTooSmall { .. } => write!(f, "output buffer too small at byte {at}"),
        }
    }
}

impl std::error::Error for XmlError {}

/// A parsed message: borrowed, struct-of-arrays views of the parser's tables plus the input.
/// Row `i` of every `elem_*` slice describes element `i` (document order, root is 0); likewise
/// `attr_*` and `text_*`. An element's attributes are rows `first_attr .. first_attr + count`.
/// For a prefixed name, `name` is the local part and `prefix` the part before the colon (empty
/// span when absent). The `*_entity` bitsets (RULESET A6) mark values containing references
/// that need decoding; CDATA text never does.
#[derive(Clone, Copy, Debug)]
pub struct Doc<'a> {
    pub input: &'a [u8],
    pub elem_name: &'a [Span],
    pub elem_prefix: &'a [Span],
    pub elem_parent: &'a [ElemIx],
    pub elem_first_attr: &'a [AttrIx],
    pub elem_attr_count: &'a [u32],
    pub elem_first_child: &'a [ElemIx],
    pub elem_next_sibling: &'a [ElemIx],
    pub attr_name: &'a [Span],
    pub attr_prefix: &'a [Span],
    pub attr_value: &'a [Span],
    pub attr_entity: &'a [u64],
    pub text_span: &'a [Span],
    pub text_owner: &'a [ElemIx],
    pub text_entity: &'a [u64],
}

/// The bytes a span covers (empty if the span is out of range, which parsed spans never are).
pub fn span_bytes(input: &[u8], s: Span) -> &[u8] {
    let a = s.start as usize;
    let b = a.saturating_add(s.len as usize);
    input.get(a..b).unwrap_or(&[])
}

/// Whether bit `i` of a bitset is set.
pub fn bit_get(bits: &[u64], i: usize) -> bool {
    bits.get(i >> 6).is_some_and(|w| (w >> (i & 63)) & 1 == 1)
}

/// Whether attribute `a` has a value containing entity or character references.
pub fn attr_needs_decode(doc: &Doc<'_>, a: AttrIx) -> bool {
    bit_get(doc.attr_entity, a.0 as usize)
}

/// Whether text node `t` contains entity or character references.
pub fn text_needs_decode(doc: &Doc<'_>, t: TextIx) -> bool {
    bit_get(doc.text_entity, t.0 as usize)
}
