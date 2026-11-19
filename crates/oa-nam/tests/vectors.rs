//! Every example message printed in the ICDs, as a test vector. The text is transcribed with the
//! PDF's typography: an en dash for a hyphen in places, a space beside many hyphens, and a line
//! wrap joined with one space, except where the line ends in a hyphen (CHG 1 and 2, CPL), which
//! is joined with none: the break is a line break, not a space. `ascii` turns the en dashes into hyphens (the wire is ASCII, Part
//! III 2.4); [`oa_nam::parse_tolerant`] drops the spaces beside hyphens and says how many; the
//! canonical form is the text without them. NAS-IC-82422100 and NAS-IR-82422100 print no NAM
//! messages (their only strings are the load-balancer health-monitoring words, which are not
//! NAM messages), so every vector is from NAS-IC-21009205 Rev G.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use oa_nam::frame::{frame, unframe};
use oa_nam::*;

fn ascii(printed: &str) -> String {
    printed.replace('\u{2013}', "-")
}

/// The canonical text: no space beside a hyphen.
fn canonical(printed: &str) -> String {
    let a = ascii(printed);
    let mut out = String::new();
    let b: Vec<char> = a.chars().collect();
    for (i, &c) in b.iter().enumerate() {
        let before_hyphen = c == ' ' && b.get(i + 1) == Some(&'-');
        let after_hyphen = c == ' ' && i > 0 && b[i - 1] == '-';
        if !(before_hyphen || after_hyphen) {
            out.push(c);
        }
    }
    out
}

fn deviations(printed: &str) -> usize {
    ascii(printed).len() - canonical(printed).len()
}

/// The checks every printed example passes.
fn check(printed: &str) -> Message {
    let a = ascii(printed);
    let canon = canonical(printed);
    // Strict parse takes the canonical text and refuses the printed text if it carries spaces.
    let strict = parse(a.as_bytes());
    if deviations(printed) == 0 {
        assert!(strict.is_ok(), "{printed}: {strict:?}");
    } else {
        assert!(
            strict.is_err(),
            "strict parse must refuse spacing: {printed}"
        );
    }
    let (m, dev) = parse_tolerant(a.as_bytes()).unwrap_or_else(|e| panic!("{printed}: {e}"));
    assert_eq!(
        usize::from(dev.space_before_hyphen + dev.space_after_hyphen),
        deviations(printed),
        "{printed}"
    );
    // The canonical text parses strictly to the same message and encodes back to itself.
    assert_eq!(parse(canon.as_bytes()).unwrap(), m, "{printed}");
    let enc = encode(&m).unwrap_or_else(|e| panic!("{printed}: {e}"));
    assert_eq!(String::from_utf8(enc.clone()).unwrap(), canon, "{printed}");
    // And frames and unframes.
    let framed = frame(&enc).unwrap();
    let (back, used) = unframe(&framed).unwrap().unwrap();
    assert_eq!(used, framed.len());
    assert_eq!(back, enc.as_slice());
    m
}

// REQ: OA-NAM-003
#[test]
fn fpl_1_sherbrooke_to_montpelier() {
    // Rev G Part II 3.1.1.3 p.23
    let m = check("(FPLCZUL/KZBW043 -N12345 -IG -C172/L -SD/C -CYST2055 -N0120A070 CYSC DCT YSC V447 MPV DCT KMPV –KMPV0053-EET/KZBW0003)");
    assert_eq!(m.header.kind, Kind::Fpl);
}

// REQ: OA-NAM-003
#[test]
fn fpl_2_duluth_to_amsterdam() {
    // Rev G Part II 3.1.1.3 p.23
    let m = check("(FPLKZMP/CZWG223 –DAL56-IS -B742/H-SXWDHGRZJ3M1/SB2 -KDLH0255 -N0492F330 DCT DLH J533 YQT DCT YDP/M084F330 DCT PRAWN/M084F370 DCT 59N050W 58N040W 57N030W 56N020W 55N010W UN551 TADEX/N0485F370 UN551 BEL UB3 IOM UL603 BLUFA4 -EHAM0721 EBBR-PBN/D2 NAV/RNVD1E2A1 REG/N642NW EET/CZWG0032 CZYZ0113 CZUL0126 CZQX0226 59N050W0328 58N040W0404 EGGX0441 56N020W0521 EGPX0603 EGTT0623 SEL/CMAD TALT/KMSP)");
    assert_eq!(m.header.kind, Kind::Fpl);
}

// REQ: OA-NAM-003
#[test]
fn chg_1_with_dof() {
    // Rev G Part II 3.1.2.3 p.24
    let m = check("(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD1905-MMEX-DOF/121115-10/SFGHRWXZ/SB2-18/PBN/D2 NAV/RNVD1E2A1 EET/MMZT0023 MMEX0057 RMK/TCAS EQUIPPED)");
    assert_eq!(m.header.kind, Kind::Chg);
}

// REQ: OA-NAM-003
#[test]
fn chg_2_with_zero_field_18() {
    // Rev G Part II 3.1.2.3 p.24
    let m = check("(CHGKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD1905-MMEX-0-10/SFGHRWXZ/SB2-18/PBN/D2 NAV/RNVD1E2A1 EET/MMZT0023 MMEX0057 RMK/TCAS EQUIPPED)");
    assert_eq!(m.header.kind, Kind::Chg);
}

// REQ: OA-NAM-003, OA-NAM-008
#[test]
fn chg_3_acid_change() {
    // Rev G Part II 3.1.2.3 p.24
    let m = check("(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA-CYOW-07/AAL73)");
    assert_eq!(m.header.kind, Kind::Chg);
}

// REQ: OA-NAM-003
#[test]
fn est_1_humboldt() {
    // Rev G Part II 3.1.3.3 p.25
    let m = check("(ESTKZMP/CZWG992KZMP/CZWG991 -DAL122/A4322 –KFAR-HML/2042F350 -CYOW)");
    assert_eq!(m.header.kind, Kind::Est);
}

// REQ: OA-NAM-003
#[test]
fn cnl_1_houston_to_mazatlan() {
    // Rev G Part II 3.1.4.3 p.25
    let m = check("(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)");
    assert_eq!(m.header.kind, Kind::Cnl);
}

// REQ: OA-NAM-003
#[test]
fn cpl_1_houston_to_mexico_city() {
    // Rev G Part II 3.2.1.3 p.26
    let m = check("(CPLKZHU/MMTY005-UAL1021/A2173-IX-A320/M-SE3HIRWXZ/SB2-KIAD-MAM/2042F350-N0420F350 MAM UJ35 AVSAR DCT-MMMX-PBN/D2 NAV/RNVD1E2A1 DOF/121130)");
    assert_eq!(m.header.kind, Kind::Cpl);
}

// REQ: OA-NAM-003
#[test]
fn mod_1_fields_10_and_18() {
    // Rev G Part II 3.2.2.3 p.27
    let m = check("(MODKZHU/MMTY776KZHU/MMTY720-UAL1021–KIAD –MMEX-10/SE3HIRWX/S-18/PBN/D2 NAV/RNVD1E2A1 REG/N431UP EET/MMTY0312 MMEX0338 SEL/EFPQ)");
    assert_eq!(m.header.kind, Kind::Mod);
}

// REQ: OA-NAM-003, OA-NAM-008
#[test]
fn abi_1_minimum_field_22() {
    // Rev G Part II 3.2.3.3 p.28
    let m = check("(ABIKZWY/CZQM005-IBE6175/A2537-LEMD-41N040W/0700F330-CYUL-9/B744/H -15/M084F350 41N030W 41N040W 41N050W 40N060W 38N065W DANER A699 MAPLE DCT GOALL)");
    assert_eq!(m.header.kind, Kind::Abi);
}

// REQ: OA-NAM-003
#[test]
fn mis_1_flight_id() {
    // Rev G Part II 3.3.1.3 p.29
    let m = check("(MISKZLC/CZWG876 –DAL1311 -RMK/DAL1311 ABLE 350 AT 1322Z)");
    assert_eq!(m.header.kind, Kind::Mis);
}

// REQ: OA-NAM-003
#[test]
fn mis_2_functional_address() {
    // Rev G Part II 3.3.1.3 p.29
    let m = check("(MISCZQM/KZBW999 -/S1 -RMK/COMBINING SECTOR 21 INTO 22 AT 1415Z)");
    assert_eq!(m.header.kind, Kind::Mis);
}

// REQ: OA-NAM-003
#[test]
fn irq_1_moncton_to_boston() {
    // Rev G Part II 3.4.1.3 p.29
    let m = check("(IRQCZQM/KZBW491)");
    assert_eq!(m.header.kind, Kind::Irq);
}

// REQ: OA-NAM-003
#[test]
fn irs_1_boston_to_moncton() {
    // Rev G Part II 3.4.2.3 p.30
    let m = check("(IRSKZBW/CZQM232CZQM/KZBW491)");
    assert_eq!(m.header.kind, Kind::Irs);
}

// REQ: OA-NAM-003
#[test]
fn trq_1_vancouver_to_seattle() {
    // Rev G Part II 3.4.3.3 p.30
    let m = check("(TRQCZVR/KZSE491-RMK/SHUTDOWN FOR SOFTWARE CHANGE)");
    assert_eq!(m.header.kind, Kind::Trq);
}

// REQ: OA-NAM-003
#[test]
fn trs_1_seattle_to_vancouver() {
    // Rev G Part II 3.4.4.3 p.31
    let m = check("(TRSKZSE/CZVR232CZVR/KZSE491-0)");
    assert_eq!(m.header.kind, Kind::Trs);
}

// REQ: OA-NAM-003
#[test]
fn asm_1_seattle_to_vancouver() {
    // Rev G Part II 3.4.5.4 p.31
    let m = check("(ASMKZSE/CZVR021)");
    assert_eq!(m.header.kind, Kind::Asm);
}

// REQ: OA-NAM-003
#[test]
fn lam_1_houston_to_monterrey() {
    // Rev G Part II 3.5.1.3 p.32
    let m = check("(LAMKZHU/MMTY035MMTY/KZHU021)");
    assert_eq!(m.header.kind, Kind::Lam);
}

// REQ: OA-NAM-003, OA-NAM-009
#[test]
fn lrm_1_illegal_acid() {
    // Rev G Part II 3.5.2.3 p.33
    let m = check("(LRMKZLC/CZWG035CZWG/KZLC021-RMK/06/07/AAL98295)");
    assert_eq!(m.header.kind, Kind::Lrm);
}

// REQ: OA-NAM-003, OA-NAM-009
#[test]
fn lrm_2_non_field_specific() {
    // Rev G Part II 3.5.2.3 p.33
    let m = check("(LRMCZYZ/KZOB001KZOB/CZYZ210-RMK/53/00/MESSAGE LOGICALLY TOO LONG)");
    assert_eq!(m.header.kind, Kind::Lrm);
}

// REQ: OA-NAM-003
#[test]
fn rti_1_undirected() {
    // Rev G Part II 3.6.1.3 p.33
    let m = check("(RTIKZMP/CZWG812KZMP/CZWG801-DLH499/A3407-KMSP–CYOW–CZWG -13242934462034N0780521WN043327629F349)");
    assert_eq!(m.header.kind, Kind::Rti);
}

// REQ: OA-NAM-003
#[test]
fn rti_2_sector_08() {
    // Rev G Part II 3.6.1.3 p.33
    let m = check("(RTICZYZ/KZBW123CZYZ/KZBW102-ACA202/A2201-CYYZ–KIAD–KZBW08 -13242934444055N0752756WN043327629F350)");
    assert_eq!(m.header.kind, Kind::Rti);
}

// REQ: OA-NAM-003
#[test]
fn rla_1_sector_53() {
    // Rev G Part II 3.6.2.3 p.34
    let m = check("(RLAKZBW/CZUL202CZUL/KZBW445-KZBW53)");
    assert_eq!(m.header.kind, Kind::Rla);
}

// REQ: OA-NAM-003
#[test]
fn rla_2_tracon_sector_1a() {
    // Rev G Part II 3.6.2.3 p.34
    let m = check("(RLAKZBW/CZUL202CZUL/KZBW445-KBTV1A)");
    assert_eq!(m.header.kind, Kind::Rla);
}

// REQ: OA-NAM-003
#[test]
fn rtu_1_update() {
    // Rev G Part II 3.6.3.3 p.34
    let m = check("(RTUKZMP/CZWG000KZMP/CZWG801-DLH499/A3407-KMSP-CYOW -13242934462034N0720521WN043327629F349)");
    assert_eq!(m.header.kind, Kind::Rtu);
}

// REQ: OA-NAM-003
#[test]
fn rta_1_accept() {
    // Rev G Part II 3.6.4.3 p.35
    let m = check("(RTACZWG/KZMP438KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-CZWGAA)");
    assert_eq!(m.header.kind, Kind::Rta);
}

// REQ: OA-NAM-003
#[test]
fn rta_2_retraction() {
    // Rev G Part II 3.6.4.3 p.35
    let m = check("(RTAKZMP/CZWG222KZMP/CZWG812-DLH499/A4222-KZMP-CYOW-KZMP42)");
    assert_eq!(m.header.kind, Kind::Rta);
}

// REQ: OA-NAM-003
#[test]
fn poi_1_no_sector_cpl_sent() {
    // Rev G Part II 3.7.1.3 p.36
    let m = check("(POIKZOB/CZYZ812KZOB/CZYZ743-DLH499/A3407-KROC–KBOI–CZYZ -13242934462034N0780521WN043327629F349)");
    assert_eq!(m.header.kind, Kind::Poi);
}

// REQ: OA-NAM-003
#[test]
fn poi_2_sector_aa() {
    // Rev G Part II 3.7.1.3 p.36
    let m = check("(POIKZOB/CZYZ812KZOB/CZYZ743-DLH499/A3407-KROC–KBOI–CZYZAA -13242934462034N0780521WN043327629F349)");
    assert_eq!(m.header.kind, Kind::Poi);
}

// REQ: OA-NAM-003
#[test]
fn poi_3_sector_08_no_reference() {
    // Rev G Part II 3.7.1.3 p.36
    let m = check(
        "(POICZYZ/KZBW123-ACA202/A2201-CYYZ–CYQB–KZBW08 -13242934444055N0752756WN043327629F350)",
    );
    assert_eq!(m.header.kind, Kind::Poi);
}

// REQ: OA-NAM-003
#[test]
fn pla_1_sector_53() {
    // Rev G Part II 3.7.2.3 p.37
    let m = check("(PLAKZBW/CZUL202CZUL/KZBW445-KZBW53)");
    assert_eq!(m.header.kind, Kind::Pla);
}

// REQ: OA-NAM-003
#[test]
fn poj_1_denial() {
    // Rev G Part II 3.7.4.3 p.38
    let m = check("(POJCZWG/KZMP438KZMP/CZWG812-DLH499 -KSLC-KBUF-CZWGBB)");
    assert_eq!(m.header.kind, Kind::Poj);
}

// REQ: OA-NAM-003
#[test]
fn toc_1_paza_to_czvr() {
    // Rev G Part II 3.8.1.3 p.39
    let m = check("(TOCPAZA/CZVR776PAZA/CZVR720-AAL451- RJTD0122-CZVR)");
    assert_eq!(m.header.kind, Kind::Toc);
}

// REQ: OA-NAM-003
#[test]
fn toc_2_kzak_to_czvr() {
    // Rev G Part II 3.8.1.3 p.39
    let m = check("(TOCKZAK/CZVR115KZAK/CZVR278-QFA135/A2217-PHNL0344-CZVR)");
    assert_eq!(m.header.kind, Kind::Toc);
}

// REQ: OA-NAM-003
#[test]
fn aoc_1_paza_to_czvr() {
    // Rev G Part II 3.8.2.3 p.39
    let m = check("(AOCPAZA/CZVR776PAZA/CZVR720-AAL451- RJTD0122-CZVR)");
    assert_eq!(m.header.kind, Kind::Aoc);
}

// REQ: OA-NAM-003
#[test]
fn aoc_2_kzak_to_czvr() {
    // Rev G Part II 3.8.2.3 p.39
    let m = check("(AOCKZAK/CZVR115KZAK/CZVR278-QFA135/A2217-YMML2200-CZVR)");
    assert_eq!(m.header.kind, Kind::Aoc);
}

// REQ: OA-NAM-003, OA-NAM-004
#[test]
fn poa_1_printed_with_a_defect_is_refused_and_the_corrected_form_parses() {
    // Rev G Part II 3.7.3.3 p.37 prints `DLH499/ KSLC`: Field 07 ends in a slash and a space
    // and the hyphen before Field 13 is missing. No reading of Field 07 or of the hyphen rule
    // makes that a message; the example is taken to be a typing error (README). The corrected
    // form, the way the POJ example of 3.7.4.3 is written, parses.
    let printed = "(POACZWG/KZMP438KZMP/CZWG812-DLH499/ KSLC-KBUF-CZWGBB)";
    assert!(parse(printed.as_bytes()).is_err());
    assert!(parse_tolerant(printed.as_bytes()).is_err());
    let m = check("(POACZWG/KZMP438KZMP/CZWG812-DLH499-KSLC-KBUF-CZWGBB)");
    assert_eq!(m.header.kind, Kind::Poa);
}
