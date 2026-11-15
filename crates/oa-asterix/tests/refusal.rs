//! Refusals: each rule of the decoder and encoder is broken once and the typed error is checked
//! for its kind, category, record, item and byte offset.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
use common::*;
use oa_asterix::cat021::*;
use oa_asterix::*;

fn dec(bytes: &[u8]) -> AsterixError {
    decode_block(bytes).unwrap_err()
}

const SRC: [u8; 2] = [0x19, 0x03];

// REQ: OA-AST-004
#[test]
fn block_level_refusals() {
    // Fewer than three octets.
    let e = dec(&[21, 0]);
    assert_eq!(
        (e.kind, e.cat, e.record, e.offset),
        (ErrorKind::Truncated, 21, NO_RECORD, 2)
    );
    let e = dec(&[]);
    assert_eq!(e.kind, ErrorKind::Truncated);
    // LEN below the header, and past the input.
    let e = dec(&[21, 0, 2, 0x80]);
    assert_eq!((e.kind, e.offset), (ErrorKind::BlockLengthBelowHeader, 1));
    let e = dec(&[21, 0, 9, 0x80, 0x19, 0x03]);
    assert_eq!((e.kind, e.offset), (ErrorKind::BlockLengthExceedsInput, 1));
    // No record.
    let e = dec(&[21, 0, 3]);
    assert_eq!((e.kind, e.offset), (ErrorKind::EmptyBlock, 3));
    // Category not carried.
    let e = dec(&[34, 0, 6, 0x80, 1, 2]);
    assert_eq!((e.kind, e.cat), (ErrorKind::UnsupportedCategory, 34));
    // The second record is the broken one: the error says so, with a stream offset.
    let good = rec(&[(1, &SRC)]);
    let bad = rec(&[(1, &[0x19])]);
    let b = blk(21, &[good, bad]);
    let e = dec(&b);
    assert_eq!(e.record, 1);
    assert_eq!((e.kind, e.item), (ErrorKind::Truncated, 10));
    assert_eq!(e.offset, 3 + 3 + 1);
    // The same block inside a stream reports stream offsets.
    let mut stream = blk(21, &[rec(&[(1, &SRC)])]);
    let at = stream.len();
    stream.extend(&b);
    let e = decode_stream(&stream).unwrap_err();
    assert_eq!(e.offset, at + 3 + 3 + 1);
}

// REQ: OA-AST-005
#[test]
fn fspec_refusals() {
    // No bit set.
    let e = dec(&[21, 0, 4, 0x00]);
    assert_eq!((e.kind, e.record, e.offset), (ErrorKind::EmptyFspec, 0, 3));
    // FX set on the last octet of the block.
    let e = dec(&[21, 0, 4, 0x81]);
    assert_eq!((e.kind, e.offset), (ErrorKind::Truncated, 4));
    // A trailing all-zero octet is not the shortest FSPEC.
    let e = dec(&[21, 0, 8, 0x81, 0x00, 0x19, 0x03, 0, 0]);
    assert_eq!((e.kind, e.offset), (ErrorKind::FspecNotMinimal, 4));
    // CAT048 has 28 slots, four FSPEC octets: FX on the fourth is too long.
    let e = dec(&[48, 0, 7, 0x81, 0x01, 0x01, 0x01]);
    assert_eq!((e.kind, e.offset), (ErrorKind::FspecTooLong, 6));
    // CAT062 FRN 2 is not used.
    let e = dec(&[62, 0, 7, 0xC0, 0x19, 0x03, 0x00]);
    assert_eq!(
        (e.kind, e.offset),
        (ErrorKind::UnknownUapSlot { frn: 2 }, 3)
    );
    // CAT021 FRN 43 is not used (octet 7, first bit).
    let e = dec(&[
        21, 0, 12, 0x81, 0x01, 0x01, 0x01, 0x01, 0x01, 0x80, 0x19, 0x03,
    ]);
    assert_eq!(
        (e.kind, e.offset),
        (ErrorKind::UnknownUapSlot { frn: 43 }, 9)
    );
    // CAT062 FRN 29 to 33 are spare.
    let e = dec(&[62, 0, 8, 0x81, 0x01, 0x01, 0x01, 0x80]);
    assert_eq!(e.kind, ErrorKind::UnknownUapSlot { frn: 29 });
    // The data source identifier is mandatory.
    let e = dec(&blk(21, &[rec(&[(2, &[0x08])])]));
    assert_eq!(
        (e.kind, e.record),
        (ErrorKind::MissingMandatory { frn: 1 }, 0)
    );
}

// REQ: OA-AST-006
#[test]
fn item_level_refusals() {
    // I021/040 whose FX chain ends with the block.
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (2, &[0x01])])]));
    assert_eq!(
        (e.kind, e.item, e.offset),
        (ErrorKind::ExtensionUnterminated, 40, 6)
    );
    // 32 octets with the last FX clear is the longest chain accepted; 33 is refused.
    let mut chain = vec![0x01u8; 31];
    chain.push(0x00);
    let ok = blk(21, &[rec(&[(1, &SRC), (2, &chain)])]);
    let (b, _) = decode_block(&ok).unwrap();
    assert_eq!(encode_block(&b).unwrap(), ok);
    let mut long = vec![0x01u8; 32];
    long.push(0x00);
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (2, &long)])]));
    assert_eq!((e.kind, e.item), (ErrorKind::ExtensionUnterminated, 40));
    assert_eq!(MAX_EXTENSIONS, 32);
    // I048/250 with a repetition factor of zero.
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (10, &[0x00])])]));
    assert_eq!(
        (e.kind, e.item, e.offset),
        (ErrorKind::ZeroRepetition, 250, 7)
    );
    // ... and with a factor that runs past the block.
    let e = dec(&blk(
        48,
        &[rec(&[(1, &SRC), (10, &[0x02, 1, 2, 3, 4, 5, 6, 7, 8])])],
    ));
    assert_eq!((e.kind, e.item), (ErrorKind::Truncated, 250));
    // CAT048 SP (FRN 27): explicit length one, and a length past the block.
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (27, &[0x01])])]));
    assert_eq!(e.kind, ErrorKind::ExplicitLengthInvalid);
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (28, &[0x09, 1, 2])])]));
    assert_eq!(e.kind, ErrorKind::ExplicitLengthInvalid);
    // I048/130 with no subitem present.
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (7, &[0x00])])]));
    assert_eq!(
        (e.kind, e.item, e.offset),
        (ErrorKind::EmptyCompound, 130, 6)
    );
    // I062/290 sets the bit of subitem 11, which does not exist; the error names it.
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (14, &[0x01, 0x10])])]));
    assert_eq!(
        (e.kind, e.item, e.sub),
        (ErrorKind::UnknownSubitem, 290, 11)
    );
    // I062/290 with FX on the last defined primary octet.
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (14, &[0x81, 0x01])])]));
    assert_eq!((e.kind, e.item), (ErrorKind::UnknownSubitem, 290));
    // A trailing all-zero primary octet is not the shortest form.
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (14, &[0x81, 0x00, 0x05])])]));
    assert_eq!(
        (e.kind, e.item, e.offset),
        (ErrorKind::CompoundNotMinimal, 290, 8)
    );
    // A subitem cut short inside a compound item names the subitem: I062/290 subitem 5 (the
    // ADS-C age) has two octets.
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (14, &[0x08, 0x00])])]));
    assert_eq!((e.kind, e.item, e.sub), (ErrorKind::Truncated, 290, 5));
    // A fixed item cut short: I048/040 needs four octets.
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (4, &[1, 2, 3])])]));
    assert_eq!((e.kind, e.item, e.offset), (ErrorKind::Truncated, 40, 6));
}

// REQ: OA-AST-007
#[test]
fn spare_bits_fixed_to_zero_are_refused() {
    // I021/070 bits 16/13.
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (19, &[0x10, 0x00])])]));
    assert_eq!((e.kind, e.item, e.offset), (ErrorKind::SpareBitSet, 70, 8));
    // I021/161 bits 16/13.
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (3, &[0x80, 0x01])])]));
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 161));
    // I021/090 second extension, bits 8/7: octet index 2 of the item.
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (17, &[0x01, 0x01, 0x80])])]));
    assert_eq!((e.kind, e.item, e.offset), (ErrorKind::SpareBitSet, 90, 10));
    // The same bits in the first extension are assigned (NICBARO): accepted.
    assert!(decode_block(&blk(21, &[rec(&[(1, &SRC), (17, &[0x01, 0x80])])])).is_ok());
    // I048/161 bits 16/13; I048/070 bit 13.
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (11, &[0x10, 0x00])])]));
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 161));
    let e = dec(&blk(48, &[rec(&[(1, &SRC), (5, &[0x10, 0x00])])]));
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 70));
    // I062/080 sixth extent, bits 7/2: octet index 6.
    let e = dec(&blk(
        62,
        &[rec(&[
            (1, &SRC),
            (13, &[0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02]),
        ])],
    ));
    assert_eq!((e.kind, e.item, e.offset), (ErrorKind::SpareBitSet, 80, 13));
    // I062/245 bits 54/49.
    let e = dec(&blk(
        62,
        &[rec(&[(1, &SRC), (10, &[0x01, 0, 0, 0, 0, 0, 0])])],
    ));
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 245));
    // Spare bit of a subitem inside a compound item: I062/340 subitem 6 bits 2/1.
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (28, &[0x04, 0x01])])]));
    assert_eq!((e.kind, e.item, e.sub), (ErrorKind::SpareBitSet, 340, 6));
    // Reserved enumeration codes are not refused: ATP = 7 is "reserved for future use".
    assert!(decode_block(&blk(21, &[rec(&[(1, &SRC), (2, &[0xE0])])])).is_ok());
}

// REQ: OA-AST-018
#[test]
fn six_bit_characters_and_latitude_range() {
    // Character code 0 is not in the table (six octets of zero are kept raw, see captures.rs).
    let e = dec(&blk(21, &[rec(&[(1, &SRC), (29, &[0, 0, 0, 0, 0, 1])])]));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 170));
    // Every valid code round-trips: A to Z, space, 0 to 9.
    for s in ["ABCDEFGH", "IJKLMNOP", "QRSTUVWX", "YZ 01234", "56789   "] {
        let id: Ident = s.as_bytes().try_into().unwrap();
        let r = Cat021 {
            source: SourceId { sac: 1, sic: 2 },
            target_id: Some(id),
            ..Cat021::default()
        };
        let bytes = encode_block(&Block::Cat021(vec![r.clone()])).unwrap();
        let (b, _) = decode_block(&bytes).unwrap();
        assert_eq!(b, Block::Cat021(vec![r]), "{s}");
    }
    // Lower case and punctuation are not.
    for bad in [
        *b"abcdefgh",
        *b"AB-CD   ",
        [b'A', 0, b'B', b'C', b' ', b' ', b' ', b' '],
    ] {
        let r = Cat021 {
            target_id: Some(bad),
            ..Cat021::default()
        };
        let e = encode_block(&Block::Cat021(vec![r])).unwrap_err();
        assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 170));
    }
    // I021/130 latitude: 2^22 counts is exactly 90 degrees and is allowed; one more is not.
    let lat = |v: u32| {
        let mut item = v.to_be_bytes()[1..].to_vec();
        item.extend_from_slice(&[0, 0, 0]);
        blk(21, &[rec(&[(1, &SRC), (6, &item)])])
    };
    assert!(decode_block(&lat(0x40_0000)).is_ok());
    let e = dec(&lat(0x40_0001));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 130));
    // -90 degrees is -2^22: allowed; below it is not.
    assert!(decode_block(&lat(0xC0_0000)).is_ok());
    assert!(decode_block(&lat(0xBF_FFFF)).is_err());
}

// REQ: OA-AST-014
#[test]
fn encoder_refusals() {
    let base = || Cat021 {
        source: SourceId { sac: 1, sic: 2 },
        ..Cat021::default()
    };
    let enc = |r: Cat021| encode_block(&Block::Cat021(vec![r])).unwrap_err();
    // A value that does not fit its element: track number is 12 bits.
    let e = enc(Cat021 {
        track_number: Some(4096),
        ..base()
    });
    assert_eq!(
        (e.kind, e.item, e.record),
        (ErrorKind::ValueOutOfRange, 161, 0)
    );
    assert_eq!(e.offset, NO_OFFSET);
    // Mode 3/A is 12 bits, the address 24, a 13-bit altitude is +-4096.
    let e = enc(Cat021 {
        mode3a: Some(0x1000),
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::ValueOutOfRange, 70));
    let e = enc(Cat021 {
        address: Some(1 << 24),
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::ValueOutOfRange, 80));
    let e = enc(Cat021 {
        selected_altitude: Some(oa_asterix::SelectedAltitude {
            altitude: 4096,
            ..Default::default()
        }),
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::ValueOutOfRange, 146));
    // A typed item given again as a raw item.
    let e = enc(Cat021 {
        address: Some(1),
        raw: vec![RawItem {
            frn: 11,
            bytes: vec![0, 0, 2],
        }],
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::DuplicateItem, 80));
    // A raw item twice.
    let e = enc(Cat021 {
        raw: vec![
            RawItem {
                frn: 4,
                bytes: vec![1],
            },
            RawItem {
                frn: 4,
                bytes: vec![2],
            },
        ],
        ..base()
    });
    assert_eq!(e.kind, ErrorKind::DuplicateItem);
    // A raw item that is not exactly one item: I021/015 is one octet.
    let e = enc(Cat021 {
        raw: vec![RawItem {
            frn: 4,
            bytes: vec![1, 2],
        }],
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 15));
    // A raw item whose FX chain never ends, and one with a spare bit set.
    let e = enc(Cat021 {
        raw: vec![RawItem {
            frn: 37,
            bytes: vec![0x01],
        }],
        ..base()
    });
    assert_eq!(e.kind, ErrorKind::ExtensionUnterminated);
    let e = enc(Cat021 {
        raw: vec![RawItem {
            frn: 37,
            bytes: vec![0x80],
        }],
        ..base()
    });
    assert_eq!(e.kind, ErrorKind::SpareBitSet);
    // A raw item for a slot that does not exist.
    let e = enc(Cat021 {
        raw: vec![RawItem {
            frn: 43,
            bytes: vec![1],
        }],
        ..base()
    });
    assert_eq!(e.kind, ErrorKind::UnknownFrn { frn: 43 });
    // A typed extension that ends with FX set.
    let e = enc(Cat021 {
        descriptor: Some(Trd021 {
            ext: vec![0x01],
            ..Trd021::default()
        }),
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::ExtensionUnterminated, 40));
    // Latitude beyond 90 degrees.
    let e = enc(Cat021 {
        position_hr: Some(RawPosition {
            lat: (1 << 29) + 1,
            lon: 0,
        }),
        ..base()
    });
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 131));
    // An empty block; too many records; too long a block, naming the record that overflowed.
    let e = encode_block(&Block::Cat021(vec![])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::EmptyBlock);
    let many = vec![base(); MAX_RECORDS_PER_BLOCK + 1];
    assert_eq!(
        encode_block(&Block::Cat021(many)).unwrap_err().kind,
        ErrorKind::TooManyRecords
    );
    let wide = Cat021 {
        address: Some(5),
        ..base()
    };
    // Each record is FSPEC (2) + source (2) + address (3) = 7 octets; 9362 of them overflow.
    let e = encode_block(&Block::Cat021(vec![wide.clone(); 9400])).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BlockTooLong);
    assert_eq!(e.record, 9361);
    // The largest block that fits (9361 records = 3 + 65 527 octets) is written.
    let ok = encode_block(&Block::Cat021(vec![wide; 9361])).unwrap();
    assert_eq!(ok.len(), 3 + 9361 * 7);
    assert!(ok.len() <= MAX_BLOCK_LEN);
}

// REQ: OA-AST-021
#[test]
fn errors_name_category_record_item_and_byte() {
    let b = blk(
        21,
        &[rec(&[(1, &SRC)]), rec(&[(1, &SRC), (3, &[0x80, 0x01])])],
    );
    let e = dec(&b);
    let s = e.to_string();
    assert!(s.contains("CAT021"), "{s}");
    assert!(s.contains("record 1"), "{s}");
    assert!(s.contains("I021/161"), "{s}");
    assert!(s.contains("byte 9"), "{s}");
    assert!(s.contains("SpareBitSet"), "{s}");
    let e = dec(&blk(62, &[rec(&[(1, &SRC), (14, &[0x01, 0x10])])]));
    assert!(e.to_string().contains("subitem 11"), "{e}");
    let e = dec(&[21, 0, 2]);
    let s = e.to_string();
    assert!(!s.contains("record"), "{s}");
    let e = encode_block(&Block::Cat021(vec![])).unwrap_err();
    assert!(!e.to_string().contains("byte"), "{e}");
}

// REQ: OA-AST-017
#[test]
fn bounds_are_constants_and_hold_at_the_edge() {
    assert_eq!(MAX_BLOCK_LEN, 65_535);
    assert_eq!(MAX_RECORDS_PER_BLOCK, (65_535 - 3) / 3);
    assert_eq!(MAX_REPETITIONS, 255);
    assert_eq!(MAX_EXTENSIONS, 32);
    assert_eq!(MAX_PRIMARY_OCTETS, 8);
    // The longest block there is: 21 844 minimal records of three octets, LEN = 65 535.
    let r = vec![0x80, 0x19, 0x03];
    let big = blk(21, &vec![r; MAX_RECORDS_PER_BLOCK]);
    assert_eq!(big.len(), MAX_BLOCK_LEN);
    assert_eq!(&big[1..3], &[0xFF, 0xFF]);
    let (b, n) = decode_block(&big).unwrap();
    assert_eq!(n, MAX_BLOCK_LEN);
    let Block::Cat021(recs) = &b else { panic!() };
    assert_eq!(recs.len(), MAX_RECORDS_PER_BLOCK);
    assert_eq!(encode_block(&b).unwrap(), big);
    // The same LEN with the input cut short is refused, not read past.
    let e = dec(&big[..big.len() - 1]);
    assert_eq!(e.kind, ErrorKind::BlockLengthExceedsInput);
    // 255 repetitions of I048/250 are the most one factor can say.
    let mut rep = vec![255u8];
    rep.extend(std::iter::repeat_n(0xA5u8, 255 * 8));
    let (b, _) = decode_block(&blk(48, &[rec(&[(1, &SRC), (10, &rep)])])).unwrap();
    let Block::Cat048(r) = b else { panic!() };
    assert_eq!(r[0].mb_data.as_ref().unwrap().len(), 255);
}

// REQ: OA-AST-107
#[test]
fn cat021_longitude_range_is_refused_both_ways() {
    // I021/131 (FRN 7): latitude then longitude, four octets each, LSB 180/2^30 degrees, so the
    // field can hold +-720 degrees; 2^30 counts is exactly 180 and is allowed, one more is not.
    let pos = |lat: u32, lon: u32| {
        let mut item = lat.to_be_bytes().to_vec();
        item.extend_from_slice(&lon.to_be_bytes());
        blk(21, &[rec(&[(1, &SRC), (7, &item)])])
    };
    assert!(decode_block(&pos(0, 0x4000_0000)).is_ok());
    assert!(decode_block(&pos(0, 0xC000_0000)).is_ok());
    let e = dec(&pos(0, 0x4000_0001));
    assert_eq!((e.kind, e.cat, e.item), (ErrorKind::InvalidValue, 21, 131));
    let e = dec(&pos(0, 0xBFFF_FFFF));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 131));
    let e = dec(&pos(0, 0x7FFF_FFFF));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 131));
    // The encoder refuses the same value and takes the limit.
    let enc = |lon: i32| {
        let r = Cat021 {
            source: SourceId { sac: 1, sic: 2 },
            position_hr: Some(RawPosition { lat: 0, lon }),
            ..Cat021::default()
        };
        encode_block(&Block::Cat021(vec![r]))
    };
    assert!(enc(1 << 30).is_ok());
    assert!(enc(-(1 << 30)).is_ok());
    let e = enc((1 << 30) + 1).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 131));
    // The three-octet I021/130 cannot exceed 180 degrees: its widest value is one count short.
    let lo = |lon: u32| {
        let mut item = vec![0, 0, 0];
        item.extend_from_slice(&lon.to_be_bytes()[1..]);
        blk(21, &[rec(&[(1, &SRC), (6, &item)])])
    };
    assert!(decode_block(&lo(0x7F_FFFF)).is_ok());
    assert!(decode_block(&lo(0x80_0000)).is_ok());
}

// REQ: OA-AST-105
#[test]
fn cat062_longitude_range_is_refused_both_ways() {
    // I062/105 (FRN 5): latitude then longitude, LSB 180/2^25 degrees, so 2^25 counts is exactly
    // 180 degrees and is allowed; one count more is not.
    let pos = |lat: u32, lon: u32| {
        let mut item = lat.to_be_bytes().to_vec();
        item.extend_from_slice(&lon.to_be_bytes());
        blk(62, &[rec(&[(1, &SRC), (5, &item)])])
    };
    assert!(decode_block(&pos(0, 0x0200_0000)).is_ok());
    assert!(decode_block(&pos(0, 0xFE00_0000)).is_ok());
    let e = dec(&pos(0, 0x0200_0001));
    assert_eq!((e.kind, e.cat, e.item), (ErrorKind::InvalidValue, 62, 105));
    let e = dec(&pos(0, 0xFDFF_FFFF));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 105));
    // The byte string of the report: 00 00 00 00 02 00 00 01.
    let e = dec(&blk(
        62,
        &[rec(&[(1, &SRC), (5, &[0, 0, 0, 0, 0x02, 0, 0, 0x01])])],
    ));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 105));
    // A raw longitude of 0x40000000 is 5760 degrees, not a wrapped 176.5.
    let e = dec(&pos(0, 0x4000_0000));
    assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 105));
    // The encoder refuses the same values, and takes the limits.
    let enc = |lon: i32| {
        let Block::Cat062(mut r) = decode_block(&pos(0, 0)).unwrap().0 else {
            panic!()
        };
        r[0].position = Some(RawPosition { lat: 0, lon });
        encode_block(&Block::Cat062(r))
    };
    assert!(enc(1 << 25).is_ok());
    assert!(enc(-(1 << 25)).is_ok());
    for lon in [(1 << 25) + 1, -(1 << 25) - 1, 0x4000_0000, i32::MIN] {
        let e = enc(lon).unwrap_err();
        assert_eq!((e.kind, e.item), (ErrorKind::InvalidValue, 105), "{lon}");
    }
}
