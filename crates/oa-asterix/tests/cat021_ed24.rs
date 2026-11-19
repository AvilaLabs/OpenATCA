//! CAT021 Ed 2.4 (EUROCONTROL-SPEC-0149-12, Released Edition 2.4): the UAP of section 5.3 and the
//! items the export uses (5.2.x), tested from the specification's own item definitions. Every
//! expected octet below is worked out by hand from the structure drawings and LSBs of Ed 2.4,
//! not produced by the encoder under test. The differences to Ed 2.7 are checked in both
//! directions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_asterix::cat021::*;
use oa_asterix::uap::{uap_021, Cat021Edition, Kind, Slot};
use oa_asterix::*;

const ED24: Cat021Edition = Cat021Edition::Ed24;
const ED27: Cat021Edition = Cat021Edition::Ed27;

/// Table 2 of Ed 2.4 (section 5.3): FRN, item, length (0 for the variable-length "1+" items).
/// Compound and repetitive items are listed with their kind letter in the second table.
const TABLE_2: [(u8, u16, u8); 42] = [
    (1, 10, 2),
    (2, 40, 0),
    (3, 161, 2),
    (4, 15, 1),
    (5, 71, 3),
    (6, 130, 6),
    (7, 131, 8),
    (8, 72, 3),
    (9, 150, 2),
    (10, 151, 2),
    (11, 80, 3),
    (12, 73, 3),
    (13, 74, 4),
    (14, 75, 3),
    (15, 76, 4),
    (16, 140, 2),
    (17, 90, 0),
    (18, 210, 1),
    (19, 70, 2),
    (20, 230, 2),
    (21, 145, 2),
    (22, 152, 2),
    (23, 200, 1),
    (24, 155, 2),
    (25, 157, 2),
    (26, 160, 4),
    (27, 165, 2),
    (28, 77, 3),
    (29, 170, 6),
    (30, 20, 1),
    (32, 146, 2),
    (33, 148, 2),
    (35, 16, 1),
    (36, 8, 1),
    (38, 132, 1),
    (40, 260, 7),
    (41, 400, 1),
    (31, 220, 0),
    (34, 110, 0),
    (37, 271, 0),
    (39, 250, 0),
    (42, 295, 0),
];

// REQ: OA-AST-108
#[test]
fn the_ed24_uap_is_table_2_of_the_specification() {
    let uap = uap_021(ED24);
    assert_eq!(uap.slots.len(), 49);
    for (frn, id, len) in TABLE_2 {
        let Some(Slot::Item { id: got, field }) = oa_asterix::uap::slot_of(uap, frn) else {
            panic!("FRN {frn} is not an item");
        };
        assert_eq!(*got, id, "FRN {frn}");
        if len > 0 {
            assert!(matches!(field.kind, Kind::Fixed(n) if n == len), "FRN {frn} length");
        }
    }
    // Variable-length "1+" items: 040 and 090 are extended, 220 and 110 compound, 271 extended,
    // 250 repetitive (1+N*8), 295 compound.
    for (frn, extended) in [(2u8, true), (17, true), (37, true)] {
        let Some(Slot::Item { field, .. }) = oa_asterix::uap::slot_of(uap, frn) else {
            panic!();
        };
        assert_eq!(matches!(field.kind, Kind::Extended(1)), extended, "FRN {frn}");
    }
    let Some(Slot::Item { field, .. }) = oa_asterix::uap::slot_of(uap, 39) else {
        panic!();
    };
    assert!(matches!(field.kind, Kind::Repetitive { size: 8 }));
    // FRN 43 to 47 are not used, 48 is RE and 49 is SP.
    for frn in 43..=47 {
        assert!(matches!(oa_asterix::uap::slot_of(uap, frn), Some(Slot::Unused)));
    }
    assert!(matches!(oa_asterix::uap::slot_of(uap, 48), Some(Slot::Re)));
    assert!(matches!(oa_asterix::uap::slot_of(uap, 49), Some(Slot::Sp)));
}

// REQ: OA-AST-109
#[test]
fn the_two_editions_share_every_slot_but_the_spare_bits_of_the_quality_item() {
    let (a, b) = (uap_021(ED24), uap_021(ED27));
    for frn in 1..=49u8 {
        let (x, y) = (
            oa_asterix::uap::slot_of(a, frn).unwrap(),
            oa_asterix::uap::slot_of(b, frn).unwrap(),
        );
        if frn == 17 {
            assert_ne!(format!("{x:?}"), format!("{y:?}"));
        } else {
            assert_eq!(format!("{x:?}"), format!("{y:?}"), "FRN {frn}");
        }
    }
}

/// One record built by hand from the definitions of Ed 2.4:
///
/// * I021/010 (5.2.2): SAC 255, SIC 3.
/// * I021/040 (5.2.6): primary ATP 0 (bits 8/6), ARC 2 unknown (bits 5/4 = 10), RC 0, RAB 0, FX 1
///   = 0001 0001; first extension DCR 0, GBS 1, SIM 0, TST 0, SAA 0, CL 2 (bits 3/2 = 10), FX 0 =
///   0100 0100.
/// * I021/131 (5.2.19): latitude 50 deg = 50 / 180 * 2^30 = 298 261 617.8, count 298 261 618 =
///   0x11C71C72; longitude -10 deg = -59 652 323.6, count -59 652 324 = 0xFC71C71C (32 bit).
/// * I021/080 (5.2.15): 0x3C66B0.
/// * I021/073 (5.2.10): 12:00:00 = 43 200 s * 128 = 5 529 600 = 0x546000.
/// * I021/140 (5.2.21): 25 875 ft / 6.25 = 4140 = 0x102C.
/// * I021/090 (5.2.16): primary NACv 2 (bits 8/6 = 010), NIC 8 (bits 5/2 = 1000), FX 1 = 0101
///   0001; first extension NICBARO 1, SIL 3, NACp 8, FX 1 = 1111 0001 (0xF1); second extension
///   spare 00, SILS 1, SDA 2, GVA 2, FX 0 = 0011 0100.
/// * I021/210 (5.2.35): VNS 0, VN 2 (bits 6/4), LTT 2 (bits 3/1) = 0001 0010.
/// * I021/145 (5.2.22): FL 300 = 1200 quarter levels = 0x04B0.
/// * I021/155 (5.2.28): RE 0, -2176 ft/min / 6.25 = -348.16, count -348 in 15 bits = 0x7EA4.
/// * I021/160 (5.2.30): RE 0, 446.5 kt = 0.124028 NM/s * 2^14 = 2032.1, count 2032 = 0x07F0;
///   track 309 deg / 360 * 2^16 = 56 251.7, count 56 252 = 0xDBBC.
/// * I021/170 (5.2.33): "DLH7YA  " in the six-bit code (A = 1, D = 4, H = 8, L = 12, Y = 25,
///   space = 32, 7 = 55): 000100 001100 001000 110111 011001 000001 100000 100000.
/// * I021/020 (5.2.5): 3.
///
/// FSPEC: FRN 1, 2, 7 (0xC3); 11, 12 (0x19); 16, 17, 18, 21 (0x73); 24, 26 (0x29); 29, 30 (0xC0).
fn hand_record() -> Vec<u8> {
    let mut r = vec![0xC3, 0x19, 0x73, 0x29, 0xC0];
    r.extend_from_slice(&[0xFF, 0x03]);
    r.extend_from_slice(&[0x11, 0x44]);
    r.extend_from_slice(&[0x11, 0xC7, 0x1C, 0x72, 0xFC, 0x71, 0xC7, 0x1C]);
    r.extend_from_slice(&[0x3C, 0x66, 0xB0]);
    r.extend_from_slice(&[0x54, 0x60, 0x00]);
    r.extend_from_slice(&[0x10, 0x2C]);
    r.extend_from_slice(&[0x51, 0xF1, 0x34]);
    r.push(0x12);
    r.extend_from_slice(&[0x04, 0xB0]);
    r.extend_from_slice(&[0x7E, 0xA4]);
    r.extend_from_slice(&[0x07, 0xF0, 0xDB, 0xBC]);
    r.extend_from_slice(&[0x10, 0xC2, 0x37, 0x64, 0x18, 0x20]);
    r.push(0x03);
    r
}

fn hand_block() -> Vec<u8> {
    let rec = hand_record();
    let len = u16::try_from(3 + rec.len()).unwrap();
    let mut b = vec![21];
    b.extend_from_slice(&len.to_be_bytes());
    b.extend_from_slice(&rec);
    b
}

fn hand_values() -> Cat021 {
    Cat021 {
        source: SourceId { sac: 255, sic: 3 },
        descriptor: Some(Trd021 {
            atp: 0,
            arc: 2,
            range_check: false,
            field_monitor: false,
            ext: vec![0x44],
        }),
        position_hr: Some(RawPosition {
            lat: 298_261_618,
            lon: -59_652_324,
        }),
        address: Some(0x3C_66B0),
        tomr_position: Some(5_529_600),
        geometric_height: Some(4140),
        quality: Some(Quality021 {
            nacv: 2,
            nic: 8,
            ext: vec![0xF1, 0x34],
        }),
        mops: Some(Mops {
            not_supported: false,
            version: 2,
            link: 2,
        }),
        flight_level: Some(1200),
        baro_rate: Some(FlagS15 {
            range_exceeded: false,
            raw: -348,
        }),
        ground_vector: Some(GroundVector {
            range_exceeded: false,
            speed: 2032,
            track: 56_252,
        }),
        target_id: Some(*b"DLH7YA  "),
        emitter_category: Some(3),
        ..Cat021::default()
    }
}

// REQ: OA-AST-110
#[test]
fn a_record_worked_out_from_the_ed24_item_definitions_decodes_and_encodes() {
    let bytes = hand_block();
    let (block, used) = decode_block_with(&bytes, ED24).unwrap();
    assert_eq!(used, bytes.len());
    assert_eq!(block, Block::Cat021(vec![hand_values()]));
    assert_eq!(encode_block_with(&block, ED24).unwrap(), bytes);
    // The same octets are an Ed 2.7 record too: the items used here did not change.
    assert_eq!(decode_block_with(&bytes, ED27).unwrap().0, block);
    assert_eq!(quality_nacp(&hand_values().quality.unwrap()), Some(8));
    assert_eq!(quality_sil(&hand_values().quality.unwrap()), Some(3));
    assert_eq!(quality_nicbaro(&hand_values().quality.unwrap()), Some(1));
    assert_eq!(quality_sils(&hand_values().quality.unwrap()), Some(1));
    assert_eq!(quality_sda(&hand_values().quality.unwrap()), Some(2));
    assert_eq!(quality_gva(&hand_values().quality.unwrap()), Some(2));
    assert_eq!(descriptor_ground(&hand_values().descriptor.unwrap()), Some(true));
    assert_eq!(descriptor_confidence(&hand_values().descriptor.unwrap()), Some(2));
}

/// The record of [`hand_values`] with `f` applied, as one block.
fn block_with(f: impl FnOnce(&mut Cat021)) -> Block {
    let mut r = hand_values();
    f(&mut r);
    Block::Cat021(vec![r])
}

// REQ: OA-AST-111
#[test]
fn ed24_refuses_what_only_ed27_defines_and_ed27_accepts_it() {
    // I021/210 VN 3 (ED-102B/DO-260C) was added in Ed 2.7.
    let vn3 = block_with(|r| r.mops.as_mut().unwrap().version = 3);
    let e = encode_block_with(&vn3, ED24).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::ValueOutOfRange, 210));
    let wire = encode_block_with(&vn3, ED27).unwrap();
    let e = decode_block_with(&wire, ED24).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::ValueOutOfRange, 210));
    assert_eq!(decode_block_with(&wire, ED27).unwrap().0, vn3);
    // I021/040 third extension (TBC) and I021/090 fourth extension (validation): Ed 2.7 only.
    let third = block_with(|r| {
        let t = r.descriptor.as_mut().unwrap();
        t.ext = vec![0x45, 0x05, 0x04];
    });
    assert_eq!(encode_block_with(&third, ED24).unwrap_err().item, 40);
    let wire = encode_block_with(&third, ED27).unwrap();
    assert_eq!(decode_block_with(&wire, ED24).unwrap_err().item, 40);
    assert_eq!(decode_block_with(&wire, ED27).unwrap().0, third);
    let fourth = block_with(|r| r.quality.as_mut().unwrap().ext = vec![0xF1, 0x35, 0x61, 0x00]);
    assert_eq!(encode_block_with(&fourth, ED24).unwrap_err().item, 90);
    let wire = encode_block_with(&fourth, ED27).unwrap();
    assert_eq!(decode_block_with(&wire, ED24).unwrap_err().item, 90);
    assert!(decode_block_with(&wire, ED27).is_ok());
}

// REQ: OA-AST-112
#[test]
fn bit_4_of_the_third_quality_extension_is_spare_in_ed24_and_the_source_bit_in_ed27() {
    // Third extension PIC 5 (bits 8/5 = 0101) with bit 4 set: SRC in Ed 2.7 (5.2.16), a spare bit
    // that must be zero in Ed 2.4.
    let b = block_with(|r| r.quality.as_mut().unwrap().ext = vec![0xF1, 0x35, 0x50 | 0x08]);
    let wire = encode_block_with(&b, ED27).unwrap();
    let e = decode_block_with(&wire, ED24).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::SpareBitSet, 90));
    assert_eq!(decode_block_with(&wire, ED27).unwrap().0, b);
    // Without it the third extension is the same in both.
    let b = block_with(|r| r.quality.as_mut().unwrap().ext = vec![0xF1, 0x35, 0x50]);
    let wire = encode_block_with(&b, ED24).unwrap();
    assert_eq!(decode_block_with(&wire, ED24).unwrap().0, b);
    assert_eq!(decode_block_with(&wire, ED27).unwrap().0, b);
    // Spare bits 3/2 stay spare in both.
    let b = block_with(|r| r.quality.as_mut().unwrap().ext = vec![0xF1, 0x35, 0x50 | 0x04]);
    for ed in [ED24, ED27] {
        assert_eq!(encode_block_with(&b, ed).unwrap_err().kind, ErrorKind::SpareBitSet);
    }
}

// REQ: OA-AST-113
#[test]
fn the_default_functions_are_ed27_and_the_ed24_codec_round_trips_what_it_accepts() {
    let b = Block::Cat021(vec![hand_values()]);
    assert_eq!(encode_block(&b).unwrap(), encode_block_with(&b, ED27).unwrap());
    let bytes = hand_block();
    assert_eq!(decode_block(&bytes).unwrap(), decode_block_with(&bytes, ED27).unwrap());
    // A position at the limits of I021/131 (latitude +-90, longitude -180) and a time at the
    // last count of the day round-trip in Ed 2.4.
    let edge = block_with(|r| {
        r.position_hr = Some(RawPosition {
            lat: 1 << 29,
            lon: -(1 << 30),
        });
        r.tomr_position = Some(86_400 * 128 - 1);
    });
    let wire = encode_block_with(&edge, ED24).unwrap();
    assert_eq!(decode_block_with(&wire, ED24).unwrap().0, edge);
}
