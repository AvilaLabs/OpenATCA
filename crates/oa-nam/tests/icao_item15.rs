//! ICAO Doc 4444 Appendix 2 Item 15 forms used by the NAM FPL and CHG adapters.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oa_nam::{
    field::parse_f15,
    item15::{filed_change, lrm_reply, FiledChange, Item15Reject},
    lrm::lrm,
    *,
};

// REQ: OA-NAM-036
#[test]
fn item15_accepts_speed_level_and_significant_point_forms() {
    for field in [
        "N0120F350 ABC DCT J12 DEF",
        "K0450S1130 49N050W 4830N01615W",
        "M080F350 ABC123045",
        "N0120VFR ABC",
    ] {
        assert!(parse_f15(field.as_bytes()).is_ok(), "{field}");
    }
}

// REQ: OA-NAM-036
#[test]
fn item15_accepts_change_groups_cruise_climbs_and_rule_transitions() {
    for field in [
        "N0120F350 ABC/N0120F370 J12",
        "N0120F350 ABC/F370 DCT DEF/A070",
        "N0120F350 ABC IFR DEF VFR GHI",
        "N0120F350 ABC C/DEF/N0120F350F370",
        "N0120F350 ABC C/DEF/N0120F350PLUS",
    ] {
        assert!(parse_f15(field.as_bytes()).is_ok(), "{field}");
    }
}

// REQ: OA-NAM-036
#[test]
fn malformed_points_and_change_groups_return_stable_field15_refusals() {
    for field in [
        "N0120F350 ABC/N12",
        "N0120F350 C/DEF/N0120F350F370F390",
        "N0120F350 ABC/",
    ] {
        let error = parse_f15(field.as_bytes()).expect_err(field);
        assert_eq!(error.field, 15, "{field}");
        assert!(lrm(&error).0 > 0, "{field}");
    }
}

// REQ: OA-NAM-036
#[test]
fn fpl_chg_and_cnl_item15_handling_is_confined_to_defined_messages() {
    let fpl = b"(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120F350 CYSC DCT 49N050W KMPV-KMPV0053-EET/KZBW0003)";
    assert_eq!(parse(fpl).unwrap().f15.unwrap().route.len(), 4);
    let chg = b"(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA2042-CYOW-RMK/ROUTE UPDATE-15/N0120F350 KSEA DCT CYOW)";
    assert!(parse(chg)
        .unwrap()
        .f22
        .iter()
        .any(|a| matches!(&a.data, AmendData::F15(_))));
    let cnl = b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)";
    assert!(parse(cnl).unwrap().f15.is_none());
}

// REQ: OA-NAM-036, OA-NAM-037
#[test]
fn filed_change_classifies_fpl_chg_dla_and_cnl_route_effects() {
    let fpl = parse(b"(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120F350 CYSC DCT KMPV-KMPV0053-EET/KZBW0003)").unwrap();
    assert!(matches!(filed_change(&fpl), Ok(FiledChange::Replace(_))));
    let mut chg_without_15 = fpl.clone();
    chg_without_15.header.kind = Kind::Chg;
    assert_eq!(
        filed_change(&chg_without_15),
        Ok(FiledChange::NoRouteChange)
    );
    let chg = parse(b"(CHGKZMP/CZWG776KZMP/CZWG603-AAL72-KSEA2042-CYOW-RMK/ROUTE UPDATE-15/N0120F350 KSEA DCT CYOW)").unwrap();
    assert!(matches!(filed_change(&chg), Ok(FiledChange::Replace(_))));
    let mut duplicate_chg = chg.clone();
    let amended = duplicate_chg
        .f22
        .iter()
        .find_map(|amendment| match &amendment.data {
            AmendData::F15(field) => Some(field.clone()),
            _ => None,
        })
        .unwrap();
    duplicate_chg.f22.push(Amendment {
        padded: false,
        data: AmendData::F15(amended),
    });
    assert_eq!(
        filed_change(&duplicate_chg),
        Err(Item15Reject::DuplicateItem15Amendment)
    );
    let dla = parse(b"(DLAKZMP/CZWG776KZMP/CZWG603-KLM671-LIRF0900-LYDU-0)").unwrap();
    assert!(matches!(filed_change(&dla), Ok(FiledChange::Delay { .. })));
    let cnl = parse(b"(CNLKZHU/MMZT776KZHU/MMZT603-UAL1021-KIAD0818-MMEX-0)").unwrap();
    assert_eq!(filed_change(&cnl), Ok(FiledChange::Cancel));
}

// REQ: OA-NAM-037
#[test]
fn dla_uses_public_appendix3_fields_and_carries_no_route_update() {
    // ICAO Doc 4444 Appendix 3 2.3.4: DLA has fields 3, 7, 13, 16 and 18.
    // It changes the revised off-block time; it has no Item 15 route.
    let dla = b"(DLAKZMP/CZWG776KZMP/CZWG603-KLM671-LIRF0900-LYDU-0)";
    let message = parse(dla).unwrap();
    assert_eq!(message.header.kind, Kind::Dla);
    assert_eq!(message.f13.unwrap().time.unwrap().hour, 9);
    assert_eq!(message.f16.as_ref().unwrap().aerodrome.0, *b"LYDU");
    assert!(message.f15.is_none());
    assert_eq!(encode(&message).unwrap(), dla);
    let public_example = b"(DLAKZMP/CZWG776-KLM671-LIRF0900-LYDU-0)";
    let public_message = parse(public_example).unwrap();
    assert!(public_message.header.reference.is_none());
    assert_eq!(encode(&public_message).unwrap(), public_example);
}

// REQ: OA-NAM-036
#[test]
fn vfr_cruising_level_requires_vfr_flight_rules_when_context_is_present() {
    let base = "(FPLCZUL/KZBW043-N12345-IG-C172/L-SD/C-CYST2055-N0120F350 CYSC DCT KMPV-KMPV0053-EET/KZBW0003)";
    let vfr = base
        .replace("-IG-", "-VG-")
        .replace("N0120F350", "N0120VFR");
    assert_eq!(
        parse(vfr.as_bytes()).unwrap().f15.unwrap().level,
        Level::Vfr
    );
    let ifr = base.replace("N0120F350", "N0120VFR");
    let error = parse(ifr.as_bytes()).unwrap_err();
    assert_eq!((error.kind, error.field), (ErrorKind::Level, 15));
}

// REQ: OA-NAM-036
#[test]
fn high_altitude_and_duplicate_names_remain_bounded_syntax_without_guessed_resolution() {
    // Public incident shape only: the same spelling may resolve to distinct physical fixes.
    // Item 15 preserves the filed spelling; exact resolution belongs to the NASR/CIFP caller.
    let duplicate_name = parse_f15(b"N0120F600 ABC J12 ABC").unwrap();
    assert_eq!(duplicate_name.route, ["ABC", "J12", "ABC"]);

    // LA2014 shape: absent filed level is a normal Field 15 refusal, never a process failure.
    assert_eq!(parse_f15(b"N0120 ABC").unwrap_err().field, 15);

    let long_route = format!(
        "N0120F600 {}",
        vec!["ABC"; MAX_ROUTE_ELEMENTS + 1].join(" ")
    );
    assert_eq!(parse_f15(long_route.as_bytes()).unwrap_err().field, 15);
}

fn facility(value: &str) -> Facility {
    Facility(value.as_bytes().try_into().unwrap())
}

fn bare_fpl() -> Message {
    bare(Header {
        kind: Kind::Fpl,
        id: MsgId {
            sender: facility("KAAA"),
            receiver: facility("KBBB"),
            number: 12,
        },
        reference: None,
    })
}

// REQ: OA-NAM-038
#[test]
fn item15_refusal_catalogue_uses_route_field_codes() {
    assert_eq!(
        Item15Reject::MalformedGroup.lrm(Profile::default(), Kind::Fpl),
        (40, 15)
    );
    assert_eq!(
        Item15Reject::UnknownPoint.lrm(Profile::default(), Kind::Fpl),
        (43, 15)
    );
    assert_eq!(
        Item15Reject::UnknownAirway.lrm(Profile::default(), Kind::Fpl),
        (41, 15)
    );
    assert_eq!(
        Item15Reject::DuplicateItem15Amendment.lrm(Profile::default(), Kind::Chg),
        (50, 22)
    );
    assert_eq!(
        Item15Reject::MissingRouteLevel.lrm(Profile::default(), Kind::Fpl),
        (30, 15)
    );
    assert_eq!(
        Item15Reject::AltitudeBelowMea.lrm(Profile::default(), Kind::Fpl),
        (29, 15)
    );
}

// REQ: OA-NAM-038
#[test]
fn item15_semantic_refusal_builds_correlated_lrm() {
    let reply = lrm_reply(
        &bare_fpl(),
        Profile::default(),
        13,
        Item15Reject::AmbiguousPoint,
    )
    .unwrap();
    assert_eq!(reply.header.kind, Kind::Lrm);
    assert_eq!(reply.header.id.sender, facility("KBBB"));
    assert_eq!(reply.header.id.receiver, facility("KAAA"));
    assert_eq!(
        reply.header.reference,
        Some(MsgId {
            sender: facility("KAAA"),
            receiver: facility("KBBB"),
            number: 12
        })
    );
    assert!(matches!(&reply.f18, Some(Field18::Items(_))));
    let text = match &reply.f18 {
        Some(Field18::Items(items)) => items.first().map(|item| item.text.as_str()).unwrap_or(""),
        _ => "",
    };
    assert!(text.starts_with("43/15/"));
    let bytes = encode(&reply).unwrap();
    assert_eq!(parse(&bytes).unwrap(), reply);
}
