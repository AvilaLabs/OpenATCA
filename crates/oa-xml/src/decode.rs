//! Entity and character-reference decoding. Core (pure); allocates nothing.

use crate::{span_bytes, Span, XmlError};

pub(crate) fn off(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The XML 1.0 `Char` production.
pub(crate) fn is_xml_char(c: u32) -> bool {
    matches!(c, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

/// Parses the reference starting at `input[i]` (which must be `&`). Returns the code point and
/// the length of the whole reference including `&` and `;`.
pub(crate) fn parse_entity(input: &[u8], i: usize) -> Result<(u32, usize), XmlError> {
    let at = off(i);
    let rest = input.get(i + 1..).unwrap_or(&[]);
    // A reference is short; look no further than this for its `;`.
    let window = rest.get(..rest.len().min(12)).unwrap_or(&[]);
    let Some(semi) = window.iter().position(|&b| b == b';') else {
        return Err(XmlError::BadEntity { at });
    };
    let body = window.get(..semi).unwrap_or(&[]);
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
    let mut v: u32 = 0;
    for &d in digits {
        let Some(n) = char::from(d).to_digit(radix) else {
            return Err(XmlError::BadEntity { at });
        };
        v = match v.checked_mul(radix).and_then(|x| x.checked_add(n)) {
            Some(x) => x,
            None => return Err(XmlError::BadChar { at }),
        };
    }
    if is_xml_char(v) {
        Ok((v, total))
    } else {
        Err(XmlError::BadChar { at })
    }
}

/// Decodes the entity and character references in `span` of `input` into `out` and returns
/// the number of bytes written. Use it only on spans whose entity flag is set; a span without
/// the flag (and every CDATA span) is already literal and is copied as is. Fails with
/// `OutputTooSmall` when `out` cannot hold the result; nothing is written past `out`.
pub fn decode_into(span: Span, input: &[u8], out: &mut [u8]) -> Result<usize, XmlError> {
    let bytes = span_bytes(input, span);
    let mut i = 0;
    let mut n = 0;
    while let Some(&b) = bytes.get(i) {
        if b != b'&' {
            // Copy the literal run up to the next `&` in one go.
            let run = bytes
                .get(i..)
                .and_then(|r| r.iter().position(|&c| c == b'&'))
                .unwrap_or(bytes.len() - i);
            let src = bytes.get(i..i + run).unwrap_or(&[]);
            let dst = out.get_mut(n..n + run).ok_or(XmlError::OutputTooSmall {
                at: span.start.saturating_add(off(i)),
            })?;
            dst.copy_from_slice(src);
            n += run;
            i += run;
            continue;
        }
        let at = span.start.saturating_add(off(i));
        let (cp, used) = parse_entity(bytes, i).map_err(|e| shift(e, span.start))?;
        let ch = char::from_u32(cp).ok_or(XmlError::BadChar { at })?;
        let mut tmp = [0u8; 4];
        let enc = ch.encode_utf8(&mut tmp).as_bytes();
        let dst = out
            .get_mut(n..n + enc.len())
            .ok_or(XmlError::OutputTooSmall { at })?;
        dst.copy_from_slice(enc);
        n += enc.len();
        i += used;
    }
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
        other => other,
    }
}

/// Whether the decoded value of `span` equals `s`, decoding on the fly without allocating.
/// Use it on spans whose entity flag is set; for a span without the flag it is a plain byte
/// comparison either way.
pub fn span_eq_str(span: Span, input: &[u8], s: &str) -> Result<bool, XmlError> {
    let bytes = span_bytes(input, span);
    let want = s.as_bytes();
    let mut i = 0;
    let mut n = 0;
    while let Some(&b) = bytes.get(i) {
        if b != b'&' {
            if want.get(n) != Some(&b) {
                return Ok(false);
            }
            i += 1;
            n += 1;
            continue;
        }
        let (cp, used) = parse_entity(bytes, i).map_err(|e| shift(e, span.start))?;
        let at = span.start.saturating_add(off(i));
        let ch = char::from_u32(cp).ok_or(XmlError::BadChar { at })?;
        let mut tmp = [0u8; 4];
        let enc = ch.encode_utf8(&mut tmp).as_bytes();
        if want.get(n..n + enc.len()) != Some(enc) {
            return Ok(false);
        }
        n += enc.len();
        i += used;
    }
    Ok(n == want.len())
}
