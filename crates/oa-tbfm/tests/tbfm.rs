//! Handwritten-fixture tests for oa-tbfm. Every identifier here is made up; none comes from the
//! real feed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use oa_tbfm::rows::{change, pt, sch_flag, section};
use oa_tbfm::{
    clear_rows, cross_check, decode_bytes, decode_doc, default_limits, field_name, is_cleared,
    name_str, new_rows, new_stats, observe, parse_iso_ms, push_row, row_at, rows_len, sch_get,
    truncate_rows, DocError, Field, FieldError, FieldErrorKind, MsgKind, Outcome, Rows, TimeError,
    BLANK_ROW, FIELD_COUNT, NO_BCN, NO_NUM, NO_TIME,
};
use oa_xml::Parser;
use std::collections::BTreeSet;

const NS: &str = "urn:us:gov:dot:faa:atm:tfm:tbfmmeteringpublication:1.1.0";
/// 2026-11-14T00:00:00Z in Unix ms.
const DAY: i64 = 1_794_614_400_000;

fn t(h: i64, m: i64, s: i64) -> i64 {
    DAY + ((h * 60 + m) * 60 + s) * 1000
}

fn env(tma_attrs: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><env xmlns=\"{NS}\" \
         targetNamespace=\"{NS}\" envTime=\"2026-11-14T14:11:42.420Z\" envSrce=\"TMA.ZTT.FAA.GOV\">\
         <tma msgTime=\"2026-11-14T14:11:42.420Z\" msgId=\"4711\"{tma_attrs}>{body}</tma></env>"
    )
}

const AIR_ATTRS: &str =
    "gufi=\"KQ000001Xa\" cid=\"123\" tmaId=\"T00001\" apt=\"QQA\" dap=\"QQB\" aid=\"TST123\" airType=\"AMD\"";

fn air(body: &str) -> String {
    env("", &format!("<air {AIR_ATTRS}>{body}</air>"))
}

fn run(xml: &str, cap: usize) -> (Result<Outcome, DocError>, Rows) {
    let mut rows = new_rows(cap, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let mut p = Parser::new(default_limits()).unwrap();
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    (r, rows)
}

fn ok(xml: &str) -> (Outcome, Rows) {
    let (r, rows) = run(xml, 16);
    (r.unwrap(), rows)
}

/// Decodes a message expected to be refused: either the message itself (structure, capacity) or
/// its one flight (the field error). Nothing of it may be left in the rows.
fn err(xml: &str) -> DocError {
    let (r, rows) = run(xml, 16);
    assert_eq!(rows_len(&rows), 0, "a refused message left rows behind");
    match r {
        Ok(o) => {
            assert_eq!(o.rejected, 1, "accepted: {o:?}");
            DocError::Field(o.first_reject.unwrap())
        }
        Err(e) => e,
    }
}

fn n<const N: usize>(s: &str) -> [u8; N] {
    let mut o = [0u8; N];
    o[..s.len()].copy_from_slice(s.as_bytes());
    o
}

const FLT: &str = "<flt><aid>TST123</aid><dap>QQB</dap><apt>QQA</apt><fps>ACTIVE</fps>\
    <acs>TRACKED</acs><typ>T123/L</typ><eng>JET</eng><bcn>0777</bcn><spd>250.5</spd><ara>12345</ara>\
    <ina>11000</ina><trw>30</trw><drw>QQB_09L</drw><tds>ZTT</tds><cfx>FIXA1/12</cfx>\
    <ctm>2026-11-14T14:30:00Z</ctm><etd>2026-11-14T14:00:00Z</etd><std/>\
    <a10>QQB./.FIXAA..FIXAB.QQA/0400</a10><tcr>FIXAA/1430 FIXAB/1445</tcr><est>FAA</est><etm/></flt>";

const MRP: &str = "<mrp mrpType=\"NEW\"><tra>TRA</tra><mfx>MFXA1</mfx><gat>EAST_GATE</gat>\
    <dfx>DFXA1</dfx><sfx>SFXA1</sfx><oma/><ooa>OOA1</ooa><o3a/><o4a/><rwy>09L</rwy>\
    <cfg>CFG_09L_09R</cfg><cat>ARRIVAL</cat><scn>QQA_MFXA1_JETS</scn></mrp>";

const ETA: &str = "<eta><mfx>MFXA1</mfx><eta_dfx>2026-11-14T14:30:00Z</eta_dfx>\
    <eta_sfx>2026-11-14T14:30:00Z</eta_sfx><eta_mfx>2026-11-14T14:31:15Z</eta_mfx>\
    <eta_oma/><eta_ooa>2026-11-14T14:20:00Z</eta_ooa><eta_rwy>2026-11-14T14:45:30Z</eta_rwy></eta>";

const STA: &str = "<sta><mfx>MFXA1</mfx><sta_dfx>2026-11-14T14:32:00Z</sta_dfx>\
    <sta_sfx>2026-11-14T14:32:00Z</sta_sfx><sta_o3a>2026-11-14T14:10:00Z</sta_o3a>\
    <sta_rwy>2026-11-14T14:47:00Z</sta_rwy></sta>";

const SCH: &str =
    "<sch><mfx>MFXA1</mfx><man>NO</man><rfz>NO</rfz><sfz>YES</sfz><sus>NO</sus></sch>";

// REQ: OA-TBFM-001
#[test]
fn iso_times_parse_to_unix_ms() {
    assert_eq!(parse_iso_ms(b"1970-01-01T00:00:00Z"), Ok(0));
    assert_eq!(
        parse_iso_ms(b"2026-11-14T14:11:42.420Z"),
        Ok(1_794_665_502_420)
    );
    assert_eq!(parse_iso_ms(b"2026-11-14T14:11:42Z"), Ok(1_794_665_502_000));
    assert_eq!(
        parse_iso_ms(b"2026-11-14T14:11:42.4Z"),
        Ok(1_794_665_502_400)
    );
    assert_eq!(
        parse_iso_ms(b"2026-11-14T14:11:42.42Z"),
        Ok(1_794_665_502_420)
    );
    assert_eq!(parse_iso_ms(b"2024-02-29T23:59:59Z"), Ok(1_709_251_199_000));
    assert_eq!(
        parse_iso_ms(b"9999-12-31T23:59:59.999Z"),
        Ok(253_402_300_799_999)
    );
    // Century rules: 2000 is a leap year, 2100 is not.
    assert!(parse_iso_ms(b"2000-02-29T00:00:00Z").is_ok());
    assert_eq!(parse_iso_ms(b"2100-02-29T00:00:00Z"), Err(TimeError::Range));
    assert_eq!(t(14, 30, 0), 1_794_666_600_000);
    assert_eq!(parse_iso_ms(b"2026-11-14T14:30:00Z"), Ok(t(14, 30, 0)));
}

// REQ: OA-TBFM-002
#[test]
fn iso_times_refuse_anything_else() {
    let shape = [
        &b""[..],
        b"2026-11-14",
        b"2026-11-14T14:11:42",
        b"2026-11-14T14:11:42z",
        b"2026-11-14 14:11:42Z",
        b"2026-11-14T14:11:42+00:00",
        b"2026-11-14T14:11:42.Z",
        b"20261114T141142Z",
        b"2026-11-14T14:11:42ZZ",
        b" 2026-11-14T14:11:42Z",
        b"2026-11-14T14:11:42Z ",
        b"2026/11/14T14:11:42Z",
    ];
    for s in shape {
        assert_eq!(
            parse_iso_ms(s),
            Err(TimeError::Shape),
            "{:?}",
            String::from_utf8_lossy(s)
        );
    }
    assert_eq!(
        parse_iso_ms(b"2026-11-14T14:11:42.4200Z"),
        Err(TimeError::Fraction)
    );
    assert_eq!(parse_iso_ms(b"2026-1x-14T14:11:42Z"), Err(TimeError::Digit));
    assert_eq!(
        parse_iso_ms(b"2026-11-14T14:11:4\xffZ"),
        Err(TimeError::Digit)
    );
    for s in [
        &b"2026-00-14T14:11:42Z"[..],
        b"2026-13-14T14:11:42Z",
        b"2026-11-00T14:11:42Z",
        b"2026-11-31T14:11:42Z",
        b"2026-02-29T14:11:42Z",
        b"2026-11-14T24:11:42Z",
        b"2026-11-14T14:60:42Z",
        b"2026-11-14T14:11:60Z",
        b"1969-12-31T23:59:59Z",
    ] {
        assert_eq!(
            parse_iso_ms(s),
            Err(TimeError::Range),
            "{:?}",
            String::from_utf8_lossy(s)
        );
    }
}

// REQ: OA-TBFM-003
#[test]
fn rows_round_trip_through_the_columns() {
    let mut rows = new_rows(2, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let mut a = BLANK_ROW;
    a.section = section::ETA;
    a.eta_ms[pt::MFX] = 77;
    a.sta_ms[pt::RWY] = 88;
    a.aid = n("TST1");
    a.cfx = n("FIXA1/12");
    a.bcn = 0o7777;
    a.sch = 2 << (sch_flag::SFZ * 2);
    assert!(push_row(&mut rows, &a));
    assert!(push_row(&mut rows, &BLANK_ROW));
    assert!(!push_row(&mut rows, &a), "a third row must not fit");
    assert_eq!(rows_len(&rows), 2);
    assert_eq!(row_at(&rows, 0), Some(a));
    assert_eq!(row_at(&rows, 1), Some(BLANK_ROW));
    assert_eq!(row_at(&rows, 2), None);
    assert_eq!(name_str(&rows.aid[0]), "TST1");
    assert_eq!(name_str(&rows.aid[1]), "");
    assert_eq!(sch_get(rows.sch[0], sch_flag::SFZ), sch_flag::YES);
    truncate_rows(&mut rows, 1);
    assert_eq!(rows_len(&rows), 1);
    clear_rows(&mut rows);
    assert_eq!(rows_len(&rows), 0);
    assert_eq!(BLANK_ROW.bcn, NO_BCN);
    assert_eq!(BLANK_ROW.spd_milli, NO_NUM);
    assert_eq!(BLANK_ROW.env_ms, NO_TIME);
    // The size guard is in the definition (a compile-time assert); this states the figure.
    assert_eq!(std::mem::size_of::<oa_tbfm::Row>(), 488);
}

// REQ: OA-TBFM-004
#[test]
fn a_flight_section_becomes_one_row() {
    let (o, rows) = ok(&air(FLT));
    assert_eq!(o.kind, MsgKind::Air);
    assert_eq!(o.rows, 1);
    assert_eq!(o.unknown_paths, 0);
    // a10, tcr, est and etm are known but not stored.
    assert_eq!(o.ignored_paths, 4);
    assert_eq!(name_str(&o.center), "ZTT");
    assert_eq!(o.env_ms, 1_794_665_502_420);
    let r = row_at(&rows, 0).unwrap();
    assert_eq!(r.section, section::FLT);
    assert_eq!(r.air_type, change::AMD);
    assert_eq!(r.sect_type, change::NONE);
    assert_eq!(name_str(&r.center), "ZTT");
    assert_eq!(r.env_ms, 1_794_665_502_420);
    assert_eq!(r.msg_ms, 1_794_665_502_420);
    assert_eq!(r.msg_id, 4711);
    assert_eq!(name_str(&r.tma_id), "T00001");
    assert_eq!(name_str(&r.gufi), "KQ000001Xa");
    assert_eq!(name_str(&r.cid), "123");
    assert_eq!(name_str(&r.aid), "TST123");
    assert_eq!(name_str(&r.apt), "QQA");
    assert_eq!(name_str(&r.dap), "QQB");
    assert_eq!(name_str(&r.fps), "ACTIVE");
    assert_eq!(name_str(&r.acs), "TRACKED");
    assert_eq!(name_str(&r.typ), "T123/L");
    assert_eq!(name_str(&r.eng), "JET");
    assert_eq!(r.bcn, 0o777);
    assert_eq!(r.spd_milli, 250_500);
    assert_eq!(name_str(&r.ara), "12345");
    assert_eq!(r.ina, 11_000);
    assert_eq!(name_str(&r.trw), "30");
    assert_eq!(name_str(&r.drw), "QQB_09L");
    assert_eq!(name_str(&r.tds), "ZTT");
    assert_eq!(name_str(&r.cfx), "FIXA1/12");
    assert_eq!(r.ctm_ms, t(14, 30, 0));
    assert_eq!(r.etd_ms, t(14, 0, 0));
    assert_eq!(r.std_ms, NO_TIME, "an empty element has no value");
    assert!(
        is_cleared(r.cleared, Field::Std),
        "but it is recorded as sent empty"
    );
    assert!(!is_cleared(r.cleared, Field::Etd));
    assert_eq!(r.eta_ms, [NO_TIME; 8]);
    assert_eq!(r.sch, 0);
    assert_eq!(r.mfx, [0; 8]);
}

// REQ: OA-TBFM-004
#[test]
fn flt_repeats_of_air_attributes_must_agree_or_fill_gaps() {
    // `air` without dap: the flt copy fills the gap.
    let xml = env(
        "",
        "<air tmaId=\"T00001\" apt=\"QQA\" aid=\"TST123\" airType=\"NEW\">\
         <flt><aid>TST123</aid><dap>QQB</dap><apt>QQA</apt></flt></air>",
    );
    let (_, rows) = ok(&xml);
    assert_eq!(name_str(&rows.dap[0]), "QQB");
    assert_eq!(rows.gufi[0], [0; 12], "gufi is optional");
    assert_eq!(rows.cid[0], [0; 4], "cid is optional");
    // Disagreement is refused.
    for (tag, bad) in [("aid", "TST999"), ("apt", "QQZ"), ("dap", "QQZ")] {
        let xml = air(&format!("<flt><{tag}>{bad}</{tag}></flt>"));
        assert!(matches!(
            err(&xml),
            DocError::Field(FieldError {
                kind: FieldErrorKind::Inconsistent,
                ..
            })
        ));
    }
    // No dap on either side is fine (a departure with no origin airport).
    let xml = env(
        "",
        "<air tmaId=\"T00001\" aid=\"TST123\" airType=\"DEL\"><flt><aid>TST123</aid></flt></air>",
    );
    let (_, rows) = ok(&xml);
    assert_eq!(rows.air_type[0], change::DEL);
    assert_eq!(rows.apt[0], [0; 4]);
}

// REQ: OA-TBFM-005
#[test]
fn a_meter_route_section_becomes_one_row() {
    let (o, rows) = ok(&air(MRP));
    assert_eq!((o.rows, o.unknown_paths), (1, 0));
    let r = row_at(&rows, 0).unwrap();
    assert_eq!(r.section, section::MRP);
    assert_eq!(r.air_type, change::AMD);
    assert_eq!(r.sect_type, change::NEW, "mrpType is its own column");
    assert_eq!(name_str(&r.tra), "TRA");
    assert_eq!(name_str(&r.mfx), "MFXA1");
    assert_eq!(name_str(&r.gat), "EAST_GATE");
    assert_eq!(name_str(&r.dfx), "DFXA1");
    assert_eq!(name_str(&r.sfx), "SFXA1");
    assert_eq!(name_str(&r.oma), "");
    assert!(is_cleared(r.cleared, Field::Oma) && is_cleared(r.cleared, Field::O3a));
    assert!(is_cleared(r.cleared, Field::O4a) && !is_cleared(r.cleared, Field::Ooa));
    assert_eq!(name_str(&r.ooa), "OOA1");
    assert_eq!(name_str(&r.o3a), "");
    assert_eq!(name_str(&r.o4a), "");
    assert_eq!(name_str(&r.rwy), "09L");
    assert_eq!(name_str(&r.cfg), "CFG_09L_09R");
    assert_eq!(name_str(&r.cat), "ARRIVAL");
    assert_eq!(name_str(&r.scn), "QQA_MFXA1_JETS");
    // An mrp without mrpType has change type 0.
    let (_, rows) = ok(&air("<mrp><mfx>MFXA1</mfx></mrp>"));
    assert_eq!(rows.sect_type[0], change::NONE);
}

// REQ: OA-TBFM-006
#[test]
fn estimated_and_scheduled_times_are_filed_by_place() {
    let (o, rows) = ok(&air(&format!("{ETA}{STA}")));
    assert_eq!((o.rows, o.unknown_paths), (2, 0));
    let e = row_at(&rows, 0).unwrap();
    assert_eq!(e.section, section::ETA);
    assert_eq!(name_str(&e.mfx), "MFXA1");
    assert_eq!(e.eta_ms[pt::DFX], t(14, 30, 0));
    assert_eq!(e.eta_ms[pt::SFX], t(14, 30, 0));
    assert_eq!(e.eta_ms[pt::MFX], t(14, 31, 15));
    assert_eq!(e.eta_ms[pt::OMA], NO_TIME);
    assert!(is_cleared(e.cleared, Field::EtaOma), "sent empty");
    assert!(!is_cleared(e.cleared, Field::EtaO3a), "not sent at all");
    assert_eq!(e.eta_ms[pt::OOA], t(14, 20, 0));
    assert_eq!(e.eta_ms[pt::O3A], NO_TIME);
    assert_eq!(e.eta_ms[pt::RWY], t(14, 45, 30));
    assert_eq!(e.sta_ms, [NO_TIME; 8]);
    let s = row_at(&rows, 1).unwrap();
    assert_eq!(s.section, section::STA);
    assert_eq!(s.sta_ms[pt::DFX], t(14, 32, 0));
    assert_eq!(s.sta_ms[pt::SFX], t(14, 32, 0));
    assert_eq!(s.sta_ms[pt::MFX], NO_TIME, "the feed has no sta_mfx");
    assert_eq!(s.sta_ms[pt::O3A], t(14, 10, 0));
    assert_eq!(s.sta_ms[pt::RWY], t(14, 47, 0));
    assert_eq!(s.eta_ms, [NO_TIME; 8]);
    // Several eta sections in one air (several meter points) are several rows.
    let (o, rows) = ok(&air(&format!("{ETA}{ETA}{ETA}")));
    assert_eq!(o.rows, 3);
    assert_eq!(rows.section, vec![section::ETA; 3]);
}

// REQ: OA-TBFM-007
#[test]
fn schedule_flags_are_tri_state() {
    let (_, rows) = ok(&air(SCH));
    assert_eq!(rows.section[0], section::SCH);
    let s = rows.sch[0];
    assert_eq!(sch_get(s, sch_flag::MAN), sch_flag::NO);
    assert_eq!(sch_get(s, sch_flag::RFZ), sch_flag::NO);
    assert_eq!(sch_get(s, sch_flag::SFZ), sch_flag::YES);
    assert_eq!(sch_get(s, sch_flag::SUS), sch_flag::NO);
    let (_, rows) = ok(&air("<sch><mfx>MFXA1</mfx><sfz>YES</sfz></sch>"));
    assert_eq!(sch_get(rows.sch[0], sch_flag::SFZ), sch_flag::YES);
    assert_eq!(sch_get(rows.sch[0], sch_flag::MAN), sch_flag::ABSENT);
}

// REQ: OA-TBFM-008
#[test]
fn messages_are_classified_and_non_flight_kinds_make_no_rows() {
    let mis = |child: &str| {
        format!(
            "<mis xmlns=\"{NS}\" misTime=\"2026-11-14T14:11:42Z\" misSrce=\"TMA.ZTT.FAA.GOV\">{child}</mis>"
        )
    };
    let cases = [
        (mis("<hb/>"), MsgKind::Heartbeat),
        (mis("<sync/>"), MsgKind::Sync),
        (mis(""), MsgKind::MisOther),
        (
            env("", "<adp><gans><gan><nam>X</nam></gan></gans></adp>"),
            MsgKind::Adaptation,
        ),
        (
            env("", "<con><aar aarType=\"NOW\"><apt>QQA</apt></aar></con>"),
            MsgKind::Control,
        ),
        (
            env("", "<oth><int><ifn>ZTT</ifn></int></oth>"),
            MsgKind::Other,
        ),
        (env("", ""), MsgKind::Empty),
        (air(FLT), MsgKind::Air),
    ];
    let mut stats = new_stats();
    for (xml, kind) in &cases {
        let (r, rows) = run(xml, 8);
        let o = r.unwrap();
        assert_eq!(o.kind, *kind, "{xml}");
        assert_eq!(name_str(&o.center), "ZTT");
        assert_eq!(rows_len(&rows) as u32, o.rows);
        if *kind != MsgKind::Air {
            assert_eq!(o.rows, 0);
        }
        assert_eq!(o.unknown_paths, 0, "{xml}");
        observe(&mut stats, &Ok(o), &rows.section);
    }
    assert_eq!(stats.messages, 8);
    assert_eq!(stats.accepted, 8);
    assert_eq!(stats.rows, 1);
    assert_eq!(stats.kinds[MsgKind::Heartbeat as usize], 1);
    assert_eq!(stats.kinds[MsgKind::Air as usize], 1);
    assert_eq!(stats.sections[section::FLT as usize], 1);
    // misTime has no milliseconds in the feed and is parsed like any time.
    let (r, _) = run(&mis("<hb/>"), 1);
    assert_eq!(r.unwrap().env_ms, t(14, 11, 42));
}

// REQ: OA-TBFM-009
#[test]
fn a_refused_flight_yields_no_rows_at_all() {
    // The first section is fine, the second is bad: nothing from the first survives.
    let xml = air(&format!("{FLT}{ETA}<sta><sta_rwy>garbage</sta_rwy></sta>"));
    let (r, rows) = run(&xml, 16);
    let o = r.unwrap();
    assert_eq!((o.rows, o.rejected), (0, 1));
    assert_eq!(
        o.first_reject,
        Some(FieldError {
            field: Field::StaRwy,
            kind: FieldErrorKind::Time(oa_tbfm::TimeError::Shape),
            at: xml.find("sta_rwy>garbage").unwrap() as u32,
        })
    );
    assert_eq!(rows_len(&rows), 0);
    // Rows already held by the caller are left exactly as they were.
    let mut rows = new_rows(16, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let mut p = Parser::new(default_limits()).unwrap();
    decode_bytes(&mut p, air(FLT).as_bytes(), &mut rows).unwrap();
    let before = row_at(&rows, 0).unwrap();
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    assert_eq!(r.unwrap().rejected, 1);
    assert_eq!(rows_len(&rows), 1);
    assert_eq!(row_at(&rows, 0), Some(before));
    // Every column has the same length after a refusal.
    assert_eq!(rows.eta_ms[3].len(), 1);
    assert_eq!(rows.cfx.len(), 1);
}

// REQ: OA-TBFM-009
#[test]
fn wrong_roots_namespaces_and_xml_are_refused() {
    let wrong_ns = air(FLT).replace(NS, "urn:example:other");
    assert!(matches!(err(&wrong_ns), DocError::NotTbfm { .. }));
    let no_ns = air(FLT).replace(&format!(" xmlns=\"{NS}\""), "");
    assert!(matches!(err(&no_ns), DocError::NotTbfm { .. }));
    let other_root = format!("<tma xmlns=\"{NS}\"/>");
    assert!(matches!(err(&other_root), DocError::NotTbfm { .. }));
    let old_version = air(FLT).replace(
        "tbfmmeteringpublication:1.1.0",
        "tbfmmeteringpublication:1.2.0",
    );
    assert!(matches!(err(&old_version), DocError::NotTbfm { .. }));
    for bad in ["", "not xml", "<env", "<env xmlns=\"x\"></tma>"] {
        assert!(matches!(err(bad), DocError::Xml(_)), "{bad}");
    }
    // A DOCTYPE is refused by the parser, never expanded.
    let doctype = format!("<!DOCTYPE env [<!ENTITY x \"y\">]><env xmlns=\"{NS}\"/>");
    assert!(matches!(err(&doctype), DocError::Xml(_)));
    // The message is refused before the rows are touched when decoding a document directly.
    let mut p = Parser::new(default_limits()).unwrap();
    let mut rows = new_rows(4, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let bytes = wrong_ns.into_bytes();
    let doc = p.parse(&bytes).unwrap();
    assert!(decode_doc(&doc, &mut rows).is_err());
}

/// One refused-field fixture: the message and the field and kind it must be refused with.
fn bad_case(f: Field) -> (String, FieldErrorKind) {
    use FieldErrorKind as K;
    let long = |k: usize| "X".repeat(k);
    let time = K::Time(TimeError::Range);
    let bad_time = "2026-13-01T00:00:00Z";
    let leaf =
        |wrap: &str, tag: &str, v: &str| air(&format!("<{wrap}><{tag}>{v}</{tag}></{wrap}>"));
    // An `air` whose attribute `name` is set to `v` (the others are valid).
    let attr = |name: &str, v: &str| {
        let mut a = vec![("tmaId", "T00001"), ("aid", "TST123"), ("airType", "AMD")];
        match a.iter_mut().find(|(k, _)| *k == name) {
            Some(e) => e.1 = v,
            None => a.push((name, v)),
        }
        let a: Vec<String> = a.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
        env("", &format!("<air {}><flt/></air>", a.join(" ")))
    };
    match f {
        Field::EnvSrce => (air(FLT).replace("TMA.ZTT.FAA.GOV", "ZTT"), K::Format),
        Field::EnvTime => (
            air(FLT).replace("envTime=\"2026-11-14T14:11:42.420Z\"", "envTime=\"x\""),
            time_shape(),
        ),
        Field::MsgTime => (
            air(FLT).replace(
                "msgTime=\"2026-11-14T14:11:42.420Z\"",
                "msgTime=\"2026-11-14T14:11:42+01:00\"",
            ),
            time_shape(),
        ),
        Field::MsgId => (
            air(FLT).replace("msgId=\"4711\"", "msgId=\"47x\""),
            K::Format,
        ),
        Field::TmaId => (attr("tmaId", &long(9)), K::TooLong),
        Field::Gufi => (attr("gufi", &long(13)), K::TooLong),
        Field::Cid => (attr("cid", &long(5)), K::TooLong),
        Field::Aid => (attr("aid", &long(9)), K::TooLong),
        Field::AirType => (attr("airType", "XYZ"), K::Vocabulary),
        Field::Apt => (attr("apt", &long(5)), K::TooLong),
        Field::Dap => (attr("dap", &long(5)), K::TooLong),
        Field::MrpType => (air("<mrp mrpType=\"XYZ\"/>"), K::Vocabulary),
        Field::Tra => (leaf("mrp", "tra", &long(5)), K::TooLong),
        Field::Mfx => (leaf("eta", "mfx", &long(9)), K::TooLong),
        Field::Dfx => (leaf("mrp", "dfx", &long(9)), K::TooLong),
        Field::Sfx => (leaf("mrp", "sfx", &long(9)), K::TooLong),
        Field::Oma => (leaf("mrp", "oma", &long(13)), K::TooLong),
        Field::Ooa => (leaf("mrp", "ooa", &long(13)), K::TooLong),
        Field::O3a => (leaf("mrp", "o3a", &long(13)), K::TooLong),
        Field::O4a => (leaf("mrp", "o4a", &long(13)), K::TooLong),
        Field::Gat => (leaf("mrp", "gat", &long(17)), K::TooLong),
        Field::Rwy => (leaf("mrp", "rwy", &long(5)), K::TooLong),
        Field::Cfg => (leaf("mrp", "cfg", &long(21)), K::TooLong),
        Field::Cat => (leaf("mrp", "cat", &long(25)), K::TooLong),
        Field::Scn => (leaf("mrp", "scn", &long(21)), K::TooLong),
        Field::Man => (leaf("sch", "man", "MAYBE"), K::Vocabulary),
        Field::Rfz => (leaf("sch", "rfz", "no"), K::Vocabulary),
        Field::Sfz => (leaf("sch", "sfz", "Y"), K::Vocabulary),
        Field::Sus => (leaf("sch", "sus", "TRUE"), K::Vocabulary),
        Field::Fps => (leaf("flt", "fps", &long(13)), K::TooLong),
        Field::Acs => (leaf("flt", "acs", &long(13)), K::TooLong),
        Field::Typ => (leaf("flt", "typ", &long(9)), K::TooLong),
        Field::Eng => (leaf("flt", "eng", &long(13)), K::TooLong),
        Field::Bcn => (leaf("flt", "bcn", "1289"), K::Format),
        Field::Spd => (leaf("flt", "spd", "1.2345"), K::Format),
        Field::Ara => (leaf("flt", "ara", &long(9)), K::TooLong),
        Field::Ina => (leaf("flt", "ina", "12x"), K::Format),
        Field::Trw => (leaf("flt", "trw", &long(5)), K::TooLong),
        Field::Drw => (leaf("flt", "drw", &long(9)), K::TooLong),
        Field::Tds => (leaf("flt", "tds", &long(5)), K::TooLong),
        Field::Cfx => (leaf("flt", "cfx", &long(15)), K::TooLong),
        Field::Ctm => (leaf("flt", "ctm", bad_time), time),
        Field::Etd => (leaf("flt", "etd", bad_time), time),
        Field::Std => (leaf("flt", "std", bad_time), time),
        Field::EtaDfx => (leaf("eta", "eta_dfx", bad_time), time),
        Field::EtaSfx => (leaf("eta", "eta_sfx", bad_time), time),
        Field::EtaMfx => (leaf("eta", "eta_mfx", bad_time), time),
        Field::EtaOma => (leaf("eta", "eta_oma", bad_time), time),
        Field::EtaOoa => (leaf("eta", "eta_ooa", bad_time), time),
        Field::EtaO3a => (leaf("eta", "eta_o3a", bad_time), time),
        Field::EtaO4a => (leaf("eta", "eta_o4a", bad_time), time),
        Field::EtaRwy => (leaf("eta", "eta_rwy", bad_time), time),
        Field::StaDfx => (leaf("sta", "sta_dfx", bad_time), time),
        Field::StaSfx => (leaf("sta", "sta_sfx", bad_time), time),
        Field::StaMfx => (leaf("sta", "sta_mfx", bad_time), time),
        Field::StaOma => (leaf("sta", "sta_oma", bad_time), time),
        Field::StaOoa => (leaf("sta", "sta_ooa", bad_time), time),
        Field::StaO3a => (leaf("sta", "sta_o3a", bad_time), time),
        Field::StaO4a => (leaf("sta", "sta_o4a", bad_time), time),
        Field::StaRwy => (leaf("sta", "sta_rwy", bad_time), time),
    }
}

fn time_shape() -> FieldErrorKind {
    FieldErrorKind::Time(TimeError::Shape)
}

const ALL_FIELDS: [Field; FIELD_COUNT] = [
    Field::EnvSrce,
    Field::EnvTime,
    Field::MsgTime,
    Field::MsgId,
    Field::TmaId,
    Field::Gufi,
    Field::Cid,
    Field::Aid,
    Field::AirType,
    Field::Apt,
    Field::Dap,
    Field::MrpType,
    Field::Tra,
    Field::Mfx,
    Field::Dfx,
    Field::Sfx,
    Field::Oma,
    Field::Ooa,
    Field::O3a,
    Field::O4a,
    Field::Gat,
    Field::Rwy,
    Field::Cfg,
    Field::Cat,
    Field::Scn,
    Field::Man,
    Field::Rfz,
    Field::Sfz,
    Field::Sus,
    Field::Fps,
    Field::Acs,
    Field::Typ,
    Field::Eng,
    Field::Bcn,
    Field::Spd,
    Field::Ara,
    Field::Ina,
    Field::Trw,
    Field::Drw,
    Field::Tds,
    Field::Cfx,
    Field::Ctm,
    Field::Etd,
    Field::Std,
    Field::EtaDfx,
    Field::EtaSfx,
    Field::EtaMfx,
    Field::EtaOma,
    Field::EtaOoa,
    Field::EtaO3a,
    Field::EtaO4a,
    Field::EtaRwy,
    Field::StaDfx,
    Field::StaSfx,
    Field::StaMfx,
    Field::StaOma,
    Field::StaOoa,
    Field::StaO3a,
    Field::StaO4a,
    Field::StaRwy,
];

// REQ: OA-TBFM-010
#[test]
fn every_field_has_a_refusal_with_its_own_error() {
    let mut covered = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (i, f) in ALL_FIELDS.iter().enumerate() {
        assert_eq!(*f as usize, i, "ALL_FIELDS must be in discriminant order");
        names.insert(field_name(*f));
        if *f == Field::StaMfx {
            // There is no sta_mfx in the feed: it is an unknown element, not a field.
            let (xml, _) = bad_case(*f);
            let (o, rows) = ok(&xml);
            assert_eq!(o.unknown_paths, 1);
            assert_eq!(rows.sta_ms[pt::MFX][0], NO_TIME);
            continue;
        }
        let (xml, kind) = bad_case(*f);
        if matches!(*f, Field::Man | Field::Rfz | Field::Sfz | Field::Sus) {
            // A schedule flag with another word is kept as unknown, not refused.
            let (o, rows) = ok(&xml);
            assert_eq!((o.rows, o.rejected, o.unknown_words), (1, 0, 1));
            let flag = match *f {
                Field::Man => sch_flag::MAN,
                Field::Rfz => sch_flag::RFZ,
                Field::Sfz => sch_flag::SFZ,
                _ => sch_flag::SUS,
            };
            assert_eq!(sch_get(rows.sch[0], flag), sch_flag::UNKNOWN);
            continue;
        }
        match err(&xml) {
            DocError::Field(e) => {
                assert_eq!((e.field, e.kind), (*f, kind), "{}", field_name(*f));
                assert!((e.at as usize) < xml.len());
            }
            other => panic!("{}: {other:?}", field_name(*f)),
        }
        covered.insert(i);
    }
    assert_eq!(covered.len(), FIELD_COUNT - 1 - 4);
    assert_eq!(names.len(), FIELD_COUNT, "field names are distinct");
}

// REQ: OA-TBFM-010
#[test]
fn required_attributes_and_other_field_error_kinds() {
    use FieldErrorKind as K;
    let field_err = |xml: &str| match err(xml) {
        DocError::Field(e) => (e.field, e.kind),
        other => panic!("{other:?}"),
    };
    let strip = |xml: String, what: &str| xml.replace(what, "");
    for (what, f) in [
        (" envSrce=\"TMA.ZTT.FAA.GOV\"", Field::EnvSrce),
        (" envTime=\"2026-11-14T14:11:42.420Z\"", Field::EnvTime),
        (" msgTime=\"2026-11-14T14:11:42.420Z\"", Field::MsgTime),
        (" msgId=\"4711\"", Field::MsgId),
        (" tmaId=\"T00001\"", Field::TmaId),
        (" aid=\"TST123\"", Field::Aid),
        (" airType=\"AMD\"", Field::AirType),
    ] {
        assert_eq!(field_err(&strip(air(FLT), what)), (f, K::Missing), "{what}");
    }
    // Duplicates: an attribute twice is an XML error (the parser refuses it); an element twice
    // is a field error.
    assert!(matches!(
        err(&air(FLT).replace("cid=\"123\"", "cid=\"123\" cid=\"124\"")),
        DocError::Xml(_)
    ));
    assert_eq!(
        field_err(&air("<flt><fps>A</fps><fps>B</fps></flt>")),
        (Field::Fps, K::Duplicate)
    );
    assert_eq!(
        field_err(&air(
            "<eta><eta_mfx/><eta_mfx>2026-11-14T14:30:00Z</eta_mfx></eta>"
        )),
        (Field::EtaMfx, K::Duplicate),
        "an empty first copy still counts"
    );
    assert_eq!(
        field_err(&air("<mrp mrpType=\"NEW\" ><tra>A</tra><tra>B</tra></mrp>")),
        (Field::Tra, K::Duplicate)
    );
    // Bad characters: a space, a control character, a non-ASCII byte.
    assert_eq!(
        field_err(&air("<flt><typ>A B</typ></flt>")),
        (Field::Typ, K::BadChar)
    );
    assert_eq!(
        field_err(&air("<flt><typ>A\u{e9}</typ></flt>")),
        (Field::Typ, K::BadChar)
    );
    assert_eq!(
        field_err(&air("<flt><typ>A&#9;B</typ></flt>")),
        (Field::Typ, K::BadChar)
    );
    // Not a leaf.
    assert_eq!(
        field_err(&air("<flt><fps><x/></fps></flt>")),
        (Field::Fps, K::NotLeaf)
    );
    // Numbers: signs, spaces, empty fraction, too many digits, overflow.
    for (tag, v) in [
        ("bcn", "777"),
        ("bcn", "07777"),
        ("bcn", "-123"),
        ("spd", "250."),
        ("spd", ".5"),
        ("spd", "-1"),
        ("spd", "123456"),
        ("spd", "1.5.5"),
        ("ina", "-1"),
        ("ina", "1234567"),
    ] {
        assert_eq!(
            field_err(&air(&format!("<flt><{tag}>{v}</{tag}></flt>"))).1,
            K::Format,
            "{tag} {v}"
        );
    }
    assert_eq!(
        field_err(&air(
            &FLT.replace("<bcn>0777</bcn>", "<bcn>0777</bcn><bcn>0777</bcn>")
        )),
        (Field::Bcn, K::Duplicate)
    );
    // msgId: digits only, at most ten.
    for v in ["", "-1", "12345678901", "1 2", "1.5"] {
        let xml = air(FLT).replace("msgId=\"4711\"", &format!("msgId=\"{v}\""));
        assert_eq!(field_err(&xml), (Field::MsgId, K::Format), "{v:?}");
    }
    for (v, want) in [
        ("4294967295", u64::from(u32::MAX)),
        ("4294967296", 4_294_967_296),
        ("9999999999", 9_999_999_999),
    ] {
        let xml = air(FLT).replace("msgId=\"4711\"", &format!("msgId=\"{v}\""));
        assert_eq!(ok(&xml).1.msg_id[0], want);
    }
    // envSrce shapes.
    for v in [
        "TMA.Z.FAA.GOV",
        "TMA.zau.FAA.GOV",
        "TMA.ZAUXX.FAA.GOV",
        "TMA.ZAU.FAA",
        "ZAU",
        "",
    ] {
        let xml = air(FLT).replace("TMA.ZTT.FAA.GOV", v);
        assert_eq!(field_err(&xml), (Field::EnvSrce, K::Format), "{v:?}");
    }
    // Numbers at their limits are accepted.
    let (_, rows) = ok(&air(
        "<flt><spd>0.85</spd><bcn>7777</bcn><ina>999999</ina></flt>",
    ));
    assert_eq!(
        (rows.spd_milli[0], rows.bcn[0], rows.ina[0]),
        (850, 0o7777, 999_999)
    );
    let (_, rows) =
        ok(&air("<flt><spd>99999.999</spd><bcn>0000</bcn><spd/></flt>").replace("<spd/>", ""));
    assert_eq!((rows.spd_milli[0], rows.bcn[0]), (99_999_999, 0));
}

// REQ: OA-TBFM-011
#[test]
fn capacity_is_a_reported_state() {
    let xml = air(&format!("{FLT}{MRP}{ETA}"));
    for cap in 0..3 {
        let (r, rows) = run(&xml, cap);
        assert_eq!(r, Err(DocError::Capacity), "cap {cap}");
        assert_eq!(rows_len(&rows), 0);
        assert_eq!(rows.cap, cap);
    }
    let (r, rows) = run(&xml, 3);
    assert_eq!(r.unwrap().rows, 3);
    assert_eq!(rows_len(&rows), 3);
    // Rows the caller already holds count against the capacity, and survive the overrun.
    let mut rows = new_rows(3, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let mut p = Parser::new(default_limits()).unwrap();
    decode_bytes(&mut p, air(FLT).as_bytes(), &mut rows).unwrap();
    assert_eq!(
        decode_bytes(&mut p, xml.as_bytes(), &mut rows),
        Err(DocError::Capacity)
    );
    assert_eq!(rows_len(&rows), 1);
    clear_rows(&mut rows);
    assert_eq!(
        decode_bytes(&mut p, xml.as_bytes(), &mut rows)
            .unwrap()
            .rows,
        3
    );
    // A message with no rows needs no capacity.
    let (r, _) = run(&env("", "<con/>"), 0);
    assert_eq!(r.unwrap().rows, 0);
}

// REQ: OA-TBFM-012
#[test]
fn unknown_paths_are_counted_not_refused() {
    // An unknown element at each level of the tree.
    let cases = [
        air(&format!("{FLT}<newsection><x/></newsection>")),
        air("<flt><aid>TST123</aid><newfield>1</newfield></flt>"),
        air("<mrp><newfield/></mrp>"),
        air("<eta><newfield/></eta>"),
        air("<sta><sta_mfx>2026-11-14T14:30:00Z</sta_mfx></sta>"),
        air("<sch><newflag>YES</newflag></sch>"),
        env("", "<newbody/>"),
        env(" extra=\"1\"", "<con/>"),
        air("<flt/>").replace("<flt/>", "<flt other=\"1\"/>"),
        air("<mrp other=\"1\" mrpType=\"NEW\"/>"),
        air("<flt/>").replace(AIR_ATTRS, &format!("{AIR_ATTRS} newattr=\"1\"")),
        air("<flt/>").replace("envTime", "newenv=\"1\" envTime"),
        air("<flt/>").replace("</env>", "<other/></env>"),
    ];
    for xml in &cases {
        let (o, _) = ok(xml);
        assert!(o.unknown_paths >= 1, "{xml}");
    }
    // A whole unknown subtree counts once.
    let (o, _) = ok(&air("<other><a><b><c/></b></a><d/></other>"));
    assert_eq!(o.unknown_paths, 1);
    // The rows of a message with unknown parts are unchanged by them.
    let (_, plain) = ok(&air(FLT));
    let (o, noisy) = ok(&air(&FLT.replace("</flt>", "<newfield/></flt>")));
    assert_eq!(o.unknown_paths, 1);
    assert_eq!(row_at(&plain, 0), row_at(&noisy, 0));
    // An element in another namespace is unknown, even with a known local name.
    let xml = air("<flt xmlns=\"urn:example:other\"><aid>TST123</aid></flt>");
    let (o, rows) = ok(&xml);
    assert_eq!((o.unknown_paths, rows_len(&rows)), (1, 0));
    // A prefixed attribute is unknown; namespace declarations are not paths.
    let xml = air("<flt/>").replace("<tma ", "<tma xmlns:q=\"urn:example:q\" q:msgId=\"9\" ");
    let (o, _) = ok(&xml);
    assert_eq!(o.unknown_paths, 1);
    let (o, _) = ok(&air(FLT));
    assert_eq!(
        o.unknown_paths, 0,
        "xmlns and targetNamespace are not unknown"
    );
    // Statistics add them up over messages, only for accepted messages.
    let mut stats = new_stats();
    for xml in [
        air(&format!("{FLT}<x/>")),
        air(FLT),
        air("<flt><fps>A B</fps></flt>"),
    ] {
        let (r, rows) = run(&xml, 4);
        observe(&mut stats, &r, &rows.section);
    }
    // The message with a refused flight is accepted: it has no rows, and the refusal is counted.
    assert_eq!((stats.messages, stats.accepted), (3, 3));
    assert_eq!(stats.unknown_paths, 1);
    assert_eq!(stats.ignored_paths, 8);
    assert_eq!(stats.rejects, [0, 0, 0, 0]);
    assert_eq!(stats.rejected_flights, 1);
    assert_eq!(stats.field_errors[Field::Fps as usize], 1);
    assert_eq!(
        stats.reject_kinds[oa_tbfm::error_kind_index(FieldErrorKind::BadChar)],
        1
    );
}

// REQ: OA-TBFM-013
#[test]
fn mutated_messages_never_panic_and_never_leave_partial_rows() {
    let base = air(&format!("{FLT}{MRP}{ETA}{STA}{SCH}"));
    let mut p = Parser::new(default_limits()).unwrap();
    let mut rows = new_rows(16, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let bytes = base.as_bytes();
    let mut checked = 0usize;
    let mut accepted = 0usize;
    let mut run_one = |input: &[u8], rows: &mut Rows| {
        let before = rows_len(rows);
        let r = decode_bytes(&mut p, input, rows);
        let n = rows_len(rows);
        // All columns agree in length whatever happened.
        assert_eq!(rows.cfx.len(), n);
        assert_eq!(rows.eta_ms[7].len(), n);
        assert_eq!(rows.sta_ms[0].len(), n);
        match r {
            Ok(o) => {
                assert_eq!(n - before, o.rows as usize);
                accepted += 1;
                clear_rows(rows);
            }
            Err(_) => assert_eq!(n, before, "a refused message changed the rows"),
        }
        checked += 1;
    };
    let fills = [
        0u8, b'<', b'>', b'&', b'"', b'\'', b' ', b'9', b'Z', b'-', 0xff, b'/',
    ];
    for i in 0..bytes.len() {
        run_one(&bytes[..i], &mut rows);
        let mut v = bytes.to_vec();
        v.remove(i);
        run_one(&v, &mut rows);
        for &f in &fills {
            let mut v = bytes.to_vec();
            v[i] = f;
            run_one(&v, &mut rows);
        }
        let mut v = bytes.to_vec();
        v.insert(i, bytes[i]);
        run_one(&v, &mut rows);
    }
    // Seeded noise and a field-by-field swap of values for other fields' values.
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
        run_one(&v, &mut rows);
    }
    assert!(checked > 10_000);
    assert!(
        accepted > 0,
        "some mutations (whitespace-insensitive ones) must still decode"
    );
}

// REQ: OA-TBFM-014
#[test]
fn references_and_normalisation_are_decoded_before_validation() {
    // A character reference in an attribute and in text decodes to its character.
    let xml = air("<flt><typ>&#84;12&#51;</typ></flt>").replace("TST123", "TST&#49;23");
    let (_, rows) = ok(&xml);
    assert_eq!(name_str(&rows.aid[0]), "TST123");
    assert_eq!(name_str(&rows.typ[0]), "T123");
    // CDATA text is the text.
    let (_, rows) = ok(&air("<flt><typ><![CDATA[T9]]></typ></flt>"));
    assert_eq!(name_str(&rows.typ[0]), "T9");
    // A reference to a space is decoded and then refused as a name character.
    assert!(matches!(
        err(&air("<flt><typ>A&#32;B</typ></flt>")),
        DocError::Field(FieldError {
            field: Field::Typ,
            kind: FieldErrorKind::BadChar,
            ..
        })
    ));
    // Undefined entities and references that decode to more than the buffer are refused.
    assert!(matches!(
        err(&air("<flt><typ>&bogus;</typ></flt>")),
        DocError::Xml(_)
    ));
    let long = format!("<flt><typ>{}</typ></flt>", "&#65;".repeat(70));
    assert!(matches!(
        err(&air(&long)),
        DocError::Field(FieldError {
            field: Field::Typ,
            kind: FieldErrorKind::TooLong,
            ..
        })
    ));
    // A time written with references is the same time.
    let xml = air("<flt><ctm>2026-11-14T14:30:00&#90;</ctm></flt>");
    assert_eq!(ok(&xml).1.ctm_ms[0], t(14, 30, 0));
    // An attribute value with a tab is normalised to a space and then refused.
    assert!(matches!(
        err(&air(FLT).replace("cid=\"123\"", "cid=\"1\t2\"")),
        DocError::Field(FieldError {
            field: Field::Cid,
            kind: FieldErrorKind::BadChar,
            ..
        })
    ));
}

// REQ: OA-TBFM-015
#[test]
fn decoding_reuses_its_memory() {
    let mut rows = new_rows(8, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    let mut p = Parser::new(default_limits()).unwrap();
    let caps = |r: &Rows| {
        let mut c = vec![r.cfx.capacity(), r.gufi.capacity(), r.env_ms.capacity()];
        c.extend(r.eta_ms.iter().map(Vec::capacity));
        c.extend(r.sta_ms.iter().map(Vec::capacity));
        c
    };
    let parser_cap = p.table_capacity();
    let before = caps(&rows);
    assert!(before.iter().all(|&c| c >= 8));
    let big = air(&format!("{FLT}{MRP}{ETA}{STA}{SCH}"));
    let too_big = air(&format!("{FLT}{FLT}{FLT}{FLT}{FLT}{FLT}{FLT}{FLT}{FLT}"));
    for i in 0..200 {
        clear_rows(&mut rows);
        let xml = if i % 7 == 0 { &too_big } else { &big };
        let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
        assert_eq!(r.is_err(), i % 7 == 0);
    }
    assert_eq!(caps(&rows), before, "a column was reallocated");
    assert_eq!(p.table_capacity(), parser_cap);
}

// REQ: OA-TBFM-018
#[test]
fn cross_check_counts_resolved_and_unresolved_identifiers() {
    use oa_airspace::model::{empty_model, point_kind, Meta, StrIx};
    let meta = Meta {
        nasr_effective: 20261029,
        cifp_cycle: 2611,
        artcc: "ZTT".to_string(),
        buffer_m: 0,
        nasr_sha256: [0; 32],
        cifp_sha256: [0; 32],
    };
    let mut m = empty_model(meta);
    let mut s = |text: &str| {
        let ix = StrIx::new((m.strings.offsets.len() - 1) as u32);
        m.strings.text.push_str(text);
        m.strings.offsets.push(m.strings.text.len() as u32);
        ix
    };
    let fixa = s("MFXA1");
    let apt1 = s("KQQA");
    let apt1_alt = s("QQA");
    let apt2 = s("QQB");
    let thr = s("QQC");
    for (kind, ident, alt) in [
        (point_kind::FIX, fixa, StrIx::NONE),
        (point_kind::AIRPORT, apt1, apt1_alt),
        (point_kind::AIRPORT, apt2, StrIx::NONE),
        // A threshold named like an airport must not resolve an airport.
        (point_kind::THRESHOLD, thr, StrIx::NONE),
    ] {
        m.points.kind.push(kind);
        m.points.ident.push(ident);
        m.points.alt_ident.push(alt);
    }
    // Rows: two from the model's centre, one from another.
    let mk = |center: &str, mfx: &str, apt: &str, dap: &str| {
        let mut r = BLANK_ROW;
        r.section = section::ETA;
        r.center = n(center);
        r.mfx = n(mfx);
        r.apt = n(apt);
        r.dap = n(dap);
        r
    };
    let mut rows = new_rows(8, oa_sealed::DropWindow::empty_for_tests_and_fuzz());
    for r in [
        mk("ZTT", "MFXA1", "QQA", "QQB"),
        mk("ZTT", "NOFIX", "QQC", ""),
        mk("ZXX", "MFXA1", "QQA", "QQZ"),
        mk("ZXX", "", "", ""),
    ] {
        assert!(push_row(&mut rows, &r));
    }
    let x = cross_check(&m, &rows);
    let tl = |d, r, rr, rrr| oa_tbfm::Tally {
        distinct: d,
        resolved: r,
        rows: rr,
        rows_resolved: rrr,
    };
    assert_eq!(x.all.meter_fix, tl(2, 1, 3, 2));
    assert_eq!(x.own.meter_fix, tl(2, 1, 2, 1));
    assert_eq!(x.all.arrival_airport, tl(2, 1, 3, 2));
    assert_eq!(x.own.arrival_airport, tl(2, 1, 2, 1));
    assert_eq!(x.all.departure_airport, tl(2, 1, 2, 1));
    assert_eq!(x.own.departure_airport, tl(1, 1, 1, 1));
    assert_eq!(x.own.unresolved_fixes, vec!["NOFIX".to_string()]);
    assert_eq!(x.own.unresolved_arrival, vec!["QQC".to_string()]);
    assert_eq!(x.all.unresolved_departure, vec!["QQZ".to_string()]);
    assert!(oa_tbfm::format_cross_check(&x).contains("meter fix: 1/2 distinct resolve"));
    // No rows, no counts.
    let none = cross_check(
        &m,
        &new_rows(1, oa_sealed::DropWindow::empty_for_tests_and_fuzz()),
    );
    assert_eq!(none.all.meter_fix, oa_tbfm::Tally::default());
}

// ---- OA-TBFM-019 .. 023: flights refused alone, speed units, unknown words, labels, text ----

/// A message of several flights: `air` elements with their own identities.
fn airs(bodies: &[&str]) -> String {
    let body: String = bodies
        .iter()
        .enumerate()
        .map(|(i, b)| format!("<air tmaId=\"T{i:05}\" aid=\"TST{i:03}\" airType=\"AMD\">{b}</air>"))
        .collect();
    env("", &body)
}

// REQ: OA-TBFM-019
#[test]
fn a_refused_flight_leaves_its_sibling_flights() {
    use oa_tbfm::error_kind_index;
    let good = "<flt><spd>250.5</spd><fps>ACTIVE</fps></flt>";
    let dup = "<flt><fps>ACTIVE</fps><unseen/><spd>1.5</spd><spd>2.5</spd></flt><eta><eta_mfx>2026-11-14T14:30:00Z</eta_mfx></eta>";
    let bad_time = "<eta><eta_mfx>soon</eta_mfx></eta>";
    let (o, rows) = ok(&airs(&[good, dup, bad_time, good]));
    assert_eq!((o.kind, o.rows, o.rejected), (MsgKind::Air, 2, 2));
    assert_eq!(rows.aid, [n::<8>("TST000"), n::<8>("TST003")]);
    let first = o.first_reject.unwrap();
    assert_eq!(
        (first.field, first.kind),
        (Field::Spd, FieldErrorKind::Duplicate)
    );
    assert_eq!(o.reject_fields[Field::Spd as usize], 1);
    assert_eq!(o.reject_fields[Field::EtaMfx as usize], 1);
    assert_eq!(o.reject_fields.iter().sum::<u32>(), 2);
    assert_eq!(
        o.reject_kinds[error_kind_index(FieldErrorKind::Duplicate)],
        1
    );
    assert_eq!(o.reject_kinds.iter().sum::<u32>(), 2);
    // the path counters are back to what the good flights alone give
    let (clean, _) = ok(&airs(&[good, good]));
    assert_eq!(
        (o.unknown_paths, o.ignored_paths),
        (clean.unknown_paths, clean.ignored_paths)
    );
    assert_eq!(clean.rejected, 0);
    assert_eq!(clean.first_reject, None);
    // a flight without its identity is refused alone; so is a tma with a bad message number
    let nameless = env(
        "",
        &format!(
            "<air tmaId=\"T9\" airType=\"AMD\"><flt/></air>{}",
            air_one()
        ),
    );
    let (o, rows) = ok(&nameless);
    assert_eq!((o.rows, o.rejected), (1, 1));
    assert_eq!(rows_len(&rows), 1);
    let two_tma = format!(
        "<env xmlns=\"{NS}\" targetNamespace=\"{NS}\" envTime=\"2026-11-14T14:11:42.420Z\" envSrce=\"TMA.ZTT.FAA.GOV\">\
         <tma msgTime=\"2026-11-14T14:11:42.420Z\" msgId=\"x\">{}</tma>\
         <tma msgTime=\"2026-11-14T14:11:42.420Z\" msgId=\"5\">{}</tma></env>",
        air_one(),
        air_one()
    );
    let (o, rows) = ok(&two_tma);
    assert_eq!((o.rows, o.rejected, o.kind), (1, 1, MsgKind::Air));
    assert_eq!(rows.msg_id, [5]);
    assert_eq!(o.first_reject.map(|e| e.field), Some(Field::MsgId));
    // a bad envelope still refuses the whole message
    assert!(matches!(
        err(&air(FLT).replace("TMA.ZTT.FAA.GOV", "ZTT")),
        DocError::Field(FieldError {
            field: Field::EnvSrce,
            ..
        })
    ));
    // and a capacity overrun still refuses it, leaving the rows alone
    let (r, rows) = run(&airs(&[good, good, good]), 2);
    assert_eq!(r.unwrap_err(), DocError::Capacity);
    assert_eq!(rows_len(&rows), 0);
}

fn air_one() -> String {
    "<air tmaId=\"T1\" aid=\"TST1\" airType=\"AMD\"><flt><fps>ACTIVE</fps></flt></air>".to_string()
}

// REQ: OA-TBFM-020
#[test]
fn the_speed_unit_is_stored_with_the_value() {
    use oa_tbfm::spd_unit;
    for (v, milli, unit) in [
        ("441.0", 441_000, spd_unit::KNOTS),
        ("140.0", 140_000, spd_unit::KNOTS),
        ("99.5", 99_500, spd_unit::KNOTS),
        ("1000.0", 1_000_000, spd_unit::KNOTS),
        ("0.84", 840, spd_unit::MACH),
        ("0.77", 770, spd_unit::MACH),
        ("1.20", 1_200, spd_unit::MACH),
        // forms that are neither: kept, with the unknown unit
        ("250", 250_000, spd_unit::UNKNOWN),
        ("0.8", 800, spd_unit::UNKNOWN),
        ("5.0", 5_000, spd_unit::UNKNOWN),
        ("250.55", 250_550, spd_unit::UNKNOWN),
        ("12345.0", 12_345_000, spd_unit::UNKNOWN),
        ("99999.999", 99_999_999, spd_unit::UNKNOWN),
    ] {
        let (o, rows) = ok(&air(&format!("<flt><spd>{v}</spd></flt>")));
        assert_eq!(rows.spd_milli[0], milli, "{v}");
        assert_eq!(rows.spd_unit[0], unit, "{v}");
        assert_eq!(o.unknown_units, u32::from(unit == spd_unit::UNKNOWN), "{v}");
        assert_eq!(o.rejected, 0, "{v}");
    }
    // no speed, no unit; an empty speed is cleared, with no unit either
    let (_, rows) = ok(&air("<flt><fps>A</fps></flt>"));
    assert_eq!(
        (rows.spd_milli[0], rows.spd_unit[0]),
        (NO_NUM, spd_unit::NONE)
    );
    let (_, rows) = ok(&air("<flt><spd/></flt>"));
    assert_eq!(
        (rows.spd_milli[0], rows.spd_unit[0]),
        (NO_NUM, spd_unit::NONE)
    );
    assert_eq!(BLANK_ROW.spd_unit, spd_unit::NONE);
    assert_eq!(spd_unit::NAMES[usize::from(spd_unit::MACH)], "mach");
    // the unit survives the columns
    let (_, rows) = ok(&air("<flt><spd>0.84</spd></flt>"));
    assert_eq!(row_at(&rows, 0).unwrap().spd_unit, spd_unit::MACH);
    // and the statistics count the speeds that could not be placed
    let mut stats = new_stats();
    for v in ["441.0", "250", "0.84", "7"] {
        let (r, rows) = run(&air(&format!("<flt><spd>{v}</spd></flt>")), 4);
        observe(&mut stats, &r, &rows.section);
    }
    assert_eq!(stats.unknown_units, 2);
}

// REQ: OA-TBFM-021
#[test]
fn a_schedule_flag_with_another_word_is_kept_as_unknown() {
    let (o, rows) = ok(&air(
        "<sch><mfx>FIXA</mfx><man>YES</man><rfz>MAYBE</rfz><sfz>no</sfz><sus>NO</sus></sch>",
    ));
    assert_eq!((o.rows, o.rejected, o.unknown_words), (1, 0, 2));
    let s = rows.sch[0];
    assert_eq!(sch_get(s, sch_flag::MAN), sch_flag::YES);
    assert_eq!(sch_get(s, sch_flag::RFZ), sch_flag::UNKNOWN);
    assert_eq!(sch_get(s, sch_flag::SFZ), sch_flag::UNKNOWN);
    assert_eq!(sch_get(s, sch_flag::SUS), sch_flag::NO);
    assert_eq!(rows.mfx[0], n::<8>("FIXA"));
    // a change type is not a flag: an unknown one still refuses the flight
    assert_eq!(
        err(&air("<mrp mrpType=\"XYZ\"/>")),
        DocError::Field(FieldError {
            field: Field::MrpType,
            kind: FieldErrorKind::Vocabulary,
            at: air("<mrp mrpType=\"XYZ\"/>").find("XYZ").unwrap() as u32,
        })
    );
    let mut stats = new_stats();
    let (r, rows) = run(&air("<sch><man>X</man></sch>"), 4);
    observe(&mut stats, &r, &rows.section);
    assert_eq!((stats.unknown_words, stats.rejected_flights), (1, 0));
}

// REQ: OA-TBFM-022
#[test]
fn labels_may_hold_spaces_and_values_are_trimmed() {
    let (_, rows) = ok(&air(
        "<mrp mrpType=\"NEW\"><cfg>RWY 17 L</cfg><scn>SCENARIO  2</scn><cat>HEAVY JET ARR</cat><gat>GATE A 1</gat><tra> ZTT </tra></mrp>",
    ));
    assert_eq!(name_str(&rows.cfg[0]), "RWY 17 L");
    assert_eq!(name_str(&rows.scn[0]), "SCENARIO  2");
    assert_eq!(name_str(&rows.cat[0]), "HEAVY JET ARR");
    assert_eq!(name_str(&rows.gat[0]), "GATE A 1");
    assert_eq!(name_str(&rows.tra[0]), "ZTT");
    // only the outer whitespace goes, wherever it comes from
    let (_, rows) = ok(&air("<mrp><cfg>&#32;&#9;A B&#10; </cfg></mrp>"));
    assert_eq!(name_str(&rows.cfg[0]), "A B");
    let xml = air(FLT).replace("aid=\"TST123\"", "aid=\" TST123 \"");
    assert_eq!(name_str(&ok(&xml).1.aid[0]), "TST123");
    // trimmed numbers and times parse; a value that is only whitespace is an empty element
    let (_, rows) = ok(&air(
        "<flt><spd> 441.0 </spd><etd>&#32;2026-11-14T14:00:00Z </etd></flt>",
    ));
    assert_eq!(rows.spd_milli[0], 441_000);
    assert_eq!(rows.etd_ms[0], t(14, 0, 0));
    let (_, rows) = ok(&air("<mrp><cfg>&#32;&#32;</cfg></mrp>"));
    assert_eq!(rows.cfg[0], [0; 20]);
    assert!(is_cleared(rows.cleared[0], Field::Cfg));
    // identifiers and fixes still refuse an interior space, and a label a control character
    for body in [
        "<flt><aid>TS T</aid></flt>",
        "<mrp><mfx>FI XA</mfx></mrp>",
        "<flt><typ>A B</typ></flt>",
    ] {
        assert!(matches!(
            err(&air(body)),
            DocError::Field(FieldError {
                kind: FieldErrorKind::BadChar,
                ..
            })
        ));
    }
    assert!(matches!(
        err(&air("<mrp><cfg>A&#9;B</cfg></mrp>")),
        DocError::Field(FieldError {
            field: Field::Cfg,
            kind: FieldErrorKind::BadChar,
            ..
        })
    ));
    // the width bound counts the spaces
    assert!(matches!(
        err(&air(&format!("<mrp><cfg>{}</cfg></mrp>", "A B ".repeat(6)))),
        DocError::Field(FieldError {
            field: Field::Cfg,
            kind: FieldErrorKind::TooLong,
            ..
        })
    ));
}

// REQ: OA-TBFM-023
#[test]
fn text_split_by_a_comment_or_cdata_is_joined() {
    for (v, want) in [
        ("25<!--x-->0.5", 250_500),
        ("<![CDATA[25]]>0.5", 250_500),
        ("2<!--a-->5<![CDATA[0]]>.5", 250_500),
    ] {
        let (o, rows) = ok(&air(&format!("<flt><spd>{v}</spd></flt>")));
        assert_eq!((rows.spd_milli[0], o.rejected), (want, 0), "{v}");
    }
    let (_, rows) = ok(&air("<flt><fps>AB<!-- c -->CD</fps></flt>"));
    assert_eq!(name_str(&rows.fps[0]), "ABCD");
    let (_, rows) = ok(&air(
        "<flt><fps>AB<![CDATA[CD]]></fps><aid>TS<!---->T123</aid></flt>",
    ));
    assert_eq!(name_str(&rows.fps[0]), "ABCD");
    assert_eq!(name_str(&rows.aid[0]), "TST123");
    // pieces joined are validated as one value
    assert!(matches!(
        err(&air("<flt><bcn>07<!--x-->8<!--y-->9</bcn></flt>")),
        DocError::Field(FieldError {
            field: Field::Bcn,
            kind: FieldErrorKind::Format,
            ..
        })
    ));
    // the 64-byte working bound applies to the total
    let half = "A".repeat(40);
    assert!(matches!(
        err(&air(&format!("<flt><fps>{half}<!--x-->{half}</fps></flt>"))),
        DocError::Field(FieldError {
            field: Field::Fps,
            kind: FieldErrorKind::TooLong,
            ..
        })
    ));
}

// REQ: OA-TBFM-024
#[test]
fn a_namespace_written_with_a_character_reference_is_the_same_namespace() {
    let plain = air("<flt><aid>TST123</aid></flt>");
    let escaped = plain.replacen(
        &format!("xmlns=\"{NS}\""),
        &format!("xmlns=\"{}\"", NS.replacen('u', "&#117;", 1)),
        1,
    );
    assert_ne!(plain, escaped);
    let (o, rows) = ok(&plain);
    let (o2, rows2) = ok(&escaped);
    assert_eq!(o, o2);
    assert_eq!(rows_len(&rows2), rows_len(&rows));
    assert_eq!(name_str(&rows2.aid[0]), name_str(&rows.aid[0]));
}

/// The sealed window of the list in Unix ms: 2026-11-20T00:00:00Z to 2026-11-23T00:17:00Z.
const SEAL: (i64, i64) = (1_795_132_800_000, 1_795_393_020_000);

/// `xml` with its message time (`msgTime`) set to `ts`.
fn timed(xml: &str, ts: &str) -> String {
    xml.replace(
        "msgTime=\"2026-11-14T14:11:42.420Z\"",
        &format!("msgTime=\"{ts}\""),
    )
}

// REQ: OA-SEAL-014
#[test]
fn a_flight_timed_in_the_drop_window_is_dropped_before_any_count_even_when_it_has_a_refused_field()
{
    let w = oa_sealed::DropWindow::new(SEAL.0, SEAL.1);
    let mut p = Parser::new(default_limits()).unwrap();
    let mut stats = new_stats();
    let mut rows = new_rows(16, w);
    let bad = air(&format!("{FLT}{ETA}<sta><sta_rwy>garbage</sta_rwy></sta>"));
    // Timed in the window: with a good flight and with a refused one. Neither is in a count.
    for xml in [air(FLT), bad.clone()] {
        let xml = timed(&xml, "2026-11-21T00:00:00Z");
        let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
        let o = r.as_ref().unwrap();
        assert_eq!(
            (o.sealed_dropped, o.rows, o.rejected, o.dropped),
            (1, 0, 0, 0)
        );
        assert_eq!(o.reject_fields.iter().sum::<u32>(), 0);
        assert_eq!(o.reject_kinds.iter().sum::<u32>(), 0);
        observe(&mut stats, &r, &rows.section);
    }
    assert_eq!(rows_len(&rows), 0);
    assert_eq!(stats.sealed_window_dropped, 2);
    assert_eq!((stats.messages, stats.accepted, stats.rows), (0, 0, 0));
    assert_eq!(stats.kinds.iter().sum::<u64>(), 0);
    assert_eq!(stats.rejected_flights, 0);
    assert_eq!(stats.field_errors.iter().sum::<u64>(), 0);
    assert_eq!(stats.reject_kinds.iter().sum::<u64>(), 0);
    // Outside it (the end is excluded), the same flights are kept and refused as usual.
    let xml = timed(&air(FLT), "2026-11-23T00:17:00Z");
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    assert_eq!(r.as_ref().unwrap().sealed_dropped, 0);
    observe(&mut stats, &r, &rows.section);
    let xml = timed(&bad, "2026-11-19T23:59:59Z");
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    assert_eq!(r.as_ref().unwrap().rejected, 1);
    observe(&mut stats, &r, &rows.section);
    assert_eq!((stats.messages, stats.sealed_window_dropped), (2, 2));
}

// REQ: OA-SEAL-014
#[test]
fn a_message_with_no_known_time_is_not_dropped_and_a_refused_one_is_counted_as_refused() {
    let w = oa_sealed::DropWindow::new(SEAL.0, SEAL.1);
    let mut p = Parser::new(default_limits()).unwrap();
    let mut rows = new_rows(16, w);
    // The message time is refused, so the message has no time to drop by: refused as before.
    let xml = timed(&air(FLT), "not-a-time");
    let r = decode_bytes(&mut p, xml.as_bytes(), &mut rows);
    let o = r.unwrap();
    assert_eq!((o.sealed_dropped, o.rejected), (0, 1));
    assert_eq!(rows_len(&rows), 0);
}
