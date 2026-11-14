//! Field readers over an `oa-json` document. A missing key and a JSON `null` both mean "absent"
//! (the AWC API uses both); a present value of the wrong type is a refusal.

use crate::error::{refuse, Reason, Refusal};
use crate::units::{parse_visib, NONE_U16};
use oa_json::{Doc, Kind, NodeIx};

/// The value node of `key`, or `None` if absent or null.
fn present(doc: &Doc<'_>, obj: NodeIx, key: &str) -> Option<NodeIx> {
    let n = doc.get(obj, key)?;
    (doc.kind(n)? != Kind::Null).then_some(n)
}

/// Optional string.
pub fn opt_str<'a>(
    doc: &'a Doc<'_>,
    obj: NodeIx,
    key: &'static str,
) -> Result<Option<&'a str>, Refusal> {
    match present(doc, obj, key) {
        None => Ok(None),
        Some(n) => match doc.str(n) {
            Some(s) => Ok(Some(s)),
            None => refuse(Reason::BadType, key),
        },
    }
}

/// Required string.
pub fn req_str<'a>(doc: &'a Doc<'_>, obj: NodeIx, key: &'static str) -> Result<&'a str, Refusal> {
    match opt_str(doc, obj, key)? {
        Some(s) => Ok(s),
        None => refuse(Reason::MissingField, key),
    }
}

/// Optional number.
pub fn opt_f64(doc: &Doc<'_>, obj: NodeIx, key: &'static str) -> Result<Option<f64>, Refusal> {
    match present(doc, obj, key) {
        None => Ok(None),
        Some(n) => match doc.as_f64(n) {
            Some(v) => Ok(Some(v)),
            None => refuse(Reason::BadType, key),
        },
    }
}

/// Required number.
pub fn req_f64(doc: &Doc<'_>, obj: NodeIx, key: &'static str) -> Result<f64, Refusal> {
    match opt_f64(doc, obj, key)? {
        Some(v) => Ok(v),
        None => refuse(Reason::MissingField, key),
    }
}

/// Optional integer-valued number (`12` or `12.0`), range-checked to `lo..=hi`.
pub fn opt_int(
    doc: &Doc<'_>,
    obj: NodeIx,
    key: &'static str,
    lo: i64,
    hi: i64,
) -> Result<Option<i64>, Refusal> {
    match opt_f64(doc, obj, key)? {
        None => Ok(None),
        Some(v) => {
            if v.fract() != 0.0 {
                return refuse(Reason::BadType, key);
            }
            if v < lo as f64 || v > hi as f64 {
                return refuse(Reason::OutOfRange, key);
            }
            Ok(Some(v as i64))
        }
    }
}

/// Required integer-valued number in `lo..=hi`.
pub fn req_int(
    doc: &Doc<'_>,
    obj: NodeIx,
    key: &'static str,
    lo: i64,
    hi: i64,
) -> Result<i64, Refusal> {
    match opt_int(doc, obj, key, lo, hi)? {
        Some(v) => Ok(v),
        None => refuse(Reason::MissingField, key),
    }
}

/// The array node of `key` (required).
pub fn req_array(doc: &Doc<'_>, obj: NodeIx, key: &'static str) -> Result<NodeIx, Refusal> {
    match present(doc, obj, key) {
        None => refuse(Reason::MissingField, key),
        Some(n) if doc.kind(n) == Some(Kind::Array) => Ok(n),
        Some(_) => refuse(Reason::BadType, key),
    }
}

/// The array node of `key` if present and not null.
pub fn opt_array(doc: &Doc<'_>, obj: NodeIx, key: &'static str) -> Result<Option<NodeIx>, Refusal> {
    match present(doc, obj, key) {
        None => Ok(None),
        Some(n) if doc.kind(n) == Some(Kind::Array) => Ok(Some(n)),
        Some(_) => refuse(Reason::BadType, key),
    }
}

/// Checks that an array element is an object.
pub fn require_object(doc: &Doc<'_>, n: NodeIx) -> Result<(), Refusal> {
    if doc.kind(n) == Some(Kind::Object) {
        Ok(())
    } else {
        refuse(Reason::NotObject, "")
    }
}

/// Visibility as the API gives it: a string (`"10+"`, `"6+"`, `"2.5"`, or empty meaning "not
/// given"), a number of statute miles, or null. Returns hundredths of a mile (`NONE_U16` if not
/// given) and sets `plus_flag` bits in `flags` for an "or more" text.
pub fn visib(doc: &Doc<'_>, obj: NodeIx, plus_flag: u8, flags: &mut u8) -> Result<u16, Refusal> {
    let Some(n) = present(doc, obj, "visib") else {
        return Ok(NONE_U16);
    };
    match doc.kind(n) {
        Some(Kind::String) => match doc.str(n) {
            Some("") => Ok(NONE_U16),
            Some(s) => match parse_visib(s) {
                Some((v, plus)) => {
                    if plus {
                        *flags |= plus_flag;
                    }
                    Ok(v)
                }
                None => refuse(Reason::BadType, "visib"),
            },
            None => refuse(Reason::BadType, "visib"),
        },
        Some(Kind::Number) => match doc.as_f64(n) {
            Some(v) if (0.0..=100.0).contains(&v) => Ok((v * 100.0).round() as u16),
            _ => refuse(Reason::OutOfRange, "visib"),
        },
        _ => refuse(Reason::BadType, "visib"),
    }
}
