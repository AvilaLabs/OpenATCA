//! Blocks: decode a buffer of data blocks into typed records, encode typed records into a block.
//! Core (pure). The first stage ([`crate::wire::parse_block`]) validates the framing and every
//! item; the second types the items the tracker needs and keeps the rest as raw bytes.

use crate::cat021::{self, Cat021};
use crate::cat048::{self, Cat048};
use crate::cat062::{self, Cat062};
use crate::common::{ItemError, Items, RawItem};
use crate::error::{AsterixError, ErrorKind, NO_ITEM, NO_OFFSET, NO_RECORD};
use crate::uap::{uap_for_edition, Cat021Edition, Uap};
use crate::wire::{
    assemble_record, check_compound_minimal, check_item, item_number, parse_block_with, RawBlock,
    BLOCK_HEADER_LEN, MAX_BLOCK_LEN, MAX_RECORDS_PER_BLOCK,
};

/// A decoded data block: the category and its records.
#[derive(Debug, Clone, PartialEq, Eq)]
// A7.2: cold struct, size guard waived
pub enum Block {
    Cat021(Vec<Cat021>),
    Cat048(Vec<Cat048>),
    Cat062(Vec<Cat062>),
}

fn at(cat: u8, record: usize, frn: u8, uap: &Uap, offset: usize, kind: ErrorKind) -> AsterixError {
    AsterixError {
        cat,
        record: u32::try_from(record).unwrap_or(NO_RECORD),
        item: item_number(uap, frn),
        sub: 0,
        offset,
        kind,
    }
}

/// Types the records of a parsed block. `buf` is the buffer `raw` was parsed from, `base` its
/// offset in the caller's stream.
fn type_block(
    buf: &[u8],
    base: usize,
    raw: &RawBlock,
    ed: Cat021Edition,
) -> Result<Block, AsterixError> {
    let cat = raw.cat;
    let Some(uap) = uap_for_edition(cat, ed) else {
        return Err(AsterixError {
            cat,
            record: NO_RECORD,
            item: NO_ITEM,
            sub: 0,
            offset: base,
            kind: ErrorKind::UnsupportedCategory,
        });
    };
    let mut r21 = Vec::new();
    let mut r48 = Vec::new();
    let mut r62 = Vec::new();
    for (ri, rec) in raw.records.iter().enumerate() {
        let mut c21 = Cat021::default();
        let mut c48 = Cat048::default();
        let mut c62 = Cat062::default();
        let mut raws: Vec<RawItem> = Vec::new();
        for s in &rec.spans {
            let bytes = buf.get(s.start..s.end).unwrap_or(&[]);
            // A compound primary subfield that is not the shortest is fine in a raw item and
            // refused in a typed one (see `wire`); the primary is the first thing wrong.
            let minimal = check_compound_minimal(uap, s.frn, bytes);
            let not_minimal = |i: usize| {
                at(
                    cat,
                    ri,
                    s.frn,
                    uap,
                    base + s.start + i,
                    ErrorKind::CompoundNotMinimal,
                )
            };
            let typed = match cat {
                21 => cat021::decode_item(&mut c21, s.frn, bytes),
                48 => cat048::decode_item(&mut c48, s.frn, bytes),
                _ => cat062::decode_item(&mut c62, s.frn, bytes),
            }
            .map_err(|k| match minimal {
                Err(i) => not_minimal(i),
                Ok(()) => at(cat, ri, s.frn, uap, base + s.start, k),
            })?;
            if typed {
                minimal.map_err(not_minimal)?;
            } else {
                raws.push(RawItem {
                    frn: s.frn,
                    bytes: bytes.to_vec(),
                });
            }
        }
        match cat {
            21 => {
                cat021::check_edition(&c21, ed)
                    .map_err(|(frn, k)| at(cat, ri, frn, uap, base, k))?;
                c21.raw = raws;
                r21.push(c21);
            }
            48 => {
                c48.raw = raws;
                r48.push(c48);
            }
            _ => {
                c62.raw = raws;
                r62.push(c62);
            }
        }
    }
    Ok(match cat {
        21 => Block::Cat021(r21),
        48 => Block::Cat048(r48),
        _ => Block::Cat062(r62),
    })
}

/// Decodes the data block at the start of `buf`. Returns the block and the number of octets it
/// occupied (its LEN). `base` is the offset of `buf[0]` in the caller's stream, added to the
/// offsets in errors.
pub fn decode_block_at(buf: &[u8], base: usize) -> Result<(Block, usize), AsterixError> {
    decode_block_at_with(buf, base, Cat021Edition::Ed27)
}

/// [`decode_block_at`] with CAT021 read as edition `ed`.
pub fn decode_block_at_with(
    buf: &[u8],
    base: usize,
    ed: Cat021Edition,
) -> Result<(Block, usize), AsterixError> {
    let raw = parse_block_with(buf, base, ed)?;
    let block = type_block(buf, base, &raw, ed)?;
    Ok((block, raw.len))
}

/// [`decode_block`] with CAT021 read as edition `ed`.
pub fn decode_block_with(buf: &[u8], ed: Cat021Edition) -> Result<(Block, usize), AsterixError> {
    decode_block_at_with(buf, 0, ed)
}

/// Decodes the data block at the start of `buf` and returns it with the octets it occupied.
pub fn decode_block(buf: &[u8]) -> Result<(Block, usize), AsterixError> {
    decode_block_at(buf, 0)
}

/// Decodes a buffer holding any number of consecutive data blocks (nothing between, nothing
/// after). Offsets in errors are positions in `buf`. A malformed block refuses the whole buffer.
pub fn decode_stream(buf: &[u8]) -> Result<Vec<Block>, AsterixError> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let rest = buf.get(pos..).unwrap_or(&[]);
        let (b, n) = decode_block_at(rest, pos)?;
        out.push(b);
        pos += n;
    }
    Ok(out)
}

fn enc_err(cat: u8, record: usize, uap: &Uap, frn: u8, kind: ErrorKind) -> AsterixError {
    at(cat, record, frn, uap, NO_OFFSET, kind)
}

/// Merges the typed items and the raw items of one record into FRN order and checks them.
fn merge(
    uap: &Uap,
    record: usize,
    mut items: Vec<(u8, Vec<u8>)>,
    raws: &[RawItem],
) -> Result<Vec<(u8, Vec<u8>)>, AsterixError> {
    let cat = uap.cat;
    for r in raws {
        check_item(uap, r.frn, &r.bytes).map_err(|k| enc_err(cat, record, uap, r.frn, k))?;
        items.push((r.frn, r.bytes.clone()));
    }
    items.sort_by_key(|i| i.0);
    for w in items.windows(2) {
        if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
            if a.0 == b.0 {
                return Err(enc_err(cat, record, uap, a.0, ErrorKind::DuplicateItem));
            }
        }
    }
    // Typed items were encoded from their own fields, which can still make a malformed item
    // (an extension octet with FX set at the end, a spare bit set in `ext`): walk them too.
    for (frn, bytes) in &items {
        check_item(uap, *frn, bytes).map_err(|k| enc_err(cat, record, uap, *frn, k))?;
    }
    Ok(items)
}

/// Encodes the records of one category as one data block: CAT, LEN, then the records. Refuses
/// an empty list, more than [`MAX_RECORDS_PER_BLOCK`] records, a value that does not fit its
/// element, a raw item that is not exactly one well-formed item, an item given twice, and a
/// block over [`MAX_BLOCK_LEN`] octets.
pub fn encode_block(block: &Block) -> Result<Vec<u8>, AsterixError> {
    encode_block_with(block, Cat021Edition::Ed27)
}

/// [`encode_block`] with CAT021 written as edition `ed`; a record the edition does not define
/// (see [`cat021::check_edition`]) is refused.
pub fn encode_block_with(block: &Block, ed: Cat021Edition) -> Result<Vec<u8>, AsterixError> {
    let (cat, n) = match block {
        Block::Cat021(r) => (21, r.len()),
        Block::Cat048(r) => (48, r.len()),
        Block::Cat062(r) => (62, r.len()),
    };
    let Some(uap) = uap_for_edition(cat, ed) else {
        return Err(AsterixError {
            cat,
            record: NO_RECORD,
            item: NO_ITEM,
            sub: 0,
            offset: NO_OFFSET,
            kind: ErrorKind::UnsupportedCategory,
        });
    };
    let block_err = |kind| AsterixError {
        cat,
        record: NO_RECORD,
        item: NO_ITEM,
        sub: 0,
        offset: NO_OFFSET,
        kind,
    };
    if n == 0 {
        return Err(block_err(ErrorKind::EmptyBlock));
    }
    if n > MAX_RECORDS_PER_BLOCK {
        return Err(block_err(ErrorKind::TooManyRecords));
    }
    let mut body = Vec::new();
    for i in 0..n {
        let (typed, raws): (Result<Items, ItemError>, &[RawItem]) = match block {
            Block::Cat021(r) => match r.get(i) {
                Some(x) => {
                    cat021::check_edition(x, ed).map_err(|(frn, k)| enc_err(cat, i, uap, frn, k))?;
                    (cat021::encode_items(x), &x.raw)
                }
                None => continue,
            },
            Block::Cat048(r) => match r.get(i) {
                Some(x) => (cat048::encode_items(x), &x.raw),
                None => continue,
            },
            Block::Cat062(r) => match r.get(i) {
                Some(x) => (cat062::encode_items(x), &x.raw),
                None => continue,
            },
        };
        let typed = typed.map_err(|(frn, k)| enc_err(cat, i, uap, frn, k))?;
        let items = merge(uap, i, typed, raws)?;
        body.extend_from_slice(&assemble_record(&items));
        if BLOCK_HEADER_LEN + body.len() > MAX_BLOCK_LEN {
            return Err(AsterixError {
                record: u32::try_from(i).unwrap_or(NO_RECORD),
                ..block_err(ErrorKind::BlockTooLong)
            });
        }
    }
    let len = u16::try_from(BLOCK_HEADER_LEN + body.len())
        .map_err(|_| block_err(ErrorKind::BlockTooLong))?;
    let mut out = Vec::with_capacity(usize::from(len));
    out.push(cat);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}
