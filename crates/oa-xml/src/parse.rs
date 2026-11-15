//! The one-pass parser. Core (pure): a function of the input and the limits. The tables are
//! sized once in [`Parser::new`]; every push is preceded by a limit check, so none reallocates.

use crate::decode::{off, parse_entity};
use crate::{
    validate_limits, AttrIx, BindIx, Binding, Doc, ElemIx, Frame, LimitKind, Limits, NsIx, Span,
    XmlError,
};

/// The XML namespace, bound to the `xml` prefix and to nothing else (Namespaces section 3).
const XML_NS: &[u8] = b"http://www.w3.org/XML/1998/namespace";
/// The namespace of the `xmlns` prefix, which no declaration may bind.
const XMLNS_NS: &[u8] = b"http://www.w3.org/2000/xmlns/";

/// Parser with its preallocated tables. Create once, call [`Parser::parse`] per message.
pub struct Parser {
    limits: Limits,
    elem_name: Vec<Span>,
    elem_prefix: Vec<Span>,
    elem_parent: Vec<ElemIx>,
    elem_first_attr: Vec<AttrIx>,
    elem_attr_count: Vec<u32>,
    elem_first_child: Vec<ElemIx>,
    elem_next_sibling: Vec<ElemIx>,
    attr_name: Vec<Span>,
    attr_prefix: Vec<Span>,
    attr_value: Vec<Span>,
    attr_entity: Vec<u64>,
    text_span: Vec<Span>,
    text_owner: Vec<ElemIx>,
    text_entity: Vec<u64>,
    elem_ns_head: Vec<NsIx>,
    ns_prefix: Vec<Span>,
    ns_uri: Vec<Span>,
    ns_next: Vec<NsIx>,
    stack: Vec<Frame>,
    bindings: Vec<Binding>,
    // Per-element scratch: the namespace URI of each attribute of the element being parsed
    // (empty for an unprefixed, `xml` or `xmlns` attribute), for the expanded-name check.
    attr_uri: Vec<Span>,
}

/// A table of capacity `cap` made with `try_reserve_exact`, so an allocation failure is an
/// error and not an abort.
fn table<T>(cap: usize, limit: LimitKind) -> Result<Vec<T>, XmlError> {
    let mut v = Vec::new();
    v.try_reserve_exact(cap)
        .map_err(|_| XmlError::LimitsInvalid { limit, at: 0 })?;
    Ok(v)
}

/// A zeroed bitset of `bits` bits, allocated the same way.
fn bitset(bits: usize, limit: LimitKind) -> Result<Vec<u64>, XmlError> {
    let words = bits.div_ceil(64);
    let mut v = table(words, limit)?;
    v.resize(words, 0);
    Ok(v)
}

impl Parser {
    /// Allocates every table at `limits`. This is the only allocation the parser makes. The
    /// limits are checked first ([`validate_limits`]: a zero where zero cannot work, or a value
    /// above its [`Limits::CEILING`], is `LimitsInvalid`), and the tables are reserved with
    /// `try_reserve_exact`, so a limit too large for the machine's memory is also
    /// `LimitsInvalid` (naming the limit whose table failed), never an abort.
    pub fn new(limits: Limits) -> Result<Parser, XmlError> {
        validate_limits(&limits)?;
        let el = LimitKind::Elements;
        let at = LimitKind::Attributes;
        let tx = LimitKind::TextNodes;
        Ok(Parser {
            elem_name: table(limits.max_elements, el)?,
            elem_prefix: table(limits.max_elements, el)?,
            elem_parent: table(limits.max_elements, el)?,
            elem_first_attr: table(limits.max_elements, el)?,
            elem_attr_count: table(limits.max_elements, el)?,
            elem_first_child: table(limits.max_elements, el)?,
            elem_next_sibling: table(limits.max_elements, el)?,
            attr_name: table(limits.max_attributes, at)?,
            attr_prefix: table(limits.max_attributes, at)?,
            attr_value: table(limits.max_attributes, at)?,
            attr_entity: bitset(limits.max_attributes, at)?,
            text_span: table(limits.max_text_nodes, tx)?,
            text_owner: table(limits.max_text_nodes, tx)?,
            text_entity: bitset(limits.max_text_nodes, tx)?,
            elem_ns_head: table(limits.max_elements, el)?,
            // Every namespace declaration is an attribute, so rows never outnumber them.
            ns_prefix: table(limits.max_attributes, at)?,
            ns_uri: table(limits.max_attributes, at)?,
            ns_next: table(limits.max_attributes, at)?,
            stack: table(limits.max_depth, LimitKind::Depth)?,
            bindings: table(limits.max_ns_bindings, LimitKind::NsBindings)?,
            attr_uri: table(limits.max_attrs_per_elem, LimitKind::AttributesPerElement)?,
            limits,
        })
    }

    /// Total capacity, in rows and words, of every table. It is fixed at construction; tests use
    /// it to show that parsing never grows a table (and so never reallocates).
    pub fn table_capacity(&self) -> usize {
        self.elem_name.capacity()
            + self.elem_prefix.capacity()
            + self.elem_parent.capacity()
            + self.elem_first_attr.capacity()
            + self.elem_attr_count.capacity()
            + self.elem_first_child.capacity()
            + self.elem_next_sibling.capacity()
            + self.attr_name.capacity()
            + self.attr_prefix.capacity()
            + self.attr_value.capacity()
            + self.attr_entity.capacity()
            + self.text_span.capacity()
            + self.text_owner.capacity()
            + self.text_entity.capacity()
            + self.elem_ns_head.capacity()
            + self.ns_prefix.capacity()
            + self.ns_uri.capacity()
            + self.ns_next.capacity()
            + self.stack.capacity()
            + self.bindings.capacity()
            + self.attr_uri.capacity()
    }

    /// The limits in force.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Parses one complete message. The returned document borrows the parser's tables, so the
    /// next call needs the previous document dropped. On error nothing partial is exposed.
    pub fn parse<'a>(&'a mut self, input: &'a [u8]) -> Result<Doc<'a>, XmlError> {
        self.reset();
        run(self, input)?;
        Ok(Doc {
            input,
            elem_name: &self.elem_name,
            elem_prefix: &self.elem_prefix,
            elem_parent: &self.elem_parent,
            elem_first_attr: &self.elem_first_attr,
            elem_attr_count: &self.elem_attr_count,
            elem_first_child: &self.elem_first_child,
            elem_next_sibling: &self.elem_next_sibling,
            attr_name: &self.attr_name,
            attr_prefix: &self.attr_prefix,
            attr_value: &self.attr_value,
            attr_entity: &self.attr_entity,
            text_span: &self.text_span,
            text_owner: &self.text_owner,
            text_entity: &self.text_entity,
            elem_ns_head: &self.elem_ns_head,
            ns_prefix: &self.ns_prefix,
            ns_uri: &self.ns_uri,
            ns_next: &self.ns_next,
        })
    }

    /// Clears the tables for reuse, zeroing only the bitset words the last run could touch.
    fn reset(&mut self) {
        let aw = self.attr_name.len().div_ceil(64);
        if let Some(w) = self.attr_entity.get_mut(..aw) {
            w.fill(0);
        }
        let tw = self.text_span.len().div_ceil(64);
        if let Some(w) = self.text_entity.get_mut(..tw) {
            w.fill(0);
        }
        self.elem_name.clear();
        self.elem_prefix.clear();
        self.elem_parent.clear();
        self.elem_first_attr.clear();
        self.elem_attr_count.clear();
        self.elem_first_child.clear();
        self.elem_next_sibling.clear();
        self.attr_name.clear();
        self.attr_prefix.clear();
        self.attr_value.clear();
        self.text_span.clear();
        self.text_owner.clear();
        self.elem_ns_head.clear();
        self.ns_prefix.clear();
        self.ns_uri.clear();
        self.ns_next.clear();
        self.stack.clear();
        self.bindings.clear();
        self.attr_uri.clear();
    }
}

// Byte classes for the content scanners.
const C_PLAIN: u8 = 0;
const C_LT: u8 = 1;
const C_AMP: u8 = 2;
const C_CTRL: u8 = 3; // control character outside the Char production
const C_EF: u8 = 4; // 0xEF: lead byte of U+FFFE and U+FFFF
const C_RBRACKET: u8 = 5;
const C_QUOTE: u8 = 6; // `"` or `'`
const C_CR: u8 = 7; // carriage return: normalised to a line feed (text) or space (attribute)
const C_NL: u8 = 8; // tab or line feed: normalised to a space in an attribute value only

// Flags returned by `scan_run`: what a value needs from `decode_into`.
const F_REF: u8 = 1; // contains an entity or character reference
const F_NORM: u8 = 2; // contains a byte that normalisation changes

// Const-evaluated: an out-of-range index here is a compile error, not a runtime panic.
#[allow(clippy::indexing_slicing)]
const fn class_table() -> [u8; 256] {
    let mut t = [C_PLAIN; 256];
    let mut i = 0;
    while i < 0x20 {
        if i != 9 && i != 10 && i != 13 {
            t[i] = C_CTRL;
        }
        i += 1;
    }
    t[b'<' as usize] = C_LT;
    t[b'&' as usize] = C_AMP;
    t[0xEF] = C_EF;
    t[b']' as usize] = C_RBRACKET;
    t[b'"' as usize] = C_QUOTE;
    t[b'\'' as usize] = C_QUOTE;
    t[b'\r' as usize] = C_CR;
    t[b'\t' as usize] = C_NL;
    t[b'\n' as usize] = C_NL;
    t
}
static CLASS: [u8; 256] = class_table();

// Name byte classes: 0 not a name byte, 1 name byte but not a start, 2 name start, 3 colon,
// 4 a byte of a multi-byte character (judged by `nonascii_name`; the input is valid UTF-8).
// Const-evaluated: an out-of-range index here is a compile error, not a runtime panic.
#[allow(clippy::indexing_slicing)]
const fn name_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let b = i as u8;
        t[i] = if b >= 0x80 {
            4
        } else if b.is_ascii_alphabetic() || b == b'_' {
            2
        } else if b.is_ascii_digit() || b == b'-' || b == b'.' {
            1
        } else if b == b':' {
            3
        } else {
            0
        };
        i += 1;
    }
    t
}
static NAME: [u8; 256] = name_table();

/// Decodes the UTF-8 character at `input[i]` into (code point, length). `None` on anything
/// malformed (cannot happen after validation; refused rather than assumed).
fn decode_cp(input: &[u8], i: usize) -> Option<(u32, usize)> {
    let b0 = u32::from(*input.get(i)?);
    let cont = |k: usize| input.get(i + k).map(|&b| u32::from(b & 0x3F));
    match b0 {
        0xC2..=0xDF => Some((((b0 & 0x1F) << 6) | cont(1)?, 2)),
        0xE0..=0xEF => Some((((b0 & 0x0F) << 12) | (cont(1)? << 6) | cont(2)?, 3)),
        0xF0..=0xF4 => Some((
            ((b0 & 0x07) << 18) | (cont(1)? << 12) | (cont(2)? << 6) | cont(3)?,
            4,
        )),
        _ => None,
    }
}

/// XML 1.0 (fifth edition) `NameStartChar`, minus `:` and the ASCII range (which the byte
/// table decides); applied to code points from U+0080 up.
fn is_name_start_cp(c: u32) -> bool {
    matches!(c,
        0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF
        | 0x200C..=0x200D | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF
        | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
}

/// XML 1.0 (fifth edition) `NameChar` for code points from U+0080 up: a start character, or
/// U+00B7, U+0300-U+036F, U+203F-U+2040.
fn is_name_cp(c: u32) -> bool {
    is_name_start_cp(c) || matches!(c, 0xB7 | 0x300..=0x36F | 0x203F..=0x2040)
}

/// Whether the non-ASCII character at `input[i]` is a name character (`start`: a name-start
/// character); returns its byte length when it is.
fn nonascii_name(input: &[u8], i: usize, start: bool) -> Option<usize> {
    let (c, len) = decode_cp(input, i)?;
    let ok = if start {
        is_name_start_cp(c)
    } else {
        is_name_cp(c)
    };
    ok.then_some(len)
}

/// Whether a name-start character (not a colon) is at `input[i]`.
fn name_start_at(input: &[u8], i: usize) -> bool {
    match at(input, i).map_or(0, |b| NAME.get(usize::from(b)).copied().unwrap_or(0)) {
        2 => true,
        4 => nonascii_name(input, i, true).is_some(),
        _ => false,
    }
}

/// Whether `rest` is a proper prefix of `pat` (input that stops where `pat` would go on).
fn cut_short(rest: &[u8], pat: &[u8]) -> bool {
    rest.len() < pat.len() && pat.starts_with(rest)
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_ws(input: &[u8], mut i: usize) -> usize {
    while input.get(i).is_some_and(|&b| is_ws(b)) {
        i += 1;
    }
    i
}

fn at(input: &[u8], i: usize) -> Option<u8> {
    input.get(i).copied()
}

fn sub(input: &[u8], a: usize, b: usize) -> &[u8] {
    input.get(a..b).unwrap_or(&[])
}

fn starts(input: &[u8], i: usize, pat: &[u8]) -> bool {
    input.get(i..).is_some_and(|r| r.starts_with(pat))
}

fn starts_nocase(input: &[u8], i: usize, pat: &[u8]) -> bool {
    input
        .get(i..i + pat.len())
        .is_some_and(|r| r.eq_ignore_ascii_case(pat))
}

fn syntax(i: usize, what: &'static str) -> XmlError {
    XmlError::Syntax { at: off(i), what }
}

fn span(a: usize, b: usize) -> Span {
    Span {
        start: off(a),
        len: off(b.saturating_sub(a)),
    }
}

fn set_bit(bits: &mut [u64], i: usize) {
    if let Some(w) = bits.get_mut(i >> 6) {
        *w |= 1 << (i & 63);
    }
}

/// Checks the two byte classes that need more than one byte to judge: a control character, and
/// U+FFFE / U+FFFF (EF BF BE / EF BF BF). Called with `class` already looked up.
fn check_special(input: &[u8], i: usize, class: u8) -> Result<(), XmlError> {
    match class {
        C_CTRL => Err(XmlError::BadChar { at: off(i) }),
        C_EF if at(input, i + 1) == Some(0xBF) && matches!(at(input, i + 2), Some(0xBE | 0xBF)) => {
            Err(XmlError::BadChar { at: off(i) })
        }
        _ => Ok(()),
    }
}

/// Scans text (`quote == 0`, ends at `<`) or an attribute value (ends at `quote`), validating
/// characters and references. Returns the end offset and the `F_*` flags of what `decode_into`
/// will have to do with the value. Hitting the end of the input returns `input.len()`; the
/// caller decides what that means.
fn scan_run(input: &[u8], start: usize, quote: u8) -> Result<(usize, u8), XmlError> {
    let mut i = start;
    let mut flags = 0u8;
    while let Some(&b) = input.get(i) {
        let c = CLASS.get(usize::from(b)).copied().unwrap_or(C_PLAIN);
        match c {
            C_PLAIN => {}
            C_LT => {
                if quote == 0 {
                    return Ok((i, flags));
                }
                return Err(syntax(i, "'<' in attribute value"));
            }
            C_AMP => {
                let (_, used) = parse_entity(input, i)?;
                flags |= F_REF;
                i += used;
                continue;
            }
            C_QUOTE => {
                if b == quote {
                    return Ok((i, flags));
                }
            }
            C_RBRACKET => {
                if quote == 0 && starts(input, i, b"]]>") {
                    return Err(syntax(i, "']]>' in text"));
                }
            }
            C_CR => flags |= F_NORM,
            C_NL => {
                if quote != 0 {
                    flags |= F_NORM;
                }
            }
            _ => check_special(input, i, c)?,
        }
        i += 1;
    }
    Ok((input.len(), flags))
}

/// Reads a qualified name at `start`. Returns the end offset and the colon offset (or
/// `usize::MAX`). Enforces the name length limit. Names are the fifth-edition productions:
/// ASCII by the byte table, anything else by decoding the code point.
fn read_qname(limits: &Limits, input: &[u8], start: usize) -> Result<(usize, usize), XmlError> {
    if !name_start_at(input, start) {
        if at(input, start).is_none() {
            return Err(XmlError::UnexpectedEof { at: off(start) });
        }
        if at(input, start) == Some(0xEF) {
            check_special(input, start, C_EF)?;
        }
        return Err(syntax(start, "name expected"));
    }
    let mut i = start;
    let mut colon = usize::MAX;
    while let Some(&b) = input.get(i) {
        match NAME.get(usize::from(b)).copied().unwrap_or(0) {
            1 | 2 => i += 1,
            3 => {
                if colon != usize::MAX {
                    return Err(syntax(i, "second colon in name"));
                }
                colon = i;
                if at(input, i + 1).is_none() {
                    return Err(XmlError::UnexpectedEof { at: off(i + 1) });
                }
                if !name_start_at(input, i + 1) {
                    return Err(syntax(i + 1, "name expected after colon"));
                }
                i += 1;
            }
            4 => match nonascii_name(input, i, false) {
                Some(len) => i += len,
                None => {
                    // U+FFFE and U+FFFF are not characters at all: say so.
                    if b == 0xEF {
                        check_special(input, i, C_EF)?;
                    }
                    break;
                }
            },
            _ => break,
        }
    }
    if i - start > limits.max_name_len {
        return Err(XmlError::Limit {
            limit: LimitKind::NameLength,
            at: off(start),
        });
    }
    Ok((i, colon))
}

fn run(p: &mut Parser, input: &[u8]) -> Result<(), XmlError> {
    let max_in = p.limits.max_input_bytes;
    if input.len() > max_in {
        return Err(XmlError::Limit {
            limit: LimitKind::InputBytes,
            at: off(max_in),
        });
    }
    if let Err(e) = core::str::from_utf8(input) {
        return Err(XmlError::Utf8 {
            at: off(e.valid_up_to()),
        });
    }
    let mut pos = if starts(input, 0, &[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    // Input that stops inside the opening of an XML declaration is cut short, not a PI.
    if input.get(pos..).is_some_and(|r| cut_short(r, b"<?xml ")) && pos < input.len() {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    if starts(input, pos, b"<?xml") && at(input, pos + 5).is_some_and(is_ws) {
        pos = xml_decl(input, pos)?;
    }
    pos = prolog(input, pos)?;
    content(p, input, pos)
}

/// The error for a malformed XML declaration at `p`: `UnexpectedEof` if the input simply stops
/// there or inside the next keyword, otherwise `BadXmlDecl`.
fn decl_err(input: &[u8], p: usize) -> XmlError {
    let rest = input.get(p..).unwrap_or(&[]);
    let stops = rest.is_empty()
        || [&b"version"[..], b"encoding", b"standalone", b"?>"]
            .iter()
            .any(|k| cut_short(rest, k));
    if stops {
        XmlError::UnexpectedEof {
            at: off(input.len()),
        }
    } else {
        XmlError::BadXmlDecl { at: off(p) }
    }
}

/// Reads one pseudo-attribute (`key = "value"`) of the XML declaration if the next one is
/// `key`. Whitespace must precede it.
fn decl_pair<'a>(
    input: &'a [u8],
    pos: &mut usize,
    key: &[u8],
) -> Result<Option<(usize, &'a [u8])>, XmlError> {
    let p0 = skip_ws(input, *pos);
    if !starts(input, p0, key) {
        return Ok(None);
    }
    if p0 == *pos {
        return Err(XmlError::BadXmlDecl { at: off(p0) });
    }
    let mut p = skip_ws(input, p0 + key.len());
    if at(input, p) != Some(b'=') {
        return Err(decl_err(input, p));
    }
    p = skip_ws(input, p + 1);
    let q = match at(input, p) {
        Some(q @ (b'"' | b'\'')) => q,
        _ => return Err(decl_err(input, p)),
    };
    let vstart = p + 1;
    let Some(len) = input
        .get(vstart..)
        .and_then(|r| r.iter().position(|&b| b == q))
    else {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    };
    *pos = vstart + len + 1;
    Ok(Some((vstart, sub(input, vstart, vstart + len))))
}

/// Parses the XML declaration at `pos` (which starts with `<?xml` and a space) and returns the
/// offset after it.
fn xml_decl(input: &[u8], pos: usize) -> Result<usize, XmlError> {
    let mut p = pos + 5;
    match decl_pair(input, &mut p, b"version")? {
        Some((_, b"1.0")) => {}
        Some((v, _)) => return Err(XmlError::UnsupportedVersion { at: off(v) }),
        None => return Err(decl_err(input, skip_ws(input, p))),
    }
    if let Some((v, enc)) = decl_pair(input, &mut p, b"encoding")? {
        if !enc.eq_ignore_ascii_case(b"utf-8") {
            return Err(XmlError::UnsupportedEncoding { at: off(v) });
        }
    }
    if let Some((v, sa)) = decl_pair(input, &mut p, b"standalone")? {
        if sa != b"yes" && sa != b"no" {
            return Err(XmlError::BadXmlDecl { at: off(v) });
        }
    }
    p = skip_ws(input, p);
    if starts(input, p, b"?>") {
        Ok(p + 2)
    } else {
        Err(decl_err(input, p))
    }
}

/// Handles markup that may appear outside the root: a comment (returns the offset after it),
/// otherwise an error for DOCTYPE, a processing instruction, or anything else.
fn misc_markup(input: &[u8], pos: usize) -> Result<usize, XmlError> {
    if input.get(pos..).is_some_and(|r| cut_short(r, b"<!--")) {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    if starts(input, pos, b"<!--") {
        return comment(input, pos);
    }
    if starts_nocase(input, pos, b"<!DOCTYPE") {
        return Err(XmlError::Doctype { at: off(pos) });
    }
    if starts(input, pos, b"<?") {
        return Err(XmlError::ProcessingInstruction { at: off(pos) });
    }
    Err(XmlError::OutsideRoot { at: off(pos) })
}

/// Skips whitespace and comments before the root; returns the offset of the root's `<`.
fn prolog(input: &[u8], mut pos: usize) -> Result<usize, XmlError> {
    loop {
        pos = skip_ws(input, pos);
        match at(input, pos) {
            None => return Err(XmlError::NoRoot { at: off(pos) }),
            Some(b'<') => {
                if name_start_at(input, pos + 1) {
                    return Ok(pos);
                }
                if at(input, pos + 1) == Some(0xEF) {
                    check_special(input, pos + 1, C_EF)?;
                }
                if at(input, pos + 1).is_none() {
                    return Err(XmlError::UnexpectedEof { at: off(pos + 1) });
                }
                if starts(input, pos, b"</") || starts(input, pos, b"<![CDATA[") {
                    return Err(XmlError::OutsideRoot { at: off(pos) });
                }
                if !starts(input, pos, b"<!") && !starts(input, pos, b"<?") {
                    return Err(syntax(pos + 1, "name expected"));
                }
                pos = misc_markup(input, pos)?;
            }
            Some(_) => return Err(XmlError::OutsideRoot { at: off(pos) }),
        }
    }
}

/// After the root: whitespace and comments only.
fn epilogue(input: &[u8], mut pos: usize) -> Result<(), XmlError> {
    loop {
        pos = skip_ws(input, pos);
        if pos >= input.len() {
            return Ok(());
        }
        if at(input, pos) != Some(b'<') {
            return Err(XmlError::OutsideRoot { at: off(pos) });
        }
        pos = misc_markup(input, pos)?;
    }
}

/// Parses a comment starting at `pos` (`<!--`); returns the offset after `-->`. A `--` not
/// followed by `>` is an error, as the XML grammar requires.
fn comment(input: &[u8], pos: usize) -> Result<usize, XmlError> {
    let mut i = pos + 4;
    while let Some(&b) = input.get(i) {
        if b == b'-' && at(input, i + 1) == Some(b'-') {
            return match at(input, i + 2) {
                Some(b'>') => Ok(i + 3),
                Some(_) => Err(syntax(i, "'--' inside comment")),
                None => Err(XmlError::UnexpectedEof {
                    at: off(input.len()),
                }),
            };
        }
        let c = CLASS.get(usize::from(b)).copied().unwrap_or(C_PLAIN);
        check_special(input, i, c)?;
        i += 1;
    }
    Err(XmlError::UnexpectedEof {
        at: off(input.len()),
    })
}

fn content(p: &mut Parser, input: &[u8], start: usize) -> Result<(), XmlError> {
    let mut pos = start;
    // Whether the content just before `pos` ended in stored text (a text run or a CDATA
    // section, comments aside), so that a whitespace run next to it is data, not layout.
    let mut data = false;
    loop {
        let Some(b) = at(input, pos) else {
            return Err(XmlError::Unclosed {
                at: off(input.len()),
            });
        };
        let before = p.text_span.len();
        if b != b'<' {
            pos = text(p, input, pos, data)?;
            data |= p.text_span.len() > before;
            continue;
        }
        pos = match at(input, pos + 1) {
            Some(b'/') => {
                data = false;
                end_tag(p, input, pos)?
            }
            Some(b'!') => {
                let next = bang(p, input, pos)?;
                data |= p.text_span.len() > before;
                next
            }
            Some(b'?') => return Err(XmlError::ProcessingInstruction { at: off(pos) }),
            Some(_) => {
                data = false;
                start_tag(p, input, pos)?
            }
            None => return Err(XmlError::UnexpectedEof { at: off(pos + 1) }),
        };
        if p.stack.is_empty() {
            return epilogue(input, pos);
        }
    }
}

fn push_text(p: &mut Parser, input_pos: usize, s: Span, ent: bool) -> Result<(), XmlError> {
    if p.text_span.len() >= p.limits.max_text_nodes {
        return Err(XmlError::Limit {
            limit: LimitKind::TextNodes,
            at: off(input_pos),
        });
    }
    let owner = p.stack.last().map_or(ElemIx::NONE, |f| f.elem);
    if ent {
        set_bit(&mut p.text_entity, p.text_span.len());
    }
    p.text_span.push(s);
    p.text_owner.push(owner);
    Ok(())
}

fn text(p: &mut Parser, input: &[u8], pos: usize, after_data: bool) -> Result<usize, XmlError> {
    let (end, flags) = scan_run(input, pos, 0)?;
    // Whitespace-only runs between markup are layout, not data (unless a reference is in it, or
    // the run sits next to text or a CDATA section of the same element: `a<!--c--> <!--d-->b`).
    let layout = flags & F_REF == 0
        && sub(input, pos, end).iter().all(|&b| is_ws(b))
        && !after_data
        && !data_follows(input, end);
    if !layout {
        push_text(p, pos, span(pos, end), flags != 0)?;
    }
    Ok(end)
}

/// Whether the next content after `pos`, skipping comments and further whitespace, is text or a
/// CDATA section. (Whitespace in a run that ends at a comment is judged with the run after it.)
fn data_follows(input: &[u8], mut pos: usize) -> bool {
    loop {
        if starts(input, pos, b"<!--") {
            match comment(input, pos) {
                Ok(next) => pos = next,
                Err(_) => return false,
            }
        } else if starts(input, pos, b"<![CDATA[") {
            return true;
        } else {
            match at(input, pos) {
                Some(b) if is_ws(b) => pos += 1,
                Some(b'<') | None => return false,
                Some(_) => return true,
            }
        }
    }
}

/// `<!...` inside an element: a comment or CDATA section.
fn bang(p: &mut Parser, input: &[u8], pos: usize) -> Result<usize, XmlError> {
    if starts(input, pos, b"<!--") {
        return comment(input, pos);
    }
    if starts(input, pos, b"<![CDATA[") {
        let from = pos + 9;
        let mut i = from;
        while let Some(&b) = input.get(i) {
            if b == b']' && starts(input, i, b"]]>") {
                if i > from {
                    push_text(p, from, span(from, i), false)?;
                }
                return Ok(i + 3);
            }
            let c = CLASS.get(usize::from(b)).copied().unwrap_or(C_PLAIN);
            check_special(input, i, c)?;
            i += 1;
        }
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    if starts_nocase(input, pos, b"<!DOCTYPE") {
        return Err(XmlError::Doctype { at: off(pos) });
    }
    let rest = input.get(pos..).unwrap_or(&[]);
    if cut_short(rest, b"<!--") || cut_short(rest, b"<![CDATA[") {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    Err(syntax(pos, "unknown markup declaration"))
}

fn end_tag(p: &mut Parser, input: &[u8], pos: usize) -> Result<usize, XmlError> {
    let (q_end, _) = read_qname(&p.limits, input, pos + 2)?;
    let Some(top) = p.stack.last().copied() else {
        return Err(XmlError::MismatchedTag { at: off(pos) });
    };
    let open = sub(
        input,
        top.qname_start as usize,
        (top.qname_start as usize).saturating_add(top.qname_len as usize),
    );
    let name = sub(input, pos + 2, q_end);
    if name != open {
        // A name that runs into the end of the input and is a start of the open name is a cut.
        if q_end >= input.len() && open.starts_with(name) {
            return Err(XmlError::UnexpectedEof {
                at: off(input.len()),
            });
        }
        return Err(XmlError::MismatchedTag { at: off(pos) });
    }
    let i = skip_ws(input, q_end);
    match at(input, i) {
        Some(b'>') => {}
        Some(_) => return Err(syntax(i, "'>' expected")),
        None => {
            return Err(XmlError::UnexpectedEof {
                at: off(input.len()),
            })
        }
    }
    p.stack.pop();
    p.bindings.truncate(top.ns_mark.0 as usize);
    Ok(i + 1)
}

/// The URI of the innermost in-scope declaration for `prefix` (empty = default), if any. At
/// most `max_ns_bindings` comparisons.
fn bound(bindings: &[Binding], input: &[u8], prefix: &[u8]) -> Option<Span> {
    bindings
        .iter()
        .rev()
        .find(|b| bytes_eq(input, b.prefix, prefix))
        .map(|b| b.uri)
}

fn bytes_eq(input: &[u8], s: Span, want: &[u8]) -> bool {
    sub(
        input,
        s.start as usize,
        (s.start as usize).saturating_add(s.len as usize),
    ) == want
}

fn start_tag(p: &mut Parser, input: &[u8], lt: usize) -> Result<usize, XmlError> {
    if p.stack.len() >= p.limits.max_depth {
        return Err(XmlError::Limit {
            limit: LimitKind::Depth,
            at: off(lt),
        });
    }
    if p.elem_name.len() >= p.limits.max_elements {
        return Err(XmlError::Limit {
            limit: LimitKind::Elements,
            at: off(lt),
        });
    }
    let q_start = lt + 1;
    let (q_end, colon) = read_qname(&p.limits, input, q_start)?;
    let first_attr = p.attr_name.len();
    let mark = BindIx(off(p.bindings.len()));
    let mut i = q_end;
    let self_close;
    loop {
        let ws_start = i;
        i = skip_ws(input, i);
        match at(input, i) {
            Some(b'>') => {
                i += 1;
                self_close = false;
                break;
            }
            Some(b'/') => {
                match at(input, i + 1) {
                    Some(b'>') => {}
                    Some(_) => return Err(syntax(i, "'>' expected after '/'")),
                    None => {
                        return Err(XmlError::UnexpectedEof {
                            at: off(input.len()),
                        })
                    }
                }
                i += 2;
                self_close = true;
                break;
            }
            Some(_) => {
                if i == ws_start {
                    return Err(syntax(i, "whitespace expected before attribute"));
                }
                i = attribute(p, input, i, first_attr)?;
            }
            None => {
                return Err(XmlError::UnexpectedEof {
                    at: off(input.len()),
                })
            }
        }
    }

    // Namespace declarations take effect for the element's own name and attributes. Each goes
    // on the in-scope stack (limit `max_ns_bindings`) and on the document's chain.
    let attr_end = p.attr_name.len();
    let mut head = match p.stack.last() {
        Some(top) => p
            .elem_ns_head
            .get(top.elem.0 as usize)
            .copied()
            .unwrap_or(NsIx::NONE),
        None => NsIx::NONE,
    };
    for a in first_attr..attr_end {
        let (Some(&ap), Some(&an), Some(&av)) = (
            p.attr_prefix.get(a),
            p.attr_name.get(a),
            p.attr_value.get(a),
        ) else {
            continue;
        };
        let name = sub(input, an.start as usize, (an.start + an.len) as usize);
        let pre = sub(input, ap.start as usize, (ap.start + ap.len) as usize);
        let uri = sub(input, av.start as usize, (av.start + av.len) as usize);
        let binding = if pre == b"xmlns" {
            if name == b"xmlns" {
                return Err(syntax(ap.start as usize, "reserved prefix 'xmlns'"));
            }
            if av.len == 0 {
                return Err(syntax(av.start as usize, "empty namespace URI"));
            }
            if name == b"xml" {
                // `xml` may be declared, but only as the XML namespace; there is nothing to
                // record, it is always in scope.
                if uri != XML_NS {
                    return Err(syntax(
                        av.start as usize,
                        "prefix 'xml' bound to another URI",
                    ));
                }
                continue;
            }
            Binding {
                prefix: an,
                uri: av,
            }
        } else if pre.is_empty() && name == b"xmlns" {
            Binding {
                prefix: Span {
                    start: an.start,
                    len: 0,
                },
                uri: av,
            }
        } else {
            continue;
        };
        if uri == XML_NS {
            return Err(syntax(
                av.start as usize,
                "XML namespace bound to another prefix",
            ));
        }
        if uri == XMLNS_NS {
            return Err(syntax(
                av.start as usize,
                "the 'xmlns' namespace cannot be bound",
            ));
        }
        if p.bindings.len() >= p.limits.max_ns_bindings {
            return Err(XmlError::Limit {
                limit: LimitKind::NsBindings,
                at: ap.start,
            });
        }
        p.bindings.push(binding);
        let row = NsIx(off(p.ns_prefix.len()));
        p.ns_prefix.push(binding.prefix);
        p.ns_uri.push(binding.uri);
        p.ns_next.push(head);
        head = row;
    }
    // Every prefix used must be declared (`xml` is predefined).
    if colon != usize::MAX {
        let pre = sub(input, q_start, colon);
        if pre != b"xml" && bound(&p.bindings, input, pre).is_none() {
            return Err(XmlError::UndeclaredPrefix { at: off(q_start) });
        }
    }
    // Attribute prefixes, and their URIs for the expanded-name check below.
    let mut prefixed = 0usize;
    for a in first_attr..attr_end {
        let Some(&ap) = p.attr_prefix.get(a) else {
            continue;
        };
        let mut uri = Span::default();
        if ap.len > 0 {
            let pre = sub(input, ap.start as usize, (ap.start + ap.len) as usize);
            if pre != b"xml" && pre != b"xmlns" {
                match bound(&p.bindings, input, pre) {
                    Some(u) => {
                        uri = u;
                        prefixed += 1;
                    }
                    None => return Err(XmlError::UndeclaredPrefix { at: ap.start }),
                }
            }
        }
        p.attr_uri.push(uri);
    }
    // Two attributes with the same local name in the same namespace are duplicates even under
    // different prefixes (Namespaces in XML 1.0, section 6.3). Quadratic in the attributes of
    // one element, which are at most `max_attrs_per_elem` (64 by default); only prefixed
    // attributes take part, and only when there are two or more.
    if prefixed >= 2 {
        check_expanded_duplicates(p, input, first_attr)?;
    }
    p.attr_uri.clear();

    let e = ElemIx(off(p.elem_name.len()));
    let (name, prefix) = if colon == usize::MAX {
        (
            span(q_start, q_end),
            Span {
                start: off(q_start),
                len: 0,
            },
        )
    } else {
        (span(colon + 1, q_end), span(q_start, colon))
    };
    let parent = match p.stack.last_mut() {
        Some(top) => {
            let parent = top.elem;
            if top.last_child == ElemIx::NONE {
                if let Some(slot) = p.elem_first_child.get_mut(parent.0 as usize) {
                    *slot = e;
                }
            } else if let Some(slot) = p.elem_next_sibling.get_mut(top.last_child.0 as usize) {
                *slot = e;
            }
            top.last_child = e;
            parent
        }
        None => ElemIx::NONE,
    };
    p.elem_name.push(name);
    p.elem_prefix.push(prefix);
    p.elem_parent.push(parent);
    p.elem_first_attr.push(AttrIx(off(first_attr)));
    p.elem_attr_count.push(off(attr_end - first_attr));
    p.elem_first_child.push(ElemIx::NONE);
    p.elem_next_sibling.push(ElemIx::NONE);
    p.elem_ns_head.push(head);

    if self_close {
        p.bindings.truncate(mark.0 as usize);
    } else {
        p.stack.push(Frame {
            elem: e,
            last_child: ElemIx::NONE,
            qname_start: off(q_start),
            qname_len: off(q_end - q_start),
            ns_mark: mark,
        });
    }
    Ok(i)
}

/// Rejects two prefixed attributes of the element whose attributes start at `first_attr` that
/// share a local name and a namespace URI. `p.attr_uri` holds each attribute's URI (empty for
/// an attribute that takes no part).
fn check_expanded_duplicates(p: &Parser, input: &[u8], first_attr: usize) -> Result<(), XmlError> {
    for (j, &uj) in p.attr_uri.iter().enumerate() {
        if uj.len == 0 {
            continue;
        }
        let Some(&nj) = p.attr_name.get(first_attr + j) else {
            continue;
        };
        for (i, &ui) in p.attr_uri.iter().enumerate().take(j) {
            if ui.len != uj.len {
                continue;
            }
            let Some(&ni) = p.attr_name.get(first_attr + i) else {
                continue;
            };
            if ni.len == nj.len
                && sub(input, ni.start as usize, (ni.start + ni.len) as usize)
                    == sub(input, nj.start as usize, (nj.start + nj.len) as usize)
                && sub(input, ui.start as usize, (ui.start + ui.len) as usize)
                    == sub(input, uj.start as usize, (uj.start + uj.len) as usize)
            {
                let at = p
                    .attr_prefix
                    .get(first_attr + j)
                    .map_or(nj.start, |pre| pre.start);
                return Err(XmlError::DuplicateAttribute { at });
            }
        }
    }
    Ok(())
}

/// Parses one attribute starting at `i`; returns the offset after its closing quote.
fn attribute(
    p: &mut Parser,
    input: &[u8],
    start: usize,
    first_attr: usize,
) -> Result<usize, XmlError> {
    if p.attr_name.len() - first_attr >= p.limits.max_attrs_per_elem {
        return Err(XmlError::Limit {
            limit: LimitKind::AttributesPerElement,
            at: off(start),
        });
    }
    if p.attr_name.len() >= p.limits.max_attributes {
        return Err(XmlError::Limit {
            limit: LimitKind::Attributes,
            at: off(start),
        });
    }
    let (q_end, colon) = read_qname(&p.limits, input, start)?;
    let mut i = skip_ws(input, q_end);
    match at(input, i) {
        Some(b'=') => {}
        Some(_) => return Err(syntax(i, "'=' expected after attribute name")),
        None => return Err(XmlError::UnexpectedEof { at: off(i) }),
    }
    i = skip_ws(input, i + 1);
    let quote = match at(input, i) {
        Some(q @ (b'"' | b'\'')) => q,
        Some(_) => return Err(syntax(i, "quoted attribute value expected")),
        None => return Err(XmlError::UnexpectedEof { at: off(i) }),
    };
    let vstart = i + 1;
    let (vend, flags) = scan_run(input, vstart, quote)?;
    if at(input, vend) != Some(quote) {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    let (name, prefix) = if colon == usize::MAX {
        (
            span(start, q_end),
            Span {
                start: off(start),
                len: 0,
            },
        )
    } else {
        (span(colon + 1, q_end), span(start, colon))
    };
    // Duplicates by qualified name here (cheap, needs no namespace); by expanded name once the
    // element's declarations are known (`check_expanded_duplicates`). At most
    // `max_attrs_per_elem` earlier rows to compare.
    let this_q = sub(input, start, q_end);
    for a in first_attr..p.attr_name.len() {
        let (Some(&an), Some(&ap)) = (p.attr_name.get(a), p.attr_prefix.get(a)) else {
            continue;
        };
        if an.len == name.len && ap.len == prefix.len {
            let q_start = if ap.len == 0 { an.start } else { ap.start } as usize;
            let q_len = (an.start + an.len) as usize - q_start;
            if sub(input, q_start, q_start + q_len) == this_q {
                return Err(XmlError::DuplicateAttribute { at: off(start) });
            }
        }
    }
    if flags != 0 {
        set_bit(&mut p.attr_entity, p.attr_name.len());
    }
    p.attr_name.push(name);
    p.attr_prefix.push(prefix);
    p.attr_value.push(span(vstart, vend));
    Ok(vend + 1)
}

#[cfg(test)]
mod tests {
    use crate::decode::is_xml_char;

    // REQ: OA-XML-008
    #[test]
    fn char_production_boundaries() {
        assert!(is_xml_char(0x9) && is_xml_char(0xA) && is_xml_char(0xD));
        assert!(!is_xml_char(0x0) && !is_xml_char(0x8) && !is_xml_char(0xB) && !is_xml_char(0xC));
        assert!(!is_xml_char(0xD800) && !is_xml_char(0xFFFE) && !is_xml_char(0xFFFF));
        assert!(is_xml_char(0xFFFD) && is_xml_char(0x10000) && !is_xml_char(0x110000));
    }
}
