//! Round trips: bytes to records to the same bytes, records to bytes to the same records, for
//! hand-built blocks, blocks of untyped items of every kind, and records with every typed item.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
use common::*;
use oa_asterix::cat021::Cat021;
use oa_asterix::cat048::Cat048;
use oa_asterix::cat062::Cat062;
use oa_asterix::*;

// REQ: OA-AST-011
#[test]
fn bytes_to_records_to_bytes_is_the_identity() {
    for (i, bytes) in corpus().iter().enumerate() {
        let (b, n) = decode_block(bytes).unwrap_or_else(|e| panic!("corpus {i}: {e}"));
        assert_eq!(n, bytes.len(), "corpus {i}");
        assert_eq!(&encode_block(&b).unwrap(), bytes, "corpus {i}");
    }
}

// REQ: OA-AST-011
#[test]
fn records_to_bytes_to_records_is_the_identity() {
    let r21 = Block::Cat021(vec![cat021_full(), Cat021::default(), cat021_full()]);
    let r48 = Block::Cat048(vec![cat048_full(), Cat048::default()]);
    let r62 = Block::Cat062(vec![Cat062::default(), cat062_full()]);
    for b in [r21, r48, r62] {
        let bytes = encode_block(&b).unwrap();
        let (back, n) = decode_block(&bytes).unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(back, b);
    }
}

// REQ: OA-AST-003
#[test]
fn every_item_kind_is_walked_from_its_table_and_kept_raw() {
    // The untyped items of a record come back as exactly the bytes put in, whatever their
    // structure; the expected FSPEC comes from the test's own writer.
    for (cat, items) in [
        (21u8, cat021_raw_items()),
        (48, cat048_raw_items()),
        (62, cat062_raw_items()),
    ] {
        let bytes = blk(cat, &[raw_record(&items)]);
        let (b, _) = decode_block(&bytes).unwrap();
        let raw = match &b {
            Block::Cat021(r) => r[0].raw.clone(),
            Block::Cat048(r) => r[0].raw.clone(),
            Block::Cat062(r) => r[0].raw.clone(),
        };
        let got: Vec<(u8, Vec<u8>)> = raw.into_iter().map(|r| (r.frn, r.bytes)).collect();
        assert_eq!(got, items, "CAT{cat}");
    }
}

// REQ: OA-AST-012
#[test]
fn untyped_items_round_trip_through_the_record() {
    // A raw item built by hand is written back to the same place: the FSPEC from the encoder
    // equals the FSPEC of the independent writer.
    let items = cat062_raw_items();
    let want = blk(62, &[raw_record(&items)]);
    let r = oa_asterix::cat062::Cat062 {
        source: SourceId {
            sac: 0x19,
            sic: 0x03,
        },
        raw: items
            .into_iter()
            .map(|(frn, bytes)| RawItem { frn, bytes })
            .collect(),
        ..Default::default()
    };
    assert_eq!(encode_block(&Block::Cat062(vec![r])).unwrap(), want);
    // Raw items given out of order are written in FRN order.
    let r = oa_asterix::cat021::Cat021 {
        source: SourceId {
            sac: 0x19,
            sic: 0x03,
        },
        raw: vec![
            RawItem {
                frn: 41,
                bytes: vec![5],
            },
            RawItem {
                frn: 4,
                bytes: vec![7],
            },
        ],
        ..Default::default()
    };
    let bytes = encode_block(&Block::Cat021(vec![r])).unwrap();
    let want = blk(21, &[rec(&[(1, &[0x19, 3]), (4, &[7]), (41, &[5])])]);
    assert_eq!(bytes, want);
}
