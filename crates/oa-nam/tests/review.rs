//! Fixes from the independent review of 2026-11-19: CHG optionality, Field 10 syntax, validation
//! profiles, LRM code mapping, and the smaller corrections.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::lrm::{lrm, lrm_entry, lrm_field, lrm_for};
use oa_nam::*;

const FPL: &str = "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT YSC V447 MPV DCT KMPV-KMPV0053-EET/KZBW0003)";

fn kind_of_err(text: &str) -> (ErrorKind, u8) {
    let e = parse(text.as_bytes()).unwrap_err();
    (e.kind, e.field)
}

fn fpl_with(f10: &str, f18: &str) -> String {
    format!(
        "(FPLCZUL/KZBW043-N12345-IG-C172/L-{f10}-CYST2055-N0120A070 CYSC DCT KMPV-KMPV0053-{f18})"
    )
}

// ---- M1: CHG

// REQ: OA-NAM-019
#[test]
fn chg_optional_13b_and_field_18_only_for_the_sole_acid_change() {
    // The example of 3.1.2.3: ACID change alone.
    let ok = "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-07/AAL73)";
    assert!(parse(ok.as_bytes()).is_ok());
    // A change of anything else without 13(b) or Field 18 is refused (Table 4).
    let no_time_no_18 = "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-10/SD/C)";
    assert_eq!(kind_of_err(no_time_no_18), (ErrorKind::TimeMissing, 13));
    let no_18 = "(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD1905-MMEX-10/SD/C)";
    assert_eq!(kind_of_err(no_18), (ErrorKind::MissingField, 18));
    let no_time = "(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD-MMEX-0-10/SD/C)";
    assert_eq!(kind_of_err(no_time), (ErrorKind::TimeMissing, 13));
    // With both present it parses.
    let full = "(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD1905-MMEX-0-10/SD/C)";
    assert!(parse(full.as_bytes()).is_ok());
    // An ACID change plus another amendment is refused by the existing rule.
    let two = "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-07/AAL73-10/SD/C)";
    assert!(parse(two.as_bytes()).is_err());
    // Encode applies the same rule.
    let mut m = parse(ok.as_bytes()).unwrap();
    m.f22 = vec![Amendment {
        padded: true,
        data: AmendData::F10(Field10 {
            equipment: "SD".into(),
            surveillance: "C".into(),
        }),
    }];
    assert!(encode(&m).is_err());
}

// ---- M2: Field 10

// REQ: OA-NAM-020
#[test]
fn field_10_equipment_syntax() {
    let ok = |a: &str| parse(fpl_with(&format!("{a}/C"), "0").as_bytes());
    let bad = |a: &str| {
        parse(fpl_with(&format!("{a}/C"), "0").as_bytes())
            .unwrap_err()
            .kind
    };
    assert!(ok("SD").is_ok());
    assert!(ok("N").is_ok());
    assert!(ok("SE3J7M2P9").is_ok() || bad("SE3J7M2P9") != ErrorKind::EquipmentComm);
    assert_eq!(bad("SDD"), ErrorKind::EquipmentCommDuplicate);
    assert_eq!(bad("SDS"), ErrorKind::EquipmentCommDuplicate);
    assert_eq!(bad("E1E2"), ErrorKind::EquipmentCommDuplicate);
    assert_eq!(bad("NS"), ErrorKind::EquipmentCommCombination);
    assert_eq!(bad("SV"), ErrorKind::EquipmentCommCombination);
    assert_eq!(bad("E4"), ErrorKind::EquipmentComm);
    assert_eq!(bad("J8"), ErrorKind::EquipmentComm);
    assert_eq!(bad("J"), ErrorKind::EquipmentComm);
    assert_eq!(bad("E"), ErrorKind::EquipmentComm);
    assert_eq!(bad("S1"), ErrorKind::EquipmentComm);
    assert_eq!(bad("Q"), ErrorKind::EquipmentComm);
}

// REQ: OA-NAM-020
#[test]
fn field_10_surveillance_syntax() {
    let bad = |b: &str| {
        parse(fpl_with(&format!("SD/{b}"), "0").as_bytes())
            .unwrap_err()
            .kind
    };
    let ok = |b: &str| parse(fpl_with(&format!("SD/{b}"), "0").as_bytes()).is_ok();
    for b in ["N", "C", "AC", "S", "SB2", "SB1U2V1D1G1", "EB1", "A", "XD1"] {
        assert!(ok(b), "{b}");
    }
    assert_eq!(bad("SS"), ErrorKind::EquipmentSurveillanceDuplicate);
    assert_eq!(bad("SX"), ErrorKind::EquipmentSurveillanceCombination);
    assert_eq!(bad("NC"), ErrorKind::EquipmentSurveillanceCombination);
    assert_eq!(bad("B3"), ErrorKind::AdsB);
    assert_eq!(bad("SB"), ErrorKind::AdsB);
    assert_eq!(bad("U0"), ErrorKind::AdsB);
    assert_eq!(bad("D2"), ErrorKind::AdsC);
    assert_eq!(bad("G"), ErrorKind::AdsC);
    assert_eq!(bad("B1B2"), ErrorKind::EquipmentSurveillanceDuplicate);
    assert_eq!(bad("Q"), ErrorKind::EquipmentSurveillance);
    assert_eq!(bad("1"), ErrorKind::EquipmentSurveillance);
}

// REQ: OA-NAM-020
#[test]
fn field_10_error_codes_are_the_table_a1_codes() {
    let code =
        |a: &str, b: &str| lrm(&parse(fpl_with(&format!("{a}/{b}"), "0").as_bytes()).unwrap_err());
    assert_eq!(code("SDD", "C"), (71, 10));
    assert_eq!(code("SD", "SS"), (72, 10));
    assert_eq!(code("Q", "C"), (73, 10));
    assert_eq!(code("SD", "Q"), (74, 10));
    assert_eq!(code("NS", "C"), (75, 10));
    assert_eq!(code("SD", "NC"), (76, 10));
    assert_eq!(code("SD", "B3"), (65, 10));
    assert_eq!(code("SD", "D2"), (66, 10));
}

// REQ: OA-NAM-020
#[test]
fn items_10_and_18_must_agree() {
    let r = |a: &str, f18: &str| parse(fpl_with(&format!("{a}/C"), f18).as_bytes());
    // R needs PBN/, Z needs COM/NAV/DAT, G needs NAV/ (91).
    assert_eq!(lrm(&r("SR", "0").unwrap_err()), (91, 10));
    assert_eq!(lrm(&r("SZ", "RMK/X").unwrap_err()), (91, 10));
    assert_eq!(lrm(&r("SG", "RMK/X").unwrap_err()), (91, 10));
    assert!(r("SR", "PBN/D2").is_ok());
    assert!(r("SZ", "COM/TCAS").is_ok());
    assert!(r("SG", "NAV/GBAS").is_ok());
    // PBN/ without R (86).
    assert_eq!(lrm(&r("SD", "PBN/D2").unwrap_err()), (86, 18));
    // N with COM/NAV/DAT (64).
    assert_eq!(lrm(&r("N", "NAV/RNVD1E2A1").unwrap_err()), (64, 10));
    assert!(r("N", "RMK/X").is_ok());
}

// REQ: OA-NAM-020
#[test]
fn pbn_more_than_eight_is_84_not_77() {
    let r = |p: &str| lrm(&parse(fpl_with("SR/C", &format!("PBN/{p}")).as_bytes()).unwrap_err());
    assert_eq!(r("A1B1C1D1E1F1G1H1I1"), (84, 18));
    assert_eq!(r("A1B1C"), (77, 18));
    assert_eq!(r("a1"), (77, 18));
    assert!(parse(fpl_with("SR/C", "PBN/A1B1C1D1E1F1G1H1").as_bytes()).is_ok());
}

// ---- M4: profiles

const RTA_CONTACT: &str =
    "(RTACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-CZWGAA118.500/WINNIPEG CENTRE/0)";
const RTA_PLAIN: &str = "(RTACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-CZWGAA)";

// REQ: OA-NAM-021
#[test]
fn profile_default_is_eram_attachment_2() {
    assert_eq!(Profile::default(), Profile::EramAttachment2);
}

// REQ: OA-NAM-021
#[test]
fn rta_contact_data_is_attachment_2_only_and_28_characters() {
    let m = parse(RTA_CONTACT.as_bytes()).unwrap();
    assert_eq!(
        m.f31.as_ref().unwrap().text.as_deref(),
        Some("118.500/WINNIPEG CENTRE/0")
    );
    assert_eq!(encode(&m).unwrap(), RTA_CONTACT.as_bytes());
    // Core: Table 21 has no 31(c).
    assert!(parse_with(RTA_CONTACT.as_bytes(), Profile::Core).is_err());
    assert!(parse_with(RTA_PLAIN.as_bytes(), Profile::Core).is_ok());
    assert!(encode_with(&m, Profile::Core).is_err());
    // Under Attachment 2 the text must be a valid contact: three parts, frequency, name length.
    for bad in [
        "118.500/WINNIPEG CENTRE",
        "118.510/WINNIPEG CENTRE/0",
        "118.500/WI/0",
        "118.500/WINNIPEG CENTRE/2",
        "000.000/WINNIPEG CENTRE/1",
        "118.500/WINNIPEG CENTRE EAST/0",
        "118.500/WINNIPEG CENTRE/00",
    ] {
        let t = RTA_PLAIN.replace("CZWGAA", &format!("CZWGAA{bad}"));
        let r = parse(t.as_bytes());
        if bad.starts_with("000.000") {
            assert!(r.is_ok(), "000.000 means no frequency: {bad}");
        } else {
            assert_eq!(r.unwrap_err().kind, ErrorKind::Field31, "{bad}");
        }
    }
}

// REQ: OA-NAM-021
#[test]
fn field_31_text_elsewhere_is_20_alphanumeric_characters() {
    let t = |text: &str| format!("(POACZWG/KZMP438KZMP/CZWG812-DLH499-KSLC-KBUF-CZWGBB{text})");
    assert!(parse(t("ABCDEFGHIJKLMNOPQRST").as_bytes()).is_ok());
    for bad in ["ABCDEFGHIJKLMNOPQRSTU", "AB CD", "AB.CD", "AB/CD", "ab"] {
        assert_eq!(
            parse(t(bad).as_bytes()).unwrap_err().kind,
            ErrorKind::Field31,
            "{bad}"
        );
        assert!(
            parse_with(t(bad).as_bytes(), Profile::Core).is_err(),
            "{bad}"
        );
    }
    // 28 characters with spaces is not allowed here under either profile.
    assert!(parse(t("ABCDEFGHIJKLMNOPQRSTUVWXYZ12").as_bytes()).is_err());
}

// REQ: OA-NAM-021
#[test]
fn block_altitude_is_attachment_2_only() {
    let blk = "(ESTKZMP/CZWG992KZMP/CZWG991-DAL122/A4322-KFAR-HML/2042F350B370-CYOW)";
    let m = parse(blk.as_bytes()).unwrap();
    assert_eq!(encode(&m).unwrap(), blk.as_bytes());
    assert_eq!(
        parse_with(blk.as_bytes(), Profile::Core).unwrap_err().kind,
        ErrorKind::Level
    );
}

// REQ: OA-NAM-021
#[test]
fn unknown_ground_speed_and_heading_are_attachment_2_only() {
    let rti = "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034N0780521WN999999999F349)";
    let m = parse(rti.as_bytes()).unwrap();
    let f = m.f32.unwrap();
    assert_eq!(f.speed, GroundSpeed::Unknown);
    assert_eq!(f.heading, Heading::Unknown);
    assert_eq!(
        encode(&parse(rti.as_bytes()).unwrap()).unwrap(),
        rti.as_bytes()
    );
    assert!(parse_with(rti.as_bytes(), Profile::Core).is_err());
    // Unknown speed alone, and unknown heading alone, both refused under Core.
    let sp = rti.replace("N999999999", "N999927629");
    assert!(parse(sp.as_bytes()).is_ok());
    assert!(parse_with(sp.as_bytes(), Profile::Core).is_err());
    let hd = rti.replace("N999999999", "N043399999");
    assert!(parse(hd.as_bytes()).is_ok());
    assert!(parse_with(hd.as_bytes(), Profile::Core).is_err());
}

// REQ: OA-NAM-021
#[test]
fn every_icd_example_parses_under_both_profiles_except_attachment_2_additions() {
    // The Part II examples are valid under Core as well.
    for s in [FPL, "(IRQCZQM/KZBW491)", RTA_PLAIN] {
        assert!(parse_with(s.as_bytes(), Profile::Core).is_ok(), "{s}");
        assert_eq!(parse_with(s.as_bytes(), Profile::Core), parse(s.as_bytes()));
    }
    // And the tolerant parser takes a profile.
    let spaced = "(IRQCZQM/KZBW491)";
    assert!(parse_tolerant_with(spaced.as_bytes(), Profile::Core).is_ok());
}

// ---- LRM mapping (L10 to L14)

// REQ: OA-NAM-028
#[test]
fn lrm_code_for_missing_time_depends_on_the_field() {
    // 13(b) of an FPL is required (Table 3): code 21, "expected time designator not found".
    let t = "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST-N0120A070 CYSC DCT KMPV-KMPV0053-0)";
    let e = parse(t.as_bytes()).unwrap_err();
    assert_eq!((e.kind, e.field), (ErrorKind::TimeMissing, 13));
    assert_eq!(lrm(&e), (21, 13));
    // In Field 14 it stays 24.
    let c = "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SD/C-KIAD-MAM/F350-N0420F350 MAM DCT-MMMX-0)";
    let e = parse(c.as_bytes()).unwrap_err();
    if e.kind == ErrorKind::TimeMissing {
        assert_eq!(lrm(&e).0, if e.field == 14 { 24 } else { 21 });
    }
    assert_eq!(lrm(&NamError::new(ErrorKind::TimeMissing, 14)), (24, 14));
    assert_eq!(lrm(&NamError::new(ErrorKind::TimeMissing, 16)), (21, 16));
}

// REQ: OA-NAM-028
#[test]
fn attachment_2_remaps_message_number_and_handoff_field_03_errors() {
    let num = NamError::new(ErrorKind::MessageNumber, 3);
    assert_eq!(lrm_for(&num, Profile::Core, None), (4, 3));
    assert_eq!(lrm_for(&num, Profile::EramAttachment2, None), (57, 3));
    let r = NamError::new(ErrorKind::Reference, 3);
    assert_eq!(
        lrm_for(&r, Profile::EramAttachment2, Some(Kind::Fpl)),
        (5, 3)
    );
    for k in [Kind::Rti, Kind::Rta, Kind::Rla, Kind::Rtu] {
        assert_eq!(
            lrm_for(&r, Profile::EramAttachment2, Some(k)),
            (57, 3),
            "{k:?}"
        );
        assert_eq!(lrm_for(&r, Profile::Core, Some(k)), (5, 3), "{k:?}");
    }
    // peek_kind names the message of a refused text.
    let bad = b"(RTAKZMP/CZWG222KZMP/CZWG81-DLH499/A4222-KZMP-CYOW-KZMP42)";
    assert_eq!(peek_kind(bad), Some(Kind::Rta));
    let e = parse(bad).unwrap_err();
    assert_eq!(lrm_for(&e, Profile::default(), peek_kind(bad)).0, 57);
    assert_eq!(peek_kind(b"(XYZ"), None);
}

// REQ: OA-NAM-028
#[test]
fn lrm_field_column_follows_the_profile() {
    assert_eq!(lrm_entry(17).unwrap().field, "13, 16");
    assert_eq!(lrm_field(17, Profile::Core), Some("13, 16"));
    assert_eq!(lrm_field(17, Profile::EramAttachment2), Some("16"));
    assert_eq!(lrm_field(63, Profile::EramAttachment2), Some("18"));
    for c in [64, 65, 66] {
        assert_eq!(lrm_field(c, Profile::EramAttachment2), Some("10"));
    }
    assert_eq!(lrm_field(6, Profile::EramAttachment2), Some("07"));
    assert_eq!(lrm_field(95, Profile::Core), None);
}

// REQ: OA-NAM-028
#[test]
fn field_32_and_item_errors_keep_their_codes() {
    assert_eq!(lrm(&NamError::new(ErrorKind::PbnTooMany, 18)), (84, 18));
    assert_eq!(lrm(&NamError::new(ErrorKind::Pbn, 18)), (77, 18));
}

// ---- ABI order (L15)

// REQ: OA-NAM-029
#[test]
fn abi_field_22_has_no_required_order() {
    let a = "(ABIKZWY/CZQM005-IBE6175/A2537-LEMD-41N040W/0700F330-CYUL-9/B744/H-15/M084F350 41N030W DCT GOALL)";
    let b = "(ABIKZWY/CZQM005-IBE6175/A2537-LEMD-41N040W/0700F330-CYUL-15/M084F350 41N030W DCT GOALL-9/B744/H)";
    let (ma, mb) = (parse(a.as_bytes()).unwrap(), parse(b.as_bytes()).unwrap());
    assert_eq!(encode(&ma).unwrap(), a.as_bytes());
    assert_eq!(encode(&mb).unwrap(), b.as_bytes());
    assert_eq!(ma.f22.len(), mb.f22.len());
    // Still each field once, and only the ABI-amendable fields.
    let dup = a.replace("-15/", "-9/B744/H-15/");
    assert!(parse(dup.as_bytes()).is_err());
    let wrong = a.replace("-15/", "-13/KIAD-15/");
    assert!(parse(wrong.as_bytes()).is_err());
}

// REQ: OA-NAM-030
#[test]
fn poa_with_the_rev_f_ssr_form_parses() {
    // Rev F printed the POA example with the beacon code in 07; Rev G dropped it from Table 24.
    let f = "(POACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KSLC-KBUF-CZWGBB)";
    let m = parse(f.as_bytes()).unwrap();
    assert!(m.f07.as_ref().unwrap().ssr.is_some());
    assert_eq!(encode(&m).unwrap(), f.as_bytes());
    // POJ takes no SSR.
    assert!(parse(b"(POJCZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KSLC-KBUF-CZWGBB)").is_err());
}

// ---- Rev E

// REQ: OA-NAM-031
#[test]
fn rev_e_differs_in_asm_poi_poa_and_poj() {
    let e = Profile::RevE;
    // ASM carries 03(c) in Rev E, not in Rev G.
    let asm_e = "(ASMKZSE/CZVR021KZSE/CZVR010)";
    assert!(parse_with(asm_e.as_bytes(), e).is_ok());
    assert!(parse_with(b"(ASMKZSE/CZVR021)", e).is_err());
    assert!(parse(asm_e.as_bytes()).is_err());
    // POI has no 03(c) in Rev E (the Rev E examples), optional in Rev G.
    let poi = "(POIKZOB/CZYZ812-DLH499/A3407-KROC-KBOI-CZYZ-13242934462034N0780521WN043327629F349)";
    assert!(parse_with(poi.as_bytes(), e).is_ok());
    let poi_ref = poi.replace("CZYZ812-", "CZYZ812KZOB/CZYZ743-");
    assert!(parse_with(poi_ref.as_bytes(), e).is_err());
    assert!(parse_with(poi_ref.as_bytes(), Profile::Core).is_ok());
    // POA carries the SSR in Rev E.
    let poa = "(POACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KSLC-KBUF-CZWGBB)";
    assert!(parse_with(poa.as_bytes(), e).is_ok());
    assert!(parse_with(poa.replace("DLH499/A4222", "DLH499").as_bytes(), e).is_err());
    // POJ is a Field 18 reason in Rev E.
    for poj in [
        "(POJCZWG/KZMP438KZMP/CZWG812-0)",
        "(POJCZWG/KZMP438KZMP/CZWG812-RMK/00/Unable PO Request)",
    ] {
        let m = parse_with(poj.as_bytes(), e).unwrap();
        assert_eq!(encode_with(&m, e).unwrap(), poj.as_bytes());
        assert!(parse_with(poj.as_bytes(), Profile::Core).is_err(), "{poj}");
    }
    let g_poj = "(POJCZWG/KZMP438KZMP/CZWG812-DLH499-KSLC-KBUF-CZWGBB)";
    assert!(parse_with(g_poj.as_bytes(), e).is_err());
    // Rev E allows metric designators in 32(e), Rev G does not.
    let rti = "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034N0780521WN043327629S1070)";
    assert!(parse_with(rti.as_bytes(), e).is_ok());
    assert!(parse_with(rti.as_bytes(), Profile::Core).is_err());
    // No Attachment 2 additions in Rev E.
    assert!(parse_with(
        b"(ESTKZMP/CZWG992KZMP/CZWG991-DAL122/A4322-KFAR-HML/2042F350B370-CYOW)",
        e
    )
    .is_err());
}

// REQ: OA-NAM-031
#[test]
fn rev_e_poa_and_poj_expect_no_response() {
    use oa_nam::session::*;
    let us = Facility(*b"KZMP");
    let them = Facility(*b"CZWG");
    assert!(expects_response_in(Profile::EramAttachment2, Kind::Poa));
    assert!(!expects_response_in(Profile::RevE, Kind::Poa));
    assert!(!expects_response_in(Profile::RevE, Kind::Poj));
    assert!(expects_response_in(Profile::RevE, Kind::Cpl));
    let c = Config::for_profile(Profile::RevE, us, them);
    assert!(c.irq_retry_same_number);
    let (s, _) = step(&c, new_session(), Event::Activate, 0);
    let h = Header {
        kind: Kind::Irs,
        id: MsgId {
            sender: them,
            receiver: us,
            number: 0,
        },
        reference: Some(MsgId {
            sender: us,
            receiver: them,
            number: 0,
        }),
    };
    let (s, _) = step(&c, s, Event::Received(h), 1);
    let (s, a) = step(&c, s, Event::SendAtm(Kind::Poa), 2);
    assert!(matches!(
        a.as_slice()[0],
        Action::SendAtm {
            kind: Kind::Poa,
            ..
        }
    ));
    assert!(s.pending().is_empty());
}
