//! Malformed variants: for every field of the core set, an input that breaks that field's rule
//! and must be refused with the right Table A-1 reason and field (RULESET E5). Each case is a
//! valid message with one defect.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::frame::{frame, read_header, unframe};
use oa_nam::lrm::lrm;
use oa_nam::*;

const CASES: &[(&str, &str, ErrorKind, u8)] = &[
    ("hdr_sender_short", "(FPLCZU/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT-KMPV0053-EET/KZBW0003)", ErrorKind::Field03Syntax, 3),
    ("hdr_number_short", "(CNLKZHU/MMZT77-UAL1021-KIAD0818-MMEX-0)", ErrorKind::Field03Syntax, 3),
    ("hdr_number_alpha", "(CNLKZHU/MMZT77A-UAL1021-KIAD0818-MMEX-0)", ErrorKind::MessageNumber, 3),
    ("hdr_lower", "(cnlKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)", ErrorKind::Mnemonic, 3),
    ("no_open", "CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)", ErrorKind::Parens, 0),
    ("no_close", "(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0", ErrorKind::Parens, 0),
    ("paren_inside", "(CNLKZHU/MMZT776KZHU/MMZT603-UAL(1021-KIAD0818-MMEX-0)", ErrorKind::ParensInside, 0),
    ("mnemonic", "(XYZKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)", ErrorKind::Mnemonic, 3),
    ("ref_missing", "(CNLKZHU/MMZT776-UAL1021-KIAD0818-MMEX-0)", ErrorKind::ReferencePresence, 3),
    ("ref_on_fpl", "(FPLCZUL/KZBW043KZBW/CZUL001-N12345-IG-C172/L-SD/C-CYST2055-N0120A070 CYSC DCT-KMPV0053)", ErrorKind::ReferencePresence, 3),
    ("irq_extra", "(IRQCZQM/KZBW491-0)", ErrorKind::TooManyFields, 0),
    ("acid_long", "(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021ABCDEF-KIAD0818-MMEX-0)", ErrorKind::Acid, 7),
    ("acid_lower", "(CNLKZHU/MMZT776KZHU/MMZT603-ual1021-KIAD0818-MMEX-0)", ErrorKind::Acid, 7),
    ("acid_empty", "(CNLKZHU/MMZT776KZHU/MMZT603--KIAD0818-MMEX-0)", ErrorKind::Acid, 7),
    ("ssr_bad_mode", "(CPLKZHU/MMTY005-UAL1021/Z2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::SsrMode, 7),
    ("ssr_bad_code", "(CPLKZHU/MMTY005-UAL1021/A2189-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::SsrCode, 7),
    ("rules_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-QX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::FlightRules, 8),
    ("type_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IQ-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::FlightType, 8),
    ("wake_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/Q-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::WakeCategory, 9),
    ("dep_short", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIA-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::TimeInvalid, 13),
    ("bdy_time_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2062F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::TimeInvalid, 14),
    ("bdy_level_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F35-N0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::Level, 14),
    ("speed_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-X0420F350 MAM UJ35 AVSAR DCT-MMMX)", ErrorKind::Speed, 15),
    ("route_bad_token", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM U-J35 AVSAR DCT-MMMX)", ErrorKind::RouteElement, 15),
    ("dest_short", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMM)", ErrorKind::DestinationAerodrome, 16),
    ("dof_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX-DOF/121330)", ErrorKind::DateOfFlight, 18),
    ("eet_bad", "(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX-EET/KZBW9999)", ErrorKind::Eet, 18),
    ("f22_unknown", "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-99/AAL73)", ErrorKind::AmendmentField, 22),
    ("f22_missing", "(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW)", ErrorKind::AmendmentMissing, 22),
    ("too_many", "(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0-0)", ErrorKind::TooManyFields, 0),
    ("rti_f32_short", "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-1324293446203)", ErrorKind::Field32, 32),
    ("rti_f32_lat", "(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP-CYOW-CZWG-13242934462034X0780521WN043327629F349)", ErrorKind::Field32, 32),
    ("rla_sector", "(RLAKZBW/CZUL202CZUL/KZBW445-KZBW5)", ErrorKind::Field31, 31),
    ("lrm_text", "(LRMKZLC/CZWG035CZWG/KZLC021-RMK/xx/07/AAL)", ErrorKind::LrmText, 18),
    ("lrm_code", "(LRMKZLC/CZWG035CZWG/KZLC021-RMK/6/07/AAL)", ErrorKind::LrmText, 18),
];

// REQ: OA-NAM-005
#[test]
fn every_malformed_variant_is_refused_with_its_reason_and_field() {
    for (name, text, kind, field) in CASES {
        let e = parse(text.as_bytes()).expect_err(name);
        assert_eq!((e.kind, e.field), (*kind, *field), "{name}");
        // The tolerant parser refuses them too: it only forgives spacing.
        assert!(parse_tolerant(text.as_bytes()).is_err(), "{name}");
    }
}

// REQ: OA-NAM-005
#[test]
fn a_refusal_names_the_lrm_code_a_receiver_would_answer_with() {
    let (_, text, _, _) = CASES.iter().find(|c| c.0 == "acid_long").unwrap();
    let e = parse(text.as_bytes()).unwrap_err();
    assert_eq!(lrm(&e), (6, 7));
    let e = parse(b"(XYZKZHU/MMZT776)").unwrap_err();
    assert_eq!(lrm(&e).0, 60);
}

// REQ: OA-NAM-005
#[test]
fn non_ascii_and_control_bytes_are_refused() {
    assert_eq!(
        parse("(IRQCZQM/KZB\u{e9}491)".as_bytes()).unwrap_err().kind,
        ErrorKind::Charset
    );
    assert_eq!(
        parse(b"(IRQCZQM/KZBW\n491)").unwrap_err().kind,
        ErrorKind::Charset
    );
    assert_eq!(
        parse(b"(IRQCZQM/KZBW\x00491)").unwrap_err().kind,
        ErrorKind::Charset
    );
}

// REQ: OA-NAM-005, OA-NAM-006
#[test]
fn length_bounds_are_enforced() {
    assert_eq!(parse(b"(IRQ)").unwrap_err().kind, ErrorKind::Length);
    assert!(parse(b"").is_err());
    let mut big = b"(MISCZQM/KZBW999-/S1-RMK/".to_vec();
    big.extend(std::iter::repeat_n(b'A', MAX_MESSAGE_LEN));
    big.push(b')');
    assert_eq!(parse(&big).unwrap_err().kind, ErrorKind::Length);
    assert_eq!(MIN_MESSAGE_LEN, 17);
    assert_eq!(MAX_MESSAGE_LEN, 1800);
    assert!(parse(b"(IRQCZQM/KZBW491)").is_ok());
    assert_eq!(b"(IRQCZQM/KZBW491)".len(), MIN_MESSAGE_LEN);
}

// REQ: OA-NAM-005
#[test]
fn encode_refuses_an_invalid_value() {
    let mut m = parse(b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)").unwrap();
    m.header.id.number = 1000;
    assert!(encode(&m).is_err());
    let mut m = parse(b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)").unwrap();
    m.f07 = None;
    assert!(encode(&m).is_err());
    let mut m = parse(b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)").unwrap();
    m.f08 = m.f08.or(Some(Field08 {
        rules: b'I',
        flight_type: None,
    }));
    assert!(encode(&m).is_err());
}

// REQ: OA-NAM-007
#[test]
fn frame_roundtrip_and_refusals() {
    let msg = b"(IRQCZQM/KZBW491)";
    let f = frame(msg).unwrap();
    assert_eq!(&f[..4], b"0011");
    assert_eq!(read_header(&f).unwrap(), Some(17));
    let (m, used) = unframe(&f).unwrap().unwrap();
    assert_eq!((m, used), (&msg[..], 21));
    // Partial input waits.
    assert_eq!(unframe(&f[..10]).unwrap(), None);
    assert_eq!(read_header(&f[..3]).unwrap(), None);
    // Lowercase hex, non-hex, and out-of-range lengths are refused.
    assert!(read_header(b"001a").is_err());
    assert!(read_header(b"00G1").is_err());
    assert!(read_header(b"0010").is_err());
    assert!(read_header(b"0709").is_err());
    assert!(frame(b"(IRQ)").is_err());
    // Two frames back to back: the first is returned with its length.
    let mut two = f.clone();
    two.extend_from_slice(&f);
    assert_eq!(unframe(&two).unwrap().unwrap().1, 21);
}
