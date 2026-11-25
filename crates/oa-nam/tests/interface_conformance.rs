//! Version-pinned NAM examples and profile boundaries from the public ICD inventory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_nam::{parse, parse_with, Kind, Profile};

const REGISTRY: &str = include_str!("../../../validation/interface-conformance-v1.json");

// REQ: OA-IFACE-001
#[test]
fn registry_pins_nam_sources_profiles_and_partial_scope() {
    for marker in [
        "FAA-NAM-ACC-TO-ACC",
        "NAS-IC-21009205",
        "NAS-IC-82422100",
        "NAS-IR-82422100",
        "3fbbe3eba9ea9616a39da624e871c23ca79862aca0d640c85ad70f5ee63d0d87",
        "9a3e796063de64dfa251358e011104ddeaed19aee1f7190e01b2e9f819a79214",
        "d54e1ab7e15d8320af64fbcecb5bd90294f56f92b7a22857505b4c554ad85126",
        "926283b4293a5121e331a64496a14b6f7a5d9e7e103f6e177f9704e29b889bb7",
        "a8df0324e7f111abeb1891711c91ffda6539a6698228b8a1038681c66e344dac",
        "\"profile\": \"Core\"",
        "\"profile\": \"EramAttachment2\"",
        "\"profile\": \"RevE\"",
        "No comparison to unavailable ERAM/NAM production messages or live traffic is made.",
    ] {
        assert!(REGISTRY.contains(marker), "registry lacks {marker}");
    }
}

// REQ: OA-IFACE-001
#[test]
fn public_nam_example_defect_is_refused_and_rev_e_profile_is_explicit() {
    let printed_defect = b"(POACZWG/KZMP438KZMP/CZWG812-DLH499/ KSLC-KBUF-CZWGBB)";
    let corrected = b"(POACZWG/KZMP438KZMP/CZWG812-DLH499-KSLC-KBUF-CZWGBB)";
    assert!(parse(printed_defect).is_err());
    assert_eq!(parse(corrected).unwrap().header.kind, Kind::Poa);

    // Rev G's ASM spelling is not accepted in the explicitly tested Rev E profile.
    let rev_g_only = b"(ASMKZSE/CZVR021)";
    assert!(parse_with(rev_g_only, Profile::RevE).is_err());
    assert!(parse(rev_g_only).is_ok());
}
