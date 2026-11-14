//! Entity and character-reference decoding, with line-end and attribute-value normalisation.
//! Core (pure); allocates nothing.

use crate::{span_bytes, Span, XmlError};

pub(crate) fn off(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The XML 1.0 `Char` production.
pub(crate) fn is_xml_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// What a span holds, which decides how its literal bytes are normalised when decoded.
/// Character references are never normalised: `&#10;` stays a line feed in both kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// Element text: `\r\n` and a lone `\r` become `\n` (XML 1.0 section 2.11).
    Text,
    /// An attribute value, CDATA type (section 3.3.3): a literal tab, `\n`, `\r` or `\r\n`
    /// each become one space.
    AttrValue,
}

/// A byte that can appear between `&` and `;` of a reference: a name byte or `#`.
fn is_ref_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'#' | b'_' | b'-' | b'.' | b':')
}

/// Parses the reference starting at `input[i]` (which must be `&`). Returns the code point and
/// the length of the whole reference including `&` and `;`. The body is scanned up to its `;`
/// with no length bound other than the input (so `&#x0000000041;` is well formed); the scan
/// stops at the first byte that cannot be in a reference, so it never runs past the reference.
/// A reference that runs into the end of the input is `UnexpectedEof`.
pub(crate) fn parse_entity(input: &[u8], i: usize) -> Result<(u32, usize), XmlError> {
    let at = off(i);
    let rest = input.get(i + 1..).unwrap_or(&[]);
    let mut semi = 0;
    while let Some(&b) = rest.get(semi) {
        if b == b';' {
            break;
        }
        if !is_ref_byte(b) {
            return Err(XmlError::BadEntity { at });
        }
        semi += 1;
    }
    if rest.get(semi) != Some(&b';') {
        return Err(XmlError::UnexpectedEof {
            at: off(input.len()),
        });
    }
    let body = rest.get(..semi).unwrap_or(&[]);
    let total = semi + 2;
    match body {
        b"lt" => return Ok((u32::from(b'<'), total)),
        b"gt" => return Ok((u32::from(b'>'), total)),
        b"amp" => return Ok((u32::from(b'&'), total)),
        b"quot" => return Ok((u32::from(b'"'), total)),
        b"apos" => return Ok((u32::from(b'\''), total)),
        _ => {}
    }
    let (digits, radix) = match body {
        [b'#', b'x', d @ ..] => (d, 16),
        [b'#', d @ ..] => (d, 10),
        [first, ..] if first.is_ascii_alphabetic() || *first == b'_' => {
            return Err(XmlError::UndefinedEntity { at });
        }
        _ => return Err(XmlError::BadEntity { at }),
    };
    if digits.is_empty() {
        return Err(XmlError::BadEntity { at });
    }
    // Any number of digits (leading zeros included). `v` never exceeds 0x10FFFF before a step,
    // so the step cannot overflow; once past it the value is only marked too big, and the
    // remaining digits are still checked, so a bad digit is `BadEntity` wherever it falls.
    let mut v: u32 = 0;
    let mut too_big = false;
    for &d in digits {
        let Some(n) = char::from(d).to_digit(radix) else {
            return Err(XmlError::BadEntity { at });
        };
        if !too_big {
            v = v * radix + n;
            too_big = v > 0x10_FFFF;
        }
    }
    if !too_big && is_xml_char(v) {
        Ok((v, total))
    } else {
        Err(XmlError::BadChar { at })
    }
}

/// Walks the decoded form of `bytes`, calling `emit(piece, offset)` for each piece in order:
/// a literal run, a normalised line end or space, or the UTF-8 of a reference. `offset` is the
/// piece's start in `bytes`. Errors carry offsets relative to `bytes`. Nothing is written
/// anywhere; the three public functions differ only in what `emit` does.
fn walk(bytes: &[u8], kind: SpanKind, emit: &mut impl FnMut(&[u8], usize)) -> Result<(), XmlError> {
    let attr = kind == SpanKind::AttrValue;
    let mut i = 0;
    while let Some(rest) = bytes.get(i..).filter(|r| !r.is_empty()) {
        let run = rest
            .iter()
            .position(|&c| c == b'&' || c == b'\r' || (attr && (c == b'\t' || c == b'\n')))
            .unwrap_or(rest.len());
        if run > 0 {
            emit(rest.get(..run).unwrap_or(&[]), i);
            i += run;
            continue;
        }
        match rest.first() {
            Some(b'&') => {
                let (cp, used) = parse_entity(bytes, i)?;
                let ch = char::from_u32(cp).ok_or(XmlError::BadChar { at: off(i) })?;
                let mut tmp = [0u8; 4];
                emit(ch.encode_utf8(&mut tmp).as_bytes(), i);
                i += used;
            }
            Some(b'\r') => {
                emit(if attr { b" " } else { b"\n" }, i);
                i += if rest.get(1) == Some(&b'\n') { 2 } else { 1 };
            }
            _ => {
                emit(b" ", i);
                i += 1;
            }
        }
    }
    Ok(())
}

/// Decodes `span` of `input` into `out` and returns the number of bytes written: references
/// are expanded and the literal bytes normalised for `kind`. Use it on spans whose entity flag
/// is set; a span without the flag (and every CDATA span) is already its own decoded value.
///
/// All or nothing: on any error `out` is unchanged. The span is validated and measured in a
/// first pass and written in a second (a write-then-restore scheme would need a saved copy of
/// `out`, which cannot be had without allocating), so a bad reference anywhere in the span is
/// reported before `OutputTooSmall`. A span outside `input` is `SpanOutOfRange`.
pub fn decode_into(
    span: Span,
    input: &[u8],
    out: &mut [u8],
    kind: SpanKind,
) -> Result<usize, XmlError> {
    let bytes = span_bytes(input, span)?;
    let cap = out.len();
    let mut n = 0usize;
    let mut over: Option<usize> = None;
    walk(bytes, kind, &mut |piece, i| {
        if over.is_none() && n + piece.len() > cap {
            over = Some(i);
        }
        n += piece.len();
    })
    .map_err(|e| shift(e, span.start))?;
    if let Some(i) = over {
        return Err(XmlError::OutputTooSmall {
            at: span.start.saturating_add(off(i)),
        });
    }
    let mut w = 0usize;
    walk(bytes, kind, &mut |piece, _| {
        if let Some(dst) = out.get_mut(w..w + piece.len()) {
            dst.copy_from_slice(piece);
        }
        w += piece.len();
    })
    .map_err(|e| shift(e, span.start))?;
    Ok(n)
}

/// Re-bases an error offset from span-relative to input-relative.
fn shift(e: XmlError, base: u32) -> XmlError {
    match e {
        XmlError::BadEntity { at } => XmlError::BadEntity {
            at: at.saturating_add(base),
        },
        XmlError::UndefinedEntity { at } => XmlError::UndefinedEntity {
            at: at.saturating_add(base),
        },
        XmlError::BadChar { at } => XmlError::BadChar {
            at: at.saturating_add(base),
        },
        XmlError::UnexpectedEof { at } => XmlError::UnexpectedEof {
            at: at.saturating_add(base),
        },
        other => other,
    }
}

/// Whether the decoded value of `span` (as [`decode_into`] would produce it for `kind`)
/// equals `s`, decoding on the fly without allocating. It returns the error `decode_into`
/// would return for the same span (bad reference, `SpanOutOfRange`): it does not stop at the
/// first mismatch but keeps walking, so the rest of the span is validated either way.
pub fn span_eq_str(span: Span, input: &[u8], s: &str, kind: SpanKind) -> Result<bool, XmlError> {
    let bytes = span_bytes(input, span)?;
    let want = s.as_bytes();
    let mut n = 0usize;
    let mut eq = true;
    walk(bytes, kind, &mut |piece, _| {
        if want.get(n..n + piece.len()) != Some(piece) {
            eq = false;
        }
        n += piece.len();
    })
    .map_err(|e| shift(e, span.start))?;
    Ok(eq && n == want.len())
}
