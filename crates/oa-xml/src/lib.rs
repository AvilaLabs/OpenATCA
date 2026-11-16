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
//! stored as a text node (unless it sits next to text or a CDATA section of the same element,
//! where it is data); the pieces of an element's text that a comment or CDATA section splits are
//! several text nodes, joined by [`decode_text_run`]; spans are raw input bytes, and line-end and attribute-value
//! normalisation (XML 1.0 sections 2.11 and 3.3.3) happen when a span is decoded, by
//! [`decode_into`] and [`span_eq_str`], selected by a [`SpanKind`]; a value that needs
//! decoding (a reference, or a byte that normalisation changes) is flagged in the `*_entity`
//! bitsets; CDATA sections are the one deviation: they are never flagged and keep their line
//! ends raw. Names follow the XML 1.0 fifth-edition `NameStartChar` and `NameChar` productions
//! exactly, with an ASCII fast path; a namespace URI is the raw attribute value; duplicate
//! attributes are rejected both by qualified name and by expanded name (Namespaces in XML 1.0,
//! section 6.3); the reserved `xml` and `xmlns` prefixes and namespaces are enforced
//! (section 3). Input that ends inside markup is `UnexpectedEof`; input that ends between
//! complete markup with elements still open is `Unclosed`.

mod decode;
mod ns;
mod parse;

pub use decode::{decode_into, decode_text_run, span_eq_str, SpanKind};
pub use ns::{resolve_attr_ns, resolve_elem_ns, resolve_prefix, Ns};
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

/// Index of a namespace declaration row (the namespace chain of a [`Doc`]).
/// `NONE` ends a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NsIx(pub u32);
const _: () = assert!(core::mem::size_of::<NsIx>() == 4);

impl NsIx {
    pub const NONE: NsIx = NsIx(u32::MAX);
}

/// Index into the parser's stack of in-scope namespace bindings (never leaves the parser).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub(crate) struct BindIx(pub u32);
const _: () = assert!(core::mem::size_of::<BindIx>() == 4);

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
    pub ns_mark: BindIx,
}
const _: () = assert!(core::mem::size_of::<Frame>() == 20);

/// A namespace declaration in scope during parsing.
// Budget: at most `max_ns_bindings` live at once; two spans.
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
/// data produces. `max_ns_bindings` counts the namespace declarations in scope at any point
/// (the sum over the open elements); the real corpus (19,025 messages) peaks at 15, and the
/// default 64 is 4x that rounded up to a power of two. It bounds every prefix lookup, while
/// parsing and in [`resolve_elem_ns`], to that many comparisons.
///
/// [`Parser::new`] refuses a value above its [`Limits::CEILING`] entry, and a zero in a field
/// where zero cannot work (input bytes, depth, elements, name length), with
/// [`XmlError::LimitsInvalid`].
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
    pub max_ns_bindings: usize,
}

impl Limits {
    /// The hard ceiling of every limit. Input bytes: spans are 32-bit. Rows (elements,
    /// attributes, text nodes): 16,777,216, about 0.6 GB of tables at the worst. Depth: 65,536
    /// open elements. Attributes per element: 1,024 (duplicate detection is quadratic in it).
    /// Name length: 65,536 bytes. Namespace bindings: 1,024 (every prefix lookup is linear in
    /// it).
    pub const CEILING: Limits = Limits {
        max_input_bytes: i32::MAX as usize,
        max_depth: 65_536,
        max_elements: 16_777_216,
        max_attributes: 16_777_216,
        max_attrs_per_elem: 1_024,
        max_name_len: 65_536,
        max_text_nodes: 16_777_216,
        max_ns_bindings: 1_024,
    };
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
            max_ns_bindings: 64,
        }
    }
}

/// Checks `l` against the ceilings and the zero rules; the first offending field is reported.
pub fn validate_limits(l: &Limits) -> Result<(), XmlError> {
    let c = Limits::CEILING;
    let fields = [
        (
            LimitKind::InputBytes,
            l.max_input_bytes,
            c.max_input_bytes,
            1,
        ),
        (LimitKind::Depth, l.max_depth, c.max_depth, 1),
        (LimitKind::Elements, l.max_elements, c.max_elements, 1),
        (LimitKind::Attributes, l.max_attributes, c.max_attributes, 0),
        (
            LimitKind::AttributesPerElement,
            l.max_attrs_per_elem,
            c.max_attrs_per_elem,
            0,
        ),
        (LimitKind::NameLength, l.max_name_len, c.max_name_len, 1),
        (LimitKind::TextNodes, l.max_text_nodes, c.max_text_nodes, 0),
        (
            LimitKind::NsBindings,
            l.max_ns_bindings,
            c.max_ns_bindings,
            0,
        ),
    ];
    for (limit, v, ceiling, floor) in fields {
        if v < floor || v > ceiling {
            return Err(XmlError::LimitsInvalid { limit, at: 0 });
        }
    }
    Ok(())
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
    NsBindings,
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
    /// A span that does not lie inside the input; `at` is its start.
    SpanOutOfRange { at: u32 },
    /// A limit that is zero where zero cannot work, above its ceiling, or too large to allocate
    /// (configuration error: there is no input offset, `at` is 0).
    LimitsInvalid { limit: LimitKind, at: u32 },
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
        LimitKind::NsBindings => "max namespace bindings",
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
        | XmlError::OutputTooSmall { at }
        | XmlError::SpanOutOfRange { at }
        | XmlError::LimitsInvalid { at, .. } => at,
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
            XmlError::SpanOutOfRange { .. } => {
                write!(f, "span outside the input (starts at byte {at})")
            }
            XmlError::LimitsInvalid { limit, .. } => {
                write!(f, "invalid limit: {}", limit_name(*limit))
            }
        }
    }
}

impl std::error::Error for XmlError {}

/// A parsed message: borrowed, struct-of-arrays views of the parser's tables plus the input.
/// Row `i` of every `elem_*` slice describes element `i` (document order, root is 0); likewise
/// `attr_*` and `text_*`. An element's attributes are rows `first_attr .. first_attr + count`.
/// For a prefixed name, `name` is the local part and `prefix` the part before the colon (empty
/// span when absent). The `*_entity` bitsets (RULESET A6) mark values that need decoding:
/// they contain a reference, or a byte that line-end or attribute-value normalisation changes
/// (carriage return in text; tab, line feed or carriage return in an attribute value). CDATA
/// text is never flagged.
///
/// Namespace declarations are also kept as a chain per element: `elem_ns_head[e]` is the
/// latest declaration in scope at element `e` (its own, else its nearest ancestor's), and
/// `ns_next[n]` the one it shadows; rows `ns_prefix`/`ns_uri` hold the spans (an empty prefix
/// is the default namespace). A chain is at most [`Limits::max_ns_bindings`] long.
// A7.2: cold struct, size guard waived (a borrowing view of the parser's tables, one per parse)
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
    pub elem_ns_head: &'a [NsIx],
    pub ns_prefix: &'a [Span],
    pub ns_uri: &'a [Span],
    pub ns_next: &'a [NsIx],
    /// The expanded form of the namespace URIs that contained a reference or a normalised
    /// byte; read through [`ns_uri_bytes`].
    pub ns_buf: &'a [u8],
}

/// Marks a namespace URI span whose bytes are in [`Doc::ns_buf`] rather than the input; the
/// offset is in the low 31 bits. Input is limited to below 2 GiB, so no span of the input can
/// carry it, and `span_bytes` on such a span fails (out of range) instead of misreading.
pub(crate) const NS_BUF_FLAG: u32 = 1 << 31;

/// The bytes of a namespace URI span as returned by [`resolve_elem_ns`] and
/// [`resolve_attr_ns`]: the expanded URI, from the input when it holds no reference, else from
/// [`Doc::ns_buf`]. `None` for a span outside either.
pub fn ns_uri_bytes<'a>(doc: &Doc<'a>, s: Span) -> Option<&'a [u8]> {
    if s.start & NS_BUF_FLAG == 0 {
        return bytes_of(doc.input, s);
    }
    let a = (s.start & !NS_BUF_FLAG) as usize;
    doc.ns_buf.get(a..a.checked_add(s.len as usize)?)
}

/// The bytes a span covers, or `SpanOutOfRange` if the span does not lie inside `input`
/// (parsed spans always do; a stale span from another message may not).
pub fn span_bytes(input: &[u8], s: Span) -> Result<&[u8], XmlError> {
    bytes_of(input, s).ok_or(XmlError::SpanOutOfRange { at: s.start })
}

/// The bytes a span covers, `None` if out of range (crate-internal, for callers that hold only
/// parser-made spans).
pub(crate) fn bytes_of(input: &[u8], s: Span) -> Option<&[u8]> {
    let a = s.start as usize;
    let b = a.checked_add(s.len as usize)?;
    input.get(a..b)
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
