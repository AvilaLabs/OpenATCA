//! What the independent captures taught, rebuilt by hand from the specification: no byte of
//! the sample files is used. Each case names the capture finding it comes from
//! (`tests/captures.rs` shows the real-data side).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
use common::*;
use oa_asterix::cat062::Cat062;
use oa_asterix::*;

const SRC: [u8; 2] = [0x19, 0x64];

fn round_trips(bytes: &[u8]) -> Block {
    let (b, n) = decode_block(bytes).unwrap();
    assert_eq!(n, bytes.len());
    assert_eq!(encode_block(&b).unwrap(), bytes);
    b
}

// REQ: OA-AST-025
#[test]
fn compound_primary_with_a_trailing_zero_octet_is_kept_when_raw() {
    // I062/390 (FRN 21) with subitem 1 (FPPS identification tag, two octets) and a primary
    // subfield of three octets: 0x81 (subitem 1, FX), 0x01 (FX only), 0x00 (nothing). A real
    // tracker writes this; the item is not interpreted, so it round-trips as it came.
    let item = [0x81, 0x01, 0x00, 0x19, 0x64];
    let bytes = blk(62, &[rec(&[(1, &SRC), (21, &item)])]);
    let Block::Cat062(r) = round_trips(&bytes) else {
        panic!()
    };
    assert_eq!(r[0].raw.len(), 1);
    assert_eq!(
        (r[0].raw[0].frn, r[0].raw[0].bytes.as_slice()),
        (21, &item[..])
    );
    // A compound item that sets no bit at all is still refused, however long its primary.
    let bytes = blk(62, &[rec(&[(1, &SRC), (21, &[0x01, 0x00])])]);
    assert_eq!(
        decode_block(&bytes).unwrap_err().kind,
        ErrorKind::EmptyCompound
    );
    // The encoder takes the raw item as given.
    let r = Cat062 {
        source: SourceId { sac: 25, sic: 100 },
        raw: vec![RawItem {
            frn: 21,
            bytes: item.to_vec(),
        }],
        ..Cat062::default()
    };
    let e = encode_block(&Block::Cat062(vec![r])).unwrap();
    assert_eq!(e, blk(62, &[rec(&[(1, &SRC), (21, &item)])]));
}

// REQ: OA-AST-025
#[test]
fn compound_primary_with_a_trailing_zero_octet_is_refused_when_typed() {
    // I062/380 (FRN 11) is decoded into fields, and the encoder would write 0x80: refused,
    // naming the zero octet.
    let item = [0x81, 0x00, 0x3C, 0x66, 0xB0];
    let bytes = blk(62, &[rec(&[(1, &SRC), (11, &item)])]);
    let e = decode_block(&bytes).unwrap_err();
    assert_eq!(
        (e.kind, e.item, e.offset),
        (ErrorKind::CompoundNotMinimal, 380, 8)
    );
    // Shortest form decodes.
    let ok = blk(62, &[rec(&[(1, &SRC), (11, &[0x80, 0x3C, 0x66, 0xB0])])]);
    round_trips(&ok);
}

// REQ: OA-AST-026
#[test]
fn identification_of_all_zero_bits_is_kept_raw() {
    // I048/240 (FRN 9): six octets of zero are "no identification"; code 0 is not a character.
    let bytes = blk(48, &[rec(&[(1, &SRC), (9, &[0; 6])])]);
    let Block::Cat048(r) = round_trips(&bytes) else {
        panic!()
    };
    assert_eq!(r[0].ident, None);
    assert_eq!(r[0].raw[0].frn, 9);
    // One code 0 among real characters is still refused.
    let bytes = blk(48, &[rec(&[(1, &SRC), (9, &[0, 0, 0, 0, 0, 1])])]);
    assert_eq!(
        decode_block(&bytes).unwrap_err().kind,
        ErrorKind::InvalidValue
    );
    // I021/170 (FRN 29), the same.
    let bytes = blk(21, &[rec(&[(1, &SRC), (29, &[0; 6])])]);
    let Block::Cat021(r) = round_trips(&bytes) else {
        panic!()
    };
    assert_eq!(r[0].target_id, None);
    // I062/380 subitem 2 (primary 0x40): the subitem comes back as raw subitem 2.
    let mut item = vec![0x40];
    item.extend_from_slice(&[0; 6]);
    let bytes = blk(62, &[rec(&[(1, &SRC), (11, &item)])]);
    let Block::Cat062(r) = round_trips(&bytes) else {
        panic!()
    };
    let d = r[0].derived.as_ref().unwrap();
    assert_eq!(d.ident, None);
    assert_eq!(
        (d.other[0].number, d.other[0].bytes.as_slice()),
        (2, &[0u8; 6][..])
    );
    // A real identification beside raw subitem 2 is a duplicate on encode.
    let mut dd = d.clone();
    dd.ident = Some(*b"ABC123  ");
    let r2 = Cat062 {
        source: SourceId { sac: 25, sic: 100 },
        derived: Some(dd),
        ..Cat062::default()
    };
    assert_eq!(
        encode_block(&Block::Cat062(vec![r2])).unwrap_err().kind,
        ErrorKind::DuplicateItem
    );
}

// REQ: OA-AST-027
#[test]
fn reserved_expansion_and_special_purpose_fields_round_trip_raw() {
    // CAT021 FRN 48 (RE) and 49 (SP): explicit length octet counting itself, then content.
    let re = [0x05, 0x08, 0xF0, 0x01, 0x62];
    let sp = [0x03, 0xAA, 0xBB];
    let bytes = blk(21, &[rec(&[(1, &SRC), (48, &re), (49, &sp)])]);
    let Block::Cat021(r) = round_trips(&bytes) else {
        panic!()
    };
    let frns: Vec<(u8, Vec<u8>)> = r[0].raw.iter().map(|x| (x.frn, x.bytes.clone())).collect();
    assert_eq!(frns, vec![(48, re.to_vec()), (49, sp.to_vec())]);
    // The same in CAT048 (FRN 28 RE, 27 SP) and CAT062 (FRN 34 RE, 35 SP).
    let bytes = blk(48, &[rec(&[(1, &SRC), (27, &sp), (28, &re)])]);
    let Block::Cat048(r) = round_trips(&bytes) else {
        panic!()
    };
    assert_eq!(r[0].raw.len(), 2);
    let bytes = blk(62, &[rec(&[(1, &SRC), (34, &re), (35, &sp)])]);
    let Block::Cat062(r) = round_trips(&bytes) else {
        panic!()
    };
    assert_eq!(r[0].raw.len(), 2);
    // A length that runs past the record is refused, as for any explicit item.
    let bytes = blk(21, &[rec(&[(1, &SRC), (48, &[0x09, 1, 2])])]);
    assert_eq!(
        decode_block(&bytes).unwrap_err().kind,
        ErrorKind::ExplicitLengthInvalid
    );
}

// REQ: OA-AST-028
#[test]
fn a_cat062_record_in_another_layout_is_refused_not_misread() {
    // CAT062 Ed 1.21 puts I062/245 at FRN 10 with spare bits 6/1 of its first octet fixed to
    // zero. A record in the pre-standard layout (FRN 10 was a two-octet item) read as Ed 1.21
    // runs into that and is refused rather than decoded wrongly.
    let bytes = blk(
        62,
        &[rec(&[
            (1, &SRC),
            (10, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]),
        ])],
    );
    let e = decode_block(&bytes).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 245));
}
