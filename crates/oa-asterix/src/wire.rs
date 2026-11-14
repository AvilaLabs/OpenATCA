//! The generic core (Part 1 Ed 3.1 chapter 5): data block framing, the FSPEC, and the walk of
//! any item described by a [`Field`]. Core (pure): bytes in, spans or bytes out, no I/O (B1).
//!
//! # Decisions where the specification leaves room
//!
//! * Blocking: every category here is blocked (CAT, LEN, one or more records), the layout of
//!   Part 1 5.2.2. The records of a block must tile it exactly: the last record ends at LEN.
//! * A record with an empty FSPEC (no bit set) is refused ([`ErrorKind::EmptyFspec`]); so is a
//!   record without the data source identifier (the FRNs of [`Uap::mandatory`]), which the three
//!   categories say shall be present in every record.
//! * An FSPEC or a compound primary subfield that ends with an all-zero octet that is not the
//!   only octet is refused: Part 1 5.2.7 uses the extension "when the Data Fields with FRNs
//!   greater than seven have to be transmitted", and a compound item "shall only be included if
//!   at least one bit is set". Refusing makes `encode(decode(x)) == x` hold for everything
//!   accepted.
//! * A repetition factor of zero is refused (Part 1 5.2.5.1.4: "N consecutive subitems", and
//!   every repetitive item here says "at least one").
//! * An explicit-length item (SP, RE) must have a length octet of at least two: a field with no
//!   content is not sent.
//! * An extended item may have more extents than the category currently defines (a later
//!   edition may add some) up to [`MAX_EXTENSIONS`] octets; beyond that the FX chain is called
//!   unterminated.

use crate::error::{AsterixError, ErrorKind, NO_ITEM, NO_RECORD};
use crate::uap::{slot_of, uap_for, Field, Kind, Slot, Uap};

/// Part 1 5.2.2: the LEN field has two octets, so a block is at most 65 535 octets.
pub const MAX_BLOCK_LEN: usize = 65_535;
/// CAT (one octet) and LEN (two octets).
pub const BLOCK_HEADER_LEN: usize = 3;
/// The shortest record: one FSPEC octet and the two-octet data source identifier.
pub const MIN_RECORD_LEN: usize = 3;
/// Records per block: the block limit over the shortest record.
pub const MAX_RECORDS_PER_BLOCK: usize = (MAX_BLOCK_LEN - BLOCK_HEADER_LEN) / MIN_RECORD_LEN;
/// Octets (extents of the item's chunk size) in one extended item. The longest chain the
/// categories define is the eight extensions of I021/090; the rest is margin for later editions.
pub const MAX_EXTENSIONS: usize = 32;
/// A repetition factor is one octet, so at most 255 repetitions.
pub const MAX_REPETITIONS: usize = 255;
/// Octets of a compound primary subfield: I021/295 and I062/295 need five at most.
pub const MAX_PRIMARY_OCTETS: usize = 8;

/// Where an item sits in the input. Plain data (A1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct Span {
    pub frn: u8,
    pub start: usize,
    pub end: usize,
}

/// One record's items in FRN order.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RawRecord {
    pub spans: Vec<Span>,
}

/// A block split into records; the item bytes are still in the caller's buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub struct RawBlock {
    pub cat: u8,
    /// LEN of the block, the number of octets it occupies in the input.
    pub len: usize,
    pub records: Vec<RawRecord>,
}

/// A failure inside an item, before the item's place is known.
#[derive(Debug, Clone, Copy)]
struct Fail {
    kind: ErrorKind,
    sub: u8,
    at: usize,
}

fn fail(kind: ErrorKind, at: usize) -> Fail {
    Fail { kind, sub: 0, at }
}

fn byte(buf: &[u8], at: usize) -> Option<u8> {
    buf.get(at).copied()
}

/// The end of the item that starts at `start`, or why it is malformed.
fn walk_field(buf: &[u8], start: usize, field: &Field) -> Result<usize, Fail> {
    let end = match field.kind {
        Kind::Fixed(n) => {
            let end = start + usize::from(n);
            if end > buf.len() {
                return Err(fail(ErrorKind::Truncated, start));
            }
            end
        }
        Kind::Extended(chunk) => walk_extended(buf, start, usize::from(chunk))?,
        Kind::Repetitive { size } => {
            let Some(n) = byte(buf, start) else {
                return Err(fail(ErrorKind::Truncated, start));
            };
            if n == 0 {
                return Err(fail(ErrorKind::ZeroRepetition, start));
            }
            let end = start + 1 + usize::from(n) * usize::from(size);
            if end > buf.len() {
                return Err(fail(ErrorKind::Truncated, start));
            }
            end
        }
        Kind::Explicit => {
            let Some(n) = byte(buf, start) else {
                return Err(fail(ErrorKind::Truncated, start));
            };
            let end = start + usize::from(n);
            if n < 2 || end > buf.len() {
                return Err(fail(ErrorKind::ExplicitLengthInvalid, start));
            }
            end
        }
        Kind::Compound(subs) => walk_compound(buf, start, subs)?,
    };
    for s in field.spare {
        if let Some(b) = byte(buf, start + usize::from(s.octet)) {
            if usize::from(s.octet) < end - start && b & s.mask != 0 {
                return Err(fail(ErrorKind::SpareBitSet, start + usize::from(s.octet)));
            }
        }
    }
    Ok(end)
}

fn walk_extended(buf: &[u8], start: usize, chunk: usize) -> Result<usize, Fail> {
    let mut at = start;
    for _ in 0..MAX_EXTENSIONS {
        let last = at + chunk - 1;
        let Some(b) = byte(buf, last) else {
            return Err(fail(ErrorKind::ExtensionUnterminated, start));
        };
        at += chunk;
        if b & 1 == 0 {
            return Ok(at);
        }
    }
    Err(fail(ErrorKind::ExtensionUnterminated, start))
}

fn walk_compound(buf: &[u8], start: usize, subs: &[Field]) -> Result<usize, Fail> {
    // Primary subfield: one octet per seven subitems, extended by FX.
    let defined_octets = subs.len().div_ceil(7);
    let mut at = start;
    let mut octets = 0usize;
    loop {
        let Some(b) = byte(buf, at) else {
            return Err(fail(ErrorKind::Truncated, start));
        };
        octets += 1;
        at += 1;
        if b & 1 == 0 {
            if octets > 1 && b & 0xFE == 0 {
                return Err(fail(ErrorKind::CompoundNotMinimal, at - 1));
            }
            break;
        }
        if octets >= defined_octets || octets >= MAX_PRIMARY_OCTETS {
            // FX set on the last octet that has defined subitems: the next octet could only
            // describe subitems the category does not define.
            return Err(Fail {
                kind: ErrorKind::UnknownSubitem,
                sub: 0,
                at: at - 1,
            });
        }
    }
    let primary_end = at;
    let mut any = false;
    for i in 0..octets * 7 {
        let Some(b) = byte(buf, start + i / 7) else {
            return Err(fail(ErrorKind::Truncated, start));
        };
        if b & (0x80 >> (i % 7)) == 0 {
            continue;
        }
        any = true;
        let sub = u8::try_from(i + 1).unwrap_or(u8::MAX);
        let Some(f) = subs.get(i) else {
            return Err(Fail {
                kind: ErrorKind::UnknownSubitem,
                sub,
                at: start + i / 7,
            });
        };
        at = match walk_field(buf, at, f) {
            Ok(e) => e,
            Err(mut e) => {
                if e.sub == 0 {
                    e.sub = sub;
                }
                return Err(e);
            }
        };
    }
    if !any {
        return Err(fail(ErrorKind::EmptyCompound, start));
    }
    debug_assert!(at >= primary_end);
    Ok(at)
}

/// The subitems of a compound item: `(zero-based subitem index, start, end)` in `buf`, which
/// holds exactly the compound item. Used by the typed decoders of compound items.
pub(crate) fn compound_parts(
    buf: &[u8],
    subs: &[Field],
) -> Result<Vec<(usize, usize, usize)>, ErrorKind> {
    let mut at = 0usize;
    let mut octets = 0usize;
    loop {
        let b = byte(buf, at).ok_or(ErrorKind::Truncated)?;
        octets += 1;
        at += 1;
        if b & 1 == 0 {
            break;
        }
    }
    let mut parts = Vec::new();
    for i in 0..octets * 7 {
        let b = byte(buf, i / 7).ok_or(ErrorKind::Truncated)?;
        if b & (0x80 >> (i % 7)) == 0 {
            continue;
        }
        let f = subs.get(i).ok_or(ErrorKind::UnknownSubitem)?;
        let end = walk_field(buf, at, f).map_err(|e| e.kind)?;
        parts.push((i, at, end));
        at = end;
    }
    Ok(parts)
}

/// Builds a compound item from `(subitem index, bytes)` pairs in ascending index order: the
/// primary subfield with the fewest octets that holds the highest index, then the subitems.
pub(crate) fn compound_join(parts: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let top = parts.iter().map(|p| p.0).max().unwrap_or(0);
    let octets = top / 7 + 1;
    let mut out = vec![0u8; octets];
    for (i, _) in parts {
        if let Some(b) = out.get_mut(i / 7) {
            *b |= 0x80 >> (i % 7);
        }
    }
    for o in 0..octets.saturating_sub(1) {
        if let Some(b) = out.get_mut(o) {
            *b |= 1;
        }
    }
    for (_, bytes) in parts {
        out.extend_from_slice(bytes);
    }
    out
}

/// The FRNs set in an FSPEC and the offset just past it.
fn parse_fspec(
    buf: &[u8],
    start: usize,
    uap: &Uap,
) -> Result<(Vec<u8>, usize), (ErrorKind, usize)> {
    let max_octets = uap.slots.len() / 7;
    let mut frns = Vec::new();
    let mut at = start;
    let mut octets = 0usize;
    loop {
        let Some(b) = byte(buf, at) else {
            return Err((ErrorKind::Truncated, at));
        };
        for bit in 0..7 {
            if b & (0x80 >> bit) == 0 {
                continue;
            }
            let frn = u8::try_from(octets * 7 + bit + 1).unwrap_or(u8::MAX);
            match slot_of(uap, frn) {
                Some(Slot::Unused) | None => {
                    return Err((ErrorKind::UnknownUapSlot { frn }, at));
                }
                Some(_) => frns.push(frn),
            }
        }
        octets += 1;
        at += 1;
        if b & 1 == 0 {
            if octets > 1 && b & 0xFE == 0 {
                return Err((ErrorKind::FspecNotMinimal, at - 1));
            }
            break;
        }
        if octets >= max_octets {
            return Err((ErrorKind::FspecTooLong, at - 1));
        }
    }
    if frns.is_empty() {
        return Err((ErrorKind::EmptyFspec, start));
    }
    Ok((frns, at))
}

fn err(cat: u8, record: u32, item: u16, sub: u8, offset: usize, kind: ErrorKind) -> AsterixError {
    AsterixError {
        cat,
        record,
        item,
        sub,
        offset,
        kind,
    }
}

fn item_id(slot: &Slot) -> u16 {
    match slot {
        Slot::Item { id, .. } => *id,
        // SP and RE have no number; the FRN is reported through the offset.
        _ => NO_ITEM,
    }
}

/// Splits one record that starts at `start` in `buf` (which ends where the block ends).
/// `base` is added to offsets in errors.
fn parse_record(
    buf: &[u8],
    start: usize,
    uap: &Uap,
    index: u32,
    base: usize,
) -> Result<(RawRecord, usize), AsterixError> {
    let cat = uap.cat;
    let (frns, mut at) =
        parse_fspec(buf, start, uap).map_err(|(k, o)| err(cat, index, NO_ITEM, 0, base + o, k))?;
    for m in uap.mandatory {
        if !frns.contains(m) {
            return Err(err(
                cat,
                index,
                NO_ITEM,
                0,
                base + start,
                ErrorKind::MissingMandatory { frn: *m },
            ));
        }
    }
    let mut spans = Vec::with_capacity(frns.len());
    for frn in frns {
        let Some(slot) = slot_of(uap, frn) else {
            return Err(err(
                cat,
                index,
                NO_ITEM,
                0,
                base + at,
                ErrorKind::UnknownUapSlot { frn },
            ));
        };
        let explicit = Field {
            kind: Kind::Explicit,
            spare: &[],
        };
        let field = match slot {
            Slot::Item { field, .. } => field,
            Slot::Re | Slot::Sp => &explicit,
            Slot::Unused => {
                return Err(err(
                    cat,
                    index,
                    NO_ITEM,
                    0,
                    base + at,
                    ErrorKind::UnknownUapSlot { frn },
                ))
            }
        };
        let end = walk_field(buf, at, field)
            .map_err(|f| err(cat, index, item_id(slot), f.sub, base + f.at, f.kind))?;
        spans.push(Span {
            frn,
            start: at,
            end,
        });
        at = end;
    }
    Ok((RawRecord { spans }, at))
}

/// Parses one data block at the start of `buf`. `base` is the offset of `buf[0]` in the
/// caller's stream; offsets in errors include it. The bytes after LEN are not looked at.
pub fn parse_block(buf: &[u8], base: usize) -> Result<RawBlock, AsterixError> {
    let (Some(&cat), Some(&l0), Some(&l1)) = (buf.first(), buf.get(1), buf.get(2)) else {
        return Err(err(
            buf.first().copied().unwrap_or(0),
            NO_RECORD,
            NO_ITEM,
            0,
            base + buf.len(),
            ErrorKind::Truncated,
        ));
    };
    let len = usize::from(u16::from_be_bytes([l0, l1]));
    if len < BLOCK_HEADER_LEN {
        return Err(err(
            cat,
            NO_RECORD,
            NO_ITEM,
            0,
            base + 1,
            ErrorKind::BlockLengthBelowHeader,
        ));
    }
    if len > buf.len() {
        return Err(err(
            cat,
            NO_RECORD,
            NO_ITEM,
            0,
            base + 1,
            ErrorKind::BlockLengthExceedsInput,
        ));
    }
    let Some(uap) = uap_for(cat) else {
        return Err(err(
            cat,
            NO_RECORD,
            NO_ITEM,
            0,
            base,
            ErrorKind::UnsupportedCategory,
        ));
    };
    let body = buf.get(..len).unwrap_or(buf);
    if len == BLOCK_HEADER_LEN {
        return Err(err(
            cat,
            NO_RECORD,
            NO_ITEM,
            0,
            base + BLOCK_HEADER_LEN,
            ErrorKind::EmptyBlock,
        ));
    }
    let mut records = Vec::new();
    let mut at = BLOCK_HEADER_LEN;
    while at < len {
        let index = u32::try_from(records.len()).unwrap_or(u32::MAX);
        if records.len() >= MAX_RECORDS_PER_BLOCK {
            return Err(err(
                cat,
                index,
                NO_ITEM,
                0,
                base + at,
                ErrorKind::TooManyRecords,
            ));
        }
        let (rec, next) = parse_record(body, at, uap, index, base)?;
        records.push(rec);
        at = next;
    }
    Ok(RawBlock { cat, len, records })
}

/// Validates a raw item (the bytes after the FSPEC) against its slot: it must be exactly one
/// well-formed item. Used by the encoder so a raw item never produces a block the decoder
/// refuses.
pub(crate) fn check_item(uap: &Uap, frn: u8, bytes: &[u8]) -> Result<(), ErrorKind> {
    let explicit = Field {
        kind: Kind::Explicit,
        spare: &[],
    };
    let field = match slot_of(uap, frn) {
        Some(Slot::Item { field, .. }) => field,
        Some(Slot::Re | Slot::Sp) => &explicit,
        _ => return Err(ErrorKind::UnknownFrn { frn }),
    };
    match walk_field(bytes, 0, field) {
        Ok(end) if end == bytes.len() => Ok(()),
        Ok(_) => Err(ErrorKind::InvalidValue),
        Err(f) => Err(f.kind),
    }
}

/// The item number of `frn` in `uap` (0 for SP, RE and unknown slots).
pub(crate) fn item_number(uap: &Uap, frn: u8) -> u16 {
    slot_of(uap, frn).map(item_id).unwrap_or(NO_ITEM)
}

/// Assembles one record from items in ascending FRN order: the FSPEC, then the items.
pub(crate) fn assemble_record(items: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let top = items.iter().map(|i| usize::from(i.0)).max().unwrap_or(1);
    let octets = top.saturating_sub(1) / 7 + 1;
    let mut out = vec![0u8; octets];
    for (frn, _) in items {
        let ix = usize::from(*frn).saturating_sub(1);
        if let Some(b) = out.get_mut(ix / 7) {
            *b |= 0x80 >> (ix % 7);
        }
    }
    for o in 0..octets.saturating_sub(1) {
        if let Some(b) = out.get_mut(o) {
            *b |= 1;
        }
    }
    for (_, bytes) in items {
        out.extend_from_slice(bytes);
    }
    out
}
