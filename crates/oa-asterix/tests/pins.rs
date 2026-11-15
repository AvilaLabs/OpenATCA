//! Values and places that other tests only touch in passing: every field of the CAT021 quality
//! and descriptor extensions at its own bit position, the exact value of each unit constant,
//! the byte offset of each refusal in a record that is not the first of a block that does not
//! start the stream, the derived-data subitem rules of CAT062, and the rounding of positions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
use common::*;
use oa_asterix::cat021::*;
use oa_asterix::cat062::*;
use oa_asterix::units::*;
use oa_asterix::*;

const SRC: [u8; 2] = [0x19, 0x03];

fn q(ext: &[u8]) -> Quality021 {
    Quality021 {
        nacv: 0,
        nic: 0,
        ext: ext.to_vec(),
    }
}

fn t(ext: &[u8]) -> Trd021 {
    Trd021 {
        ext: ext.to_vec(),
        ..Trd021::default()
    }
}

// REQ: OA-AST-078
#[test]
fn quality_extension_fields_sit_at_their_own_bits() {
    // First extension: NICBARO bit 8, SIL bits 7/6, NACp bits 5/2, FX bit 1.
    // 0xCA = 1 10 0101 0: NICBARO 1, SIL 2, NACp 5.
    let a = q(&[0xCA]);
    assert_eq!(
        (quality_nicbaro(&a), quality_sil(&a), quality_nacp(&a)),
        (Some(1), Some(2), Some(5))
    );
    // 0x34 = 0 01 1010 0: NICBARO 0, SIL 1, NACp 10.
    let a = q(&[0x34]);
    assert_eq!(
        (quality_nicbaro(&a), quality_sil(&a), quality_nacp(&a)),
        (Some(0), Some(1), Some(10))
    );
    // All ones (FX included): 1, 3, 15. FX alone and all zero: nothing set.
    let a = q(&[0xFF]);
    assert_eq!(
        (quality_nicbaro(&a), quality_sil(&a), quality_nacp(&a)),
        (Some(1), Some(3), Some(15))
    );
    for b in [0x00, 0x01] {
        let a = q(&[b]);
        assert_eq!(
            (quality_nicbaro(&a), quality_sil(&a), quality_nacp(&a)),
            (Some(0), Some(0), Some(0))
        );
    }
    // One field at a time, so a neighbour's bits cannot leak in.
    assert_eq!(quality_nacp(&q(&[0x1E])), Some(15));
    assert_eq!(quality_sil(&q(&[0x60])), Some(3));
    assert_eq!(quality_nicbaro(&q(&[0x80])), Some(1));
    assert_eq!(quality_nicbaro(&q(&[0x7F])), Some(0));
    assert_eq!(quality_sil(&q(&[0x9F])), Some(0));
    assert_eq!(quality_nacp(&q(&[0xE1])), Some(0));

    // Second extension: SILS bit 6, SDA bits 5/4, GVA bits 3/2, FX bit 1. The first extension
    // is all ones so that reading the wrong octet shows.
    let ext2 = |b: u8| q(&[0xFF, b]);
    // 0x32 = 00 1 10 01 0: SILS 1, SDA 2, GVA 1.
    let a = ext2(0x32);
    assert_eq!(
        (quality_sils(&a), quality_sda(&a), quality_gva(&a)),
        (Some(1), Some(2), Some(1))
    );
    // 0x0C = 00 0 01 10 0: SILS 0, SDA 1, GVA 2.
    let a = ext2(0x0C);
    assert_eq!(
        (quality_sils(&a), quality_sda(&a), quality_gva(&a)),
        (Some(0), Some(1), Some(2))
    );
    let a = ext2(0xFF);
    assert_eq!(
        (quality_sils(&a), quality_sda(&a), quality_gva(&a)),
        (Some(1), Some(3), Some(3))
    );
    for b in [0x00, 0x01] {
        let a = ext2(b);
        assert_eq!(
            (quality_sils(&a), quality_sda(&a), quality_gva(&a)),
            (Some(0), Some(0), Some(0))
        );
    }
    assert_eq!(quality_sils(&ext2(0x20)), Some(1));
    assert_eq!(quality_sda(&ext2(0x20)), Some(0));
    assert_eq!(quality_sda(&ext2(0x18)), Some(3));
    assert_eq!(quality_gva(&ext2(0x18)), Some(0));
    assert_eq!(quality_gva(&ext2(0x06)), Some(3));
    assert_eq!(quality_sda(&ext2(0x06)), Some(0));
    assert_eq!(quality_sils(&ext2(0x40)), Some(0));
    assert_eq!(quality_sda(&ext2(0x40)), Some(0));

    // Third extension: PIC bits 8/5. The earlier octets are all ones.
    let ext3 = |b: u8| q(&[0xFF, 0xFF, b]);
    assert_eq!(quality_pic(&ext3(0xA5)), Some(10));
    assert_eq!(quality_pic(&ext3(0x5A)), Some(5));
    assert_eq!(quality_pic(&ext3(0xFF)), Some(15));
    assert_eq!(quality_pic(&ext3(0x0F)), Some(0));
    assert_eq!(quality_pic(&ext3(0x10)), Some(1));
    assert_eq!(quality_pic(&ext3(0x00)), Some(0));

    // Absent extensions give None.
    assert_eq!(quality_nacp(&q(&[])), None);
    assert_eq!(quality_sils(&q(&[0xFF])), None);
    assert_eq!(quality_gva(&q(&[0xFF])), None);
    assert_eq!(quality_pic(&q(&[0xFF, 0xFF])), None);
}

// REQ: OA-AST-079
#[test]
fn descriptor_extension_fields_sit_at_their_own_bits() {
    // First extension: DCR bit 8, GBS bit 7, SIM bit 6, TST bit 5, SAA bit 4, CL bits 3/2.
    // 0x54 = 0 1 0 1 0 10 0: GBS 1, SIM 0, TST 1, CL 2.
    let a = t(&[0x54]);
    assert_eq!(
        (
            descriptor_ground(&a),
            descriptor_simulated(&a),
            descriptor_test(&a),
            descriptor_confidence(&a)
        ),
        (Some(true), Some(false), Some(true), Some(2))
    );
    // 0x2A = 0 0 1 0 1 01 0: GBS 0, SIM 1, TST 0, CL 1.
    let a = t(&[0x2A]);
    assert_eq!(
        (
            descriptor_ground(&a),
            descriptor_simulated(&a),
            descriptor_test(&a),
            descriptor_confidence(&a)
        ),
        (Some(false), Some(true), Some(false), Some(1))
    );
    let a = t(&[0xFF]);
    assert_eq!(
        (
            descriptor_ground(&a),
            descriptor_simulated(&a),
            descriptor_test(&a),
            descriptor_confidence(&a)
        ),
        (Some(true), Some(true), Some(true), Some(3))
    );
    // DCR (bit 8), SAA (bit 4), FX (bit 1) and the zero octet set none of the four.
    for b in [0x00, 0x80, 0x08, 0x01, 0x89] {
        let a = t(&[b]);
        assert_eq!(
            (
                descriptor_ground(&a),
                descriptor_simulated(&a),
                descriptor_test(&a),
                descriptor_confidence(&a)
            ),
            (Some(false), Some(false), Some(false), Some(0)),
            "{b:#04x}"
        );
    }
    // One at a time.
    assert_eq!(descriptor_ground(&t(&[0x40])), Some(true));
    assert_eq!(descriptor_simulated(&t(&[0x40])), Some(false));
    assert_eq!(descriptor_simulated(&t(&[0x20])), Some(true));
    assert_eq!(descriptor_test(&t(&[0x20])), Some(false));
    assert_eq!(descriptor_test(&t(&[0x10])), Some(true));
    assert_eq!(descriptor_confidence(&t(&[0x10])), Some(0));
    assert_eq!(descriptor_confidence(&t(&[0x06])), Some(3));
    assert_eq!(descriptor_confidence(&t(&[0x04])), Some(2));
    assert_eq!(descriptor_confidence(&t(&[0x02])), Some(1));
    assert_eq!(descriptor_ground(&t(&[0x06])), Some(false));
    assert_eq!(descriptor_test(&t(&[0x06])), Some(false));
    // Absent extension: None.
    assert_eq!(descriptor_ground(&t(&[])), None);
    assert_eq!(descriptor_simulated(&t(&[])), None);
    assert_eq!(descriptor_test(&t(&[])), None);
    assert_eq!(descriptor_confidence(&t(&[])), None);
}

// REQ: OA-AST-080
#[test]
fn descriptor_and_quality_items_decode_from_the_wire() {
    // I021/040: ATP 5 (bits 8/6 = 101), ARC 2 (bits 5/4 = 10), RC 1, RAB 0, FX 1:
    // 0b101_10_1_0_1 = 0xB5, then the extension 0x54 (see above) with FX clear.
    // I021/090: NACv 5 (bits 8/6 = 101), NIC 6 (bits 5/2 = 0110), FX 1: 0b101_0110_1 = 0xAD;
    // extensions 0xCB (NICBARO 1, SIL 2, NACp 5, FX 1), 0x33 (SILS 1, SDA 2, GVA 1, FX 1),
    // 0xA0 (PIC 10, FX 0).
    let b = blk(
        21,
        &[rec(&[
            (1, &SRC),
            (2, &[0xB5, 0x54]),
            (17, &[0xAD, 0xCB, 0x33, 0xA0]),
        ])],
    );
    let (blk_, _) = decode_block(&b).unwrap();
    let Block::Cat021(recs) = &blk_ else { panic!() };
    let d = recs[0].descriptor.as_ref().unwrap();
    assert_eq!(
        (d.atp, d.arc, d.range_check, d.field_monitor),
        (5, 2, true, false)
    );
    assert_eq!(d.ext, vec![0x54]);
    assert_eq!(
        (
            descriptor_ground(d),
            descriptor_simulated(d),
            descriptor_test(d),
            descriptor_confidence(d)
        ),
        (Some(true), Some(false), Some(true), Some(2))
    );
    let qi = recs[0].quality.as_ref().unwrap();
    assert_eq!((qi.nacv, qi.nic), (5, 6));
    assert_eq!(qi.ext, vec![0xCB, 0x33, 0xA0]);
    assert_eq!(
        (
            quality_nicbaro(qi),
            quality_sil(qi),
            quality_nacp(qi),
            quality_sils(qi),
            quality_sda(qi),
            quality_gva(qi),
            quality_pic(qi)
        ),
        (
            Some(1),
            Some(2),
            Some(5),
            Some(1),
            Some(2),
            Some(1),
            Some(10)
        )
    );
    assert_eq!(encode_block(&blk_).unwrap(), b);

    // Primary subfields alone: ATP, ARC, RC and RAB each at their own bits.
    let one = |p: u8| {
        let b = blk(21, &[rec(&[(1, &SRC), (2, &[p])])]);
        let (x, _) = decode_block(&b).unwrap();
        let Block::Cat021(r) = x else { panic!() };
        let d = r[0].descriptor.clone().unwrap();
        (d.atp, d.arc, d.range_check, d.field_monitor)
    };
    // 0x4A = 010 01 0 1 0.
    assert_eq!(one(0x4A), (2, 1, false, true));
    // 0xFE = 111 11 1 1 0 (spare ATP value 7 is accepted).
    assert_eq!(one(0xFE), (7, 3, true, true));
    assert_eq!(one(0x00), (0, 0, false, false));
    assert_eq!(one(0x20), (1, 0, false, false));
    assert_eq!(one(0x18), (0, 3, false, false));
    assert_eq!(one(0x04), (0, 0, true, false));
    assert_eq!(one(0x02), (0, 0, false, true));
    assert_eq!(one(0xC0), (6, 0, false, false));
}

// REQ: OA-AST-081
#[test]
fn every_unit_constant_has_its_exact_value() {
    // CAT021
    assert_eq!(CAT021_TIME_S, 0.007_812_5);
    assert_eq!(CAT021_TIME_HP_S, 2f64.powi(-30));
    assert_eq!(CAT021_POS_DEG, 180.0 * 2f64.powi(-23));
    assert_eq!(CAT021_POS_DEG, 2.145_767_211_914_062_5e-5);
    assert_eq!(CAT021_POS_BITS, 23);
    assert_eq!(CAT021_POS_HR_DEG, 180.0 * 2f64.powi(-30));
    assert_eq!(CAT021_POS_HR_BITS, 30);
    assert_eq!(CAT021_GEOM_HEIGHT_FT, 6.25);
    assert_eq!(CAT021_FL, 0.25);
    assert_eq!(CAT021_SELECTED_ALT_FT, 25.0);
    assert_eq!(CAT021_IAS_NM_S, 2f64.powi(-14));
    assert_eq!(CAT021_IAS_NM_S, 0.000_061_035_156_25);
    assert_eq!(CAT021_MACH, 0.001);
    assert_eq!(CAT021_TAS_KT, 1.0);
    assert_eq!(CAT021_ANGLE_DEG, 360.0 * 2f64.powi(-16));
    assert_eq!(CAT021_ANGLE_DEG, 0.005_493_164_062_5);
    assert_eq!(CAT021_VRATE_FPM, 6.25);
    assert_eq!(CAT021_GS_NM_S, 2f64.powi(-14));
    assert_eq!(CAT021_TAR_DEG_S, 0.031_25);
    assert_eq!(CAT021_ROLL_DEG, 0.01);
    // CAT048
    assert_eq!(CAT048_TIME_S, 0.007_812_5);
    assert_eq!(CAT048_RHO_NM, 0.003_906_25);
    assert_eq!(CAT048_THETA_DEG, 0.005_493_164_062_5);
    assert_eq!(CAT048_XY_NM, 0.007_812_5);
    assert_eq!(CAT048_FL, 0.25);
    assert_eq!(CAT048_RUNLENGTH_DEG, 360.0 * 2f64.powi(-13));
    assert_eq!(CAT048_RUNLENGTH_DEG, 0.043_945_312_5);
    assert_eq!(CAT048_AMPLITUDE_DBM, 1.0);
    assert_eq!(CAT048_RPD_NM, 0.003_906_25);
    assert_eq!(CAT048_APD_DEG, 360.0 * 2f64.powi(-14));
    assert_eq!(CAT048_APD_DEG, 0.021_972_656_25);
    // CAT062
    assert_eq!(CAT062_TIME_S, 0.007_812_5);
    assert_eq!(CAT062_POS_DEG, 180.0 * 2f64.powi(-25));
    assert_eq!(CAT062_POS_BITS, 25);
    assert_eq!(CAT062_CART_M, 0.5);
    assert_eq!(CAT062_VEL_M_S, 0.25);
    assert_eq!(CAT062_GEOM_ALT_FT, 6.25);
    assert_eq!(CAT062_FL, 0.25);
    assert_eq!(CAT062_ROC_FPM, 6.25);
    assert_eq!(CAT062_AGE_S, 0.25);
    assert_eq!(CAT062_SELECTED_ALT_FT, 25.0);
}

/// Decodes `bytes` as if its first octet were at stream offset 1000 and checks the refusal:
/// category, record, item, subitem, kind, and the offset relative to the start of `bytes`.
#[allow(clippy::too_many_arguments)]
fn refused_at(
    bytes: &[u8],
    cat: u8,
    record: u32,
    item: u16,
    sub: u8,
    offset: usize,
    kind: ErrorKind,
) {
    let e = decode_block_at(bytes, 1000).unwrap_err();
    assert_eq!(
        (e.cat, e.record, e.item, e.sub, e.offset, e.kind),
        (cat, record, item, sub, 1000 + offset, kind),
        "{e}"
    );
    // The same block read from offset zero reports the plain offset.
    let e = decode_block(bytes).unwrap_err();
    assert_eq!((e.record, e.offset, e.kind), (record, offset, kind));
}

/// A block of one good record (FSPEC 0x80 and the source: three octets, so the second record
/// starts at offset 6) and then `second`.
fn second(cat: u8, second: Vec<u8>) -> Vec<u8> {
    blk(cat, &[rec(&[(1, &SRC)]), second])
}

// REQ: OA-AST-082
#[test]
fn block_header_refusals_name_their_offset() {
    let e = decode_block_at(&[21, 0], 1000).unwrap_err();
    assert_eq!(
        (e.kind, e.offset, e.record, e.item),
        (ErrorKind::Truncated, 1002, NO_RECORD, NO_ITEM)
    );
    let e = decode_block_at(&[], 1000).unwrap_err();
    assert_eq!((e.kind, e.offset, e.cat), (ErrorKind::Truncated, 1000, 0));
    let e = decode_block_at(&[21, 0, 2, 0x80], 1000).unwrap_err();
    assert_eq!(
        (e.kind, e.offset),
        (ErrorKind::BlockLengthBelowHeader, 1001)
    );
    let e = decode_block_at(&[21, 0, 9, 0x80, 0x19, 0x03], 1000).unwrap_err();
    assert_eq!(
        (e.kind, e.offset),
        (ErrorKind::BlockLengthExceedsInput, 1001)
    );
    let e = decode_block_at(&[34, 0, 6, 0x80, 1, 2], 1000).unwrap_err();
    assert_eq!(
        (e.kind, e.offset, e.cat),
        (ErrorKind::UnsupportedCategory, 1000, 34)
    );
    let e = decode_block_at(&[21, 0, 3], 1000).unwrap_err();
    assert_eq!((e.kind, e.offset), (ErrorKind::EmptyBlock, 1003));
    // LEN below the input: the octets after the block are not looked at.
    let ok = decode_block_at(&[21, 0, 6, 0x80, 0x19, 0x03, 0xFF, 0xFF], 1000).unwrap();
    assert_eq!(ok.1, 6);
}

// REQ: OA-AST-083
#[test]
fn fspec_refusals_in_a_later_record_name_their_offset() {
    // The second record starts at offset 6.
    refused_at(
        &second(21, vec![0x00]),
        21,
        1,
        0,
        0,
        6,
        ErrorKind::EmptyFspec,
    );
    refused_at(
        &second(21, vec![0x81]),
        21,
        1,
        0,
        0,
        7,
        ErrorKind::Truncated,
    );
    refused_at(
        &second(21, vec![0x81, 0x00]),
        21,
        1,
        0,
        0,
        7,
        ErrorKind::FspecNotMinimal,
    );
    refused_at(
        &second(48, vec![0x81, 0x01, 0x01, 0x01]),
        48,
        1,
        0,
        0,
        9,
        ErrorKind::FspecTooLong,
    );
    refused_at(
        &second(62, vec![0x40]),
        62,
        1,
        0,
        0,
        6,
        ErrorKind::UnknownUapSlot { frn: 2 },
    );
    // FRN 29 of CAT062 is in the fifth octet: offset 6 + 4.
    refused_at(
        &second(62, vec![0x81, 0x01, 0x01, 0x01, 0x80]),
        62,
        1,
        0,
        0,
        10,
        ErrorKind::UnknownUapSlot { frn: 29 },
    );
    // No data source identifier: the error is at the start of the record.
    refused_at(
        &second(21, vec![0x40, 0x08]),
        21,
        1,
        0,
        0,
        6,
        ErrorKind::MissingMandatory { frn: 1 },
    );
}

// REQ: OA-AST-084
#[test]
fn item_refusals_in_a_later_record_name_their_offset() {
    // One-octet FSPEC: the source is at 7 and 8, the next item at 9 (FRN 10 needs a second
    // FSPEC octet, so its item is at 10).
    // Fixed item with no octet: I021/015.
    refused_at(
        &second(21, rec(&[(1, &SRC), (4, &[])])),
        21,
        1,
        15,
        0,
        9,
        ErrorKind::Truncated,
    );
    // Extended item with the FX chain cut by the block end: I021/040.
    refused_at(
        &second(21, rec(&[(1, &SRC), (2, &[0x01])])),
        21,
        1,
        40,
        0,
        9,
        ErrorKind::ExtensionUnterminated,
    );
    // Repetitive: I048/250 with no factor, a zero factor, a factor past the block.
    refused_at(
        &second(48, rec(&[(1, &SRC), (10, &[])])),
        48,
        1,
        250,
        0,
        10,
        ErrorKind::Truncated,
    );
    refused_at(
        &second(48, rec(&[(1, &SRC), (10, &[0x00])])),
        48,
        1,
        250,
        0,
        10,
        ErrorKind::ZeroRepetition,
    );
    refused_at(
        &second(48, rec(&[(1, &SRC), (10, &[0x02, 1, 2, 3])])),
        48,
        1,
        250,
        0,
        10,
        ErrorKind::Truncated,
    );
    // Explicit length: CAT048 SP is FRN 27, so the FSPEC has four octets (6 to 9), the source
    // 10 and 11, the item at 12. No item number is reported for SP.
    refused_at(
        &second(48, rec(&[(1, &SRC), (27, &[])])),
        48,
        1,
        0,
        0,
        12,
        ErrorKind::Truncated,
    );
    refused_at(
        &second(48, rec(&[(1, &SRC), (27, &[0x01])])),
        48,
        1,
        0,
        0,
        12,
        ErrorKind::ExplicitLengthInvalid,
    );
    refused_at(
        &second(48, rec(&[(1, &SRC), (27, &[0x09, 1, 2])])),
        48,
        1,
        0,
        0,
        12,
        ErrorKind::ExplicitLengthInvalid,
    );
    // Compound: I048/130 with no subitem.
    refused_at(
        &second(48, rec(&[(1, &SRC), (7, &[0x00])])),
        48,
        1,
        130,
        0,
        9,
        ErrorKind::EmptyCompound,
    );
    // I062/290 (FRN 14: two FSPEC octets, the source at 8 and 9, the item at 10). Primary
    // subfield cut by the block end; FX on the last defined octet; an undefined subitem bit
    // (subitem 11, in the second octet); a subitem cut short (subitem 5 has two octets).
    refused_at(
        &second(62, rec(&[(1, &SRC), (14, &[0x81])])),
        62,
        1,
        290,
        0,
        10,
        ErrorKind::Truncated,
    );
    refused_at(
        &second(62, rec(&[(1, &SRC), (14, &[0x81, 0x01])])),
        62,
        1,
        290,
        0,
        11,
        ErrorKind::UnknownSubitem,
    );
    refused_at(
        &second(62, rec(&[(1, &SRC), (14, &[0x01, 0x10])])),
        62,
        1,
        290,
        11,
        11,
        ErrorKind::UnknownSubitem,
    );
    refused_at(
        &second(62, rec(&[(1, &SRC), (14, &[0x08, 0x00])])),
        62,
        1,
        290,
        5,
        11,
        ErrorKind::Truncated,
    );
    // Spare bits: I021/070 bit 13 in its first octet (FSPEC of three octets, the item at 11),
    // and the second extension of I021/090 (item at 11, octet index 2).
    refused_at(
        &second(21, rec(&[(1, &SRC), (19, &[0x10, 0x00])])),
        21,
        1,
        70,
        0,
        11,
        ErrorKind::SpareBitSet,
    );
    refused_at(
        &second(21, rec(&[(1, &SRC), (17, &[0x01, 0x01, 0x80])])),
        21,
        1,
        90,
        0,
        13,
        ErrorKind::SpareBitSet,
    );
}

// REQ: OA-AST-085
#[test]
fn typed_layer_refusals_in_a_later_record_name_their_offset() {
    // A latitude past 90 degrees in I021/130 (FRN 6, the item at 9): the item's first octet.
    let item = [0x40, 0x00, 0x01, 0, 0, 0];
    refused_at(
        &second(21, rec(&[(1, &SRC), (6, &item)])),
        21,
        1,
        130,
        0,
        9,
        ErrorKind::InvalidValue,
    );
    // A compound primary subfield with a trailing zero octet, in an item the typed layer
    // decodes: I062/290 at 10, the zero octet is index 1.
    refused_at(
        &second(62, rec(&[(1, &SRC), (14, &[0x81, 0x00, 0x05])])),
        62,
        1,
        290,
        0,
        11,
        ErrorKind::CompoundNotMinimal,
    );
    // The same when the item also fails to decode: I062/380 with a primary of 0x41 0x00 and
    // an identification whose last character code is 0. The primary is named first.
    refused_at(
        &second(62, rec(&[(1, &SRC), (11, &[0x41, 0x00, 0, 0, 0, 0, 0, 1])])),
        62,
        1,
        380,
        0,
        11,
        ErrorKind::CompoundNotMinimal,
    );
    // And without the extra primary octet the decode failure is reported at the item start.
    refused_at(
        &second(62, rec(&[(1, &SRC), (11, &[0x40, 0, 0, 0, 0, 0, 1])])),
        62,
        1,
        380,
        0,
        10,
        ErrorKind::InvalidValue,
    );
}

// REQ: OA-AST-086
#[test]
fn refusal_offsets_in_a_stream_are_absolute() {
    let first = blk(21, &[rec(&[(1, &SRC)])]);
    let bad = second(21, rec(&[(1, &SRC), (4, &[])]));
    let mut s = first.clone();
    s.extend(&bad);
    let e = decode_stream(&s).unwrap_err();
    assert_eq!((e.record, e.item, e.kind), (1, 15, ErrorKind::Truncated));
    assert_eq!(e.offset, first.len() + 9);
}

// REQ: OA-AST-087
#[test]
fn derived_data_subitem_one_and_six_cannot_be_given_as_other() {
    let enc = |d: AircraftDerived| {
        encode_block(&Block::Cat062(vec![Cat062 {
            source: SourceId { sac: 25, sic: 3 },
            derived: Some(d),
            ..Cat062::default()
        }]))
    };
    let other = |n: u8, bytes: &[u8]| RawSub {
        number: n,
        bytes: bytes.to_vec(),
    };
    // Subitem 1 (address) as an unparsed subitem with no typed address: refused.
    let e = enc(AircraftDerived {
        other: vec![other(1, &[0, 0, 1])],
        ..AircraftDerived::default()
    })
    .unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::DuplicateItem, 380));
    // Subitem 6 (selected altitude) likewise.
    let e = enc(AircraftDerived {
        other: vec![other(6, &[0, 1])],
        ..AircraftDerived::default()
    })
    .unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::DuplicateItem, 380));
    // Subitem 2 (identification) with a typed identification: refused.
    let e = enc(AircraftDerived {
        ident: Some(*b"ABC123  "),
        other: vec![other(2, &[1, 2, 3, 4, 5, 6])],
        ..AircraftDerived::default()
    })
    .unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::DuplicateItem, 380));
    // Subitem 2 with no typed identification is allowed: FSPEC 0x81 0x10 (FRN 1, 11), the
    // source, the primary 0x40 and the six octets.
    let ok = enc(AircraftDerived {
        other: vec![other(2, &[1, 2, 3, 4, 5, 6])],
        ..AircraftDerived::default()
    })
    .unwrap();
    assert_eq!(&ok[3..], &[0x81, 0x10, 25, 3, 0x40, 1, 2, 3, 4, 5, 6]);
    // Subitem 3 is a plain subitem, kept with the typed ones.
    let ok = enc(AircraftDerived {
        address: Some(0x0A_0B0C),
        other: vec![other(3, &[0x12, 0x34])],
        ..AircraftDerived::default()
    })
    .unwrap();
    assert_eq!(
        &ok[3..],
        &[0x81, 0x10, 25, 3, 0xA0, 0x0A, 0x0B, 0x0C, 0x12, 0x34]
    );
    // Number zero is not a subitem.
    let e = enc(AircraftDerived {
        other: vec![other(0, &[0])],
        ..AircraftDerived::default()
    })
    .unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::UnknownSubitem, 380));
    // No subitem at all.
    let e = enc(AircraftDerived::default()).unwrap_err();
    assert_eq!((e.kind, e.item), (ErrorKind::EmptyCompound, 380));
}

// REQ: OA-AST-088
#[test]
fn position_rounding_is_to_nearest_half_away_from_zero() {
    // 1.8e9 / 2^10 = 1 757 812.5 exactly: a tie rounds away from zero on both signs.
    assert_eq!(raw_to_e7(1, 10), 1_757_813);
    assert_eq!(raw_to_e7(-1, 10), -1_757_813);
    assert_eq!(raw_to_e7(3, 10), 5_273_438);
    assert_eq!(raw_to_e7(-3, 10), -5_273_438);
    // Not ties: 214.577 rounds up, 53.644 up, 8.38 down, 1.68 up.
    assert_eq!(raw_to_e7(1, 23), 215);
    assert_eq!(raw_to_e7(-1, 23), -215);
    assert_eq!(raw_to_e7(1, 25), 54);
    assert_eq!(raw_to_e7(-1, 25), -54);
    assert_eq!(raw_to_e7(5, 30), 8);
    assert_eq!(raw_to_e7(-5, 30), -8);
    assert_eq!(raw_to_e7(1, 30), 2);
    assert_eq!(raw_to_e7(-1, 30), -2);
    assert_eq!(raw_to_e7(0, 30), 0);
    assert_eq!(raw_to_e7(293_867_031, 30), 492_633_000);
    assert_eq!(raw_to_e7(-293_867_031, 30), -492_633_000);
    // The inverse: 1e-7 degree to counts, ties away from zero, negatives too.
    assert_eq!(e7_to_raw(1_757_813, 10), 1);
    assert_eq!(e7_to_raw(-1_757_813, 10), -1);
    assert_eq!(e7_to_raw(-492_633_000, 30), -293_867_031);
    assert_eq!(e7_to_raw(-215, 23), -1);
    assert_eq!(e7_to_raw(100, 23), 0);
    assert_eq!(e7_to_raw(-100, 23), 0);
    assert_eq!(e7_to_raw(108, 23), 1);
    assert_eq!(e7_to_raw(-108, 23), -1);
}
