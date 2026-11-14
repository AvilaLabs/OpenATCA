//! The one-pass parser. Core (pure): a function of the input and the limits. The tables are
//! sized once in [`Parser::new`]; every push is preceded by a limit check, so none reallocates.

use crate::decode::{off, parse_entity};
use crate::{AttrIx, Binding, Doc, ElemIx, Frame, LimitKind, Limits, Span, XmlError};

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
    stack: Vec<Frame>,
    bindings: Vec<Binding>,
    binding_cap: usize,
}

impl Parser {
    /// Allocates every table at `limits`. This is the only allocation the parser makes. The
    /// input limit is capped at `u32::MAX` because spans use 32-bit offsets.
    pub fn new(limits: Limits) -> Parser {
        let mut limits = limits;
        limits.max_input_bytes = limits.max_input_bytes.min(u32::MAX as usize);
        let binding_cap = limits.max_depth.saturating_mul(limits.max_attrs_per_elem);
        Parser {
            elem_name: Vec::with_capacity(limits.max_elements),
            elem_prefix: Vec::with_capacity(limits.max_elements),
            elem_parent: Vec::with_capacity(limits.max_elements),
            elem_first_attr: Vec::with_capacity(limits.max_elements),
            elem_attr_count: Vec::with_capacity(limits.max_elements),
            elem_first_child: Vec::with_capacity(limits.max_elements),
            elem_next_sibling: Vec::with_capacity(limits.max_elements),
            attr_name: Vec::with_capacity(limits.max_attributes),
            attr_prefix: Vec::with_capacity(limits.max_attributes),
            attr_value: Vec::with_capacity(limits.max_attributes),
            attr_entity: vec![0; limits.max_attributes.div_ceil(64)],
            text_span: Vec::with_capacity(limits.max_text_nodes),
            text_owner: Vec::with_capacity(limits.max_text_nodes),
            text_entity: vec![0; limits.max_text_nodes.div_ceil(64)],
            stack: Vec::with_capacity(limits.max_depth),
            bindings: Vec::with_capacity(binding_cap),
            binding_cap,
            limits,
        }
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
            + self.stack.capacity()
            + self.bindings.capacity()
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
        self.stack.clear();
        self.bindings.clear();
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
    t
}
static CLASS: [u8; 256] = class_table();

// Name byte classes: 0 not a name byte, 1 name byte but not a start, 2 name start, 3 colon.
// Const-evaluated: an out-of-range index here is a compile error, not a runtime panic.
#[allow(clippy::indexing_slicing)]
const fn name_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let b = i as u8;
        t[i] = if b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 {
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
/// characters and references. Returns the end offset and whether a reference was seen. Hitting
/// the end of the input returns `input.len()`; the caller decides what that means.
fn scan_run(input: &[u8], start: usize, quote: u8) -> Result<(usize, bool), XmlError> {
    let mut i = start;
    let mut ent = false;
    while let Some(&b) = input.get(i) {
        let c = CLASS.get(usize::from(b)).copied().unwrap_or(C_PLAIN);
        match c {
            C_PLAIN => {}
            C_LT => {
                if quote == 0 {
                    return Ok((i, ent));
                }
                return Err(syntax(i, "'<' in attribute value"));
            }
            C_AMP => {
                let (_, used) = parse_entity(input, i)?;
                ent = true;
                i += used;
                continue;
            }
            C_QUOTE => {
                if b == quote {
                    return Ok((i, ent));
                }
            }
            C_RBRACKET => {
                if quote == 0 && starts(input, i, b"]]>") {
                    return Err(syntax(i, "']]>' in text"));
                }
            }
            _ => check_special(input, i, c)?,
        }
        i += 1;
    }
    Ok((input.len(), ent))
}

/// Reads a qualified name at `start`. Returns the end offset and the colon offset (or
/// `usize::MAX`). Enforces the name length limit.
fn read_qname(limits: &Limits, input: &[u8], start: usize) -> Result<(usize, usize), XmlError> {
    let first = at(input, start).map_or(0, |b| NAME.get(usize::from(b)).copied().unwrap_or(0));
    if first != 2 {
        return Err(syntax(start, "name expected"));
    }
    let mut i = start + 1;
    let mut colon = usize::MAX;
    while let Some(&b) = input.get(i) {
        match NAME.get(usize::from(b)).copied().unwrap_or(0) {
            1 | 2 => {}
            3 => {
                if colon != usize::MAX {
                    return Err(syntax(i, "second colon in name"));
                }
                colon = i;
                let next =
                    at(input, i + 1).map_or(0, |n| NAME.get(usize::from(n)).copied().unwrap_or(0));
                if next != 2 {
                    return Err(syntax(i + 1, "name expected after colon"));
                }
            }
            _ => break,
        }
        i += 1;
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
    if starts(input, pos, b"<?xml") && at(input, pos + 5).is_some_and(is_ws) {
        pos = xml_decl(input, pos)?;
    }
    pos = prolog(input, pos)?;
    content(p, input, pos)
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
        return Err(XmlError::BadXmlDecl { at: off(p) });
    }
    p = skip_ws(input, p + 1);
    let q = match at(input, p) {
        Some(q @ (b'"' | b'\'')) => q,
        _ => return Err(XmlError::BadXmlDecl { at: off(p) }),
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
        None => {
            return Err(XmlError::BadXmlDecl {
                at: off(skip_ws(input, p)),
            })
        }
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
        Err(XmlError::BadXmlDecl { at: off(p) })
    }
}

/// Handles markup that may appear outside the root: a comment (returns the offset after it),
/// otherwise an error for DOCTYPE, a processing instruction, or anything else.
fn misc_markup(input: &[u8], pos: usize) -> Result<usize, XmlError> {
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
                let next = at(input, pos + 1)
                    .map_or(0, |b| NAME.get(usize::from(b)).copied().unwrap_or(0));
                if next == 2 {
                    return Ok(pos);
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
            if at(input, i + 2) == Some(b'>') {
                return Ok(i + 3);
            }
            return Err(syntax(i, "'--' inside comment"));
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
    loop {
        let Some(b) = at(input, pos) else {
            return Err(XmlError::Unclosed {
                at: off(input.len()),
            });
        };
        if b != b'<' {
            pos = text(p, input, pos)?;
            continue;
        }
        pos = match at(input, pos + 1) {
            Some(b'/') => end_tag(p, input, pos)?,
            Some(b'!') => bang(p, input, pos)?,
            Some(b'?') => return Err(XmlError::ProcessingInstruction { at: off(pos) }),
            Some(_) => start_tag(p, input, pos)?,
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

fn text(p: &mut Parser, input: &[u8], pos: usize) -> Result<usize, XmlError> {
    let (end, ent) = scan_run(input, pos, 0)?;
    // Whitespace-only runs between markup are layout, not data.
    if ent || !sub(input, pos, end).iter().all(|&b| is_ws(b)) {
        push_text(p, pos, span(pos, end), ent)?;
    }
    Ok(end)
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
    if sub(input, pos + 2, q_end) != open {
        return Err(XmlError::MismatchedTag { at: off(pos) });
    }
    let i = skip_ws(input, q_end);
    if at(input, i) != Some(b'>') {
        return Err(syntax(i, "'>' expected"));
    }
    p.stack.pop();
    p.bindings.truncate(top.ns_mark as usize);
    Ok(i + 1)
}

/// Finds the namespace declaration for `prefix` (empty = default) among the bindings in scope.
fn bound(bindings: &[Binding], input: &[u8], prefix: &[u8]) -> bool {
    bindings.iter().rev().any(|b| {
        let bp = sub(
            input,
            b.prefix.start as usize,
            (b.prefix.start + b.prefix.len) as usize,
        );
        bp == prefix
    })
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
    let mark = p.bindings.len();
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
                if at(input, i + 1) != Some(b'>') {
                    return Err(syntax(i, "'>' expected after '/'"));
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

    // Namespace declarations take effect for the element's own name and attributes.
    let attr_end = p.attr_name.len();
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
        let binding = if pre == b"xmlns" {
            if name == b"xmlns" {
                return Err(syntax(ap.start as usize, "reserved prefix 'xmlns'"));
            }
            if av.len == 0 {
                return Err(syntax(av.start as usize, "empty namespace URI"));
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
        if p.bindings.len() >= p.binding_cap {
            return Err(XmlError::Limit {
                limit: LimitKind::AttributesPerElement,
                at: ap.start,
            });
        }
        p.bindings.push(binding);
    }
    // Every prefix used must be declared (`xml` is predefined).
    if colon != usize::MAX {
        let pre = sub(input, q_start, colon);
        if pre != b"xml" && !bound(&p.bindings, input, pre) {
            return Err(XmlError::UndeclaredPrefix { at: off(q_start) });
        }
    }
    for a in first_attr..attr_end {
        let Some(&ap) = p.attr_prefix.get(a) else {
            continue;
        };
        if ap.len == 0 {
            continue;
        }
        let pre = sub(input, ap.start as usize, (ap.start + ap.len) as usize);
        if pre != b"xml" && pre != b"xmlns" && !bound(&p.bindings, input, pre) {
            return Err(XmlError::UndeclaredPrefix { at: ap.start });
        }
    }

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

    if self_close {
        p.bindings.truncate(mark);
    } else {
        p.stack.push(Frame {
            elem: e,
            last_child: ElemIx::NONE,
            qname_start: off(q_start),
            qname_len: off(q_end - q_start),
            ns_mark: off(mark),
        });
    }
    Ok(i)
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
    if at(input, i) != Some(b'=') {
        return Err(syntax(i, "'=' expected after attribute name"));
    }
    i = skip_ws(input, i + 1);
    let quote = match at(input, i) {
        Some(q @ (b'"' | b'\'')) => q,
        _ => return Err(syntax(i, "quoted attribute value expected")),
    };
    let vstart = i + 1;
    let (vend, ent) = scan_run(input, vstart, quote)?;
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
    // Duplicates are by qualified name; at most `max_attrs_per_elem` earlier rows to compare.
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
    if ent {
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
