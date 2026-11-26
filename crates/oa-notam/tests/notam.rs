//! Handwritten-fixture tests for oa-notam. Every identifier here is made up; none comes from
//! the real feed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::*;
use oa_notam::scalar::{
    parse_coord, parse_eff_end, parse_fl3, parse_iso_ms, parse_limit, parse_scaled, EffEnd,
};
use oa_notam::*;

// REQ: OA-NOTAM-001
#[test]
fn an_event_becomes_one_row_with_identity_place_and_times() {
    let (n, s) = ok(&msg(&[&airport()]));
    assert_eq!(n.event_id[0], EVENT_ID);
    assert_eq!(code_str(&n.series[0]), "Z");
    assert_eq!((n.number[0], n.year[0], n.ntype[0]), (42, 2030, TYPE_NEW));
    assert_eq!(code_str(&n.location[0]), "QQA");
    assert_eq!(code_str(&n.fir[0]), "ZQQ");
    assert_eq!(code_str(&n.icao_location[0]), "KQQA");
    assert_eq!(code_str(&n.xover[0]), "Z0042/30");
    assert_eq!(
        (n.issued_ms[0], n.start_ms[0], n.end_ms[0]),
        (ISSUED, START, END)
    );
    assert_eq!(text_of(&n, 0), "RWY 09/27 CLSD");
    assert_eq!(schedule_of(&n, 0), "");
    assert_eq!(n.errors[0], 0);
    assert_eq!((s.docs, s.accepted, s.rows), (1, 1, 1));
    assert_eq!(s.unknown_elements + s.unknown_attributes, 0);
}

// REQ: OA-NOTAM-001
#[test]
fn types_new_replace_cancel_and_the_referenced_notam() {
    for (code, want) in [("N", TYPE_NEW), ("R", TYPE_REPLACE), ("C", TYPE_CANCEL)] {
        let xml = rep(
            &msg(&[]),
            "<event:type>N</event:type>",
            &format!("<event:type>{code}</event:type>"),
        );
        let (n, _) = ok(&xml);
        assert_eq!(n.ntype[0], want, "{code}");
        assert!(!has_field(&n, 0, Field::Ref));
    }
    // A cancel that names the NOTAM it cancels.
    let xml = rep(
        &rep(
            &msg(&[]),
            "<event:type>N</event:type>",
            "<event:type>C</event:type>",
        ),
        "<event:number>42</event:number>",
        "<event:number>42</event:number><event:referredSeries>Y</event:referredSeries><event:referredNumber>7</event:referredNumber><event:referredYear>2029</event:referredYear>",
    );
    let (n, _) = ok(&xml);
    assert!(has_field(&n, 0, Field::Ref));
    assert_eq!(
        (code_str(&n.ref_series[0]), n.ref_number[0], n.ref_year[0]),
        ("Y", 7, 2029)
    );
    // A NOTAM without a series (the local format) has none, which is not an error.
    let xml = rep(&msg(&[]), "<event:series>Z</event:series>", "");
    let (n, _) = ok(&xml);
    assert!(!has_field(&n, 0, Field::Series));
    assert_eq!(n.errors[0], 0);
    // A type outside N, R, C is refused (the code-list escape `OTHER:` is not accepted: no real
    // message uses it); a message without a type has none, which is not an error.
    let xml = rep(
        &msg(&[]),
        "<event:type>N</event:type>",
        "<event:type>OTHER:ICAO</event:type>",
    );
    assert_eq!(ok(&xml).0.errors[0], bit(Field::Type));
    let xml = rep(&msg(&[]), "<event:type>N</event:type>", "");
    let (n, _) = ok(&xml);
    assert_eq!((n.errors[0], has_field(&n, 0, Field::Type)), (0, false));
    // A series that is not a letter (the AIXM code-list escape, as the feed writes it).
    let xml = rep(
        &msg(&[]),
        "<event:series>Z</event:series>",
        "<event:series>OTHER:QQ</event:series>",
    );
    let (n, _) = ok(&xml);
    assert_eq!(code_str(&n.series[0]), "OTHER:QQ");
}

// REQ: OA-NOTAM-002
#[test]
fn iso_and_compact_times_parse_to_unix_ms() {
    let t = |s: &str| parse_iso_ms(s.as_bytes());
    assert_eq!(t("2030-03-05T10:00:00Z"), Some(START));
    assert_eq!(t("2030-03-05T10:00:00.000Z"), Some(START));
    assert_eq!(t("2030-03-05T09:00:30.5Z"), Some(ISSUED + 30_500));
    assert_eq!(t("2030-03-05T09:00:30.25Z"), Some(ISSUED + 30_250));
    assert_eq!(t("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(t("2028-02-29T23:59:59Z"), Some(1_835_481_599_000));
    assert_eq!(t("2100-03-01T00:00:00Z"), Some(4_107_542_400_000));
    assert_eq!(t("9999-12-31T23:59:59Z"), Some(253_402_300_799_000));
    let c = |s: &str| oa_notam::scalar::parse_compact_ms(s.as_bytes());
    assert_eq!(c("203003051000"), Some(START));
    assert_eq!(c("203003061830"), Some(END));
    assert_eq!(c("197001010000"), Some(0));
    assert_eq!(parse_eff_end(b"203003061830"), Some(EffEnd::At(END)));
    assert_eq!(
        parse_eff_end(b"203003061830EST"),
        Some(EffEnd::Estimated(END))
    );
    assert_eq!(parse_eff_end(b"PERM"), Some(EffEnd::Permanent));
}

// REQ: OA-NOTAM-002
#[test]
fn times_refuse_anything_else() {
    for bad in [
        "",
        "2030-03-05",
        "2030-03-05T10:00:00",
        "2030-03-05T10:00:00+00:00",
        "2030-03-05T10:00:00z",
        "2030-03-05 10:00:00Z",
        "20300305T100000Z",
        "2030-03-05T10:00:00.Z",
        "2030-03-05T10:00:00.1234Z",
        "2030-03-05T10:00:00Zx",
        "2030-13-05T10:00:00Z",
        "2030-00-05T10:00:00Z",
        "2030-02-29T10:00:00Z",
        "2100-02-29T10:00:00Z",
        "2030-04-31T10:00:00Z",
        "2030-03-05T24:00:00Z",
        "2030-03-05T10:60:00Z",
        "2030-03-05T10:00:60Z",
        "1969-12-31T23:59:59Z",
        "2030-03-0xT10:00:00Z",
        "２０３０-03-05T10:00:00Z",
    ] {
        assert_eq!(parse_iso_ms(bad.as_bytes()), None, "{bad:?}");
    }
    for bad in [
        "",
        "20300305100",
        "2030030510000",
        "203003051000Z",
        "203003051060",
        "203003051800x",
        "203013051000",
        "203002301000",
        "202903051000EST ",
        "perm",
        "PERM ",
        "EST",
        "203003052400",
        "196912312359",
    ] {
        assert_eq!(parse_eff_end(bad.as_bytes()), None, "{bad:?}");
    }
}

// REQ: OA-NOTAM-003
#[test]
fn rows_round_trip_through_the_columns() {
    let (n, _) = ok(&full());
    let r = row_at(&n, 0).unwrap();
    assert_eq!(r.number, 42);
    assert!(row_at(&n, 1).is_none());
    let mut m = new_notams(2, 16, 2, 8);
    assert!(push_row(&mut m, &r));
    assert!(push_row(&mut m, &BLANK_ROW));
    assert_eq!(row_at(&m, 0), Some(r));
    assert_eq!(row_at(&m, 1), Some(BLANK_ROW));
    assert_eq!(notams_len(&m), 2);
    // A push at capacity writes nothing and says so.
    assert!(!push_row(&mut m, &r));
    assert_eq!(m.len, 2);
    assert_eq!(m.present.len(), 2);
    assert_eq!(m.apt_icao.len(), 2);
    // The staged row keeps its budgeted size (RULESET A7).
    assert_eq!(std::mem::size_of::<Row>(), 272);
    clear_notams(&mut m);
    assert_eq!((m.len, m.strings_used, m.present.len()), (0, 0, 0));
    assert_eq!(
        (m.capacity, m.strings.len(), m.poly_capacity, m.vtx_capacity),
        (2, 16, 2, 8)
    );
}

// REQ: OA-NOTAM-004
#[test]
fn each_feature_kind_is_found_and_the_most_specific_names_the_row() {
    let cases: [(&str, String, u8); 12] = [
        ("airport", airport(), FEAT_AIRPORT),
        ("runway", runway(), FEAT_RUNWAY),
        ("direction", direction(), FEAT_RUNWAY_DIRECTION),
        ("runway element", runway_element(RING), FEAT_RUNWAY_ELEMENT),
        ("taxiway", taxiway(), FEAT_TAXIWAY),
        (
            "taxiway element",
            taxiway_element(RING),
            FEAT_TAXIWAY_ELEMENT,
        ),
        ("navaid", navaid(), FEAT_NAVAID),
        ("unit", unit(), FEAT_UNIT),
        ("obstacle area", obstacle_area(), FEAT_OBSTACLE_AREA),
        (
            "vertical structure",
            vertical_structure(),
            FEAT_VERTICAL_STRUCTURE,
        ),
        ("radar system", radar_system(), FEAT_RADAR_SYSTEM),
        ("primary radar", primary_radar(), FEAT_PRIMARY_RADAR),
    ];
    for (name, m, kind) in &cases {
        let (n, s) = ok(&msg(&[m]));
        assert_eq!(n.feat_kind[0], *kind, "{name}");
        assert_eq!(n.feat_mask[0], 1u32 << kind, "{name}");
        assert_eq!(n.errors[0], 0, "{name}");
        assert_eq!(s.features[usize::from(*kind) - 1], 1, "{name}");
        assert_eq!(
            s.unknown_elements + s.unknown_attributes,
            0,
            "{name}: structure is known"
        );
        assert_ne!(feat_name(*kind), "none");
    }
    // Designators.
    let (n, _) = ok(&msg(&[&airport()]));
    assert_eq!(code_str(&n.apt[0]), "QQA");
    assert_eq!(code_str(&n.apt_icao[0]), "KQQA");
    assert_eq!(code_str(&n.feat_desig[0]), "QQA");
    let (n, _) = ok(&msg(&[&runway()]));
    assert_eq!(code_str(&n.rwy[0]), "09L/27R");
    assert_eq!(code_str(&n.feat_desig[0]), "09L/27R");
    let (n, _) = ok(&msg(&[&direction()]));
    assert_eq!(code_str(&n.rwy[0]), "27R");
    let (n, _) = ok(&msg(&[&navaid()]));
    assert_eq!(code_str(&n.navaid[0]), "ILS");
    assert_eq!(code_str(&n.feat_desig[0]), "");
    let (n, _) = ok(&msg(&[&unit()]));
    assert_eq!(code_str(&n.unit[0]), "ZQQ");
    assert_eq!(code_str(&n.feat_desig[0]), "ZQQ");
    // Everything at once: the navaid is the most specific; every kind is in the mask.
    let (n, s) = ok(&full());
    assert_eq!(n.feat_kind[0], FEAT_NAVAID);
    for k in FEAT_AIRPORT..=FEAT_PRIMARY_RADAR {
        assert!(n.feat_mask[0] & (1 << k) != 0, "{}", feat_name(k));
    }
    assert_eq!(s.features, [1; FEAT_COUNT]);
    assert_eq!(s.unknown_elements + s.unknown_attributes, 0);
    // The runway designator wins over the direction's; with no runway the direction's is used.
    assert_eq!(code_str(&n.rwy[0]), "09L/27R");
    // No feature: no kind.
    let (n, _) = ok(&msg(&[]));
    assert_eq!((n.feat_kind[0], n.feat_mask[0]), (0, 0));
    // Members in a different order change nothing.
    let a = ok(&msg(&[&airport(), &runway()])).0;
    let b = ok(&msg(&[&runway(), &airport()])).0;
    assert_eq!(row_at(&a, 0), row_at(&b, 0));
}

// REQ: OA-NOTAM-005
#[test]
fn classification_and_q_line_codes_are_read() {
    let xml = rep(
        &msg(&[]),
        "<event:text>",
        "<event:traffic>IV</event:traffic><event:purpose>NBO</event:purpose><event:scope>AE</event:scope><event:text>",
    );
    let (n, _) = ok(&xml);
    assert_eq!(code_str(&n.selection[0]), "QXXXX");
    assert_eq!(n.scenario[0], 123);
    assert_eq!(n.class[0], CLASS_DOM);
    assert_eq!(n.traffic[0], TRAFFIC_I | TRAFFIC_V);
    assert_eq!(n.purpose[0], PURPOSE_N | PURPOSE_B | PURPOSE_O);
    assert_eq!(n.scope[0], SCOPE_A | SCOPE_E);
    for (c, want) in [("INTL", CLASS_INTL), ("DOM", CLASS_DOM), ("MIL", CLASS_MIL)] {
        let x = rep(
            &msg(&[]),
            "<fnse:classification>DOM",
            &format!("<fnse:classification>{c}"),
        );
        assert_eq!(ok(&x).0.class[0], want);
    }
    // Short codes are trimmed of white space (a real purpose code reads `BO `).
    let xml = rep(
        &msg(&[]),
        "<event:text>",
        "<event:purpose>BO </event:purpose><event:text>",
    );
    let (n, _) = ok(&xml);
    assert_eq!(n.purpose[0], PURPOSE_B | PURPOSE_O);
    assert_eq!(n.errors[0], 0);
    // Absent codes are absent.
    let xml = rep(
        &msg(&[]),
        "<event:selectionCode>QXXXX</event:selectionCode>",
        "",
    );
    let (n, _) = ok(&xml);
    assert!(!has_field(&n, 0, Field::Selection));
    assert_eq!(n.errors[0], 0);
}

// REQ: OA-NOTAM-006
#[test]
fn start_and_end_come_from_both_sources_which_must_agree() {
    // Only the effective times: they stand alone.
    let only_eff = msg(&[])
        .replace(
            "<gml:beginPosition>2030-03-05T10:00:00.000Z</gml:beginPosition>",
            "",
        )
        .replace(
            "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>",
            "",
        );
    let (n, _) = ok(&only_eff);
    assert_eq!((n.start_ms[0], n.end_ms[0]), (START, END));
    assert_eq!(n.errors[0], 0);
    // Only the validTime.
    let only_valid = msg(&[])
        .replace(
            "<event:effectiveStart>203003051000</event:effectiveStart>",
            "",
        )
        .replace("<event:effectiveEnd>203003061830</event:effectiveEnd>", "");
    let (n, _) = ok(&only_valid);
    assert_eq!((n.start_ms[0], n.end_ms[0]), (START, END));
    // Disagreement is an error on that field, whichever comes first, and the other is kept.
    let x = rep(&msg(&[]), "203003051000", "203003051001");
    let (n, _) = ok(&x);
    assert_eq!(n.errors[0], bit(Field::Start));
    assert!(!has_field(&n, 0, Field::Start) && has_field(&n, 0, Field::End));
    let x = rep(&msg(&[]), "203003061830", "203003061831");
    let (n, _) = ok(&x);
    assert_eq!(n.errors[0], bit(Field::End));
    // Neither: absent, no error.
    let none = only_eff
        .replace(
            "<event:effectiveStart>203003051000</event:effectiveStart>",
            "",
        )
        .replace("<event:effectiveEnd>203003061830</event:effectiveEnd>", "");
    let (n, _) = ok(&none);
    assert_eq!(
        (
            n.present[0] & (bit(Field::Start) | bit(Field::End)),
            n.errors[0]
        ),
        (0, 0)
    );
}

// REQ: OA-NOTAM-006
#[test]
fn permanent_and_estimated_ends_are_flagged() {
    let x = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>203003061830EST</event:effectiveEnd>",
    );
    let (n, _) = ok(&x);
    assert_eq!(n.flags[0], FLAG_ESTIMATED);
    assert_eq!(n.end_ms[0], END);
    assert_eq!(n.errors[0], 0);
    // PERM: flagged; the end is the validTime's if it has one, else absent.
    let x = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>PERM</event:effectiveEnd>",
    );
    let (n, _) = ok(&x);
    assert_eq!(n.flags[0], FLAG_PERMANENT);
    assert_eq!((n.errors[0], n.end_ms[0]), (0, END));
    let nov = x.replace(
        "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>",
        "",
    );
    let (n, _) = ok(&nov);
    assert_eq!(n.flags[0], FLAG_PERMANENT | FLAG_END_UNKNOWN);
    assert!(!has_field(&n, 0, Field::End));
    assert_eq!(n.errors[0], 0);
    // No marker, no flag.
    assert_eq!(ok(&msg(&[])).0.flags[0], 0);
}

// REQ: OA-NOTAM-007
#[test]
fn text_and_schedule_are_kept_in_the_string_table() {
    let xml = rep(
        &rep(
            &msg(&[]),
            "<event:text>RWY 09/27 CLSD</event:text>",
            "<event:text>A &amp; B &lt;C&gt;&#13;\nD</event:text>",
        ),
        "<event:text>",
        "<event:schedule>18 21 25 1430/2100</event:schedule><event:text>",
    );
    let (n, _) = ok(&xml);
    assert_eq!(text_of(&n, 0), "A & B <C>\r\nD");
    assert_eq!(schedule_of(&n, 0), "18 21 25 1430/2100");
    assert_eq!(
        n.strings_used,
        "A & B <C>\r\nD".len() + "18 21 25 1430/2100".len()
    );
    // White space is kept exactly.
    let x = rep(
        &msg(&[]),
        "<event:text>RWY 09/27 CLSD</event:text>",
        "<event:text>  RWY\n09/27  </event:text>",
    );
    assert_eq!(text_of(&ok(&x).0, 0), "  RWY\n09/27  ");
    // Non-ASCII text survives as UTF-8.
    let x = rep(
        &msg(&[]),
        "<event:text>RWY 09/27 CLSD",
        "<event:text>TWY \u{00c5}\u{2013}\u{1F6EB} CLSD",
    );
    assert_eq!(text_of(&ok(&x).0, 0), "TWY \u{00c5}\u{2013}\u{1F6EB} CLSD");
}

// REQ: OA-NOTAM-007
#[test]
fn text_longer_than_the_limit_or_empty_is_a_field_error_and_takes_no_space() {
    let mut p = parser();
    let mut d = new_decoder(Limits {
        max_text_len: 10,
        ..Limits::new(oa_time_window::DropWindow::empty_for_tests_and_fuzz())
    });
    let mut out = small();
    let mut s = default_stats();
    let x = rep(
        &msg(&[]),
        "<event:text>RWY 09/27 CLSD",
        "<event:text>0123456789",
    );
    decode_bytes(&mut p, &mut d, x.as_bytes(), &mut out, &mut s).unwrap();
    assert_eq!(text_of(&out, 0), "0123456789");
    clear_notams(&mut out);
    // One byte more: an error, with nothing taken from the table (plain and entity-decoded).
    for body in ["01234567890", "0123456789&amp;"] {
        let x = rep(
            &msg(&[]),
            "<event:text>RWY 09/27 CLSD</event:text>",
            &format!("<event:text>{body}</event:text>"),
        );
        decode_bytes(&mut p, &mut d, x.as_bytes(), &mut out, &mut s).unwrap();
        assert_eq!(out.errors[0], bit(Field::Text), "{body}");
        assert_eq!(out.strings_used, 0, "{body}");
        assert_eq!(text_of(&out, 0), "");
        clear_notams(&mut out);
    }
    // Empty text, and text in several nodes (CDATA beside text).
    for body in ["", "<![CDATA[a]]>b", "a<![CDATA[b]]>"] {
        let x = rep(
            &msg(&[]),
            "<event:text>RWY 09/27 CLSD</event:text>",
            &format!("<event:text>{body}</event:text>"),
        );
        decode_bytes(&mut p, &mut d, x.as_bytes(), &mut out, &mut s).unwrap();
        assert_eq!(out.errors[0], bit(Field::Text), "{body:?}");
        assert_eq!(out.strings_used, 0);
        clear_notams(&mut out);
    }
}

// REQ: OA-NOTAM-008
#[test]
fn altitude_limits_convert_to_feet_with_their_reference() {
    let l = |s: &str| parse_limit(s.as_bytes());
    assert_eq!(l("SFC"), Some((0, REF_AGL)));
    assert_eq!(l("GND"), Some((0, REF_AGL)));
    assert_eq!(l("UNL"), Some((ALT_UNLIMITED, REF_FL)));
    assert_eq!(l("FL000"), Some((0, REF_FL)));
    assert_eq!(l("FL138"), Some((13_800, REF_FL)));
    assert_eq!(l("FL999"), Some((99_900, REF_FL)));
    assert_eq!(l("400FT AGL"), Some((400, REF_AGL)));
    assert_eq!(l("400FT AMSL"), Some((400, REF_MSL)));
    assert_eq!(l("400FT MSL"), Some((400, REF_MSL)));
    assert_eq!(l("11300M AMSL"), Some((37_073, REF_MSL)));
    assert_eq!(l("100M AGL"), Some((328, REF_AGL)));
    assert_eq!(l("0M AGL"), Some((0, REF_AGL)));
    for bad in [
        "",
        "FL",
        "FL12",
        "FL1234",
        "fl100",
        "SFC ",
        "400",
        "400FT",
        "400 FT AGL",
        "400FT  AGL",
        "400FT AAGL",
        "-5FT AGL",
        "400KM AGL",
        "FT AGL",
        "999999FT AGL",
        "40000M AMSL",
        "UNLIMITED",
        "1.5FT AGL",
    ] {
        assert_eq!(l(bad), None, "{bad:?}");
    }
    assert_eq!(parse_fl3(b"050"), Some((5000, REF_FL)));
    assert_eq!(parse_fl3(b"50"), None);
    assert_eq!(parse_fl3(b"05a"), None);
    // On a row: explicit limits win over the flight-level pair.
    let x = rep(
        &msg(&[]),
        "<event:text>",
        "<event:lowerLimit>SFC</event:lowerLimit><event:upperLimit>11300M AMSL</event:upperLimit><event:text>",
    );
    let (n, _) = ok(&x);
    assert_eq!((n.floor_ft[0], n.floor_ref[0]), (0, REF_AGL));
    assert_eq!((n.ceil_ft[0], n.ceil_ref[0]), (37_073, REF_MSL));
    // The flight-level pair alone.
    let (n, _) = ok(&msg(&[]));
    assert_eq!((n.floor_ft[0], n.floor_ref[0]), (0, REF_FL));
    assert_eq!((n.ceil_ft[0], n.ceil_ref[0]), (99_900, REF_FL));
    let x = rep(
        &msg(&[]),
        "<event:maximumFL>999</event:maximumFL>",
        "<event:maximumFL>180</event:maximumFL>",
    );
    assert_eq!(ok(&x).0.ceil_ft[0], 18_000);
    // Unlimited keeps its sentinel and is never "below" the floor.
    let x = rep(
        &msg(&[]),
        "<event:text>",
        "<event:lowerLimit>FL500</event:lowerLimit><event:upperLimit>UNL</event:upperLimit><event:text>",
    );
    let (n, _) = ok(&x);
    assert_eq!((n.ceil_ft[0], n.errors[0]), (ALT_UNLIMITED, 0));
    // A floor above the ceiling in the same reference is an error on the ceiling.
    let x = rep(
        &msg(&[]),
        "<event:text>",
        "<event:lowerLimit>FL300</event:lowerLimit><event:upperLimit>FL200</event:upperLimit><event:text>",
    );
    let (n, _) = ok(&x);
    assert_eq!(n.errors[0], bit(Field::Ceiling));
    assert!(has_field(&n, 0, Field::Floor) && !has_field(&n, 0, Field::Ceiling));
    // Different references are not compared.
    let x = rep(
        &msg(&[]),
        "<event:text>",
        "<event:lowerLimit>5000FT AGL</event:lowerLimit><event:upperLimit>FL020</event:upperLimit><event:text>",
    );
    assert_eq!(ok(&x).0.errors[0], 0);
}

// REQ: OA-NOTAM-009
#[test]
fn the_circle_is_a_centre_and_a_radius_in_metres() {
    let (n, _) = ok(&msg(&[]));
    // 30 deg 12 min N is 30.2 degrees, 90 deg 45 min W is 90.75 degrees.
    assert_eq!((n.lat_e7[0], n.lon_e7[0]), (302_000_000, -907_500_000));
    assert_eq!(n.radius_m[0], 5 * 1852);
    assert!(has_circle(&n, 0) && has_geometry(&n, 0));
    let c = |s: &str| parse_coord(s.as_bytes());
    assert_eq!(c("3012N09045W"), Some((302_000_000, -907_500_000)));
    assert_eq!(c("0000N00000E"), Some((0, 0)));
    assert_eq!(c("4559S17959E"), Some((-459_833_333, 1_799_833_333)));
    assert_eq!(c("9000N18000W"), Some((900_000_000, -1_800_000_000)));
    assert_eq!(c("0001N00001E"), Some((166_667, 166_667)));
    for bad in [
        "",
        "3012N09045",
        "3012N09045WW",
        "3060N09045W",
        "3012N09060W",
        "9001N09045W",
        "3012N18001W",
        "9100N09045W",
        "3012N19045W",
        "3012X09045W",
        "3012N09045X",
        "3012n09045w",
        "30.2N09045W",
        "3012 09045W",
        "301N 09045W",
        "-312N09045W",
    ] {
        assert_eq!(c(bad), None, "{bad:?}");
    }
    // Radius: whole nautical miles, 1 to 999.
    for (r, m) in [
        ("1", 1852),
        ("005", 5 * 1852),
        ("070", 70 * 1852),
        ("999", 999 * 1852),
    ] {
        let x = rep(
            &msg(&[]),
            "<event:radius>5</event:radius>",
            &format!("<event:radius>{r}</event:radius>"),
        );
        assert_eq!(ok(&x).0.radius_m[0], m, "{r}");
    }
    // A radius alone, or a centre alone, is not a circle but is kept.
    let nr = rep(
        &msg(&[]),
        "<event:coordinates>3012N09045W</event:coordinates>",
        "",
    );
    let (n, _) = ok(&nr);
    assert!(!has_circle(&n, 0) && has_field(&n, 0, Field::Radius));
    assert!(!has_geometry(&n, 0));
    let nc = rep(&msg(&[]), "<event:radius>5</event:radius>", "");
    let (n, _) = ok(&nc);
    assert!(!has_circle(&n, 0) && has_field(&n, 0, Field::Centre));
}

// REQ: OA-NOTAM-010
#[test]
fn polygons_are_vertex_spans_in_a_shared_array() {
    let (n, s) = ok(&msg(&[
        &runway_element(RING),
        &taxiway_element("30.30 -90.30 30.30 -90.40 30.40 -90.40 30.30 -90.30"),
    ]));
    assert_eq!(polygons_of(&n, 0), 0..2);
    assert_eq!(
        (n.poly_kind[0], n.poly_kind[1]),
        (FEAT_RUNWAY_ELEMENT, FEAT_TAXIWAY_ELEMENT)
    );
    let (la, lo) = polygon_vertices(&n, 0);
    // The closing vertex is not repeated.
    assert_eq!(la, [301_000_000, 301_000_000, 302_000_000, 302_000_000]);
    assert_eq!(lo, [-901_000_000, -902_000_000, -902_000_000, -901_000_000]);
    let (la, lo) = polygon_vertices(&n, 1);
    assert_eq!((la.len(), lo.len()), (3, 3));
    assert_eq!(n.poly_vstart[1], 4);
    assert_eq!(n.vtx_lat.len(), 7);
    assert!(has_field(&n, 0, Field::Polygons) && has_geometry(&n, 0));
    assert_eq!((s.with_polygons, s.polygons, s.vertices), (1, 2, 7));
    // The `Polygons` of the next document start where the last left off.
    let mut out = small();
    let mut st = default_stats();
    for _ in 0..2 {
        run_into(msg(&[&runway_element(RING)]).as_bytes(), &mut out, &mut st).unwrap();
    }
    assert_eq!((out.poly_first[0], out.poly_first[1]), (0, 1));
    assert_eq!(out.poly_vstart[1], 4);
    check_invariants(&out);
    // Fraction digits beyond the 1e-7 degree are rounded, not refused.
    let r = "30.123456789012345 -90.1 30.1 -90.2 30.2 -90.2 30.123456789012345 -90.1";
    let (n, _) = ok(&msg(&[&runway_element(r)]));
    assert_eq!(polygon_vertices(&n, 0).0[0], 301_234_568);
    // Several white-space characters between numbers and around the list.
    let r = "\n 30.1 -90.1\t30.1  -90.2\r\n30.2 -90.2 30.1 -90.1 \n";
    let (n, _) = ok(&msg(&[&runway_element(r)]));
    assert_eq!(polygon_vertices(&n, 0).0.len(), 3);
}

// REQ: OA-NOTAM-022
#[test]
fn decimal_overflow_in_a_ring_is_refused_and_the_next_message_decodes() {
    let good = "30.1 -90.1 30.1 -90.2 30.2 -90.2 30.1 -90.1";
    let huge = "999999999999.9 0 30.1 -90.2 30.2 -90.2 999999999999.9 0";
    let (n, s) = ok(&msg(&[&runway_element(huge)]));
    assert_eq!(n.errors[0], bit(Field::Polygons));
    assert_eq!(n.poly_count[0], 0);
    assert_eq!(s.field_errors[Field::Polygons as usize], 1);
    check_invariants(&n);
    // The same decoder state takes a good message after the refused one.
    let mut out = small();
    let mut st = default_stats();
    run_into(msg(&[&runway_element(huge)]).as_bytes(), &mut out, &mut st).unwrap();
    run_into(msg(&[&runway_element(good)]).as_bytes(), &mut out, &mut st).unwrap();
    assert_eq!(out.len, 2);
    assert_eq!(out.errors[0], bit(Field::Polygons));
    assert_eq!(out.errors[1], 0);
    assert!(has_field(&out, 1, Field::Polygons));
    // The scaler at the i64 limits.
    assert_eq!(parse_scaled(b"999999999999", 0), Some(999_999_999_999));
    assert_eq!(parse_scaled(b"999999999999", 7), None);
    assert_eq!(parse_scaled(b"-999999999999.9", 7), None);
    assert_eq!(parse_scaled(b"922337203685.4775807", 7), Some(i64::MAX));
    assert_eq!(parse_scaled(b"922337203685.47758075", 7), None);
    assert_eq!(parse_scaled(b"922337203685.4775808", 7), None);
    assert_eq!(parse_scaled(b"999999999999.99999999999999999999", 12), None);
}

// REQ: OA-NOTAM-010
#[test]
fn a_bad_polygon_is_a_field_error_on_the_row_and_none_is_kept() {
    let good = "30.1 -90.1 30.1 -90.2 30.2 -90.2 30.1 -90.1";
    let bad_rings = [
        "30.1 -90.1 30.1 -90.2 30.2 -190.2 30.1 -90.1", // bad longitude
        "30.1 -90.1 91.1 -90.2 30.2 -90.2 30.1 -90.1",  // bad latitude
        "30.1 -90.1 30.1 -90.2 30.2 -90.2 30.1 -90.0",  // not closed
        "30.1 -90.1 30.1 -90.2 30.2 -90.2 30.1",        // odd number of values
        "30.1 -90.1 30.1 -90.2 30.1 -90.1",             // fewer than four points
        "",                                             // empty
        "30.1 -90.1 30.1 -90.2 x -90.2 30.1 -90.1",     // not a number
        "30.1 -90.1 30.1 -90.2 +30.2 -90.2 30.1 -90.1", // sign other than minus
        "30.1 -90.1 30.1 -90.2 3e1 -90.2 30.1 -90.1",   // exponent
        "30,1 -90,1 30,1 -90,2 30,2 -90,2 30,1 -90,1",  // comma decimal
        "30.1 -90.1 30.1 -90.2 30. -90.2 30.1 -90.1",   // trailing point
        "30.1 -90.1 30.1 -90.2 30.2 -90.&#50; 30.1 -90.1", // a reference in the numbers
    ];
    for bad in bad_rings {
        // Another good polygon next to it is dropped too: the field fails as a whole.
        let xml = msg(&[&runway_element(good), &taxiway_element(bad)]);
        let (n, s) = ok(&xml);
        assert_eq!(n.errors[0], bit(Field::Polygons), "{bad:?}");
        assert!(!has_field(&n, 0, Field::Polygons));
        assert_eq!(
            (n.poly_count[0], n.vtx_lat.len(), n.poly_vstart.len()),
            (0, 0, 0),
            "{bad:?}"
        );
        assert_eq!(s.field_errors[Field::Polygons as usize], 1);
        assert_eq!(s.polygons, 0);
        // The rest of the row is intact.
        assert_eq!(n.number[0], 42);
        assert!(has_circle(&n, 0));
    }
    // A reference system other than EPSG 4326 as a URN, or three dimensions, is refused.
    for bad in [
        SRS.replace("urn:ogc:def:crs:EPSG::4326", "urn:ogc:def:crs:EPSG::3857"),
        SRS.replace("urn:ogc:def:crs:EPSG::4326", "EPSG:4326"),
        SRS.replace("srsDimension=\"2\"", "srsDimension=\"3\""),
        SRS.replace("srsDimension=\"2\" ", ""),
        SRS.replace("srsName=\"urn:ogc:def:crs:EPSG::4326\"", ""),
    ] {
        let xml = rep(&msg(&[&runway_element(good)]), SRS, &bad);
        let (n, _) = ok(&xml);
        assert_eq!(n.errors[0], bit(Field::Polygons), "{bad}");
    }
    // A ring over the vertex limit.
    let mut p = parser();
    let mut d = new_decoder(Limits {
        max_poly_vertices: 4,
        ..Limits::new(oa_time_window::DropWindow::empty_for_tests_and_fuzz())
    });
    let mut out = small();
    let mut s = default_stats();
    let xml = msg(&[&runway_element(good)]);
    decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut s).unwrap();
    assert_eq!(out.poly_count[0], 1);
    clear_notams(&mut out);
    let five = "30.1 -90.1 30.1 -90.2 30.2 -90.2 30.3 -90.1 30.1 -90.1";
    let xml = msg(&[&runway_element(five)]);
    decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut s).unwrap();
    assert_eq!(out.errors[0], bit(Field::Polygons));
}

// REQ: OA-NOTAM-011
#[test]
fn every_field_has_its_own_refusal_and_the_row_is_kept() {
    let base = full();
    let (b, _) = ok(&base);
    assert_eq!(b.errors[0], 0);
    let base_present = b.present[0];
    // Each case breaks one value; `ins` cases add an element that the fixture lacks.
    let cases: [(Field, &str, &str); FIELD_COUNT] = [
        (
            Field::Series,
            "<event:series>Z</event:series>",
            "<event:series>Z!</event:series>",
        ),
        (
            Field::Number,
            "<event:number>42</event:number>",
            "<event:number>4x</event:number>",
        ),
        (
            Field::Year,
            "<event:year>2030</event:year>",
            "<event:year>30</event:year>",
        ),
        (
            Field::Type,
            "<event:type>N</event:type>",
            "<event:type>Q</event:type>",
        ),
        (
            Field::Ref,
            "<event:number>42</event:number>",
            "<event:number>42</event:number><event:referredSeries>Y</event:referredSeries>",
        ),
        (
            Field::Location,
            "<event:location>QQA</event:location>",
            "<event:location>QQAXY</event:location>",
        ),
        (
            Field::Fir,
            "<event:affectedFIR>ZQQ</event:affectedFIR>",
            "<event:affectedFIR>ZQ!</event:affectedFIR>",
        ),
        (
            Field::IcaoLocation,
            "<fnse:icaoLocation>KQQA</fnse:icaoLocation>",
            "<fnse:icaoLocation>K-QA</fnse:icaoLocation>",
        ),
        (
            Field::Issued,
            "<event:issued>2030-03-05T09:00:00.000Z</event:issued>",
            "<event:issued>2030-02-30T09:00:00.000Z</event:issued>",
        ),
        (
            Field::Start,
            "<event:effectiveStart>203003051000</event:effectiveStart>",
            "<event:effectiveStart>203003051060</event:effectiveStart>",
        ),
        (
            Field::End,
            "<event:effectiveEnd>203003061830</event:effectiveEnd>",
            "<event:effectiveEnd>20300306183</event:effectiveEnd>",
        ),
        (
            Field::Schedule,
            "<event:text>",
            "<event:schedule></event:schedule><event:text>",
        ),
        (
            Field::Text,
            "<event:text>RWY 09/27 CLSD</event:text>",
            "<event:text></event:text>",
        ),
        (
            Field::Selection,
            "<event:selectionCode>QXXXX</event:selectionCode>",
            "<event:selectionCode>qxxxx</event:selectionCode>",
        ),
        (
            Field::Scenario,
            "<event:scenario>123</event:scenario>",
            "<event:scenario>12a</event:scenario>",
        ),
        (
            Field::Classification,
            "<fnse:classification>DOM</fnse:classification>",
            "<fnse:classification>XYZ</fnse:classification>",
        ),
        (
            Field::Traffic,
            "<event:text>",
            "<event:traffic>X</event:traffic><event:text>",
        ),
        (
            Field::Purpose,
            "<event:text>",
            "<event:purpose>NN</event:purpose><event:text>",
        ),
        (
            Field::Scope,
            "<event:text>",
            "<event:scope>Q</event:scope><event:text>",
        ),
        (
            Field::Canceled,
            "<fnse:lastUpdated>",
            "<fnse:canceled>yesterday</fnse:canceled><fnse:lastUpdated>",
        ),
        (
            Field::Xover,
            "<fnse:xovernotamID>Z0042/30</fnse:xovernotamID>",
            "<fnse:xovernotamID>Z0042/30 !</fnse:xovernotamID>",
        ),
        (
            Field::Centre,
            "<event:coordinates>3012N09045W</event:coordinates>",
            "<event:coordinates>3012N09045X</event:coordinates>",
        ),
        (
            Field::Radius,
            "<event:radius>5</event:radius>",
            "<event:radius>1000</event:radius>",
        ),
        (
            Field::Floor,
            "<event:text>",
            "<event:lowerLimit>10 FT</event:lowerLimit><event:text>",
        ),
        (
            Field::Ceiling,
            "<event:text>",
            "<event:upperLimit>FL9</event:upperLimit><event:text>",
        ),
        (
            Field::Polygons,
            "30.20 -90.20 30.20 -90.10",
            "30.20 -90.20 30.20 -99999",
        ),
        (
            Field::Airport,
            "<aixm:designator>QQA</aixm:designator>",
            "<aixm:designator>QQ A</aixm:designator>",
        ),
        (
            Field::AirportIcao,
            "<aixm:locationIndicatorICAO>KQQA</aixm:locationIndicatorICAO>",
            "<aixm:locationIndicatorICAO>K-QA</aixm:locationIndicatorICAO>",
        ),
        (
            Field::Runway,
            "<aixm:designator>09L/27R</aixm:designator>",
            "<aixm:designator>09L 27R</aixm:designator>",
        ),
        (
            Field::Navaid,
            "<aixm:type>ILS</aixm:type><aixm:runwayDirection",
            "<aixm:type>I L S</aixm:type><aixm:runwayDirection",
        ),
        (
            Field::Unit,
            "<aixm:designator>ZQQ</aixm:designator>",
            "<aixm:designator>Z Q</aixm:designator>",
        ),
    ];
    let mut seen = [false; FIELD_COUNT];
    for (f, from, to) in cases {
        let (n, s) = ok(&rep(&base, from, to));
        assert_eq!(
            n.errors[0],
            bit(f),
            "{}: errors {:#x}",
            field_name(f),
            n.errors[0]
        );
        assert_eq!(n.present[0], base_present & !bit(f), "{}", field_name(f));
        assert_eq!(s.field_errors[f as usize], 1, "{}", field_name(f));
        assert_eq!(s.field_errors.iter().sum::<u64>(), 1);
        assert!(n.event_id[0] == EVENT_ID);
        seen[f as usize] = true;
    }
    let missing: Vec<_> = ALL_FIELDS.iter().filter(|f| !seen[**f as usize]).collect();
    assert!(missing.is_empty(), "{missing:?}");
}

// REQ: OA-NOTAM-011
#[test]
fn a_single_valued_field_given_twice_is_an_error_not_a_first_wins() {
    let dups: [(Field, &str); 14] = [
        (Field::Number, "<event:number>43</event:number>"),
        (Field::Year, "<event:year>2030</event:year>"),
        (Field::Type, "<event:type>N</event:type>"),
        (Field::Series, "<event:series>Z</event:series>"),
        (Field::Location, "<event:location>QQA</event:location>"),
        (Field::Fir, "<event:affectedFIR>ZQQ</event:affectedFIR>"),
        (
            Field::Issued,
            "<event:issued>2030-03-05T09:00:00.000Z</event:issued>",
        ),
        (
            Field::Selection,
            "<event:selectionCode>QXXXX</event:selectionCode>",
        ),
        (
            Field::Centre,
            "<event:coordinates>3012N09045W</event:coordinates>",
        ),
        (Field::Radius, "<event:radius>5</event:radius>"),
        (Field::Text, "<event:text>RWY 09/27 CLSD</event:text>"),
        (
            Field::Start,
            "<event:effectiveStart>203003051000</event:effectiveStart>",
        ),
        (
            Field::End,
            "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        ),
        (
            Field::Classification,
            "<fnse:classification>DOM</fnse:classification>",
        ),
    ];
    for (f, extra) in dups {
        let xml = rep(
            &msg(&[]),
            "</event:NOTAM>",
            &format!("{extra}</event:NOTAM>"),
        );
        let xml = if matches!(f, Field::Classification) {
            rep(
                &msg(&[]),
                "<fnse:accountId>",
                &format!("{extra}<fnse:accountId>"),
            )
        } else {
            xml
        };
        let (n, _) = ok(&xml);
        assert_eq!(n.errors[0], bit(f), "{}", field_name(f));
        assert!(!has_field(&n, 0, f));
    }
    // Referred parts given twice.
    let r = "<event:referredSeries>Y</event:referredSeries><event:referredNumber>7</event:referredNumber><event:referredYear>2029</event:referredYear>";
    let xml = rep(
        &msg(&[]),
        "</event:NOTAM>",
        &format!("{r}{r}</event:NOTAM>"),
    );
    assert_eq!(ok(&xml).0.errors[0], bit(Field::Ref));
    let xml = rep(&msg(&[]), "</event:NOTAM>", &format!("{r}</event:NOTAM>"));
    assert_eq!(ok(&xml).0.errors[0], 0);
    // A field that is present but empty, or has children, is an error, not "absent".
    let xml = rep(
        &msg(&[]),
        "<event:radius>5</event:radius>",
        "<event:radius/>",
    );
    assert_eq!(ok(&xml).0.errors[0], bit(Field::Radius));
    let xml = rep(
        &msg(&[]),
        "<event:radius>5</event:radius>",
        "<event:radius>5<event:x/></event:radius>",
    );
    let (n, s) = ok(&xml);
    assert_eq!(
        s.unknown_elements, 1,
        "the child is unknown; the text is still the radius"
    );
    assert_eq!(n.radius_m[0], 5 * 1852);
}

// REQ: OA-NOTAM-012
#[test]
fn a_message_that_is_not_one_event_is_refused_whole() {
    let good = msg(&[&airport()]);
    // Wrong root, wrong namespace.
    assert_eq!(
        err(&good.replace("message:AIXMBasicMessage", "message:Other")),
        DocError::Rejected(Reject::NotAixm)
    );
    assert_eq!(
        err(&good.replace(
            "http://www.aixm.aero/schema/5.1/message",
            "http://example.invalid/message"
        )),
        DocError::Rejected(Reject::NotAixm)
    );
    assert_eq!(err("<root/>"), DocError::Rejected(Reject::NotAixm));
    // No event, two events.
    assert_eq!(
        err(&format!("{HEAD}{}{TAIL}", airport())),
        DocError::Rejected(Reject::NoEvent)
    );
    assert_eq!(
        err(&format!("{HEAD}{TAIL}")),
        DocError::Rejected(Reject::NoEvent)
    );
    assert_eq!(
        err(&format!("{HEAD}{EVENT}{EVENT}{TAIL}")),
        DocError::Rejected(Reject::MultipleEvents)
    );
    // Identifier: missing, empty, malformed, wrong code space, twice.
    let id = "<gml:identifier codeSpace=\"urn:uuid:\">0a1b2c3d-0000-4000-8000-00000000e001</gml:identifier>";
    assert_eq!(
        err(&rep(&good, id, "")),
        DocError::Rejected(Reject::MissingId)
    );
    for bad in [
        "<gml:identifier codeSpace=\"urn:uuid:\"></gml:identifier>",
        "<gml:identifier codeSpace=\"urn:uuid:\">0a1b2c3d-0000-4000-8000-00000000e00</gml:identifier>",
        "<gml:identifier codeSpace=\"urn:uuid:\">0a1b2c3d-0000-4000-8000-00000000e00g</gml:identifier>",
        "<gml:identifier codeSpace=\"urn:uuid:\">0a1b2c3d00004000800000000000e001xxxx</gml:identifier>",
        "<gml:identifier codeSpace=\"urn:other:\">0a1b2c3d-0000-4000-8000-00000000e001</gml:identifier>",
        "<gml:identifier>0a1b2c3d-0000-4000-8000-00000000e001</gml:identifier>",
        &format!("{id}{id}"),
    ] {
        assert_eq!(
            err(&rep(&good, id, bad)),
            DocError::Rejected(Reject::BadId),
            "{bad}"
        );
    }
    // No NOTAM, two NOTAMs.
    let notam_start = good.find("<event:textNOTAM>").unwrap();
    let notam_end = good.find("</event:textNOTAM>").unwrap() + "</event:textNOTAM>".len();
    let notam = good[notam_start..notam_end].to_string();
    let mut none = good.clone();
    none.replace_range(notam_start..notam_end, "");
    assert_eq!(err(&none), DocError::Rejected(Reject::NoNotam));
    let two = rep(&good, &notam, &format!("{notam}{notam}"));
    assert_eq!(err(&two), DocError::Rejected(Reject::NoNotam));
    // An XML error.
    assert!(matches!(err(&good[..good.len() - 5]), DocError::Xml(_)));
    assert!(matches!(
        err(&good.replace("<message:hasMember>", "<!DOCTYPE x><message:hasMember>")),
        DocError::Xml(_)
    ));
    assert!(matches!(err(""), DocError::Xml(_)));
    // Neighbouring documents are unaffected, and every refusal is counted by reason.
    let mut out = small();
    let mut st = default_stats();
    let mut p = parser();
    let mut d = decoder();
    let docs = [
        good.clone(),
        good.replace("message:AIXMBasicMessage", "message:Other"),
        good.clone(),
        format!("{HEAD}{EVENT}{EVENT}{TAIL}"),
        good[..100].to_string(),
        good.clone(),
    ];
    let results: Vec<_> = docs
        .iter()
        .map(|x| decode_bytes(&mut p, &mut d, x.as_bytes(), &mut out, &mut st))
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 3);
    assert_eq!(out.len, 3);
    assert_eq!(out.present[0], out.present[2]);
    assert_eq!((st.docs, st.accepted, st.rows), (6, 3, 3));
    assert_eq!(st.rejects[Reject::NotAixm as usize], 1);
    assert_eq!(st.rejects[Reject::MultipleEvents as usize], 1);
    assert_eq!(st.rejects[REJECT_COUNT], 1);
    assert_eq!(st.rejects.iter().sum::<u64>(), 3);
    check_invariants(&out);
}

// REQ: OA-NOTAM-013
#[test]
fn reaching_a_capacity_is_a_reported_refusal_with_nothing_kept() {
    let xml = msg(&[&runway_element(RING), &taxiway_element(RING)]);
    let mut p = parser();
    let mut d = decoder();
    // (rows, string bytes, polygons, vertices) one short of what the message needs, in turn.
    let (r, sb, pl, vt) = (1, 14, 2, 8);
    for (cap, want) in [
        ((0, sb, pl, vt), CapacityKind::Rows),
        ((r, sb - 1, pl, vt), CapacityKind::Strings),
        ((r, sb, pl - 1, vt), CapacityKind::Polygons),
        ((r, sb, pl, vt - 1), CapacityKind::Vertices),
    ] {
        let mut out = new_notams(cap.0, cap.1, cap.2, cap.3);
        let mut st = default_stats();
        let before = mark(&out);
        let e = decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut st).unwrap_err();
        assert_eq!(e, DocError::Capacity(want), "{want:?}");
        assert_eq!(
            mark(&out),
            before,
            "{want:?}: the tables are exactly as before"
        );
        assert_eq!(out.strings.len(), cap.1);
        assert_eq!(st.rejects[REJECT_COUNT + 2], 1);
        assert_eq!(st.accepted, 0);
        // The same capacity plus one holds.
        let mut ok_out = new_notams(cap.0.max(1), cap.1 + 1, cap.2 + 1, cap.3 + 1);
        decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut ok_out, &mut st).unwrap();
        assert_eq!(ok_out.len, 1);
    }
    // Exactly enough holds; rows already held count.
    let mut out = new_notams(1, sb, pl, vt);
    let mut st = default_stats();
    decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut st).unwrap();
    assert_eq!((out.strings_used, out.vtx_lat.len()), (sb, vt));
    let held = mark(&out);
    let e = decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut st).unwrap_err();
    assert_eq!(e, DocError::Capacity(CapacityKind::Rows));
    assert_eq!(mark(&out), held);
    // A document that has too many elements for the decoder.
    let mut small_dec = new_decoder(Limits {
        max_elements: 10,
        ..Limits::new(oa_time_window::DropWindow::empty_for_tests_and_fuzz())
    });
    let mut out = small();
    let e = decode_bytes(&mut p, &mut small_dec, xml.as_bytes(), &mut out, &mut st).unwrap_err();
    assert!(matches!(e, DocError::TooManyElements { limit: 10, .. }));
    assert_eq!(out.len, 0);
}

// REQ: OA-NOTAM-014
#[test]
fn unknown_paths_are_counted_not_refused() {
    let base = msg(&[&airport()]);
    let (_, s0) = ok(&base);
    assert_eq!(s0.unknown_elements + s0.unknown_attributes, 0);
    // An unknown element, a whole unknown subtree (counted once), an unknown attribute, an
    // element in another namespace, a feature kind the decoder does not know.
    let xml = rep(
        &base,
        "<event:scenario>123</event:scenario>",
        "<event:scenario>123</event:scenario><event:novel a=\"1\"><event:deep><event:deeper/></event:deep></event:novel>",
    );
    let (n, s) = ok(&xml);
    assert_eq!(n.errors[0], 0);
    assert_eq!((s.unknown_elements, s.unknown_attributes), (1, 0));
    assert_eq!(s.unknown.len(), 1);
    assert_eq!(
        unknown_text(&s.unknown[0]),
        "message:hasMember/event:Event/event:timeSlice/event:EventTimeSlice/event:novel"
    );
    assert!(!s.unknown[0].is_attr);
    let xml = rep(
        &base,
        "<event:year>2030</event:year>",
        "<event:year extra=\"x\">2030</event:year>",
    );
    let (n, s) = ok(&xml);
    assert_eq!((s.unknown_elements, s.unknown_attributes), (0, 1));
    assert!(s.unknown[0].is_attr);
    assert!(unknown_text(&s.unknown[0]).ends_with("event:year@extra"));
    assert_eq!(n.year[0], 2030);
    let xml = rep(
        &base,
        "<event:year>",
        "<x:y xmlns:x=\"urn:example:other\">1</x:y><event:year>",
    );
    let (n, s) = ok(&xml);
    assert_eq!(s.unknown_elements, 1);
    assert_eq!(n.errors[0], 0);
    // A known name in another namespace is unknown.
    let xml = rep(
        &base,
        "<event:year>2030</event:year>",
        "<event:year xmlns:event=\"urn:example:other\">2030</event:year>",
    );
    // (the redeclared prefix applies to the element itself)
    let (n, s) = ok(&xml);
    assert_eq!(s.unknown_elements, 1);
    assert!(!has_field(&n, 0, Field::Year));
    let xml = rep(
        &base,
        "</message:AIXMBasicMessage>",
        "<message:hasMember><aixm:Airspace gml:id=\"X\"><aixm:timeSlice/></aixm:Airspace></message:hasMember></message:AIXMBasicMessage>",
    );
    let (n, s) = ok(&xml);
    assert_eq!(s.unknown_elements, 1);
    assert_eq!(n.feat_mask[0], 1 << FEAT_AIRPORT);
    // Namespace declarations are not counted; the same unknown path twice is one entry, twice.
    let xml = rep(
        &base,
        "<event:scenario>123</event:scenario>",
        "<event:scenario>123</event:scenario><event:novel/><event:novel/>",
    );
    let (_, s) = ok(&xml);
    assert_eq!(
        (s.unknown_elements, s.unknown.len(), s.unknown[0].count),
        (2, 1, 2)
    );
    // The table is bounded; the overflow is counted, never an error.
    let mut out = small();
    let mut st = new_stats(2);
    let xml = rep(
        &base,
        "<event:scenario>123</event:scenario>",
        "<event:scenario>123</event:scenario><event:n1/><event:n2/><event:n3/><event:n4/>",
    );
    run_into(xml.as_bytes(), &mut out, &mut st).unwrap();
    assert_eq!(
        (st.unknown.len(), st.unknown_overflow, st.unknown_elements),
        (2, 2, 4)
    );
    // Unknown paths below an unknown element are not counted separately, and nothing in an
    // unknown subtree is read.
    let xml = rep(
        &base,
        "<event:scenario>123</event:scenario>",
        "<event:scenario>123</event:scenario><event:novel><event:textNOTAM><event:NOTAM><event:number>9</event:number></event:NOTAM></event:textNOTAM></event:novel>",
    );
    let (n, s) = ok(&xml);
    assert_eq!((n.number[0], s.unknown_elements), (42, 1));
}

fn damage_corpus() -> Vec<String> {
    vec![
        full(),
        rep(
            &msg(&[&runway_element(RING)]),
            "<event:text>",
            "<event:schedule>1600-2100</event:schedule><event:lowerLimit>400FT AGL</event:lowerLimit><event:upperLimit>11300M AMSL</event:upperLimit><event:traffic>IV</event:traffic><event:purpose>NBO</event:purpose><event:scope>AE</event:scope><event:referredSeries>Y</event:referredSeries><event:referredNumber>7</event:referredNumber><event:referredYear>2029</event:referredYear><event:text>",
        ),
    ]
}

// REQ: OA-NOTAM-015
#[test]
fn mutated_messages_never_panic_and_keep_every_column_in_range() {
    let mut p = parser();
    let mut d = decoder();
    let mut out = new_notams(4, 1 << 14, 8, 1024);
    let mut st = new_stats(32);
    let mut checked = 0usize;
    let mut accepted = 0usize;
    let mut run_one = |input: &[u8], out: &mut Notams| {
        let before = mark(out);
        let r = decode_bytes(&mut p, &mut d, input, out, &mut st);
        check_invariants(out);
        match r {
            Ok(_) => {
                accepted += 1;
                clear_notams(out);
            }
            Err(_) => assert_eq!(mark(out), before, "a refused message changed the tables"),
        }
        checked += 1;
    };
    let fills = [
        0u8, b'<', b'>', b'&', b'"', b'\'', b' ', b'9', b'Z', b'-', b'.', 0xff, b'/', b'N',
    ];
    for doc in damage_corpus() {
        let bytes = doc.as_bytes();
        for i in 0..bytes.len() {
            run_one(&bytes[..i], &mut out);
            let mut v = bytes.to_vec();
            v.remove(i);
            run_one(&v, &mut out);
            for &f in &fills {
                let mut v = bytes.to_vec();
                v[i] = f;
                run_one(&v, &mut out);
            }
            let mut v = bytes.to_vec();
            v.insert(i, bytes[i]);
            run_one(&v, &mut out);
        }
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..3000 {
            let mut v = bytes.to_vec();
            for _ in 0..1 + (x % 4) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let at = (x as usize) % v.len();
                v[at] = (x >> 40) as u8;
            }
            run_one(&v, &mut out);
        }
    }
    assert!(checked > 100_000, "{checked}");
    assert!(
        accepted > 1000,
        "some mutations must still decode: {accepted}"
    );
}

// REQ: OA-NOTAM-016
#[test]
fn decoding_reuses_its_memory_and_leaks_no_state() {
    let xml = full();
    let mut p = parser();
    let mut d = decoder();
    let mut out = small();
    let mut st = default_stats();
    let caps = |o: &Notams| {
        (
            o.present.capacity(),
            o.apt.capacity(),
            o.vtx_lat.capacity(),
            o.poly_vstart.capacity(),
            o.strings.len(),
        )
    };
    let before = caps(&out);
    let parser_cap = p.table_capacity();
    decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut st).unwrap();
    let first = row_at(&out, 0).unwrap();
    for i in 0..200 {
        clear_notams(&mut out);
        // A refused document and a capacity overrun in between change nothing about the next.
        if i % 3 == 0 {
            let _ = decode_bytes(&mut p, &mut d, b"<x/>", &mut out, &mut st);
        }
        if i % 5 == 0 {
            let junk = rep(
                &xml,
                "<event:text>RWY 09/27 CLSD</event:text>",
                "<event:text>&bad;</event:text>",
            );
            assert!(decode_bytes(&mut p, &mut d, junk.as_bytes(), &mut out, &mut st).is_err());
        }
        decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut st).unwrap();
        assert_eq!(row_at(&out, 0), Some(first));
        assert_eq!(out.len, 1);
    }
    assert_eq!(caps(&out), before, "a column was reallocated");
    assert_eq!(p.table_capacity(), parser_cap);
    // The same row from a second decoder: state is only in the output.
    let mut d2 = decoder();
    let mut out2 = small();
    decode_bytes(&mut p, &mut d2, xml.as_bytes(), &mut out2, &mut st).unwrap();
    assert_eq!(row_at(&out2, 0), Some(first));
}

// REQ: OA-NOTAM-019
#[test]
fn field_names_bits_and_tables_agree() {
    assert_eq!(ALL_FIELDS.len(), FIELD_COUNT);
    assert_eq!(FIELD_NAMES.len(), FIELD_COUNT);
    let mut names = std::collections::BTreeSet::new();
    for (i, f) in ALL_FIELDS.iter().enumerate() {
        assert_eq!(*f as usize, i, "{}", field_name(*f));
        assert_eq!(field_name(*f), FIELD_NAMES[i]);
        assert_eq!(bit(*f), 1u64 << i);
        assert!(names.insert(field_name(*f)), "duplicate name");
    }
    assert_eq!(FEAT_NAMES.len(), FEAT_COUNT);
    for k in 1..=FEAT_COUNT as u8 {
        assert_eq!(feat_name(k), FEAT_NAMES[usize::from(k) - 1]);
    }
    assert_eq!((feat_name(0), feat_name(99)), ("none", "none"));
    assert_eq!(REJECT_SLOT_NAMES.len(), REJECT_SLOTS);
    assert_eq!(REJECT_NAMES.len(), REJECT_COUNT);
    assert_eq!(
        reject_index(&DocError::Rejected(Reject::NoNotam)),
        Reject::NoNotam as usize
    );
    assert_eq!(parse_scaled(b"-90.5", 7), Some(-905_000_000));
    assert_eq!(parse_scaled(b"90.", 7), None);
    assert_eq!(parse_scaled(b"", 7), None);
    assert_eq!(parse_scaled(b"-", 7), None);
}

// REQ: OA-NOTAM-023
#[test]
fn a_namespace_written_with_a_character_reference_is_the_same_namespace() {
    let plain = msg(&[&airport()]);
    let escaped = plain.replacen("xmlns:event=\"http://", "xmlns:event=\"&#104;ttp://", 1);
    assert_ne!(plain, escaped);
    let (n, s) = ok(&plain);
    let (n2, s2) = ok(&escaped);
    assert_eq!(n2.event_id[0], n.event_id[0]);
    assert_eq!(text_of(&n2, 0), text_of(&n, 0));
    assert_eq!(n2.errors[0], n.errors[0]);
    assert_eq!(
        (s2.rows, s2.unknown_elements, s2.unknown_attributes),
        (s.rows, s.unknown_elements, s.unknown_attributes)
    );
}

// REQ: OA-NOTAM-024
#[test]
fn a_referred_number_and_year_without_a_series_is_a_local_format_reference() {
    let xml = rep(
        &rep(
            &msg(&[]),
            "<event:type>N</event:type>",
            "<event:type>C</event:type>",
        ),
        "<event:number>42</event:number>",
        "<event:number>42</event:number><event:referredNumber>7</event:referredNumber><event:referredYear>2029</event:referredYear>",
    );
    let (n, _) = ok(&xml);
    assert_eq!(n.errors[0], 0);
    assert!(has_field(&n, 0, Field::Ref));
    assert_eq!(
        (code_str(&n.ref_series[0]), n.ref_number[0], n.ref_year[0]),
        ("", 7, 2029)
    );
    // A referred year alone is still not a reference.
    let xml = rep(
        &msg(&[]),
        "<event:number>42</event:number>",
        "<event:number>42</event:number><event:referredYear>2029</event:referredYear>",
    );
    assert_eq!(ok(&xml).0.errors[0], bit(Field::Ref));
}

// REQ: OA-SEAL-014
#[test]
fn a_notam_issued_or_cancelled_in_the_drop_window_is_dropped_whole_and_every_counter_is_as_for_an_empty_run(
) {
    // 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z, the sealed window, in Unix ms.
    let w = oa_time_window::DropWindow::new(1_795_132_800_000, 1_795_393_020_000);
    let issued = "<event:issued>2030-03-05T09:00:00.000Z</event:issued>";
    let in_issued = |t: &str| format!("<event:issued>{t}</event:issued>");
    let base = rep(
        &msg(&[&airport()]),
        "</event:NOTAM>",
        "<event:zzz>1</event:zzz></event:NOTAM>",
    );
    let bad = rep(
        &base,
        "<event:radius>5</event:radius>",
        "<event:radius>x</event:radius>",
    );
    // A cancel time in the window: the element is added to the NOTAM.
    let cancelled = rep(
        &base,
        "</event:NOTAM>",
        "</event:NOTAM><fnse:canceled>2026-11-22T00:00:00.000Z</fnse:canceled>",
    );
    // An invalid field comes before the issue time in the document, and a field after it.
    let early = rep(&bad, issued, &in_issued("2026-11-21T00:00:00.000Z"));
    let docs = [
        rep(&base, issued, &in_issued("2026-11-21T00:00:00.000Z")),
        early,
        rep(&cancelled, issued, issued),
    ];
    let mut p = parser();
    for xml in docs {
        let mut d = new_decoder(Limits::new(w));
        let mut out = small();
        let mut stats = default_stats();
        let o = decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut stats).unwrap();
        assert!(o.sealed_dropped);
        assert_eq!(
            o,
            Outcome {
                row: 0,
                field_errors: 0,
                unknown_elements: 0,
                unknown_attributes: 0,
                sealed_dropped: true,
            }
        );
        assert_eq!(out.len, 0);
        assert_eq!(mark(&out), mark(&small()));
        assert_eq!(
            stats,
            Stats {
                sealed_window_dropped: 1,
                ..default_stats()
            }
        );
    }
    // Outside it (the end is excluded) the NOTAM is kept and its unknown element counted.
    let mut d = new_decoder(Limits::new(w));
    let mut out = small();
    let mut stats = default_stats();
    let xml = rep(&base, issued, &in_issued("2026-11-23T00:17:00.000Z"));
    let o = decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut stats).unwrap();
    assert!(!o.sealed_dropped);
    assert_eq!((out.len, stats.docs, stats.rows), (1, 1, 1));
    assert_eq!(stats.unknown_elements, 1);
    // A document refused whole has no known time and is counted as refused.
    let r = decode_bytes(&mut p, &mut d, b"<notaixm/>", &mut out, &mut stats);
    assert!(r.is_err());
    assert_eq!(stats.sealed_window_dropped, 0);
}

/// Decodes `xml` with `w` and returns the outcome, the rows kept and the stats.
fn decode_with_window(xml: &str, w: oa_time_window::DropWindow) -> (Outcome, usize, Stats) {
    let mut p = parser();
    let mut d = new_decoder(Limits::new(w));
    let mut out = small();
    let mut stats = default_stats();
    let o = decode_bytes(&mut p, &mut d, xml.as_bytes(), &mut out, &mut stats).unwrap();
    (o, out.len, stats)
}

// REQ: OA-SEAL-014
#[test]
fn issue_and_cancel_times_are_window_keys_in_any_encoding_and_validity_times_are_not() {
    let w = oa_time_window::DropWindow::new(1_795_132_800_000, 1_795_393_020_000);
    let issued = "<event:issued>2030-03-05T09:00:00.000Z</event:issued>";
    let base = msg(&[&airport()]);
    let with = |t: &str| rep(&base, issued, &format!("<event:issued>{t}</event:issued>"));
    // Entity-encoded and padded with whitespace.
    for t in [
        "2026-11-21T00:00:00.000&#90;",
        " 2026-11-21T00:00:00.000Z\n",
    ] {
        assert!(decode_with_window(&with(t), w).0.sealed_dropped, "{t:?}");
    }
    // A text split by a comment is not a value the decoder reads (it is a bad field), so the
    // scan, which reads with the decoder's own text reader, does not read it either.
    assert!(
        !decode_with_window(&with("2026-11-21T00:00:<!--x-->00.000Z"), w)
            .0
            .sealed_dropped
    );
    // A repeated issue element: both are read, so the one in the window is found.
    let dup = rep(
        &base,
        issued,
        &format!("{issued}<event:issued>2026-11-21T00:00:00.000Z</event:issued>"),
    );
    assert!(decode_with_window(&dup, w).0.sealed_dropped);
    // A NOTAM with only a cancel time in the window, its issue time outside it.
    let canc = rep(
        &base,
        "</event:NOTAM>",
        "</event:NOTAM><fnse:canceled>2026-11-22T00:00:00.000Z</fnse:canceled>",
    );
    assert!(decode_with_window(&canc, w).0.sealed_dropped);
    // The same cancel time outside the window: kept.
    let canc_out = rep(
        &base,
        "</event:NOTAM>",
        "</event:NOTAM><fnse:canceled>2030-03-06T00:00:00.000Z</fnse:canceled>",
    );
    assert!(!decode_with_window(&canc_out, w).0.sealed_dropped);
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_roundtrips_retained_scalars_and_escapes_xml_text() {
    let source = rep(
        &msg(&[]),
        "<event:text>RWY 09/27 CLSD</event:text>",
        "<event:text>A &amp; B &lt;C&gt;</event:text>",
    );
    let (before, _) = ok(&source);
    let mut output = vec![0u8; MAX_ENCODED_BYTES];
    let len = encode_row(&before, 0, &mut output).unwrap();
    let xml = std::str::from_utf8(&output[..len]).unwrap();
    assert!(xml.contains("A &amp; B &lt;C&gt;"));
    let (after, _) = ok(xml);
    assert_eq!(row_at(&before, 0), row_at(&after, 0));
    assert_eq!(text_of(&before, 0), text_of(&after, 0));
    assert_eq!(schedule_of(&before, 0), schedule_of(&after, 0));
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_roundtrips_each_feature_and_keeps_polygon_ownership() {
    let second_ring = "31.10 -91.10 31.10 -91.20 31.20 -91.20 31.20 -91.10 31.10 -91.10";
    let second_runway_element = runway_element(second_ring);
    let source = msg(&[
        &airport(),
        &runway(),
        &direction(),
        &runway_element(RING),
        &second_runway_element,
        &taxiway(),
        &taxiway_element("30.30 -90.30 30.30 -90.40 30.40 -90.40 30.30 -90.30"),
        &navaid(),
        &unit(),
        &obstacle_area(),
        &vertical_structure(),
        &radar_system(),
        &primary_radar(),
    ]);
    let (before, _) = ok(&source);
    let mut output = vec![0u8; MAX_ENCODED_BYTES];
    let len = encode_row(&before, 0, &mut output).unwrap();
    let xml = std::str::from_utf8(&output[..len]).unwrap();
    let (after, _) = ok(xml);
    let old = row_at(&before, 0).unwrap();
    let new = row_at(&after, 0).unwrap();
    assert_eq!(old, new);
    assert_eq!(old.feature_count, 13);
    assert_eq!(old.feature_count, new.feature_count);
    for index in 0..old.feature_count as usize {
        assert_eq!(feature_at(&before, index), feature_at(&after, index));
    }
    let old_polys = polygons_of(&before, 0);
    let new_polys = polygons_of(&after, 0);
    assert_eq!(old_polys.len(), 3);
    assert_eq!(old_polys.len(), new_polys.len());
    for (left, right) in old_polys.zip(new_polys) {
        assert_eq!(before.poly_kind[left], after.poly_kind[right]);
        assert_eq!(before.poly_feature[left], after.poly_feature[right]);
        assert_eq!(
            polygon_vertices(&before, left),
            polygon_vertices(&after, right)
        );
    }

    let (scalars, _) = ok(&msg(&[]));
    let mut tiny = [0xA5; 64];
    assert_eq!(
        encode_row(&scalars, 0, &mut tiny),
        Err(EncodeError::OutputTooSmall)
    );
    assert!(tiny.iter().all(|&b| b == 0xA5));
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_roundtrips_every_single_feature_fixture() {
    let fixtures = [
        airport(),
        runway(),
        direction(),
        runway_element(RING),
        taxiway(),
        taxiway_element("30.30 -90.30 30.30 -90.40 30.40 -90.40 30.30 -90.30"),
        navaid(),
        unit(),
        obstacle_area(),
        vertical_structure(),
        radar_system(),
        primary_radar(),
    ];
    for fixture in fixtures {
        let source = msg(&[&fixture]);
        let (before, _) = ok(&source);
        let mut output = vec![0u8; MAX_ENCODED_BYTES];
        let len = encode_row(&before, 0, &mut output).unwrap();
        let (after, _) = ok(std::str::from_utf8(&output[..len]).unwrap());
        assert_eq!(row_at(&before, 0), row_at(&after, 0));
        assert_eq!(feature_at(&before, 0), feature_at(&after, 0));
        assert_eq!(polygons_of(&before, 0).len(), polygons_of(&after, 0).len());
    }
}

// REQ: OA-NOTAM-026
#[test]
fn feature_multiplicity_exceeds_default_reservation_with_explicit_capacity() {
    let mut members = Vec::new();
    for index in 0..20 {
        let code = format!("Q{index:02}");
        let member = rep(
            &airport(),
            "<aixm:designator>QQA</aixm:designator>",
            &format!("<aixm:designator>{code}</aixm:designator>"),
        );
        members.push(member);
    }
    let refs: Vec<&str> = members.iter().map(String::as_str).collect();
    let source = msg(&refs);
    let mut before = new_notams_with_feature_capacity(1, 1 << 16, 16, 4096, 24);
    let mut stats = default_stats();
    run_into(source.as_bytes(), &mut before, &mut stats).unwrap();
    assert_eq!(row_at(&before, 0).unwrap().feature_count, 20);
    let values: Vec<String> = (0..20)
        .map(|ix| {
            let f = feature_at(&before, ix).unwrap();
            let len = usize::from(f.designator_len);
            std::str::from_utf8(&f.designator[..len])
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(values.first().map(String::as_str), Some("Q00"));
    assert_eq!(values.last().map(String::as_str), Some("Q19"));

    let mut output = vec![0u8; MAX_ENCODED_BYTES];
    let len = encode_row(&before, 0, &mut output).unwrap();
    let (after, _) = ok(std::str::from_utf8(&output[..len]).unwrap());
    assert_eq!(row_at(&after, 0).unwrap().feature_count, 20);
    for index in 0..20 {
        assert_eq!(feature_at(&before, index), feature_at(&after, index));
    }
}

// REQ: OA-NOTAM-026
#[test]
fn feature_capacity_refusal_is_transactional() {
    let members: Vec<String> = (0..20).map(|_| airport()).collect();
    let refs: Vec<&str> = members.iter().map(String::as_str).collect();
    let source = msg(&refs);
    let mut output = new_notams_with_feature_capacity(1, 1 << 16, 16, 4096, 16);
    let before = mark(&output);
    let mut stats = default_stats();
    assert_eq!(
        run_into(source.as_bytes(), &mut output, &mut stats),
        Err(DocError::Capacity(CapacityKind::Features))
    );
    assert_eq!(mark(&output), before);
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_preserves_estimated_end_and_fnse_event_extension_fields() {
    let source = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>203003061830EST</event:effectiveEnd>",
    );
    let source = rep(
        &source,
        "<fnse:classification>DOM",
        "<fnse:canceled>2030-03-05T09:30:00.000Z</fnse:canceled><fnse:classification>DOM",
    );
    let (before, _) = ok(&source);
    let mut output = vec![0u8; MAX_ENCODED_BYTES];
    let len = encode_row(&before, 0, &mut output).unwrap();
    let xml = std::str::from_utf8(&output[..len]).unwrap();
    assert!(xml.contains("203003061830EST"));
    assert!(xml.contains("<fnse:canceled>2030-03-05T09:30:00.000Z</fnse:canceled>"));
    let (after, _) = ok(xml);
    assert_eq!(row_at(&before, 0), row_at(&after, 0));
}

// REQ: OA-NOTAM-026
#[test]
fn encoder_roundtrips_permanent_and_explicit_unknown_end_positions() {
    let permanent_concrete = rep(
        &msg(&[]),
        "<event:effectiveEnd>203003061830</event:effectiveEnd>",
        "<event:effectiveEnd>PERM</event:effectiveEnd>",
    );
    let (concrete_before, _) = ok(&permanent_concrete);
    assert!(has_field(&concrete_before, 0, Field::End));
    let mut output = vec![0u8; MAX_ENCODED_BYTES];
    let len = encode_row(&concrete_before, 0, &mut output).unwrap();
    let xml = std::str::from_utf8(&output[..len]).unwrap();
    assert!(xml.contains("<event:effectiveEnd>PERM</event:effectiveEnd>"));
    let (concrete_after, _) = ok(xml);
    assert_eq!(row_at(&concrete_before, 0), row_at(&concrete_after, 0));

    // The fixture's PERM case has no concrete event valid-time end. GML permits an empty
    // TimePosition with indeterminatePosition="unknown"; no date is invented.
    let permanent_unknown = rep(
        &permanent_concrete,
        "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>",
        "",
    );
    let (unknown_before, _) = ok(&permanent_unknown);
    assert!(!has_field(&unknown_before, 0, Field::End));
    assert_eq!(unknown_before.flags[0], FLAG_PERMANENT | FLAG_END_UNKNOWN);
    let len = encode_row(&unknown_before, 0, &mut output).unwrap();
    let xml = std::str::from_utf8(&output[..len]).unwrap();
    assert!(xml.contains("<gml:endPosition indeterminatePosition=\"unknown\"></gml:endPosition>"));
    assert!(xml.contains("<event:effectiveEnd>PERM</event:effectiveEnd>"));
    let (unknown_after, _) = ok(xml);
    assert_eq!(row_at(&unknown_before, 0), row_at(&unknown_after, 0));

    // GML's known-after state is not collapsed into unknown. This profile reports the end as an
    // error and the encoder refuses the row.
    let after_source = rep(
        &permanent_concrete,
        "<gml:endPosition>2030-03-06T18:30:00.000Z</gml:endPosition>",
        "<gml:endPosition indeterminatePosition=\"after\"/>",
    );
    let (after_rows, _) = ok(&after_source);
    assert_ne!(after_rows.flags[0] & FLAG_END_UNKNOWN, FLAG_END_UNKNOWN);
    assert_ne!(after_rows.errors[0] & bit(Field::End), 0);
    assert_eq!(
        encode_row(&after_rows, 0, &mut output),
        Err(EncodeError::InvalidRow)
    );
}
