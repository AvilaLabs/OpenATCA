//! Core (pure, B1): counts the records of one CAT021 data block by walking its field
//! specifications and item lengths (Ed 2.4 5.3, Table 2), without decoding any value. It is
//! written from the UAP table alone and shares nothing with the encoder or the decoder, so the
//! record count it gives is an independent check of the file. Items the export never writes
//! (compound, repetitive, special-purpose) are not sized and make the walk fail rather than
//! guess.

/// How an item is sized.
#[derive(Clone, Copy)]
enum Size {
    /// This many octets.
    Fixed(usize),
    /// One octet, and another while the last has its FX bit (bit 1) set.
    Chain,
    /// Not sized here.
    Unsized,
}

/// The length rule of the item of field reference number `frn` (Table 2).
fn item_size(frn: usize) -> Size {
    match frn {
        1 => Size::Fixed(2),
        2 => Size::Chain,
        3 => Size::Fixed(2),
        4 => Size::Fixed(1),
        5 => Size::Fixed(3),
        6 => Size::Fixed(6),
        7 => Size::Fixed(8),
        8 => Size::Fixed(3),
        9 | 10 => Size::Fixed(2),
        11 | 12 => Size::Fixed(3),
        13 => Size::Fixed(4),
        14 => Size::Fixed(3),
        15 => Size::Fixed(4),
        16 => Size::Fixed(2),
        17 => Size::Chain,
        18 => Size::Fixed(1),
        19..=21 => Size::Fixed(2),
        22 => Size::Fixed(2),
        23 => Size::Fixed(1),
        24 | 25 => Size::Fixed(2),
        26 => Size::Fixed(4),
        27 => Size::Fixed(2),
        28 => Size::Fixed(3),
        29 => Size::Fixed(6),
        30 => Size::Fixed(1),
        32 | 33 => Size::Fixed(2),
        35 | 36 | 38 | 41 => Size::Fixed(1),
        _ => Size::Unsized,
    }
}

/// The number of records in `block`, one whole data block (category, length, records).
pub fn count_cat021_records(block: &[u8]) -> Result<u64, String> {
    let (Some(&cat), Some(&l0), Some(&l1)) = (block.first(), block.get(1), block.get(2)) else {
        return Err("block shorter than its header".into());
    };
    if cat != 21 {
        return Err(format!("category {cat} is not 21"));
    }
    let len = usize::from(u16::from_be_bytes([l0, l1]));
    if len != block.len() {
        return Err(format!("LEN {len} but {} octets", block.len()));
    }
    let mut at = 3;
    let mut records = 0u64;
    while at < block.len() {
        // The field specification: octets while FX is set; bits 8 to 2 are seven FRNs.
        let mut frns = Vec::new();
        let mut octet = 0usize;
        loop {
            let b = *block.get(at).ok_or("record cut inside its FSPEC")?;
            at += 1;
            for k in 0..7 {
                if b & (0x80 >> k) != 0 {
                    frns.push(octet * 7 + k + 1);
                }
            }
            octet += 1;
            if b & 1 == 0 {
                break;
            }
        }
        for frn in frns {
            match item_size(frn) {
                Size::Fixed(n) => at += n,
                Size::Chain => loop {
                    let b = *block.get(at).ok_or("item cut inside its extension chain")?;
                    at += 1;
                    if b & 1 == 0 {
                        break;
                    }
                },
                Size::Unsized => return Err(format!("item of FRN {frn} is not sized here")),
            }
        }
        if at > block.len() {
            return Err("record runs past the end of its block".into());
        }
        records += 1;
    }
    Ok(records)
}
