//! Version-pinned, synthetic public-ICD vectors for the explicitly supported ASTERIX profiles.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_asterix::{decode_block, decode_block_with, Block, Cat021Edition, ErrorKind};

const REGISTRY: &str = include_str!("../../../validation/interface-conformance-v1.json");

// REQ: OA-IFACE-001
#[test]
fn registry_pins_supported_asterix_profiles_and_partial_scope() {
    for marker in [
        "EUROCONTROL-ASTERIX-PART1",
        "EUROCONTROL-ASTERIX-CAT021",
        "EUROCONTROL-ASTERIX-CAT048",
        "EUROCONTROL-ASTERIX-CAT062",
        "\"edition\": \"2.7\"",
        "\"edition\": \"2.4\"",
        "\"edition\": \"1.32\"",
        "\"edition\": \"1.21\"",
        "does not claim whole-ICD conformance",
        "not_verified",
    ] {
        assert!(REGISTRY.contains(marker), "registry lacks {marker}");
    }
}

// REQ: OA-IFACE-001
#[test]
fn synthetic_cat021_ed27_and_ed24_vectors_accept_and_refuse_at_profile_boundary() {
    let (Block::Cat021(rows), used) = decode_block_with(
        include_bytes!("../../../vectors/asterix/cat021-ed2.7/minimal-typed/input.bin"),
        Cat021Edition::Ed27,
    )
    .unwrap() else {
        panic!("CAT021 Ed 2.7 vector decoded as another category");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        used,
        include_bytes!("../../../vectors/asterix/cat021-ed2.7/minimal-typed/input.bin").len()
    );

    let bytes = include_bytes!("../../../vectors/asterix/cat021-ed2.4/minimal-typed/input.bin");
    let (Block::Cat021(rows), used) = decode_block_with(bytes, Cat021Edition::Ed24).unwrap() else {
        panic!("CAT021 Ed 2.4 vector decoded as another category");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(used, bytes.len());

    let refusal = decode_block_with(
        include_bytes!("../../../vectors/asterix/cat021-ed2.4/refuse-mops-v3/input.bin"),
        Cat021Edition::Ed24,
    )
    .unwrap_err();
    assert_eq!(
        (refusal.cat, refusal.item, refusal.kind),
        (21, 210, ErrorKind::ValueOutOfRange)
    );

    let truncated = decode_block(include_bytes!(
        "../../../vectors/asterix/cat021-ed2.7/refuse-truncated/input.bin"
    ))
    .unwrap_err();
    assert_eq!(
        (truncated.cat, truncated.kind, truncated.offset),
        (21, ErrorKind::Truncated, 2)
    );
}

// REQ: OA-IFACE-001
#[test]
fn synthetic_cat048_and_cat062_vectors_accept_and_refuse_declared_domains() {
    let cat048 = include_bytes!("../../../vectors/asterix/cat048-ed1.32/minimal-typed/input.bin");
    let (Block::Cat048(rows), used) = decode_block(cat048).unwrap() else {
        panic!("CAT048 vector decoded as another category");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(used, cat048.len());
    let refusal = decode_block(include_bytes!(
        "../../../vectors/asterix/cat048-ed1.32/refuse-zero-repetition/input.bin"
    ))
    .unwrap_err();
    assert_eq!(
        (refusal.cat, refusal.item, refusal.kind),
        (48, 250, ErrorKind::ZeroRepetition)
    );

    let cat062 = include_bytes!("../../../vectors/asterix/cat062-ed1.21/minimal-typed/input.bin");
    let (Block::Cat062(rows), used) = decode_block(cat062).unwrap() else {
        panic!("CAT062 vector decoded as another category");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(used, cat062.len());
    let refusal = decode_block(include_bytes!(
        "../../../vectors/asterix/cat062-ed1.21/refuse-latitude-over-90/input.bin"
    ))
    .unwrap_err();
    assert_eq!(
        (refusal.cat, refusal.item, refusal.kind),
        (62, 105, ErrorKind::InvalidValue)
    );
}
