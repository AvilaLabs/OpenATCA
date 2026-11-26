//! Version-pinned NAM examples and profile boundaries from the public ICD inventory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_nam::lrm::lrm;
use oa_nam::message::parse_header;
use oa_nam::{parse, parse_tolerant, parse_with, ErrorKind, Kind, Profile};

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
        "Part II 2.2.6 candidate future message mnemonics except DLA are explicit refusal vectors",
        "interface_conformance::public_future_message_candidates_except_dla_are_refused_as_undefined_core_mnemonics",
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

// REQ: OA-NAM-032
#[test]
fn public_future_message_candidates_except_dla_are_refused_as_undefined_core_mnemonics() {
    // NAS-IC-21009205 Rev G Part II 2.2.6 pp. 14–15 lists these as candidate messages for future
    // implementation, not as members of the 25-message core set in Table 2. DLA is separately
    // supported by the ICAO Appendix 3 field codec. Table A-1 code 60 identifies an
    // ineligible/unknown message mnemonic. MIS appears in both the future list and the core set,
    // so it is intentionally not included here.
    for mnemonic in [
        "CDN", "ACP", "REJ", "ALR", "RCF", "ARR", "DEP", "RQP", "CRQ", "CAK", "CLR", "SUA", "SPL",
        "RQS",
    ] {
        let wire = format!("({mnemonic}KZBW/CZWG001)");
        let bytes = wire.as_bytes();
        let err = parse(bytes).expect_err(mnemonic);
        assert_eq!(
            (err.kind, err.field),
            (ErrorKind::Mnemonic, 3),
            "{mnemonic}"
        );
        assert_eq!(lrm(&err), (60, 3), "{mnemonic}");
        let bare = bytes
            .strip_prefix(b"(")
            .unwrap()
            .strip_suffix(b")")
            .unwrap();
        let header_err = parse_header(bare).unwrap_err();
        assert_eq!(
            (header_err.kind, header_err.field),
            (ErrorKind::Mnemonic, 3),
            "{mnemonic}"
        );
        assert_eq!(parse_tolerant(bytes), Err(err), "{mnemonic}");
    }
    // MIS is explicitly in Table 2's core set and remains accepted.
    assert_eq!(parse_header(b"MISKZLC/CZWG876").unwrap().kind, Kind::Mis);
}
